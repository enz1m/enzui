//! Persistent chunked UTF-8 storage with cheap immutable snapshots.
//!
//! Shared source blocks and balanced summaries support bounded-range access, byte/line conversion, and editing without flattening the document.
//!
//! Lines are separated by LF alone; a CR is ordinary text, unlike the native
//! editor, which also ends lines at CR and LF+CR. An adopter loading CR or
//! CRLF text normalizes it to LF first, and may write CRLF back on save with
//! `write_crlf_to`.

#![allow(
    clippy::string_slice,
    reason = "the rope keeps every byte offset on a char boundary; a split anywhere else would \
              corrupt the text"
)]

use std::fmt;
use std::io;
use std::ops::Range;
use std::sync::Arc;

/// Target size of source pieces. A piece may be up to three bytes larger when
/// moving the split to the end of a UTF-8 scalar.
pub const TARGET_CHUNK_BYTES: usize = 64 * 1024;

// Coalesce pieces below this size so metadata remains proportional to live text rather than edit count.
const MIN_CHUNK_BYTES: usize = TARGET_CHUNK_BYTES / 2;

// Detach a small survivor when retaining its source would pin at least this allocation ratio.
const MAX_SOURCE_TO_PIECE_RATIO: usize = 8;

/// Compact subtree totals used for byte and logical-line navigation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextSummary {
    /// UTF-8 bytes in the subtree.
    pub bytes: usize,
    /// Unicode scalar values in the subtree.
    ///
    /// AccessKit and AT-SPI address text by scalar offset rather than UTF-8
    /// byte offset. Keeping this compact aggregate beside `bytes` makes an
    /// arbitrary accessibility offset an O(log chunks) lookup instead of a
    /// scan from the beginning of a potentially multi-gigabyte document.
    pub characters: usize,
    /// LF bytes in the subtree. Logical line count is `newlines + 1`.
    pub newlines: usize,
}

impl TextSummary {
    fn add(self, other: Self) -> Self {
        Self {
            bytes: self.bytes.saturating_add(other.bytes),
            characters: self.characters.saturating_add(other.characters),
            newlines: self.newlines.saturating_add(other.newlines),
        }
    }
}

#[derive(Clone)]
struct Piece {
    source: Arc<String>,
    range: Range<usize>,
    summary: TextSummary,
}

impl Piece {
    fn new(source: Arc<String>, range: Range<usize>) -> Self {
        let summary = {
            let text = &source[range.clone()];
            TextSummary {
                bytes: text.len(),
                characters: text.chars().count(),
                newlines: text.bytes().filter(|byte| *byte == b'\n').count(),
            }
        };
        Self {
            source,
            range,
            summary,
        }
    }

    fn text(&self) -> &str {
        &self.source[self.range.clone()]
    }
}

#[derive(Clone)]
enum NodeKind {
    Leaf(Piece),
    Branch { left: Arc<Node>, right: Arc<Node> },
}

#[derive(Clone)]
struct Node {
    kind: NodeKind,
    summary: TextSummary,
    height: u16,
}

impl Node {
    fn leaf(piece: Piece) -> Arc<Self> {
        Arc::new(Self {
            summary: piece.summary,
            height: 1,
            kind: NodeKind::Leaf(piece),
        })
    }

    fn branch(left: Arc<Self>, right: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            summary: left.summary.add(right.summary),
            height: 1 + left.height.max(right.height),
            kind: NodeKind::Branch { left, right },
        })
    }
}

/// An immutable chunked-text generation that clones by sharing its root and source blocks.
#[derive(Clone, Default)]
pub struct TextSnapshot {
    root: Option<Arc<Node>>,
}

impl fmt::Debug for TextSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextSnapshot")
            .field("summary", &self.summary())
            .field("chunks", &self.chunk_count())
            .finish()
    }
}

impl fmt::Display for TextSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut err = Ok(());
        self.for_each_chunk(|chunk| {
            if err.is_ok() {
                err = f.write_str(chunk);
            }
        });
        err
    }
}

