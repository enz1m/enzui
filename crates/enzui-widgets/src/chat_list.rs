//! Provider-backed chat transcript with keyed, variable-height virtual rows.
//!
//! Only visible messages plus the configured overscan are fetched; unavailable
//! messages retain empty slots. Each fetched message uses [`crate::chat_bubble`]
//! and exposes list-item semantics with its logical position and total count.
//! Stable [`MessageKey`] values retain row state across sequence mutations.
//! Use [`crate::chat_transcript`] for an eager list of arbitrary child elements.
//!
//! Rows default to 72 logical pixels. [`EnzChatList::row_heights`] accepts a
//! complete height table and rebuilds its geometry in O(total rows) on each
//! render. [`EnzChatList::on_measure`] reports intrinsic heights that disagree
//! with the assigned slots; the caller updates the table with
//! [`reconcile_measurements`] and applies the corrected offset through both
//! [`EnzChatList::scroll_offset`] and `scrollable::scroll_to` using a stable
//! [`Id`]. Reports arrive on redraw and are deduplicated, so dropping one does
//! not cause it to be retried every frame. Rows are clipped to their slots
//! until corrected measurements are applied.
//!
//! [`ChatAnchor`] preserves a stable key and intra-row offset across prepends
//! or removals; resolution fails if the key disappears. [`reveal_offset`]
//! computes an absolute offset for a distant key.
//!
//! Tail following defaults to enabled and releases when the reader scrolls
//! away from the bottom. It controls the pixel offset; the caller still feeds
//! [`EnzChatList::on_scroll`] back into [`EnzChatList::scroll_offset`] to realise
//! the matching rows while navigating history. Without explicit offset
//! feedback, each render realises the tail when following is enabled or the
//! beginning when disabled. Disable [`EnzChatList::follow_tail`] when driving
//! pixel offsets programmatically.

use crate::chat_bubble::{ChatRole as BubbleRole, enz_chat_bubble};
use crate::max_width::max_width;
use crate::row_measure::RowMeasurement;
use crate::semantics_container::SemanticsContainer;
use crate::theme_lookup;
use enzui_core::chat_stream::{ChatMessage, ChatRole, MessageKey, MessageProvider, StreamState};
use enzui_core::semantics::{Role, Semantics, SetPosition};
use enzui_core::virtual_sequence::{Anchor, HeightModel, SequenceGeometry, VariableHeightModel};
use enzui_theme::TypographyScale;
use iced::widget::scrollable::Viewport;
use iced::widget::{Id, container};
use iced::{Background, Element, Length, Padding};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

/// Default estimated complete row-slot height, including inter-bubble space.
pub const DEFAULT_ROW_HEIGHT: f32 = 72.0;
/// Default number of additional rows realised on each side of the viewport.
pub const DEFAULT_OVERSCAN: usize = 5;
/// Readable cap inherited from canonical `EnzChatBubble`.
pub const DEFAULT_BUBBLE_MAX_WIDTH: f32 = 540.0;
/// Transcript-column cap (same as eager `EnzChatTranscript`).
pub const DEFAULT_CONTENT_MAX_WIDTH: f32 = 720.0;
/// Symmetric horizontal row inset, in logical pixels, keeping bubbles clear
/// of the viewport edges without changing row-slot heights.
pub const DEFAULT_ROW_GUTTER_X: f32 = 8.0;

/// Stable scroll id for a named chat list instance.
#[must_use]
pub fn chat_list_id(label: &str) -> Id {
    Id::from(format!("enz-chat-list::{label}"))
}

/// A stable message key plus the viewport's pixel offset within that message.
///
/// This is deliberately keyed, not indexed: a front eviction shifts every
/// surviving index while leaving every surviving domain key unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatAnchor {
    /// The logical message under the viewport anchor.
    pub key: MessageKey,
    /// Pixels from the message's top edge to the viewport top.
    pub offset_in_item: f32,
}

impl ChatAnchor {
    /// Capture the message and intra-row pixel at `scroll_top`.
    ///
    /// Makes one provider lookup at the index selected by [`Anchor::from_scroll_top`].
    /// Returns `None` when the provider has no message there; geometry and
    /// provider indices must describe the same sequence.
    #[must_use]
    pub fn capture(
        provider: &dyn MessageProvider,
        geometry: &(impl SequenceGeometry + ?Sized),
        scroll_top: f32,
    ) -> Option<Self> {
        let anchor = Anchor::from_scroll_top(geometry, scroll_top);
        let message = provider.message(anchor.index() as u64)?;
        Some(Self {
            key: message.key,
            offset_in_item: anchor.offset_in_item(),
        })
    }

