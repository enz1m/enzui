//! Bounded byte classification and a random-access hexadecimal/ASCII preview.
//!
//! Prepare sources and execute page operations off-thread. The existing paged
//! text model receives only fixed-width ASCII rows; the original UTF-8 source
//! contract remains unchanged. A clean prefix does not validate an entire file.

use crate::paged_text::{
    NavigateOutcome, PagedTextNavigation, PagedTextOpenFailure, PagedTextOperation,
    PagedTextPolicy, PagedTextResult, PagedTextSource, PagedTextSourceError, PagedTextState,
    PagedTextViewport,
};
use std::ops::Range;
use std::sync::Arc;

/// Original bytes represented by one preview row.
pub const BYTES_PER_ROW: u64 = 16;
/// ASCII bytes per complete formatted row, including its newline.
pub const DISPLAY_ROW_BYTES: u64 = 86;
/// Maximum requested formatted range accepted by a preview source.
pub const MAX_FORMATTED_READ: usize = 1024 * 1024;
/// Maximum source prefix inspected for a large file.
pub const CLASSIFICATION_PREFIX: usize = 16 * 1024;
/// Files no larger than this receive complete classification, including late controls.
pub const COMPLETE_CLASSIFICATION_LIMIT: u64 = 256 * 1024;

/// Rendering route chosen from an inspected byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    /// Valid UTF-8 without binary controls; a partial prefix is only tentative.
    Utf8Text,
    /// Binary controls, malformed UTF-8, or incomplete UTF-8 at EOF.
    Bytes,
}

/// Scope of a classification result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewClassification {
    /// Route for the inspected bytes.
    pub kind: PreviewKind,
    /// Number of original bytes inspected.
    pub checked_bytes: usize,
    /// Whether the inspected range covers the complete source.
    pub complete: bool,
}

/// Classify bytes at any size, allowing an unfinished trailing scalar only before EOF.
#[must_use]
pub fn classify_bytes(bytes: &[u8], complete: bool) -> PreviewKind {
    let binary_control = bytes
        .iter()
        .any(|byte| (*byte < 0x20 && !matches!(*byte, b'\t' | b'\n' | b'\r')) || *byte == 0x7f);
    let invalid = match std::str::from_utf8(bytes) {
        Ok(_) => false,
        Err(error) => complete || error.error_len().is_some(),
    };
    if binary_control || invalid {
        PreviewKind::Bytes
    } else {
        PreviewKind::Utf8Text
    }
}

/// Inspect every small source completely and a bounded prefix of larger sources.
/// Execute off-thread; a short read or changed length refuses classification.
///
/// # Errors
/// Returns source metadata/read failures or a source violating its read contract.
pub fn classify_source(
    source: &dyn PagedTextSource,
) -> Result<PreviewClassification, PagedTextSourceError> {
    let len = source.len()?;
    let complete = len <= COMPLETE_CLASSIFICATION_LIMIT;
    let count = if complete {
        len as usize
    } else {
        CLASSIFICATION_PREFIX
    };
    let bytes = source.read_at(0, count)?;
    if bytes.len() != count || source.len()? != len {
        return Err(PagedTextSourceError::unreadable(
            "source changed or returned an invalid classification range",
        ));
    }
    Ok(PreviewClassification {
        kind: classify_bytes(&bytes, complete),
        checked_bytes: count,
        complete,
    })
}

/// Exact mapping between original byte addresses and formatted display rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytePreviewLayout {
    source_len: u64,
    display_len: u64,
}
impl BytePreviewLayout {
    /// Mapping for a closed or empty source.
    pub const EMPTY: Self = Self {
        source_len: 0,
        display_len: 0,
    };
    /// Compute the mapping without reading data.
    ///
    /// # Errors
    /// Refuses a source whose formatted length cannot fit a `u64` byte address.
    pub fn new(source_len: u64) -> Result<Self, PagedTextSourceError> {
        let rows = source_len.div_ceil(BYTES_PER_ROW);
        let display_len = if rows == 0 {
            0
        } else {
            rows.checked_mul(DISPLAY_ROW_BYTES)
                .and_then(|length| length.checked_sub(1))
                .ok_or_else(|| PagedTextSourceError::unreadable("byte preview length overflows"))?
        };
        Ok(Self {
            source_len,
            display_len,
        })
    }
    /// Length of the original source, independent of formatting overhead.
    #[must_use]
    pub const fn source_len(self) -> u64 {
        self.source_len
    }
    /// Length of the formatted ASCII source.
    #[must_use]
    pub const fn display_len(self) -> u64 {
        self.display_len
    }
    /// Display-row start containing an original address; EOF selects the tail.
    #[must_use]
    pub fn display_offset(self, source_byte: u64) -> u64 {
        if source_byte >= self.source_len {
            self.display_len.saturating_sub(1)
        } else {
            source_byte / BYTES_PER_ROW * DISPLAY_ROW_BYTES
        }
    }
    /// Original row address for a formatted position; formatted EOF maps to source EOF.
    #[must_use]
    pub fn source_offset(self, display_byte: u64) -> u64 {
        if display_byte >= self.display_len {
            self.source_len
        } else {
            (display_byte / DISPLAY_ROW_BYTES * BYTES_PER_ROW).min(self.source_len)
        }
    }
    /// Original bytes represented by a formatted range, including partial rows.
    #[must_use]
    pub fn source_range(self, display: Range<u64>) -> Range<u64> {
        let start = self.source_offset(display.start);
        if display.start >= display.end {
            return start..start;
        }
        let end = display
            .end
            .min(self.display_len)
            .div_ceil(DISPLAY_ROW_BYTES)
            .saturating_mul(BYTES_PER_ROW)
            .min(self.source_len);
        start..end.max(start)
    }
}