impl TextSnapshot {
    /// Number of UTF-8 bytes.
    pub fn len(&self) -> usize {
        self.summary().bytes
    }

    /// Whether the document contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Compact totals for this generation.
    pub fn summary(&self) -> TextSummary {
        self.root
            .as_ref()
            .map_or_else(TextSummary::default, |n| n.summary)
    }

    /// Returns the number of Unicode scalar values in constant time.
    ///
    /// Accessibility text offsets use this coordinate space rather than UTF-8 bytes.
    pub fn character_count(&self) -> usize {
        self.summary().characters
    }

    /// Logical lines, including the empty line after a trailing LF.
    pub fn line_count(&self) -> usize {
        self.summary().newlines.saturating_add(1)
    }

    /// Returns the number of material pieces.
    ///
    /// After construction or replacement, this is bounded by `4 * ceil(len / TARGET_CHUNK_BYTES) + 4`.
    pub fn chunk_count(&self) -> usize {
        fn count(node: &Node) -> usize {
            match &node.kind {
                NodeKind::Leaf(_) => 1,
                NodeKind::Branch { left, right } => count(left) + count(right),
            }
        }
        self.root.as_deref().map_or(0, count)
    }

    /// Largest material piece in bytes.
    pub fn max_chunk_bytes(&self) -> usize {
        let mut largest = 0;
        self.for_each_chunk(|chunk| largest = largest.max(chunk.len()));
        largest
    }

    /// Returns the persistent tree height for deterministic balance diagnostics.
    pub fn tree_height(&self) -> usize {
        self.root.as_ref().map_or(0, |root| root.height as usize)
    }

    /// Returns capacity reserved by distinct source allocations reachable from this generation, counting shared sources once.
    pub fn retained_source_bytes(&self) -> usize {
        Self::retained_source_bytes_across(std::iter::once(self))
    }