    /// Resolve this key against the provider's current sequence and return
    /// the compensated absolute scroll offset.
    ///
    /// This is the front-eviction/prepend compensation operation. `None` means
    /// the key is absent or its current index is outside the geometry.
    /// The result is nonnegative but is not clamped to the scrollable extent.
    #[must_use]
    pub fn resolve(
        &self,
        provider: &dyn MessageProvider,
        geometry: &(impl SequenceGeometry + ?Sized),
    ) -> Option<f32> {
        let index = usize::try_from(provider.index_of_key(&self.key)?).ok()?;
        (index < geometry.len()).then(|| {
            Anchor::new(index, self.offset_in_item)
                .scroll_top(geometry)
                .max(0.0)
        })
    }
}

/// Shared alignment policy for revealing a message.
pub use crate::virtual_scroll::RevealAlignment;

/// Absolute scroll offset that reveals `key`, or `None` when the key is absent.
///
/// Uses [`crate::virtual_scroll::reveal_offset_at`] for alignment, input
/// normalization and clamping. Also returns `None` if the key's index is
/// outside the geometry.
#[must_use]
pub fn reveal_offset(
    provider: &dyn MessageProvider,
    geometry: &(impl SequenceGeometry + ?Sized),
    key: &MessageKey,
    viewport_height: f32,
    current_scroll_top: f32,
    alignment: RevealAlignment,
) -> Option<f32> {
    let index = usize::try_from(provider.index_of_key(key)?).ok()?;
    crate::virtual_scroll::reveal_offset_at(
        geometry,
        index,
        viewport_height,
        current_scroll_top,
        alignment,
    )
}

/// Update row heights and return the offset preserving the current index anchor.
///
/// Delegates to [`crate::virtual_scroll::reconcile_measurements`]. Feed the
/// updated table into [`EnzChatList::row_heights`] and the returned offset into
/// both [`EnzChatList::scroll_offset`] and `scrollable::scroll_to` on the same
/// stable [`Id`].
#[must_use]
pub fn reconcile_measurements(
    model: &mut VariableHeightModel,
    scroll_top: f32,
    measurements: impl IntoIterator<Item = (usize, f32)>,
) -> f32 {
    crate::virtual_scroll::reconcile_measurements(model, scroll_top, measurements)
}

#[derive(Debug, Clone)]
enum Heights {
    Uniform(f32),
    Variable(Vec<f32>),
}

impl Heights {
    fn model(&self, len: usize) -> Geometry {
        match self {
            Heights::Uniform(height) => Geometry::Uniform(HeightModel::new(len, *height)),
            Heights::Variable(heights) if heights.len() == len => {
                Geometry::Variable(VariableHeightModel::new(heights.iter().copied()))
            }
            // A partial table must not truncate the reachable sequence.
            Heights::Variable(_) => Geometry::Uniform(HeightModel::new(len, DEFAULT_ROW_HEIGHT)),
        }
    }
}

enum Geometry {
    Uniform(HeightModel),
    Variable(VariableHeightModel),
}

impl Geometry {
    fn as_sequence(&self) -> &dyn SequenceGeometry {
        match self {
            Geometry::Uniform(model) => model,
            Geometry::Variable(model) => model,
        }
    }
}

fn sane_nonnegative(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        fallback
    }
}

fn sane_positive(value: f32, fallback: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

/// Provider-backed virtualized chat-list builder.
pub struct EnzChatList<'a, Message> {
    provider: &'a dyn MessageProvider,
    heights: Heights,
    viewport_height: f32,
    scroll_offset: Option<f32>,
    overscan: usize,
    width: Length,
    height: Length,
    id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'static>>,
    typography: Option<TypographyScale>,
    bubble_max_width: f32,
    content_max_width: f32,
    show_authors: bool,
    caret_visible: bool,
    row_builder_probe: Option<Rc<Cell<usize>>>,
    follow_tail: bool,
    on_measure: Option<Rc<dyn Fn(RowMeasurement) -> Message>>,
}

/// Construct a virtualized chat list over a procedural/paged provider.
///
/// No message is fetched here. Fetches happen only in
/// [`EnzChatList::into_element`] for the visible + overscan placements.
pub fn enz_chat_list<'a, Message>(provider: &'a dyn MessageProvider) -> EnzChatList<'a, Message> {
    EnzChatList {
        provider,
        heights: Heights::Uniform(DEFAULT_ROW_HEIGHT),
        viewport_height: 600.0,
        scroll_offset: None,
        overscan: DEFAULT_OVERSCAN,
        width: Length::Fill,
        height: Length::Fill,
        id: None,
        on_scroll: None,
        typography: None,
        bubble_max_width: DEFAULT_BUBBLE_MAX_WIDTH,
        content_max_width: DEFAULT_CONTENT_MAX_WIDTH,
        show_authors: true,
        caret_visible: true,
        row_builder_probe: None,
        follow_tail: true,
        on_measure: None,
    }
}

