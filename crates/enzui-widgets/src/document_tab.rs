//! Document-tab cell with single-line title, optional icon or spinner, and close control.
//!
//! Colours follow the live theme unless pinned. Short titles hug content;
//! overflowing titles elide with an end marker by default. Width bounds are
//! normalized and ordered. Bounded height fills the parent offer; `tab_height`
//! supplies the unbounded fallback and close-button geometry.
//!
//! The tab activates on completed primary click or focused Enter/Space. A close
//! handler also accepts a completed middle click on the tab; right-click
//! callbacks receive coordinates local to the tab. Both gestures require a
//! press and release on the same tab and defer to child event capture.
//!
//! [`EnzDocumentTab::embedded`] lets a strip retain one roving focus stop while
//! keeping close controls pointer- and AT-operable. Embedded identities derive
//! from the title by default; stable explicit IDs survive retitling.

use crate::focus_button::{FocusStatus, focus_button_maybe};
use crate::spinner::enz_spinner;
use enzui_core::metrics::{FiniteNonNeg, FinitePositive, OrderedRangeF32};

/// Unbounded height fallback and chrome reference in logical pixels: 32.
pub const DEFAULT_TAB_HEIGHT: f32 = 32.0;
/// Default minimum tab width, in logical pixels.
pub const DEFAULT_TAB_MIN_WIDTH: f32 = 96.0;
/// Default maximum tab width, in logical pixels.
pub const DEFAULT_TAB_MAX_WIDTH: f32 = 240.0;
/// Default leading content inset in logical pixels: 8.
pub const DEFAULT_CONTENT_PADDING_X: f32 = 8.0;
/// Default trailing content inset in logical pixels: 4.
pub const DEFAULT_CONTENT_PADDING_TRAILING: f32 = 4.0;
/// Default favicon box edge, in logical pixels.
pub const DEFAULT_FAVICON_SIZE: f32 = 14.0;
/// Base title size in logical pixels: 13, scaled at construction by the UI text scale.
pub const DEFAULT_TITLE_SIZE: f32 = 13.0;
/// Amount subtracted from the tab height to derive the close-button edge.
pub const DEFAULT_CLOSE_HEIGHT_INSET: f32 = 8.0;
/// Minimum close-button edge, in logical pixels.
pub const DEFAULT_CLOSE_MIN_SIZE: f32 = 16.0;
/// Horizontal gap between document-tab row children.
pub const DEFAULT_ROW_SPACING: f32 = 6.0;

/// Close-button edge produced by the default tab geometry.
#[must_use]
pub const fn default_close_size() -> f32 {
    let derived = DEFAULT_TAB_HEIGHT - DEFAULT_CLOSE_HEIGHT_INSET;
    if derived > DEFAULT_CLOSE_MIN_SIZE {
        derived
    } else {
        DEFAULT_CLOSE_MIN_SIZE
    }
}

/// Width consumed by the default close button and its preceding row gap.
///
/// This is the compensation a variable-width strip gives an active tab when
/// crowded inactive tabs hide their close buttons.
#[must_use]
pub const fn default_close_chrome_width() -> f32 {
    default_close_size() + DEFAULT_ROW_SPACING
}
use crate::theme_lookup;
use enzui_core::semantics::{Role, Semantics, SetPosition};
use iced::widget::button::Style as ButtonStyle;
use iced::widget::text::{Ellipsis, Wrapping};
use iced::widget::{container, row, text};
use iced::{Alignment, Background, Border, Color, Element, Length};

pub(crate) fn favicon_handle(icon: &str, loading: bool) -> Option<iced::widget::svg::Handle> {
    if loading || icon.is_empty() {
        None
    } else {
        Some(crate::icon::handle_for(icon))
    }
}
mod builder;
mod capture;
mod style;

pub use builder::*;
pub(crate) use capture::*;
pub(crate) use style::*;

/// Internal wrapper variants exposed only as a frame-routing test seam.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub enum FrameRoutingFixture {
    TitleReserveCap,
    TitleWidthPin,
    TabMouseCapture,
    TabHeightFill,
}

/// Build one private document-tab wrapper around caller-supplied content.
///
/// This keeps the routing gate on the production wrapper implementations
/// without making their layout-specific fields part of the supported API.
#[doc(hidden)]
pub fn __frame_routing_fixture<'a, Message: Clone + 'a>(
    fixture: FrameRoutingFixture,
    child: iced::Element<'a, Message>,
) -> iced::Element<'a, Message> {
    match fixture {
        FrameRoutingFixture::TitleReserveCap => TitleReserveCap {
            child,
            close_reserve: 0.0,
            ceiling_cap: f32::INFINITY,
        }
        .into(),
        FrameRoutingFixture::TitleWidthPin => TitleWidthPin {
            child,
            title: "routing probe".to_owned(),
            title_size: enzui_theme::text_scale::ui_px(13.0),
            title_font: iced::Font::default(),
            title_cap: f32::INFINITY,
            pinned_width: 800.0,
        }
        .into(),
        FrameRoutingFixture::TabMouseCapture => TabMouseCapture {
            child,
            on_close: None,
            on_context: None,
        }
        .into(),
        FrameRoutingFixture::TabHeightFill => TabHeightFill { child, floor: 1.0 }.into(),
    }
}
