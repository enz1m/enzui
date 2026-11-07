//! End-to-end keyboard focus-traversal validation.
//!
//! Validation drives the host's widget tree, subscription, update, widget operations, and focus read-back rather than inspecting task shape alone.

use crate::focus::Traversal;
use iced_core::widget::operation::{Operation, Outcome};
use iced_core::widget::{Id, operation::Focusable};
use iced_core::{Element, Rectangle, Size};
use iced_test::futures::Subscription;
use iced_test::futures::subscription::{self, into_recipes};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

/// The application hooks used to validate its production traversal path.
pub trait KeyboardTraversalHost {
    /// The application's message type.
    type Message: std::fmt::Debug + Send + 'static;
    /// The application's theme type (`iced::Theme` for most apps).
    type Theme;

    /// The application's real `subscription()`.
    fn subscription(&self) -> Subscription<Self::Message>;

    /// The application's real `update()`.
    fn update(&mut self, message: Self::Message) -> iced_test::runtime::Task<Self::Message>;

    /// Returns the application's top-level view used for traversal validation.
    fn view(&self) -> Element<'_, Self::Message, Self::Theme, Renderer>;

    /// The window that dispatches the validated input and whose view [`Self::view`] builds.
    ///
    /// The default `None` lets the driver mint one window per press, which is
    /// all a single-window host needs. A host whose `view` depends on a window
    /// id returns that id, so a window-scoped operation the reducer aims at it
    /// is recognised as this window's.
    fn window(&self) -> Option<iced_core::window::Id> {
        None
    }
}

/// Why validation could not reach a verdict, or what it found wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabTraversalError {
    /// The validated view contains too few focusable controls to test movement.
    NotEnoughFocusables {
        /// How many focusable registrations the frame published.
        found: usize,
    },
    /// The widget tree captured Tab before the application subscription received it.
    TreeCapturedTab,
    /// The application subscription produced no message for Tab.
    NoMessageForTab {
        /// How many recipes the subscription had, so a zero can be told from
        /// "several recipes, none of which decode Tab".
        recipes: usize,
    },
    /// The Tab message produced no widget operation.
    NoWidgetOperation {
        /// The message the subscription produced, for the failure text.
        message: String,
    },
    /// The Tab message produced a widget operation aimed at another window.
    OperationInWrongWindow {
        /// The window that dispatched the press.
        window: String,
        /// The windows the produced operations named instead.
        targets: Vec<String>,
    },
    /// The whole pipeline ran and focus did not move.
    FocusDidNotMove {
        /// The focused id before the Tab (`None` = nothing focused).
        before: Option<String>,
        /// The focused id after the Tab.
        after: Option<String>,
    },
    /// Tab and Shift+Tab moved focus in the same direction.
    ShiftTabDidNotReverse {
        /// Where one Tab from the start lands.
        one_tab: Option<String>,
        /// Where Tab, Tab, Shift+Tab lands. Must equal `one_tab`.
        tab_tab_shift_tab: Option<String>,
    },
}

