//! Render a keyboard-navigable tree from a host-owned, pre-flattened row list.
//!
//! The host rebuilds rows after expansion changes and handles selection and toggle
//! requests. The tree has one Tab stop; arrows navigate rows without wrapping,
//! Home/End reach the endpoints, Left/Right collapse, expand or move to a parent
//! or child, and Enter/Space activates the cursor row.
//!
//! Set [`EnzTreeView::viewport_height`] to a finite, positive viewport height to
//! realize only visible rows plus overscan. Otherwise rendering is eager. Feed
//! [`EnzTreeView::on_scroll`] back into [`EnzTreeView::scroll_offset`] so the
//! realized window follows scrolling. Logical navigation and sibling-relative
//! accessibility positions still use the full row list.
//!
//! [`EnzTreeView::row_radius`] and [`EnzTreeView::viewport_radius`] control row
//! and scroll-clip rounding independently. Flat transparent trees preserve row
//! corners at rest; painted panels default to their panel clipping radius.
//!
//! Rows publish [`Role::TreeItem`] semantics under a [`Role::Tree`] group. Use
//! stable [`EnzTreeItem::key`] segments and distinct [`EnzTreeView::id_prefix`]
//! values when multiple trees coexist. Accessible names come from item text or
//! [`EnzTreeItem::aria_label`], including when a custom row body is supplied.
//!
//! [`EnzTreeView::row_content`] supplies the body, while this widget owns indentation
//! and disclosure. Row height remains fixed by [`EnzTreeView::tree_row_height`];
//! taller bodies are clipped. Separate focusable actions belong in
//! [`EnzTreeView::row_trailing`]; decorative repeated text can use [`crate::decorative`].
//!
//! Use [`EnzTreeView::row_on_context`] for pointer, keyboard and accessibility
//! context requests. After dismissal, restore the tree cursor with [`focus_tree_row`]
//! or [`FocusRestoreTarget::tree_row_path`](crate::context_menu_host::FocusRestoreTarget::tree_row_path);
//! individual rows are not independent focus stops.
//!
//! Selection and hover follow theme motion; [`EnzTreeView::reduced_motion`] supplies
//! an override. [`EnzTreeView::row_trailing_visibility`] can hide resting action paint
//! while preserving its slot and keyboard/accessibility presence.

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, PressFocus, reveal_axis_offset};
use enzui_core::semantics::{Role, Semantics, SetPosition};
use enzui_core::tree_nav::{TreeAction, TreeKey, TreeRow, sibling_position_of, tree_key_action};
use enzui_core::virtual_sequence::{Anchor, HeightModel, RealizationWindow, SequenceKey};
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::scrollable::Viewport;
use iced::widget::{Space, container, row};
use iced::{Border, Color, Element, Event, Length, Rectangle, Size, Vector, keyboard};
use std::cell::Cell;
use std::rc::Rc;

use crate::enz_text::enz_text;
use crate::list_row_surface::{ListRowSurfaceTokens, enz_list_row_surface_lazy};
use crate::scroll_column::enz_scroll_column;
use crate::tree_disclosure::enz_tree_disclosure_lazy;

mod addressing;
mod builder;
mod item;
mod keyed_virt_column;
mod row_focus;
mod tokens;
mod widget;

#[doc(hidden)]
pub use widget::frame_updates;

pub use addressing::*;
pub use builder::*;
pub use item::*;
pub(crate) use keyed_virt_column::*;
pub use row_focus::*;
pub use tokens::*;
pub(crate) use widget::*;

// ---------------------------------------------------------------------------
// KeyedVirtColumn unit test
// ---------------------------------------------------------------------------
