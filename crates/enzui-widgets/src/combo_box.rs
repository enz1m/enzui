//! A labelled selection field with an internally managed popup and highlight.
//!
//! The caller owns the selected value and receives selection messages. Use
//! [`EnzComboBox::selected_index`] and [`EnzComboBox::on_select_index`] to
//! distinguish duplicate labels; a label-only selection marks its first match.
//! The widget tree retains focus, popup state and the keyed highlight across
//! rebuilds. Option identity uses its text plus an occurrence number.
//!
//! Enter, Space, Up or Down opens a focused operable field onto its selected
//! option, or the first/last option when none is selected. Arrows wrap,
//! Home/End jump, Enter commits, and Escape closes while retaining trigger
//! focus. Escape on a closed field remains available to enclosing dialogs.
//! Pointer selection requires a press and release on the same option.
//! An outside press closes the popup and may reach the control behind it.
//!
//! The trigger is the single Tab stop. It publishes ComboBox semantics and an
//! active-descendant relation; realized popup rows publish selected state and
//! their positions in the full list. Disabled or empty fields are inert unless
//! marked read-only, which keeps them focusable. Read-only blocks trigger and
//! keyboard selection; setting it while a popup is already open does not close
//! that popup or disable its pointer selection.
//!
//! The popup matches trigger width and caps its height at [`VISIBLE_ROWS`]
//! rows plus padding, shrinking further when placement requires it. Keyboard
//! highlight changes reveal the row with minimum scrolling. Type-ahead and
//! per-option disabling are unsupported. [`EnzComboBox::virtualized`] limits
//! realized rows using a caller-retained scroll-translation cell.
//!
//! Palette and corner radii follow the runtime theme unless pinned. Trigger
//! size controls fixed height and padding; popup and row radii remain independent.

use enzui_core::context_menu::{MenuEntry, MenuMove, MenuNavigation};
use enzui_core::focus::reveal_axis_offset;
use enzui_core::semantics::{Role, Semantics, SetPosition};
use enzui_theme::Palette;
use iced::advanced::widget::Id;
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Operation, Tree};
use iced::widget::{Space, container, row, text};
use iced::{Background, Border, Color, Element, Length, Padding, Rectangle, Vector};

use crate::enz_text::enz_text;
use crate::text_field::TextFieldSize;

mod builder;
mod overlay;
mod state;
mod style;
mod tokens;
mod widget;
mod window;

pub use builder::*;
pub(crate) use overlay::*;

/// Default namespace for label/placeholder-derived trigger identities.
/// Override with [`EnzComboBox::id_prefix`] to distinguish matching labels.
pub const DEFAULT_ID_PREFIX: &str = "enz-combo";
pub(crate) use state::*;
pub use tokens::*;
pub(crate) use widget::*;
pub(crate) use window::*;