impl std::fmt::Display for TabTraversalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughFocusables { found } => write!(
                f,
                "the validated frame registers {found} focusable control(s); at \
                 least 2 are needed for \"focus moved\" to be a question this \
                 frame can answer. Seed the route you mean before validating — \
                 a validation of an empty universe passes vacuously, which is the \
                 exact shape of gate this module exists to replace."
            ),
            Self::TreeCapturedTab => write!(
                f,
                "the widget tree CAPTURED the Tab key, so the app's \
                 subscription never saw it. That is correct for a presented \
                 modal (it contains focus on purpose) and a defect for a bare \
                 window — validate the bare window."
            ),
            Self::NoMessageForTab { recipes } => write!(
                f,
                "NO KEYBOARD NAVIGATION: the app's own subscription() \
                 ({recipes} recipe(s)) produced no message for a bare Tab \
                 press, so nothing can ever advance the focus ring. Every \
                 focusable widget registers itself and paints a ring, but \
                 registration is not traversal — only the application can run \
                 a focus Operation. Bind it:\n\n\
                 \x20   let focus = iced::keyboard::listen().filter_map(|event| match event {{\n\
                 \x20       iced::keyboard::Event::KeyPressed {{ key, modifiers, .. }} =>\n\
                 \x20           enzui_core::focus::traversal_from_key(&key, modifiers)\n\
                 \x20               .map(Message::FocusTraverse),\n\
                 \x20       _ => None,\n\
                 \x20   }});\n\n\
                 and answer Message::FocusTraverse(direction) in update() with\n\
                 `iced::advanced::widget::operate(\n\
                 \x20   enzui_core::focus::traversal_operation(direction))`.\n\
                 See docs/building-an-app.md -> \"Four things every app that \
                 adopts this shell must get right\", point 4."
            ),
            Self::NoWidgetOperation { message } => write!(
                f,
                "the subscription decoded Tab into {message}, but the update \
                 arm answering it ran NO widget operation. The ring is \
                 advertised and never advanced — the decode is present and \
                 the arm is Task::none() (or returns a non-widget Task). \
                 Answer it with `iced::advanced::widget::operate(\n\
                 \x20   enzui_core::focus::traversal_operation(direction))`."
            ),
            Self::OperationInWrongWindow { window, targets } => write!(
                f,
                "the Tab pressed in window {window} produced a widget \
                 operation aimed at {targets:?}. A traversal must run in the \
                 window that produced the input: a keypress in one window \
                 that advances another window's ring moves focus the user \
                 cannot see."
            ),
            Self::FocusDidNotMove { before, after } => write!(
                f,
                "the whole pipeline ran and focus did NOT move: {before:?} -> \
                 {after:?}. The subscription produced a message and the update \
                 arm ran a widget operation, so the operation is aimed at \
                 something that is not the app's focus ring (a scoped \
                 operation whose scope is empty, or an operation over a \
                 different tree)."
            ),
            Self::ShiftTabDidNotReverse {
                one_tab,
                tab_tab_shift_tab,
            } => write!(
                f,
                "Shift+Tab does not reverse: one Tab lands on {one_tab:?}, \
                 but Tab, Tab, Shift+Tab lands on {tab_tab_shift_tab:?} \
                 instead of the same place. Shift+Tab is wired to the FORWARD \
                 operation, so the ring is one-directional — a user who \
                 overshoots a control can never get back to it without \
                 cycling the whole window. `Task`'s Debug renders \
                 focus_next() and focus_previous() identically, so a reducer \
                 test structurally cannot see this."
            ),
        }
    }
}

/// What validation measured when it succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabTraversalReport {
    /// How many focusable controls the validated frame registered.
    pub focusables: usize,
    /// The focused id after each successive Tab, starting from the validated
    /// frame's initial focus. `None` means nothing was focused.
    pub forward_trail: Vec<Option<String>>,
    /// Where `Tab, Tab, Shift+Tab` landed — must equal `forward_trail[0]`.
    pub after_shift_tab: Option<String>,
}

#[derive(Default)]
struct AllFocusables {
    ids: Vec<Option<Id>>,
}

impl<T> Operation<T> for AllFocusables {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Focusable) {
        self.ids.push(id.cloned());
    }
}

// Include anonymous focusable registrations; iced's `find_focused` skips them.
#[derive(Default)]
struct FocusedSlot {
    index: usize,
    focused: Option<(usize, Option<Id>)>,
}

impl<T> Operation<T> for FocusedSlot {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Focusable) {
        if state.is_focused() {
            self.focused = Some((self.index, id.cloned()));
        }
        self.index += 1;
    }
}