    /// Returns capacity reserved by the union of source allocations reachable from `snapshots`.
    ///
    /// Allocations shared by multiple generations are counted once.
    pub fn retained_source_bytes_across<'a>(
        snapshots: impl IntoIterator<Item = &'a Self>,
    ) -> usize {
        fn walk(node: &Node, sources: &mut Vec<(*const String, usize)>) {
            match &node.kind {
                NodeKind::Leaf(piece) => {
                    let ptr = Arc::as_ptr(&piece.source);
                    if !sources.iter().any(|(seen, _)| *seen == ptr) {
                        sources.push((ptr, piece.source.capacity()));
                    }
                }
                NodeKind::Branch { left, right } => {
                    walk(left, sources);
                    walk(right, sources);
                }
            }
        }

        let mut sources = Vec::new();
        for snapshot in snapshots {
            if let Some(root) = &snapshot.root {
                walk(root, &mut sources);
            }
        }
        sources
            .into_iter()
            .fold(0usize, |total, (_, bytes)| total.saturating_add(bytes))
    }

    /// Reports whether two generations share at least one source allocation.
    pub fn shares_source_with(&self, other: &Self) -> bool {
        fn visit_sources(node: &Node, visit: &mut impl FnMut(&Arc<String>)) {
            match &node.kind {
                NodeKind::Leaf(piece) => visit(&piece.source),
                NodeKind::Branch { left, right } => {
                    visit_sources(left, visit);
                    visit_sources(right, visit);
                }
            }
        }
        let mut sources = Vec::new();
        if let Some(root) = &self.root {
            visit_sources(root, &mut |source| sources.push(Arc::as_ptr(source)));
        }
        let mut shared = false;
        if let Some(root) = &other.root {
            visit_sources(root, &mut |source| {
                shared |= sources.contains(&Arc::as_ptr(source));
            });
        }
        shared
    }

    /// Visit the complete text in byte order without flattening it.
    pub fn for_each_chunk(&self, mut visit: impl FnMut(&str)) {
        fn walk(node: &Node, visit: &mut impl FnMut(&str)) {
            match &node.kind {
                NodeKind::Leaf(piece) => visit(piece.text()),
                NodeKind::Branch { left, right } => {
                    walk(left, visit);
                    walk(right, visit);
                }
            }
        }
        if let Some(root) = &self.root {
            walk(root, &mut visit);
        }
    }

    /// Visits a valid UTF-8 byte range in order without copying.
    ///
    /// Reversed, out-of-bounds, or non-boundary ranges invoke no callback and do not panic.
    pub fn for_each_chunk_in(&self, range: Range<usize>, mut visit: impl FnMut(&str)) {
        if range.start > range.end
            || range.end > self.len()
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return;
        }
        fn walk(node: &Node, base: usize, wanted: &Range<usize>, visit: &mut impl FnMut(&str)) {
            let end = base + node.summary.bytes;
            if wanted.end <= base || wanted.start >= end {
                return;
            }
            match &node.kind {
                NodeKind::Leaf(piece) => {
                    let from = wanted.start.saturating_sub(base);
                    let to = wanted.end.min(end) - base;
                    if from < to {
                        visit(&piece.text()[from..to]);
                    }
                }
                NodeKind::Branch { left, right } => {
                    walk(left, base, wanted, visit);
                    walk(right, base + left.summary.bytes, wanted, visit);
                }
            }
        }
        if let Some(root) = &self.root {
            walk(root, 0, &range, &mut visit);
        }
    }

    /// Materializes a valid UTF-8 byte range, returning `None` for invalid coordinates.
    ///
    /// Callers should keep requested ranges bounded.
    pub fn slice_to_string(&self, range: Range<usize>) -> Option<String> {
        if range.start > range.end
            || range.end > self.len()
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return None;
        }
        let mut out = String::with_capacity(range.len());
        self.for_each_chunk_in(range, |chunk| out.push_str(chunk));
        Some(out)
    }

    /// Materializes the complete generation as contiguous text.
    ///
    /// Prefer chunk iteration or bounded slices in frame-sensitive code.
    pub fn materialize(&self) -> String {
        self.slice_to_string(0..self.len()).unwrap_or_default()
    }

    /// Compare against contiguous text without allocating.
    pub fn eq_str(&self, expected: &str) -> bool {
        if self.len() != expected.len() {
            return false;
        }
        let mut offset = 0;
        let mut equal = true;
        self.for_each_chunk(|chunk| {
            let end = offset + chunk.len();
            equal &= expected.get(offset..end) == Some(chunk);
            offset = end;
        });
        equal
    }

    /// Compare a byte range with contiguous text without allocating.
    pub fn range_eq(&self, range: Range<usize>, expected: &str) -> bool {
        if range.start > range.end
            || range.end > self.len()
            || range.len() != expected.len()
            || !self.is_char_boundary(range.start)
            || !self.is_char_boundary(range.end)
        {
            return false;
        }
        let mut offset = 0;
        let mut equal = true;
        self.for_each_chunk_in(range, |chunk| {
            let end = offset + chunk.len();
            equal &= expected.get(offset..end) == Some(chunk);
            offset = end;
        });
        equal
    }

    /// Reports whether `needle` occurs without flattening the document.
    ///
    /// At most `needle.len() - 1` overlap bytes are retained across piece boundaries.
    pub fn contains(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let mut tail = String::new();
        let mut found = false;
        self.for_each_chunk(|chunk| {
            if found {
                return;
            }
            let mut window = String::with_capacity(tail.len() + chunk.len());
            window.push_str(&tail);
            window.push_str(chunk);
            found = window.contains(needle);
            if !found {
                let keep_from = window.len().saturating_sub(needle.len().saturating_sub(1));
                let mut boundary = keep_from;
                while boundary < window.len() && !window.is_char_boundary(boundary) {
                    boundary += 1;
                }
                tail.clear();
                tail.push_str(&window[boundary..]);
            }
        });
        found
    }

    /// Writes exact UTF-8 bytes in chunks.
    ///
    /// # Errors
    ///
    /// Returns the first error reported by `out`.
    pub fn write_to(&self, out: &mut impl io::Write) -> io::Result<()> {
        let mut result = Ok(());
        self.for_each_chunk(|chunk| {
            if result.is_ok() {
                result = out.write_all(chunk.as_bytes());
            }
        });
        result
    }

    /// Writes each LF as CRLF without materializing a converted copy.
    ///
    /// Input is expected to use LF line endings already.
    ///
    /// # Errors
    ///
    /// Returns the first error reported by `out`.
    pub fn write_crlf_to(&self, out: &mut impl io::Write) -> io::Result<()> {
        let mut result = Ok(());
        self.for_each_chunk(|chunk| {
            if result.is_err() {
                return;
            }
            let mut rest = chunk;
            while let Some(at) = rest.find('\n') {
                result = out.write_all(&rest.as_bytes()[..at]);
                if result.is_ok() {
                    result = out.write_all(b"\r\n");
                }
                rest = &rest[at + 1..];
            }
            if result.is_ok() {
                result = out.write_all(rest.as_bytes());
            }
        });
        result
    }

    /// Byte offset at the start of `line`, or `None` past the document.
    pub fn line_start(&self, line: usize) -> Option<usize> {
        if line == 0 {
            return Some(0);
        }
        if line >= self.line_count() {
            return None;
        }
        self.byte_after_nth_newline(line - 1)
    }

    /// Byte offset at the end of `line`, excluding LF.
    pub fn line_end(&self, line: usize) -> Option<usize> {
        self.line_start(line)?;
        Some(
            self.line_start(line + 1)
                .map_or(self.len(), |next| next.saturating_sub(1)),
        )
    }

    /// Convert a native `(line, byte-column)` position, clamping to a valid
    /// UTF-8 boundary on that line.
    pub fn byte_of_line_column(&self, line: usize, column: usize) -> usize {
        let Some(start) = self.line_start(line) else {
            return self.len();
        };
        let end = self.line_end(line).unwrap_or(self.len());
        let mut byte = start.saturating_add(column).min(end);
        while byte > start && !self.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    /// Strict counterpart used to validate an edit delta.
    pub fn checked_byte_of_line_column(&self, line: usize, column: usize) -> Option<usize> {
        let start = self.line_start(line)?;
        let end = self.line_end(line)?;
        let byte = start.checked_add(column)?;
        (byte <= end && self.is_char_boundary(byte)).then_some(byte)
    }

    /// Convert a byte offset to `(line, byte-column)` in O(log chunks) plus at
    /// most one chunk scan.
    pub fn line_column_of_byte(&self, byte: usize) -> (usize, usize) {
        let mut byte = byte.min(self.len());
        while byte > 0 && !self.is_char_boundary(byte) {
            byte -= 1;
        }
        let line = self.newlines_before(byte);
        let start = self.line_start(line).unwrap_or(0);
        (line, byte - start)
    }

    /// Human-facing, one-based character column at `byte`.
    pub fn character_column(&self, byte: usize) -> usize {
        let byte = self.floor_char_boundary(byte);
        let (line, _) = self.line_column_of_byte(byte);
        let start = self.line_start(line).unwrap_or(0);
        let mut chars = 0;
        self.for_each_chunk_in(start..byte, |chunk| chars += chunk.chars().count());
        chars + 1
    }

    /// Convert a Unicode-scalar offset to its UTF-8 byte offset in O(log
    /// chunks) plus at most one chunk scan. The document end is a valid
    /// position; offsets beyond it are refused.
    pub fn byte_of_character(&self, character: usize) -> Option<usize> {
        if character > self.character_count() {
            return None;
        }
        if character == self.character_count() {
            return Some(self.len());
        }

        fn find(node: &Node, character: usize, base: usize) -> usize {
            match &node.kind {
                NodeKind::Leaf(piece) => {
                    let local = piece
                        .text()
                        .char_indices()
                        .nth(character)
                        .map_or(piece.summary.bytes, |(byte, _)| byte);
                    base.saturating_add(local)
                }
                NodeKind::Branch { left, right } => {
                    if character < left.summary.characters {
                        find(left, character, base)
                    } else {
                        find(
                            right,
                            character - left.summary.characters,
                            base.saturating_add(left.summary.bytes),
                        )
                    }
                }
            }
        }

        Some(find(self.root.as_deref()?, character, 0))
    }

    /// Convert a UTF-8 boundary to its Unicode-scalar offset in O(log chunks)
    /// plus at most one chunk scan. A non-boundary or out-of-range byte is
    /// refused rather than rounded.
    pub fn character_of_byte(&self, byte: usize) -> Option<usize> {
        if byte > self.len() || !self.is_char_boundary(byte) {
            return None;
        }
        if byte == self.len() {
            return Some(self.character_count());
        }

        fn count(node: &Node, byte: usize) -> usize {
            match &node.kind {
                NodeKind::Leaf(piece) => piece.text()[..byte].chars().count(),
                NodeKind::Branch { left, right } => {
                    if byte < left.summary.bytes {
                        count(left, byte)
                    } else {
                        left.summary.characters + count(right, byte - left.summary.bytes)
                    }
                }
            }
        }

        self.root.as_deref().map(|root| count(root, byte))
    }

    /// Character immediately before `byte`.
    pub fn char_before(&self, byte: usize) -> Option<char> {
        if byte == 0 || byte > self.len() {
            return None;
        }
        let start = byte.saturating_sub(4);
        self.slice_to_string(start..byte)
            .or_else(|| {
                let mut valid = start;
                while valid < byte && !self.is_char_boundary(valid) {
                    valid += 1;
                }
                self.slice_to_string(valid..byte)
            })
            .and_then(|text| text.chars().next_back())
    }

    /// Character beginning at `byte`.
    pub fn char_at(&self, byte: usize) -> Option<char> {
        if byte >= self.len() || !self.is_char_boundary(byte) {
            return None;
        }
        let end = byte.saturating_add(4).min(self.len());
        let mut valid_end = end;
        while valid_end > byte && !self.is_char_boundary(valid_end) {
            valid_end -= 1;
        }
        self.slice_to_string(byte..valid_end)?.chars().next()
    }

    /// Whether `byte` is a UTF-8 scalar boundary in this generation.
    pub fn is_char_boundary(&self, byte: usize) -> bool {
        if byte == self.len() {
            return true;
        }
        fn find(node: &Node, byte: usize) -> bool {
            match &node.kind {
                NodeKind::Leaf(piece) => piece.source.is_char_boundary(piece.range.start + byte),
                NodeKind::Branch { left, right } => {
                    if byte < left.summary.bytes {
                        find(left, byte)
                    } else {
                        find(right, byte - left.summary.bytes)
                    }
                }
            }
        }
        byte < self.len() && self.root.as_deref().is_some_and(|root| find(root, byte))
    }

    /// Move `byte` backward to the nearest UTF-8 scalar boundary.
    pub fn floor_char_boundary(&self, mut byte: usize) -> usize {
        byte = byte.min(self.len());
        while byte > 0 && !self.is_char_boundary(byte) {
            byte -= 1;
        }
        byte
    }

    /// Move `byte` forward to the nearest UTF-8 scalar boundary.
    pub fn ceil_char_boundary(&self, mut byte: usize) -> usize {
        byte = byte.min(self.len());
        while byte < self.len() && !self.is_char_boundary(byte) {
            byte += 1;
        }
        byte
    }

    fn byte_after_nth_newline(&self, mut nth: usize) -> Option<usize> {
        fn find(node: &Node, nth: &mut usize, base: usize) -> Option<usize> {
            match &node.kind {
                NodeKind::Leaf(piece) => {
                    for (offset, byte) in piece.text().bytes().enumerate() {
                        if byte == b'\n' {
                            if *nth == 0 {
                                return Some(base + offset + 1);
                            }
                            *nth -= 1;
                        }
                    }
                    None
                }
                NodeKind::Branch { left, right } => {
                    if *nth < left.summary.newlines {
                        find(left, nth, base)
                    } else {
                        *nth -= left.summary.newlines;
                        find(right, nth, base + left.summary.bytes)
                    }
                }
            }
        }
        find(self.root.as_deref()?, &mut nth, 0)
    }

    fn newlines_before(&self, byte: usize) -> usize {
        fn count(node: &Node, byte: usize) -> usize {
            if byte == 0 {
                return 0;
            }
            match &node.kind {
                NodeKind::Leaf(piece) => piece.text()[..byte]
                    .bytes()
                    .filter(|value| *value == b'\n')
                    .count(),
                NodeKind::Branch { left, right } => {
                    if byte <= left.summary.bytes {
                        count(left, byte)
                    } else {
                        left.summary.newlines + count(right, byte - left.summary.bytes)
                    }
                }
            }
        }
        self.root.as_deref().map_or(0, |root| count(root, byte))
    }
}

