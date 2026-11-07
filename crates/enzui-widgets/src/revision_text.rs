//! Revision-aware plain text with O(1) validation of unchanged content.
//!
//! [`enz_revision_text`] caches the current materialized snapshot and paragraph.
//! Reuse a clone of the same [`TextSnapshot`] for unchanged content; create a
//! higher revision for changed bytes under the same [`TextIdentity`]. Identity
//! or revision changes materialize and shape fresh content. Format changes and
//! backend font loads can reshape retained bytes without materializing again.
//! This avoids comparing every content byte during unchanged warm layouts.
//!
//! Layout panics in all build profiles when a cached identity/revision is reused
//! with a separately constructed snapshot, a revision decreases for the cached
//! identity, or materialized byte length differs from the declared length.
//! Materializers must return immutable content; changes inside a captured owner
//! are not detected on cache hits. Only the current snapshot is retained.

use iced::advanced::layout;
use iced::advanced::mouse;
use iced::advanced::renderer;
use iced::advanced::text::{self, Paragraph};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Layout, Widget};
use iced::widget::text::{Alignment, Ellipsis, LineHeight, Shaping, Style, Wrapping};
use iced::{Element, Font, Length, Pixels, Rectangle, Size, Theme};
use std::fmt;
use std::sync::Arc;

/// Collision-free identity shared by all revisions of one logical document.
#[derive(Clone)]
pub struct TextIdentity(Arc<()>);

impl TextIdentity {
    /// Allocates a new identity.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(()))
    }

    fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Default for TextIdentity {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for TextIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TextIdentity")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

struct SnapshotInner {
    byte_len: usize,
    materialize: Box<dyn Fn() -> String + Send + Sync>,
}

/// One immutable revision of a large plain-text owner.
///
/// Construction does not call the materializer. Layout calls it on a cache
/// miss and checks the declared byte length. It must return the same bytes
/// throughout the snapshot's lifetime. Clone this value for unchanged views;
/// reconstructing it with the same identity/revision triggers a layout panic
/// when that pair is already cached.
#[derive(Clone)]
pub struct TextSnapshot {
    identity: TextIdentity,
    revision: u64,
    inner: Arc<SnapshotInner>,
}

impl fmt::Debug for TextSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextSnapshot")
            .field("identity", &self.identity)
            .field("revision", &self.revision)
            .field("byte_len", &self.inner.byte_len)
            .finish_non_exhaustive()
    }
}

impl TextSnapshot {
    /// Creates a lazily materialized immutable snapshot.
    ///
    /// Use a higher revision when changing bytes under the same cached identity.
    /// The closure may flatten a rope or concatenate chunks; unchanged cache
    /// hits do not call it. Layout panics in all build profiles if its result
    /// has a different byte length than `byte_len`, if this replaces a separate
    /// snapshot with the same identity/revision, or if its revision decreases
    /// under the same cached identity.
    pub fn new(
        identity: TextIdentity,
        revision: u64,
        byte_len: usize,
        materialize: impl Fn() -> String + Send + Sync + 'static,
    ) -> Self {
        Self {
            identity,
            revision,
            inner: Arc::new(SnapshotInner {
                byte_len,
                materialize: Box::new(materialize),
            }),
        }
    }

    /// Creates a snapshot backed by immutable shared contiguous text.
    #[must_use]
    pub fn from_arc(identity: TextIdentity, revision: u64, content: Arc<str>) -> Self {
        let byte_len = content.len();
        Self::new(identity, revision, byte_len, move || content.to_string())
    }

    /// Returns the caller-declared byte length without reading the content.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.inner.byte_len
    }

    /// Returns the monotonic revision.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn materialize(&self) -> String {
        let content = (self.inner.materialize)();
        assert_eq!(
            content.len(),
            self.inner.byte_len,
            "revision-text materializer returned a different byte length"
        );
        content
    }
}

/// Formatting for [`RevisionText`].
#[derive(Debug, Clone, Copy)]
pub struct RevisionTextFormat {
    /// Width policy.
    pub width: Length,
    /// Height policy.
    pub height: Length,
    /// Glyph size, or the renderer default.
    pub size: Option<Pixels>,
    /// Font, or the renderer default.
    pub font: Option<Font>,
    /// Line height.
    pub line_height: LineHeight,
    /// Horizontal alignment.
    pub align_x: Alignment,
    /// Vertical alignment.
    pub align_y: iced::alignment::Vertical,
    /// Shaping strategy.
    pub shaping: Shaping,
    /// Wrapping strategy.
    pub wrapping: Wrapping,
    /// Ellipsis strategy.
    pub ellipsis: Ellipsis,
}

