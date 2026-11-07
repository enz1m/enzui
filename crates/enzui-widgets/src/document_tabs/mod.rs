//! Keyed document-tab strips with a fixed pinned cluster and a scrollable session region.
//!
//! [`EnzDocumentTabs`] uses the application-owned [`DocumentTabs`] model for
//! selection, focus, and stable keys. Construction measures every tab's width;
//! geometry-setting builders can repeat that pass. Element construction builds
//! every pinned tab and the visible session range, including configured overscan.
//!
//! Feed [`on_scroll`](EnzDocumentTabs::on_scroll) offsets back through
//! [`scroll_offset`](EnzDocumentTabs::scroll_offset) so the realized window
//! follows the session scrollbar. [`Orientation`] selects a horizontal strip
//! or vertical rail.
//!
//! Session realization is limited to [`MAX_REALIZED_SESSION`]. Tabs beyond
//! that cap are omitted without an overflow indicator, including on wide
//! viewports with very narrow tabs. Read the
//! [`realized_was_capped`](EnzDocumentTabs::realized_was_capped) handle after
//! building the element to detect truncation; pinned tabs are not capped.

use crate::keyed_row::{keyed_column, keyed_row};
use enzui_core::context_menu::{InvocationKind, Point as MenuPoint};
use enzui_core::document_tabs::{
    DocumentItem, DocumentKey, DocumentTabs, FocusMove, Orientation, StripMetrics,
};
use enzui_core::semantics::{Role, Semantics};
use iced::keyboard;
use iced::widget::scrollable::Viewport;
use iced::widget::{Space, container, scrollable};
use iced::{Element, Length};
use std::cell::Cell;
use std::rc::Rc;

mod builder;
mod config;
mod event;
mod invocation;
mod keys;
mod reveal;
mod view;

pub use builder::*;
pub use config::*;
pub use event::*;
pub use invocation::*;
pub use keys::*;
pub use reveal::*;