/// Mutable owner of the latest text generation.
#[derive(Clone, Debug, Default)]
pub struct TextStore {
    current: TextSnapshot,
}

impl TextStore {
    /// Build a store without copying `text`'s allocation.
    pub fn from_string(text: String) -> Self {
        let source = Arc::new(text);
        let root = build_source(source);
        Self {
            current: TextSnapshot { root },
        }
    }

    /// Current immutable generation. O(1).
    pub fn snapshot(&self) -> TextSnapshot {
        self.current.clone()
    }

    /// Restores a captured generation in constant time while retaining shared source blocks.
    pub fn restore(&mut self, snapshot: TextSnapshot) {
        self.current = snapshot;
    }

    /// Borrow the current generation.
    pub fn current(&self) -> &TextSnapshot {
        &self.current
    }

    /// Replaces one UTF-8 byte range and publishes a new generation.
    ///
    /// Unchanged source blocks remain shared with existing snapshots.
    ///
    /// # Errors
    ///
    /// Returns [`EditError::OutOfBounds`] for a reversed or out-of-bounds range and [`EditError::NotCharBoundary`] when an endpoint splits a UTF-8 scalar.
    pub fn replace(&mut self, range: Range<usize>, inserted: String) -> Result<(), EditError> {
        self.replace_many(std::iter::once((range, inserted)))
    }

