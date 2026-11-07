//! Bottom-sheet surfaces and a composed detent controller for
//! [`EnzModalHost`](crate::modal_host::EnzModalHost).
//!
//! Use [`SheetPresenter`] for a resizable sheet. It owns the validated ladder,
//! selected stop, signed direct manipulation, measured viewport and insets,
//! scroll-boundary policy, and theme-driven settle animation. Its view adds one
//! named button grabber; click, touch, Enter, Space, and routed accessibility
//! activation all use the same typed event path.
//!
//! Use [`enz_sheet`] directly for a static surface or when a host deliberately
//! owns custom gesture coordination. The compatibility [`EnzSheet::height`],
//! [`EnzSheet::offset`], [`EnzSheet::on_drag`], [`EnzSheet::show_handle`], and
//! [`SheetDrag`] APIs retain their downward-only behavior.

use crate::swipe::{SwipeAxis, SwipeGesture, SwipePolicy};
use enzui_core::metrics::FinitePositive;
use enzui_core::modal::DismissCause;
use enzui_core::placement::Direction;
use enzui_core::semantics::{Role, Semantics};
use enzui_core::sheet::{DetentLadder, DragPolicy, DragResolution, SheetViewport, resolve_drag};
use enzui_theme::Palette;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::time::Instant;
use iced::widget::{column, container, row, space};
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Size, touch};

mod builder;
mod drag;
mod event;
mod gesture;
mod presenter;
mod tokens;

pub use builder::*;
pub use drag::*;
pub use event::*;
pub(crate) use gesture::*;
pub use presenter::*;
pub use tokens::*;

/// Stack an optional action below content with a 12 px vertical gap.
/// Without an action, return the content directly. The caller owns the action element.
#[must_use]
pub fn sheet_body<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    bottom_action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    match bottom_action {
        None => content.into(),
        Some(action) => column![content.into(), row![action].spacing(8.0)]
            .spacing(12.0)
            .into(),
    }
}