// The fallback renderer is zero-sized, but `UserInterface::build` still requires its mutable binding.
fn headless_renderer() -> Renderer {
    use iced_core::renderer::Headless;
    iced_test::futures::futures::executor::block_on(<Renderer as Headless>::new(
        iced_core::renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer")
}

fn tab_event(shift: bool) -> iced_core::Event {
    let key = iced_core::keyboard::Key::Named(iced_core::keyboard::key::Named::Tab);
    iced_core::Event::Keyboard(iced_core::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced_core::keyboard::key::Physical::Code(iced_core::keyboard::key::Code::Tab),
        location: iced_core::keyboard::Location::Standard,
        modifiers: if shift {
            iced_core::keyboard::Modifiers::SHIFT
        } else {
            iced_core::keyboard::Modifiers::default()
        },
        text: None,
        repeat: false,
    })
}

// Follow `Outcome::Chain` to run multi-phase focus operations to completion.
fn run_to_completion<Message, Theme>(
    ui: &mut UserInterface<'_, Message, Theme, Renderer>,
    renderer: &Renderer,
    operation: Box<dyn Operation>,
) {
    let mut current = Some(operation);
    while let Some(mut op) = current.take() {
        ui.operate(renderer, op.as_mut());
        match op.finish() {
            Outcome::None | Outcome::Some(()) => {}
            Outcome::Chain(next) => current = Some(next),
        }
    }
}

// Drain only ready task actions; collecting can block on senders owned by unrun
// widget operations. Operations aimed at another window are reported rather than
// run: they would advance a ring the validated window does not own.
fn widget_operations<Message>(
    task: iced_test::runtime::Task<Message>,
    window: iced_core::window::Id,
) -> (Vec<Box<dyn Operation>>, Vec<iced_core::window::Id>) {
    let ready = crate::reducer_task::drain_ready(task);
    assert!(
        ready.outcome().is_complete(),
        "keyboard_traversal_validation: more than {} ready actions came out of one update; \
         suspect an unbounded producer rather than a truncated list ({ready:?})",
        crate::reducer_task::MAX_READY_ACTIONS,
    );
    let (mut operations, window_operations) = ready.into_operations_by_scope();
    let mut misdirected = Vec::new();
    for (target, operation) in window_operations {
        if target == window {
            operations.push(operation);
        } else {
            misdirected.push(target);
        }
    }
    (operations, misdirected)
}

// Stop after the first message yielding an operation so later subscription recipes cannot consume process-global resources.
/// What one subscription drive produced: the messages delivered, their
/// names, the operations they queued and the windows they targeted.
type SubscriptionDrive = (
    usize,
    Vec<String>,
    Vec<Box<dyn Operation>>,
    Vec<iced_core::window::Id>,
);

fn drive_subscription<H: KeyboardTraversalHost>(
    host: &mut H,
    event: iced_core::Event,
    status: iced_core::event::Status,
    window: iced_core::window::Id,
) -> SubscriptionDrive {
    use iced_test::futures::futures::StreamExt;
    use std::task::{Context, Poll, Waker};

    let recipes = into_recipes(host.subscription());
    let recipe_count = recipes.len();
    let mut labels = Vec::new();
    let mut ops: Vec<Box<dyn Operation>> = Vec::new();
    let mut misdirected = Vec::new();

    for recipe in recipes {
        let feed = subscription::Event::Interaction {
            window,
            event: event.clone(),
            status,
        };
        let mut stream = recipe.stream(iced_test::futures::futures::stream::iter([feed]).boxed());
        let mut cx = Context::from_waker(Waker::noop());
        // `Pending` and `Ready(None)` end this ready-only drain; any synthetic-input emission has already occurred.
        while let Poll::Ready(Some(message)) = stream.as_mut().poll_next(&mut cx) {
            labels.push(format!("{message:?}"));
            let (operations, wrong_window) = widget_operations(host.update(message), window);
            ops.extend(operations);
            misdirected.extend(wrong_window);
        }
        if !ops.is_empty() {
            break;
        }
    }
    (recipe_count, labels, ops, misdirected)
}

// Shared failure shapes are mapped into each public validator's error vocabulary.
pub(crate) enum PressFailure {
    TreeCaptured,
    NoMessage {
        recipes: usize,
    },
    NoWidgetOperation {
        message: String,
    },
    OperationInWrongWindow {
        window: String,
        targets: Vec<String>,
    },
}

pub(crate) struct PressOutcome {
    pub(crate) messages: Vec<String>,
    pub(crate) operations: usize,
    pub(crate) focused: Option<(usize, Option<Id>)>,
}

// Reuse `cache` across presses because focus state lives in the widget tree.
pub(crate) fn press_event<H: KeyboardTraversalHost>(
    host: &mut H,
    size: Size,
    event: iced_core::Event,
    cache: &mut Option<Cache>,
) -> Result<PressOutcome, PressFailure> {
    let mut renderer = headless_renderer();
    let mut ui = UserInterface::build(
        host.view(),
        size,
        cache.take().unwrap_or_default(),
        &mut renderer,
    );

    let mut messages = iced_core::shell::Bus::new();
    let (_state, statuses) = ui.update(
        &iced_core::window::Headless,
        &iced_core::shell::Waker::noop(),
        std::slice::from_ref(&event),
        iced_core::mouse::Cursor::Unavailable,
        &mut renderer,
        &mut messages,
    );
    let status = if statuses.contains(&iced_core::event::Status::Captured) {
        iced_core::event::Status::Captured
    } else {
        iced_core::event::Status::Ignored
    };
    if status == iced_core::event::Status::Captured {
        *cache = Some(ui.into_cache());
        return Err(PressFailure::TreeCaptured);
    }
    *cache = Some(ui.into_cache());

    let window = host.window().unwrap_or_else(iced_core::window::Id::unique);
    let (recipes, labels, ops, misdirected) = drive_subscription(host, event, status, window);
    if labels.is_empty() {
        return Err(PressFailure::NoMessage { recipes });
    }
    if !misdirected.is_empty() {
        return Err(PressFailure::OperationInWrongWindow {
            window: format!("{window:?}"),
            targets: misdirected
                .iter()
                .map(|target| format!("{target:?}"))
                .collect(),
        });
    }
    if ops.is_empty() {
        return Err(PressFailure::NoWidgetOperation {
            message: labels.join(", "),
        });
    }
    let operations = ops.len();

    let mut renderer = headless_renderer();
    let mut ui = UserInterface::build(
        host.view(),
        size,
        cache.take().unwrap_or_default(),
        &mut renderer,
    );
    for op in ops {
        run_to_completion(&mut ui, &renderer, op);
    }

    let mut slot = FocusedSlot::default();
    ui.operate(&renderer, &mut slot as &mut dyn Operation);
    *cache = Some(ui.into_cache());
    Ok(PressOutcome {
        messages: labels,
        operations,
        focused: slot.focused,
    })
}

fn press_tab<H: KeyboardTraversalHost>(
    host: &mut H,
    size: Size,
    shift: bool,
    cache: &mut Option<Cache>,
) -> Result<Option<(usize, Option<Id>)>, TabTraversalError> {
    match press_event(host, size, tab_event(shift), cache) {
        Ok(outcome) => Ok(outcome.focused),
        Err(PressFailure::TreeCaptured) => Err(TabTraversalError::TreeCapturedTab),
        Err(PressFailure::NoMessage { recipes }) => {
            Err(TabTraversalError::NoMessageForTab { recipes })
        }
        Err(PressFailure::NoWidgetOperation { message }) => {
            Err(TabTraversalError::NoWidgetOperation { message })
        }
        Err(PressFailure::OperationInWrongWindow { window, targets }) => {
            Err(TabTraversalError::OperationInWrongWindow { window, targets })
        }
    }
}

// Compare ring slots: runtime-generated widget ids can change whenever the view is rebuilt.
pub(crate) fn describe_slot(slot: Option<&(usize, Option<Id>)>) -> Option<String> {
    slot.map(|(i, id)| match id {
        // A `Custom` id is author-assigned and stable across rebuilds, so it
        // is safe (and far more readable) to carry into the key.
        Some(id) if format!("{id:?}").contains("Custom") => format!("#{i} {id:?}"),
        Some(_) => format!("#{i} <runtime-assigned id>"),
        None => format!("#{i} <anonymous>"),
    })
}

/// Returns the number of focusable controls in the host's view at `size`.
pub fn focusable_count<H: KeyboardTraversalHost>(host: &H, size: Size) -> usize {
    let _runtime_scope = crate::widget_runtime::WidgetRuntimeScope::new();
    let mut renderer = headless_renderer();
    let mut ui = UserInterface::build(host.view(), size, Cache::default(), &mut renderer);
    let mut all = AllFocusables::default();
    ui.operate(&renderer, &mut all as &mut dyn Operation);
    all.ids.len()
}

/// Validates keyboard traversal and returns the measured focus trail.
///
/// # Errors
///
/// Returns the first [`TabTraversalError`] encountered.
pub fn validate_tab_traversal<H: KeyboardTraversalHost>(
    host: &mut H,
    size: Size,
) -> Result<TabTraversalReport, TabTraversalError> {
    // One owner spans the whole multi-frame traversal so retained focus may
    // bridge a legitimate rebuild, while no prior validation can seed this one.
    let _runtime_scope = crate::widget_runtime::WidgetRuntimeScope::new();
    let focusables = focusable_count(host, size);
    if focusables < 2 {
        return Err(TabTraversalError::NotEnoughFocusables { found: focusables });
    }

    let describe = describe_slot;

    let mut cache = None;
    let mut forward_trail = Vec::new();
    for _ in 0..2 {
        let slot = press_tab(host, size, false, &mut cache)?;
        forward_trail.push(describe(slot.as_ref()));
    }
    if forward_trail[0] == forward_trail[1] {
        return Err(TabTraversalError::FocusDidNotMove {
            before: forward_trail[0].clone(),
            after: forward_trail[1].clone(),
        });
    }

    let back = press_tab(host, size, true, &mut cache)?;
    let after_shift_tab = describe(back.as_ref());
    if after_shift_tab != forward_trail[0] {
        return Err(TabTraversalError::ShiftTabDidNotReverse {
            one_tab: forward_trail[0].clone(),
            tab_tab_shift_tab: after_shift_tab,
        });
    }
    Ok(TabTraversalReport {
        focusables,
        forward_trail,
        after_shift_tab,
    })
}

/// Asserts that Tab and Shift+Tab move focus through the host.
///
/// # Panics
///
/// Panics with the [`TabTraversalError`] when validation fails.
pub fn assert_tab_moves_focus<H: KeyboardTraversalHost>(
    host: &mut H,
    size: Size,
) -> TabTraversalReport {
    match validate_tab_traversal(host, size) {
        Ok(report) => report,
        #[allow(
            clippy::panic,
            reason = "the asserting form of `validate_tab_traversal`, for tests"
        )]
        Err(e) => panic!(
            "keyboard-traversal validation failed at {}x{}:\n\n{e}\n",
            size.width, size.height
        ),
    }
}

/// Returns whether `direction` is forward.
#[must_use]
pub const fn expected_forward(direction: Traversal) -> bool {
    direction.is_forward()
}

pub mod wrap;