    /// Applies sorted, non-overlapping replacements in the current generation’s coordinates.
    ///
    /// Validation completes before mutation, so an invalid batch is atomic. Replacements are applied from the end and unchanged source chunks remain shared.
    ///
    /// # Errors
    ///
    /// Returns [`EditError::OutOfBounds`] for invalid ordering or bounds and [`EditError::NotCharBoundary`] for endpoints inside a UTF-8 scalar.
    pub fn replace_many(
        &mut self,
        replacements: impl IntoIterator<Item = (Range<usize>, String)>,
    ) -> Result<(), EditError> {
        let replacements: Vec<_> = replacements.into_iter().collect();
        let mut previous_end = 0;
        for (index, (range, _)) in replacements.iter().enumerate() {
            if range.start > range.end
                || range.end > self.current.len()
                || (index > 0 && range.start < previous_end)
            {
                return Err(EditError::OutOfBounds);
            }
            if !self.current.is_char_boundary(range.start)
                || !self.current.is_char_boundary(range.end)
            {
                return Err(EditError::NotCharBoundary);
            }
            previous_end = range.end;
        }

        let original = self.current.clone();
        let mut root = self.current.root.clone();
        let mut changed = false;
        for (range, inserted) in replacements.into_iter().rev() {
            // Compare no-op replacements against the immutable pre-batch generation because every range uses its coordinates.
            if range.len() == inserted.len() && original.range_eq(range.clone(), &inserted) {
                continue;
            }
            changed = true;
            let (left, rest) = split(root, range.start);
            let (_, right) = split(rest, range.end - range.start);
            let middle = build_source(Arc::new(inserted));
            root = concat(concat(left, middle), right);
        }
        if changed {
            self.current.root = compact(root);
        }
        Ok(())
    }
}

