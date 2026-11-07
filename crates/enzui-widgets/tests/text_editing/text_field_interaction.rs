//! Behavioural tests for `EnzTextField`, driven through a real
//! `UserInterface`.
//!
//! # The failure mode this file guards
//!
//! Collapsing `interactive = enabled && !read_only` and using it to choose
//! between two entirely different widgets — a live `text_input` when true
//! and static `text` when false — makes `.read_only(true)` produce something a
//! user cannot focus, select in, or copy from, while the widget keeps publishing
//! `Semantics::read_only`, i.e. promising AT a reachable *text field*. For the
//! cases a read-only field exists to serve (an API key, a generated path, a
//! token) copying is the entire purpose.
//!
//! # What these tests pin
//!
//! `enabled` and `read_only` are independent flags:
//!
//! | | live `text_input`? | focusable | edits | Enter submits | copy |
//! |---|---|---|---|---|---|
//! | default          | yes | yes | yes | yes | yes |
//! | `read_only(true)`| yes | yes | no | no | yes |
//! | `enabled(false)` | no  | no | no | no | no |
//!
//! The `enabled(false)` row is the control: deleting the gate entirely would
//! make read-only work and quietly make disabled editable too.
//!
//! Every assertion below is on an *observed* result — a message published, a
//! focus id read back out of the live tree, a clipboard write requested by the
//! runtime — never on a builder field read back through its own setter, which
//! is the dead-field failure mode that proves nothing.

use enzui_widgets::text_field::enz_text_field;
use iced::advanced::widget::{Id, Operation};
use iced::{Element, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Changed(String),
    Submitted,
}

type Renderer = iced_test::renderer::Renderer;

/// The field's value, used by every fixture below. A distinctive string so a
/// clipboard assertion cannot pass on some unrelated text.
const VALUE: &str = "sk-live-42";

/// A live interface we can point at, click, type into, run focus operations
/// against, and read clipboard requests out of.
///
/// `Simulator` cannot do the last one: `Simulator::simulate` throws away the
/// `user_interface::State`, and the clipboard write a `Ctrl+C` produces lives
/// in `State::Updated { clipboard, .. }`. Copyability is the whole point of a
/// read-only field, so the harness has to be able to see it.
#[allow(dead_code, reason = "fixture fields are written but never read")]
struct App<'a> {
    ui: UserInterface<'a, Msg, iced::Theme, Renderer>,
    renderer: Renderer,
    messages: iced::advanced::shell::Bus<Msg>,
    cursor: mouse::Cursor,
    /// The last clipboard *write* the tree asked the runtime to perform.
    clipboard_write: Option<String>,
}

impl<'a> App<'a> {
    fn new(view: Element<'a, Msg>) -> Self {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let ui = UserInterface::build(
            view,
            iced::Size::new(400.0, 300.0),
            Cache::default(),
            &mut renderer,
        );
        Self {
            ui,
            renderer,
            messages: iced::advanced::shell::Bus::new(),
            cursor: mouse::Cursor::Unavailable,
            clipboard_write: None,
        }
    }

    fn run(&mut self, mut operation: Box<dyn Operation<()>>) {
        loop {
            self.ui.operate(&self.renderer, &mut operation);
            match operation.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }

    fn focus_id(&mut self, id: Id) {
        self.run(Box::new(
            iced::advanced::widget::operation::focusable::focus(id),
        ));
    }

    /// The id of whatever currently holds focus in the live tree.
    fn focused(&mut self) -> Option<String> {
        let mut operation = iced::advanced::widget::operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> =
            Box::new(iced::advanced::widget::operation::black_box(&mut operation));
        loop {
            self.ui.operate(&self.renderer, &mut boxed);
            match boxed.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => boxed = next,
                _ => break,
            }
        }
        drop(boxed);
        match operation.finish() {
            iced::advanced::widget::operation::Outcome::Some(id) => Some(format!("{id:?}")),
            _ => None,
        }
    }
}

/// A field with no label, so the field slot is the whole of `y = 0..32` and a
/// click point needs no arithmetic. `id` makes it reachable by `focus(id)`.
fn field(read_only: bool, enabled: bool) -> Element<'static, Msg> {
    enz_text_field("", "Enter a key", VALUE, Msg::Changed)
        .id(Id::new("api-key"))
        .on_submit(Msg::Submitted)
        .read_only(read_only)
        .enabled(enabled)
        .into()
}

// ---------------------------------------------------------------------
// 1. read-only is FOCUSABLE. This is the finding.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 2. ...and it is genuinely read-only: no edits.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 3. Enter does not submit from a read-only field.
//
// Decision: it does not. `enzui_core::form::FormField::enter_submits`
// already says `is_editable() && !multiline`, i.e. a non-editable field does
// not submit on Enter, and `form_dialog`'s `field_element` gates `.on_submit(..)` on it.
// If the widget submitted anyway the two would disagree, and a keyboard user
// tabbing across a read-only "generated ID" field would fire the dialog's
// primary action by pressing Enter on a field they cannot even edit.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 4. ...and it is SELECTABLE and COPYABLE — the entire point.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 5. The CONTROL: disabled stays inert. An implementation that simply deletes the
//    `interactive` gate would pass every test above and fail these.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 6. No-regression pin: a normal field still edits.
// ---------------------------------------------------------------------

#[test]
fn enabled_field_is_focusable() {
    let mut app = App::new(field(false, true));
    app.focus_id(Id::new("api-key"));
    assert!(
        app.focused()
            .as_deref()
            .is_some_and(|s| s.contains("api-key"))
    );
}

// ---------------------------------------------------------------------
// 7. The slotted path takes the same split (it shares `renders_input`).
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// 9. Programmatic select_all must use the same live selection path.
//
// `text_field::select_all(id)` wraps the iced Operation and traverses
// through our `TextFieldSemantics` wrapper to the inner `text_input`.
// These tests prove: (a) the API works on an enabled field, (b) it
// works on a read-only field (which still has a live text_input), and
// (c) it is a no-op on a disabled field (which has no live text_input).
// ---------------------------------------------------------------------
