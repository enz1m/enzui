//! Hover tooltips with fading, wrapped text and runtime-theme colours.
//!
//! Uses the theme's elevated surface with primary text, 8×4px padding,
//! the theme's `tooltip` corner radius, a 240px bubble cap and a soft shadow.
//! Colours follow the runtime theme; explicit [`TooltipTokens`] override the appearance.
//!
//! Ordinary hover hints wait 500 ms before fading in. Use a tooltip to name an
//! icon-only control or supply useful extra detail, rather than repeating a
//! visible caption. Named toolbar buttons, window controls, sidebar toggles,
//! icon-only menus and collapsed tabs supply their names automatically. Labelled
//! buttons and tabs need an explicit hint. Empty `.tooltip("")` overrides suppress
//! automatic hints without removing accessible names.
//!
//! Set [`set_default_tooltip_delay`] at application startup to change the delay
//! of subsequently built controls. Their `.tooltip_delay(Duration)` builders
//! override it, including zero for immediate reveal. [`enz_tooltip_with_delay`]
//! configures a general wrapper; raw [`FadeTooltip::new`] retains its immediate
//! default for compatibility and has a [`FadeTooltip::delay`] builder.
//!
//! Hover must remain uninterrupted: leaving, clicking, scrolling, window
//! deactivation and tree cancellation withdraw the hint and cancel its deadline.
//! Reduced motion suppresses animation, not the hover delay. Standalone bubbles
//! used for live chart values or drag feedback do not wait for hover.

use crate::enz_text::enz_text;
use crate::max_width::max_width;
use enzui_core::metrics::{FiniteF32, FiniteNonNeg};
use enzui_core::placement::{Overflow, Placement, Side};
use enzui_theme::Palette;
use iced::animation::Animation;
use iced::time::{Duration, Instant};
use iced::widget::container;
use iced::widget::tooltip::Position;
use iced::{Border, Color, Element, Padding, Shadow, Vector};

mod fade;
mod style;
mod tokens;

pub(crate) use fade::tooltip_is_meaningful;
pub use fade::{
    FadeTooltip, enz_fade_tooltip, enz_tooltip, enz_tooltip_bubble, enz_tooltip_bubble_opacity,
    enz_tooltip_with_delay, enz_tooltip_with_tokens,
};
pub(crate) use style::{bubble_style_opacity, finite_color, scale_alpha};
pub use tokens::{TOOLTIP_FADE, TooltipFade, TooltipTokens};

/// Initial delay for ordinary control hints; reveal animation follows separately.
pub const TOOLTIP_DELAY: Duration = Duration::from_millis(500);

static DEFAULT_DELAY: std::sync::RwLock<Duration> = std::sync::RwLock::new(TOOLTIP_DELAY);

/// Application default for high-level hover hints, initially 500 ms.
/// Raw [`FadeTooltip::new`] retains a zero delay.
pub fn default_tooltip_delay() -> Duration {
    *DEFAULT_DELAY
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Set the application delay for subsequently built hover hints.
/// Per-control overrides take precedence; zero disables waiting, not the tooltip.
pub fn set_default_tooltip_delay(delay: Duration) {
    *DEFAULT_DELAY
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = delay;
}