/// Why a byte edit was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditError {
    /// A range was reversed, out of bounds, out of order, or overlapped its predecessor.
    OutOfBounds,
    /// An endpoint split a UTF-8 scalar.
    NotCharBoundary,
}

fn build_source(source: Arc<String>) -> Option<Arc<Node>> {
    if source.is_empty() {
        return None;
    }
    let mut leaves = Vec::with_capacity(source.len().div_ceil(TARGET_CHUNK_BYTES));
    let mut start = 0;
    while start < source.len() {
        let mut end = start.saturating_add(TARGET_CHUNK_BYTES).min(source.len());
        while end < source.len() && !source.is_char_boundary(end) {
            end += 1;
        }
        leaves.push(Node::leaf(Piece::new(Arc::clone(&source), start..end)));
        start = end;
    }
    build_balanced(&leaves)
}

fn collect_pieces(node: &Node, pieces: &mut Vec<Piece>) {
    match &node.kind {
        NodeKind::Leaf(piece) => pieces.push(piece.clone()),
        NodeKind::Branch { left, right } => {
            collect_pieces(left, pieces);
            collect_pieces(right, pieces);
        }
    }
}

// Coalesce short fragments and detach survivors that would pin disproportionately large source allocations.
fn compact(root: Option<Arc<Node>>) -> Option<Arc<Node>> {
    let root = root?;
    let mut pieces = Vec::new();
    collect_pieces(&root, &mut pieces);

    let mut merged: Vec<Piece> = Vec::with_capacity(pieces.len());
    for piece in pieces {
        if let Some(last) = merged.last_mut()
            && Arc::ptr_eq(&last.source, &piece.source)
            && last.range.end == piece.range.start
            && last.summary.bytes.saturating_add(piece.summary.bytes)
                <= TARGET_CHUNK_BYTES.saturating_add(3)
        {
            last.range.end = piece.range.end;
            last.summary = last.summary.add(piece.summary);
            continue;
        }
        merged.push(piece);
    }

    let mut nodes = Vec::with_capacity(merged.len());
    let mut pending = String::with_capacity(TARGET_CHUNK_BYTES);
    let flush_prefix = |pending: &mut String, nodes: &mut Vec<Arc<Node>>, all: bool| {
        while pending.len() >= TARGET_CHUNK_BYTES || (all && !pending.is_empty()) {
            let target = if all {
                pending.len().min(TARGET_CHUNK_BYTES)
            } else {
                TARGET_CHUNK_BYTES
            };
            let mut end = target;
            while end > 0 && !pending.is_char_boundary(end) {
                end -= 1;
            }
            if end == 0 {
                break;
            }
            let mut text = pending[..end].to_string();
            text.shrink_to_fit();
            pending.drain(..end);
            nodes.push(Node::leaf(Piece::new(Arc::new(text), 0..end)));
        }
    };

    for piece in merged {
        let short = piece.summary.bytes < MIN_CHUNK_BYTES;
        let pins_large_source = piece.summary.bytes < TARGET_CHUNK_BYTES
            && piece.source.capacity() > TARGET_CHUNK_BYTES
            && piece.source.capacity()
                > piece
                    .summary
                    .bytes
                    .saturating_mul(MAX_SOURCE_TO_PIECE_RATIO);
        if short || pins_large_source {
            pending.push_str(piece.text());
            flush_prefix(&mut pending, &mut nodes, false);
        } else {
            flush_prefix(&mut pending, &mut nodes, true);
            nodes.push(Node::leaf(piece));
        }
    }
    flush_prefix(&mut pending, &mut nodes, true);
    build_balanced(&nodes)
}