impl<'a, Message> EnzChatList<'a, Message>
where
    Message: Clone + 'static,
{
    /// Use a uniform row-slot height in logical pixels (default 72).
    /// Nonpositive or nonfinite values restore the default.
    #[must_use]
    pub fn row_height(mut self, height: f32) -> Self {
        self.heights = Heights::Uniform(sane_positive(height, DEFAULT_ROW_HEIGHT));
        self
    }

    /// Use caller-owned variable row-slot measurements.
    ///
    /// The table must contain exactly `provider.len()` entries. A mismatch
    /// falls back to the uniform default rather than truncating the logical
    /// sequence. Values are sanitized by [`VariableHeightModel`], whose
    /// geometry is rebuilt in O(total rows) each time the element is built.
    #[must_use]
    pub fn row_heights(mut self, heights: Vec<f32>) -> Self {
        self.heights = Heights::Variable(heights);
        self
    }

    /// Viewport height for realisation, in logical pixels (default 600).
    /// This does not set the outer height. Nonpositive/nonfinite values use 0,
    /// meaning no rows are visible (but the scroll surface remains present).
    #[must_use]
    pub fn viewport_height(mut self, height: f32) -> Self {
        self.viewport_height = sane_positive(height, 0.0);
        self
    }

    /// Current absolute y scroll offset, fed back from [`Self::on_scroll`].
    ///
    /// Without it the list realises the rows it opens on: the newest ones
    /// while it follows the tail, else the first ones. A long history that
    /// the reader scrolls through still needs the offset fed back.
    /// Negative or nonfinite values use zero.
    #[must_use]
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = Some(sane_nonnegative(offset, 0.0));
        self
    }

    /// Extra rows to materialise on each side of a nonempty visible range
    /// (default 5). Large values can realise the whole provider.
    #[must_use]
    pub fn overscan(mut self, overscan: usize) -> Self {
        self.overscan = overscan;
        self
    }

    /// Stable id for scroll operations (jump-to-latest / reveal / compensation).
    #[must_use]
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Observe live scroll viewport changes.
    #[must_use]
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'static) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Report intrinsic row heights that differ from their assigned slots.
    ///
    /// Adds an unbounded-height measurement pass to each realised row.
    /// [`RowMeasurement`] delivery occurs on redraw; reports within 0.5 logical
    /// pixels of the slot or the last published height are suppressed. Feed
    /// reports into [`reconcile_measurements`] and apply both the updated
    /// table and corrected scroll offset. Dropped reports are not retried
    /// until the intrinsic height changes enough to produce a new report.
    #[must_use]
    pub fn on_measure(mut self, f: impl Fn(RowMeasurement) -> Message + 'static) -> Self {
        self.on_measure = Some(Rc::new(f));
        self
    }

    /// Forward the active typography scale into every realised bubble.
    #[must_use]
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }

    /// Bubble width cap in logical pixels (default 540; zero reserves no width).
    /// Negative or nonfinite values restore the default.
    #[must_use]
    pub fn bubble_max_width(mut self, width: f32) -> Self {
        self.bubble_max_width = sane_nonnegative(width, DEFAULT_BUBBLE_MAX_WIDTH);
        self
    }

    /// Transcript column cap in logical pixels (default 720; zero disables it).
    /// Negative or nonfinite values restore the default.
    #[must_use]
    pub fn content_max_width(mut self, width: f32) -> Self {
        self.content_max_width = sane_nonnegative(width, DEFAULT_CONTENT_MAX_WIDTH);
        self
    }

    /// Show optional message authors (default true).
    #[must_use]
    pub fn show_authors(mut self, show: bool) -> Self {
        self.show_authors = show;
        self
    }

    /// Current caller-driven streaming-caret phase (default visible).
    #[must_use]
    pub fn caret_visible(mut self, visible: bool) -> Self {
        self.caret_visible = visible;
        self
    }

    /// Outer width (default [`Length::Fill`]).
    #[must_use]
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Outer height (default [`Length::Fill`]).
    #[must_use]
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Count each message row materialised as an `Element` (test probe).
    #[doc(hidden)]
    #[must_use]
    pub fn row_builder_probe(mut self, probe: Rc<Cell<usize>>) -> Self {
        self.row_builder_probe = Some(probe);
        self
    }

    /// Whether the list opens at the newest message and follows appended
    /// messages (default `true`).
    ///
    /// Released when the reader scrolls up past
    /// [`RELEASE_DISTANCE`](crate::tail_follow::RELEASE_DISTANCE), re-acquired
    /// on return to the bottom. Pass `false` for an archival transcript, or
    /// when the host drives the offset itself via [`Self::id`] + `scroll_to`.
    #[must_use]
    pub fn follow_tail(mut self, follow: bool) -> Self {
        self.follow_tail = follow;
        self
    }

    /// Build the scroll surface, fetching only visible + overscan messages.
    pub fn into_element(self) -> Element<'static, Message> {
        let len = usize::try_from(self.provider.len()).unwrap_or(usize::MAX);
        let geometry = self.heights.model(len);
        let geometry = geometry.as_sequence();
        let total_count = self.provider.len();

        // An undriven window must match the initial pixel pin at the tail.
        let scroll_offset = match self.scroll_offset {
            Some(offset) => offset,
            None if self.follow_tail => (geometry.total_height() - self.viewport_height).max(0.0),
            None => 0.0,
        };
        let content = crate::virtual_rows::realise_keyed_rows(
            geometry,
            self.viewport_height,
            scroll_offset,
            self.overscan,
            crate::virtual_rows::RowChrome {
                padding: Padding::ZERO
                    .left(DEFAULT_ROW_GUTTER_X)
                    .right(DEFAULT_ROW_GUTTER_X),
                on_measure: self.on_measure.clone(),
            },
            |index, _height| {
                let message = self.provider.message(index)?;
                if let Some(probe) = &self.row_builder_probe {
                    probe.set(probe.get().saturating_add(1));
                }
                let key = message.key.as_str().to_owned();
                Some(crate::virtual_rows::RealisedRow {
                    key,
                    content: self.message_element(message, index, total_count),
                })
            },
        );

        let mut surface = crate::scroll_view::enz_scrollable(content)
            .width(self.width)
            .height(self.height);
        if let Some(id) = self.id {
            surface = surface.id(id);
        }
        if let Some(on_scroll) = self.on_scroll {
            surface = surface.on_scroll(move |scroll: iced::widget::scrollable::Scroll| {
                on_scroll(scroll.viewport)
            });
        }

        let surface: Element<'static, Message> = if self.follow_tail {
            crate::tail_follow::enz_tail_follow(surface)
        } else {
            surface.into()
        };
        let surface =
            crate::scroll_view::contain_wheel(surface, crate::scroll_view::Axis::Vertical);

        container(surface)
            .width(self.width)
            .height(self.height)
            .style(|theme: &iced::Theme| container::Style {
                background: Some(Background::Color(theme_lookup::palette(theme).background)),
                ..Default::default()
            })
            .into()
    }

    fn message_element(
        &self,
        message: ChatMessage,
        index: u64,
        total_count: u64,
    ) -> Element<'static, Message> {
        let role = match message.role {
            ChatRole::User => BubbleRole::User,
            ChatRole::Assistant => BubbleRole::Assistant,
            ChatRole::System => BubbleRole::System,
        };
        let role_label = match message.role {
            ChatRole::User => enzui_core::localization::text("enzui-chat-list-user"),
            ChatRole::Assistant => enzui_core::localization::text("enzui-chat-list-assistant"),
            ChatRole::System => enzui_core::localization::text("enzui-chat-list-system"),
        };
        let key = match message.stream {
            StreamState::Complete => "enzui-chat-message",
            StreamState::Streaming => "enzui-chat-message-streaming",
            StreamState::Cancelled => "enzui-chat-message-cancelled",
        };
        let label = enzui_core::localized!(key, "id" => message.key.as_str(), "role" => role_label);
        let description: String = message.text.to_string();
        let mut bubble = enz_chat_bubble(Arc::clone(&message.text), role)
            .show_author(self.show_authors)
            .max_width(self.bubble_max_width);
        if let Some(author) = message.author {
            bubble = bubble.author(author);
        }
        if let Some(stamp) = message.timestamp {
            bubble = bubble.timestamp(stamp);
        }
        if matches!(message.stream, StreamState::Streaming) {
            bubble = bubble.streaming(Arc::clone(&message.text), self.caret_visible);
        }
        if let Some(typography) = self.typography {
            bubble = bubble.typography(typography);
        }

        let bubble: Element<'static, Message> = bubble.into();
        let bounded: Element<'static, Message> = if self.content_max_width > 0.0 {
            let capped = max_width(bubble, self.content_max_width);
            container(capped)
                .width(Length::Fill)
                .center_x(Length::Fill)
                .into()
        } else {
            bubble
        };

        // Collection position augments the bubble's own role and author semantics.
        let semantics = Semantics::new(Role::ListItem)
            .label(label)
            .description(description)
            .set_position(SetPosition {
                index: index.saturating_add(1) as usize,
                count: usize::try_from(total_count).unwrap_or(usize::MAX),
            });
        SemanticsContainer::new(bounded, semantics).into()
    }
}

impl<'a, Message> From<EnzChatList<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'static,
{
    fn from(list: EnzChatList<'a, Message>) -> Self {
        list.into_element()
    }
}
