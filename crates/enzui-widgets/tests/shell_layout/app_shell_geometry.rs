//! AppShell paint, collected semantics, and reported geometry checks.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::{Role, Semantics};
use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use enzui_widgets::semantics_container::SemanticsContainer;
use iced::advanced::widget::Operation;
use iced::widget::Space;
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Nav,
}

const W: u32 = 900;
const H: u32 = 600;

include!("app_shell_geometry/pane_contracts.rs");