fn build_balanced(nodes: &[Arc<Node>]) -> Option<Arc<Node>> {
    match nodes.len() {
        0 => None,
        1 => Some(Arc::clone(&nodes[0])),
        len => {
            let middle = len / 2;
            Some(Node::branch(
                build_balanced(&nodes[..middle])?,
                build_balanced(&nodes[middle..])?,
            ))
        }
    }
}

fn split(root: Option<Arc<Node>>, at: usize) -> (Option<Arc<Node>>, Option<Arc<Node>>) {
    let Some(root) = root else {
        return (None, None);
    };
    if at == 0 {
        return (None, Some(root));
    }
    if at >= root.summary.bytes {
        return (Some(root), None);
    }
    match &root.kind {
        NodeKind::Leaf(piece) => {
            let absolute = piece.range.start + at;
            (
                Some(Node::leaf(Piece::new(
                    Arc::clone(&piece.source),
                    piece.range.start..absolute,
                ))),
                Some(Node::leaf(Piece::new(
                    Arc::clone(&piece.source),
                    absolute..piece.range.end,
                ))),
            )
        }
        NodeKind::Branch { left, right } => {
            if at < left.summary.bytes {
                let (before, after) = split(Some(Arc::clone(left)), at);
                (before, concat(after, Some(Arc::clone(right))))
            } else {
                let (before, after) = split(Some(Arc::clone(right)), at - left.summary.bytes);
                (concat(Some(Arc::clone(left)), before), after)
            }
        }
    }
}