impl Default for RevisionTextFormat {
    fn default() -> Self {
        Self {
            width: Length::Shrink,
            height: Length::Shrink,
            size: None,
            font: None,
            line_height: LineHeight::default(),
            align_x: Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: Shaping::default(),
            wrapping: Wrapping::default(),
            ellipsis: Ellipsis::default(),
        }
    }
}

/// Counts content rebuilds, retained-byte reshaping and in-place bounds resizes.
///
/// Backend font generation is process-global: another widget's font load can
/// increase `revalidations` without changing this widget's snapshot or format.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RevisionTextCounts {
    /// Rebuilds that called the snapshot materializer.
    pub content_shapes: usize,
    /// Rebuilds from retained bytes (attribute or font-generation change).
    pub revalidations: usize,
    /// In-place bounds resizes.
    pub resizes: usize,
}

/// Read the process-global backend font generation.
#[doc(hidden)]
#[must_use]
pub fn font_generation() -> iced::advanced::graphics::text::Version {
    iced::advanced::graphics::text::font_system()
        .read()
        .expect("read font system")
        .version()
}

#[doc(hidden)]
pub struct RevisionTextState<P: Paragraph> {
    paragraph: P,
    content: String,
    snapshot: Option<TextSnapshot>,
    counts: RevisionTextCounts,
}

impl<P: Paragraph> RevisionTextState<P> {
    /// Return this state's layout mechanism counters.
    #[doc(hidden)]
    #[must_use]
    pub fn counts(&self) -> RevisionTextCounts {
        self.counts
    }

    /// Borrow the currently retained materialized content.
    #[doc(hidden)]
    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_str()
    }
}

impl<P: Paragraph> Default for RevisionTextState<P> {
    fn default() -> Self {
        Self {
            paragraph: P::default(),
            content: String::new(),
            snapshot: None,
            counts: RevisionTextCounts::default(),
        }
    }
}

/// Read counters from a tree containing this renderer's revision-text state.
///
/// # Panics
/// Panics if the tree's state has a different type.
#[doc(hidden)]
#[must_use]
pub fn counts_of<Renderer>(tree: &Tree) -> RevisionTextCounts
where
    Renderer: text::Renderer,
{
    tree.state
        .downcast_ref::<RevisionTextState<Renderer::Paragraph>>()
        .counts()
}

/// Plain text that validates unchanged content by snapshot identity and revision.
///
/// # Panics
/// Layout panics on stale or decreasing revisions under the cached identity,
/// or when materialized byte length differs from the snapshot declaration.
/// These checks apply in all build profiles.
#[must_use]
pub struct RevisionText<'a, Renderer = iced::Renderer> {
    snapshot: TextSnapshot,
    format: RevisionTextFormat,
    style: Box<dyn Fn(&Theme) -> Style + 'a>,
    renderer: std::marker::PhantomData<Renderer>,
}

/// Creates revision-aware plain text.
pub fn enz_revision_text(snapshot: TextSnapshot) -> RevisionText<'static> {
    revision_text_for(snapshot)
}

/// Deprecated compatibility spelling for [`enz_revision_text`].
#[deprecated(
    since = "0.2.0",
    note = "use the prefix-first `enz_revision_text` constructor"
)]
pub fn revision_text(snapshot: TextSnapshot) -> RevisionText<'static> {
    enz_revision_text(snapshot)
}

/// Build revision-aware text for an explicit renderer.
#[doc(hidden)]
pub fn revision_text_for<Renderer>(snapshot: TextSnapshot) -> RevisionText<'static, Renderer> {
    RevisionText {
        snapshot,
        format: RevisionTextFormat::default(),
        style: Box::new(|_| Style::default()),
        renderer: std::marker::PhantomData,
    }
}

impl<'a, Renderer> RevisionText<'a, Renderer> {
    /// Sets the glyph size.
    pub fn size(mut self, size: impl Into<Pixels>) -> Self {
        self.format.size = Some(size.into());
        self
    }

    /// Sets the line height.
    pub fn line_height(mut self, line_height: impl Into<LineHeight>) -> Self {
        self.format.line_height = line_height.into();
        self
    }

    /// Sets the font.
    pub fn font(mut self, font: impl Into<Font>) -> Self {
        self.format.font = Some(font.into());
        self
    }

