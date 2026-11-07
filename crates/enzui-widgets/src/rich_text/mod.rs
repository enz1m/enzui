//! Immutable styled text with typed link actions and per-link accessibility.
//!
//! [`RichText`] preserves run fonts, highlights and alignment. Routed views
//! resolve semantic ink from the active theme or a caller-owned token source.
//! [`LinkPresentationTokens`] controls resting, hovered and focused link ink
//! and underlines; link actions, selection and keyboard focus rings remain
//! independent of decoration. Mutable editing belongs to [`crate::rich_edit`].
//!
//! ```
//! use enzui_widgets::rich_text::{LinkPresentationTokens, RichSpan, RichText};
//! let text = RichText::new(vec![RichSpan::link("Source", "src/main.rs")])
//!     .link_presentation(LinkPresentationTokens::QUIET);
//! assert_eq!(text.plain_text(), "Source");
//! ```

mod link;
mod model;
mod presentation;
mod tone;

pub use link::LinkHandlers;
pub use model::ProseSemantics;
pub use model::{
    RangeError, RichSpan, RichText, SPAN_CONVERSIONS, SpanStyle, StyledRange,
    reset_span_conversions, span_conversions,
};
pub use presentation::{LinkPresentationTokens, LinkUnderline};
pub use tone::{ThemedTokens, resolve_tone_key, tone_from_key, tone_key};

use iced::Element;

/// Shaping policy used by every rich-text paragraph.
///
/// Iced shapes span-based paragraphs with Advanced shaping because font,
/// weight, and decoration can change between adjacent runs. Keeping this
/// policy named lets plain [`crate::enz_text::EnzText`] and
/// [`crate::text_measure::MeasureTypography`] select the same descriptor.
pub const RICH_TEXT_SHAPING: crate::enz_text::TextShaping = crate::enz_text::TextShaping::Advanced;

/// Preserve native selection geometry through a transparent rich-text wrapper.
///
/// The native paragraph supplies one complete selectable text region, including
/// its links. `plain_text` only avoids adding a wrapper for blank content; it
/// does not filter the child's text or add an accessibility node.
pub fn enz_prose_semantics<'a, Message: 'a>(
    element: Element<'a, Message>,
    plain_text: String,
) -> Element<'a, Message> {
    ProseSemantics::wrap(element, plain_text)
}