fn concat(left: Option<Arc<Node>>, right: Option<Arc<Node>>) -> Option<Arc<Node>> {
    match (left, right) {
        (None, other) | (other, None) => other,
        (Some(left), Some(right)) if left.height > right.height.saturating_add(1) => {
            let NodeKind::Branch {
                left: ll,
                right: lr,
            } = &left.kind
            else {
                return Some(Node::branch(left, right));
            };
            rebalance(Arc::clone(ll), concat(Some(Arc::clone(lr)), Some(right))?)
        }
        (Some(left), Some(right)) if right.height > left.height.saturating_add(1) => {
            let NodeKind::Branch {
                left: rl,
                right: rr,
            } = &right.kind
            else {
                return Some(Node::branch(left, right));
            };
            rebalance(concat(Some(left), Some(Arc::clone(rl)))?, Arc::clone(rr))
        }
        (Some(left), Some(right)) => Some(Node::branch(left, right)),
    }
}

fn rebalance(left: Arc<Node>, right: Arc<Node>) -> Option<Arc<Node>> {
    let lh = left.height;
    let rh = right.height;
    if lh > rh.saturating_add(1) {
        if let NodeKind::Branch {
            left: ll,
            right: lr,
        } = &left.kind
        {
            if ll.height >= lr.height {
                return Some(Node::branch(
                    Arc::clone(ll),
                    Node::branch(Arc::clone(lr), right),
                ));
            }
            if let NodeKind::Branch {
                left: lrl,
                right: lrr,
            } = &lr.kind
            {
                return Some(Node::branch(
                    Node::branch(Arc::clone(ll), Arc::clone(lrl)),
                    Node::branch(Arc::clone(lrr), right),
                ));
            }
        }
    } else if rh > lh.saturating_add(1)
        && let NodeKind::Branch {
            left: rl,
            right: rr,
        } = &right.kind
    {
        if rr.height >= rl.height {
            return Some(Node::branch(
                Node::branch(left, Arc::clone(rl)),
                Arc::clone(rr),
            ));
        }
        if let NodeKind::Branch {
            left: rll,
            right: rlr,
        } = &rl.kind
        {
            return Some(Node::branch(
                Node::branch(left, Arc::clone(rll)),
                Node::branch(Arc::clone(rlr), Arc::clone(rr)),
            ));
        }
    }
    Some(Node::branch(left, right))
}

#[cfg(test)]
mod tests;