/// Immutable formatted adapter over a caller-owned random-access byte source.
/// File-backed sources retain [`crate::paged_text::FilePagedTextSource`]'s mutation checks.
pub struct PagedByteSource {
    source: Arc<dyn PagedTextSource>,
    layout: BytePreviewLayout,
}
impl std::fmt::Debug for PagedByteSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PagedByteSource")
            .field("layout", &self.layout)
            .finish_non_exhaustive()
    }
}
impl PagedByteSource {
    /// Prepare off-thread; metadata can perform I/O. Subsequent `len` reads are cached.
    ///
    /// # Errors
    /// Returns metadata failures or an unrepresentable formatted length.
    pub fn new(source: Arc<dyn PagedTextSource>) -> Result<Self, PagedTextSourceError> {
        let layout = BytePreviewLayout::new(source.len()?)?;
        Ok(Self { source, layout })
    }
    /// Captured original/display coordinate mapping.
    #[must_use]
    pub const fn layout(&self) -> BytePreviewLayout {
        self.layout
    }
}
impl PagedTextSource for PagedByteSource {
    fn line_prefix_hint(&self, offset: u64) -> Option<u64> {
        (offset <= self.layout.display_len).then_some(offset / DISPLAY_ROW_BYTES)
    }
    fn len(&self) -> Result<u64, PagedTextSourceError> {
        Ok(self.layout.display_len)
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>, PagedTextSourceError> {
        if length > MAX_FORMATTED_READ {
            return Err(PagedTextSourceError::unreadable(
                "byte preview read exceeds its bound",
            ));
        }
        if self.source.len()? != self.layout.source_len {
            return Err(PagedTextSourceError::unreadable(
                "byte preview source changed",
            ));
        }
        let length = length.min(
            usize::try_from(self.layout.display_len.saturating_sub(offset)).unwrap_or(usize::MAX),
        );
        if length == 0 {
            return Ok(Vec::new());
        }
        let first_row = offset / DISPLAY_ROW_BYTES;
        let end = offset + length as u64;
        let end_row = end.div_ceil(DISPLAY_ROW_BYTES);
        let raw_start = first_row * BYTES_PER_ROW;
        let raw_end = end_row
            .saturating_mul(BYTES_PER_ROW)
            .min(self.layout.source_len);
        let raw_count = usize::try_from(raw_end - raw_start)
            .map_err(|_| PagedTextSourceError::unreadable("byte preview range overflows"))?;
        let bytes = self.source.read_at(raw_start, raw_count)?;
        if bytes.len() != raw_count || self.source.len()? != self.layout.source_len {
            return Err(PagedTextSourceError::unreadable(
                "byte preview source changed or returned a short range",
            ));
        }
        let mut output = Vec::with_capacity(length);
        for (index, chunk) in bytes.chunks(BYTES_PER_ROW as usize).enumerate() {
            let row = first_row + index as u64;
            let row_start = row * DISPLAY_ROW_BYTES;
            let rendered = format_row(row * BYTES_PER_ROW, chunk);
            let start = offset.saturating_sub(row_start) as usize;
            let stop = (end - row_start).min(rendered.len() as u64) as usize;
            if start < stop {
                output.extend_from_slice(&rendered.as_bytes()[start..stop]);
            }
        }
        if output.len() != length {
            return Err(PagedTextSourceError::unreadable(
                "byte preview formatting returned an invalid range",
            ));
        }
        Ok(output)
    }
}
fn format_row(address: u64, bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut row = format!("{address:016x}  ");
    for index in 0..BYTES_PER_ROW as usize {
        if let Some(byte) = bytes.get(index) {
            write!(row, "{byte:02x} ").expect("formatting into a String cannot fail");
        } else {
            row.push_str("   ");
        }
    }
    row.push_str(" |");
    for index in 0..BYTES_PER_ROW as usize {
        row.push(bytes.get(index).map_or(' ', |byte| {
            if (0x20..=0x7e).contains(byte) {
                char::from(*byte)
            } else {
                '.'
            }
        }));
    }
    row.push_str("|\n");
    row
}

/// Caller-owned byte-preview paging state, with fixed rows and bounded cache admission.
pub struct PagedByteState {
    text: PagedTextState,
    layout: Option<BytePreviewLayout>,
}
impl std::fmt::Debug for PagedByteState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PagedByteState")
            .field("layout", &self.layout)
            .field("identity", &self.text.identity())
            .finish_non_exhaustive()
    }
}
impl Default for PagedByteState {
    fn default() -> Self {
        Self::new()
    }
}
impl PagedByteState {
    /// Use 768-row pages, a 512 KiB decoded cache and three overscan rows.
    #[must_use]
    pub fn new() -> Self {
        Self {
            text: PagedTextState::new(PagedTextPolicy {
                eager_cutoff: 0,
                read_chunk_size: DISPLAY_ROW_BYTES as usize * 768,
                max_display_row_bytes: DISPLAY_ROW_BYTES as usize - 1,
                cache_budget: 512 * 1024,
                overscan_rows: 3,
            }),
            layout: None,
        }
    }
    /// Open a prepared source without reading original bytes; execute the operation off-thread.
    ///
    /// # Errors
    /// Returns paged-model metadata/admission failures.
    pub fn open(
        &mut self,
        source: Arc<PagedByteSource>,
    ) -> Result<PagedTextOperation, PagedTextOpenFailure> {
        let layout = source.layout;
        let operation = self.text.open(source)?;
        self.layout = Some(layout);
        Ok(operation)
    }
    /// Apply only a result owned by the current source/generation.
    pub fn apply(&mut self, result: PagedTextResult) -> bool {
        self.text.apply(result)
    }
    /// Release the source and invalidate pending results.
    pub fn close(&mut self) {
        self.text.close();
        self.layout = None;
    }
    /// Source/display mapping, absent while closed.
    #[must_use]
    pub const fn layout(&self) -> Option<BytePreviewLayout> {
        self.layout
    }
    /// Read-only native paging model used by the shared viewer.
    #[must_use]
    pub const fn text_state(&self) -> &PagedTextState {
        &self.text
    }
    /// Request the uncached viewport page; original reads remain off-thread.
    #[must_use]
    pub fn request(&mut self, viewport: &PagedByteViewport) -> Option<PagedTextOperation> {
        viewport
            .text
            .needs_fetch(&self.text)
            .and_then(|byte| self.text.request(byte))
    }
}