    /// Sets the width policy.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.format.width = width.into();
        self
    }

    /// Sets the height policy.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.format.height = height.into();
        self
    }

    /// Sets horizontal alignment.
    pub fn align_x(mut self, alignment: impl Into<Alignment>) -> Self {
        self.format.align_x = alignment.into();
        self
    }

    /// Sets vertical alignment.
    pub fn align_y(mut self, alignment: impl Into<iced::alignment::Vertical>) -> Self {
        self.format.align_y = alignment.into();
        self
    }

    /// Sets shaping.
    pub fn shaping(mut self, shaping: Shaping) -> Self {
        self.format.shaping = shaping;
        self
    }

    /// Sets wrapping.
    pub fn wrapping(mut self, wrapping: Wrapping) -> Self {
        self.format.wrapping = wrapping;
        self
    }

    /// Sets ellipsis.
    pub fn ellipsis(mut self, ellipsis: Ellipsis) -> Self {
        self.format.ellipsis = ellipsis;
        self
    }

    /// Sets the theme-dependent paint colour. Colour is not in the shaping key.
    pub fn style(mut self, style: impl Fn(&Theme) -> Style + 'a) -> Self {
        self.style = Box::new(style);
        self
    }

    /// Sets a fixed paint colour. It does not invalidate glyph layout.
    pub fn color(self, color: impl Into<iced::Color>) -> Self {
        let color = color.into();
        self.style(move |_| Style { color: Some(color) })
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for RevisionText<'_, Renderer>
where
    Renderer: iced::advanced::Renderer + text::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RevisionTextState<Renderer::Paragraph>>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(RevisionTextState::<Renderer::Paragraph>::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.format.width, self.format.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let state = tree
            .state
            .downcast_mut::<RevisionTextState<Renderer::Paragraph>>();
        let format = self.format;

        tree.size = layout::sized(limits, format.width, format.height, |limits| {
            let bounds = limits.max();
            let size = format.size.unwrap_or_else(|| renderer.text_size());
            let font = format.font.unwrap_or_else(|| renderer.font());
            let definition = text::Text {
                content: (),
                bounds,
                size,
                line_height: format.line_height,
                font,
                align_x: format.align_x,
                align_y: format.align_y,
                shaping: format.shaping,
                wrapping: format.wrapping,
                ellipsis: format.ellipsis,
                hint_factor: renderer.hint_factor(),
            };

            let unchanged = state.snapshot.as_ref().is_some_and(|cached| {
                cached.identity.same(&self.snapshot.identity)
                    && cached.revision == self.snapshot.revision
            });

            if unchanged {
                let cached = state.snapshot.as_ref().expect("checked above");
                assert!(
                    Arc::ptr_eq(&cached.inner, &self.snapshot.inner),
                    "stale revision: identity/revision reused with a different snapshot"
                );
                match state.paragraph.compare(definition) {
                    text::Difference::None => {}
                    text::Difference::Bounds => {
                        state.paragraph.resize(bounds);
                        state.counts.resizes += 1;
                    }
                    text::Difference::Shape => {
                        state.paragraph = Renderer::Paragraph::with_text(
                            definition.with_content(state.content.as_str()),
                        );
                        state.counts.revalidations += 1;
                    }
                }
            } else {
                if let Some(cached) = &state.snapshot {
                    assert!(
                        !cached.identity.same(&self.snapshot.identity)
                            || self.snapshot.revision > cached.revision,
                        "revision must increase for a TextIdentity"
                    );
                }
                state.content = self.snapshot.materialize();
                state.paragraph =
                    Renderer::Paragraph::with_text(definition.with_content(state.content.as_str()));
                state.snapshot = Some(self.snapshot.clone());
                state.counts.content_shapes += 1;
            }

            state.paragraph.min_bounds()
        });
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        defaults: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree
            .state
            .downcast_ref::<RevisionTextState<Renderer::Paragraph>>();
        iced::advanced::widget::text::draw(
            renderer,
            defaults,
            layout.bounds(),
            &state.paragraph,
            (self.style)(theme),
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let state = tree
            .state
            .downcast_ref::<RevisionTextState<Renderer::Paragraph>>();
        operation.text(None, layout.bounds(), state.content.as_str());
    }
}

impl<'a> RevisionText<'a> {
    /// Converts this builder into an iced element.
    #[must_use]
    pub fn into_element<Message: 'a>(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<RevisionText<'a>> for Element<'a, Message> {
    fn from(value: RevisionText<'a>) -> Self {
        Element::new(value)
    }
}