/// Byte-address viewport; seeks select the 16-byte row containing the requested address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PagedByteViewport {
    text: PagedTextViewport,
}
impl PagedByteViewport {
    /// Start at source byte zero with the supplied update-time row capacity.
    #[must_use]
    pub fn new(visible_rows: usize) -> Self {
        Self {
            text: PagedTextViewport::new(visible_rows),
        }
    }
    /// Read-only display viewport used by the shared viewer.
    #[must_use]
    pub const fn text_viewport(&self) -> &PagedTextViewport {
        &self.text
    }
    /// Update row capacity from the shared viewer's measured allocation.
    pub fn visible_rows(&mut self, rows: usize) {
        self.text.visible_rows = rows.max(1);
    }
    /// Apply row movement or an original-source byte seek.
    pub fn navigate(&mut self, movement: PagedTextNavigation, state: &PagedByteState) {
        let movement = match (movement, state.layout) {
            (PagedTextNavigation::SeekByte(byte), Some(layout)) => {
                if byte >= layout.source_len().saturating_sub(1) {
                    PagedTextNavigation::End
                } else {
                    PagedTextNavigation::SeekByte(layout.display_offset(byte))
                }
            }
            (movement, _) => movement,
        };
        self.text.navigate(
            movement,
            state.text.source_len(),
            state.text.policy().read_chunk_size,
        );
    }
    /// Resolve cached movement without reading data.
    pub fn resolve(&mut self, state: &PagedByteState) -> NavigateOutcome {
        self.text.resolve(&state.text)
    }
}

#[cfg(test)]
mod tests;
