//! [`EnzModalHost`]: modality-owning layers (scrims, background inertness,
//! Escape, unified dismissal routing).
//!
//! # Why a host and not a flag on each dialog
//!
//! Shared hosting keeps scrims, background inertness, keyboard containment,
//! and focus restoration consistent across dialog content.
//!
//! # What this owns, and what it does not
//!
//! Owned: above-background dialog layering, policy modality application
//! (background + keyboard traversal), Escape decoding, dismissal-why
//! reporting. Unowned: dialog contents/buttons/open-ness — application-state
//! openness ([`ModalStack`](enzui_core::modal::ModalStack)): self-visibility-owning
//! dialogs reopen indeterministically + test-undrivably.
//!
//! # Who owns the outgoing subtree
//!
//! Hosts never keep last-passed app `Element`s: iced elements borrow app models
//! single-`view()`-lifetimes; persistent-state storage demands impossible or
//! all-dialogs-`'static`. [`ModalPresentation`] app-held framework state
//! machines retain small presentation models + closing-generation keys; apps
//! rebuild per-view elements from models. Hosts retain RESULTING widget trees
//! while [`motion`](crate::motion::motion) runs, publish single settlement callbacks,
//! structurally remove children after. Generation-receipt-matched model drops;
//! post-reopen stale receipts ignored. Per-app-`Closing(..)`-enum requirements
//! would 10×-duplicate generation/race protocols. Old-element host retention
//! worse: lifetime violations + indefinite borrowed-resource keeping sans later
//! app messages. Clocks to hosts, application data to apps, explicit bounded
//! hand-offs between. Same-three-stack-slots-while-closed hosts (child-zero
//! backgrounds never root/stack-child-shifting per open/close edge): stable
//! positions let [`cached_subtree`](crate::cached_subtree)-wrapped unchanged application
//! contents retain builds/diffs/layouts/widget-states across presentation
//! transitions. Per-frame-renderer-clearing paints still traverse. Decisions
//! unowned: [`dismissal`] means input-event meanings;
//! [`modal`](enzui_core::modal) says presentation-may-end. Hosts ask, never re-derive.
//!
//! # Focus and inertness
//!
//! Beyond focus on open and scrim dismissal, the host contains Tab, restores
//! focus on close, and makes the background inert to assistive technology.
//!
//! # Keep the host mounted
//!
//! While no dialog is open, still wrap the page: `enz_modal_host(page, None,
//! ..)`. Returning the bare page then moves it one level down when a dialog
//! opens, and the whole page's widget state starts over: glass resamples and
//! flashes, scroll offsets and carets reset. Every preset's host
//! (`into_host_with`, `into_scoped_host`) puts the page at the same place.
//!
//! # Motion
//!
//! The surface scales in when a closed host opens and scales out when it is
//! dismissed. A host built already open, such as a dialog restored with its
//! window, shows the surface at once, and reduced motion never travels. The
//! surface is focusable from the opening frame but takes pointer presses
//! only once it has settled.
//!
//! # See also
//!
//! Settings-screen dialog → tabs → preference rows: host dialogs,
//! [`view_switcher`](crate::view_switcher) tabs, [`preferences_group`](crate::preferences_group) pane
//! rows. Whole [`settings_dialog`](crate::settings_dialog)-preset compositions (rarely
//! hand-wired triples). See `docs/building-an-app.md` → *Settings*.

use enzui_core::dismissal::{self, DismissPolicy, DismissRegions, Dismissal};
use enzui_core::modal::DismissCause;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::widget::{container, stack};
use iced::{Color, Element, Length};

/// App-owned modal data retained just long enough for the host's exit motion.
///
/// This modal spelling is a compatibility alias for the generic motion
/// presentation state. Panes and dialogs therefore use the same sound model
/// retention and stale-generation protocol.
pub type ModalPresentation<T> = crate::motion::RetainedPresentation<T>;

/// A generation identifying one modal presentation.
pub type ModalPresentationKey = crate::motion::PresentationKey;

/// The dismissal policy a plain modal dialog uses.
///
/// Named here rather than inlined at each call site so a host can see what it
/// is getting and override it deliberately.
#[must_use]
pub const fn modal_dismiss_policy() -> DismissPolicy {
    DismissPolicy::modal()
}

/// The dismissal policy an alert that must be answered uses: neither Escape
/// nor the scrim ends it, so the only exits are the surface's own buttons.
#[must_use]
pub const fn alert_dismiss_policy() -> DismissPolicy {
    DismissPolicy::insistent()
}

/// The space a centred dialog keeps from the window edge, in logical
/// pixels: a dialog wider than the window shrinks to leave it.
pub const DIALOG_WINDOW_MARGIN: f32 = 16.0;

/// The scrim colour for a palette.
///
/// A free function rather than the body of the style closure below, because
/// a style closure is unreachable from a unit test — the lesson already
/// learned three times in the date family (`calendar::resolve_tokens`,
/// `date_field::elevated_calendar_tokens`, `date_range_picker::popup_range`).
pub fn scrim_color(palette: &Palette) -> Color {
    palette.overlay
}

/// Resolve the scrim colour a host paints: an overridden colour as given,
/// otherwise the theme's [`scrim_color`].
/// Single render-closure + [`resolved_scrim`](EnzModalHost::resolved_scrim)-called
/// functions: painted-vs-queried undriftable (two-place same-thing decisions
/// are uncatchable mutations).
pub fn resolve_scrim(pinned: Option<Color>, palette: &Palette) -> Color {
    match pinned {
        Some(pinned) => pinned,
        None => scrim_color(palette),
    }
}

/// Where the presented surface sits inside the host: centred for dialogs,
/// attached to an edge for sheets. Placement is a property of the
/// presentation, not the surface widget, because only the host owns the
/// space; a sheet that bottom-aligned itself would still be centred by
/// whatever host wraps it. This matters for drags: a sheet that expressed
/// a drag as a growing spacer above it would, once centred, absorb half the
/// growth below and move at half the pointer's rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SurfacePlacement {
    /// Centred on both axes. The right default for a dialog.
    #[default]
    Centred,
    /// Flush against the bottom edge, centred horizontally. What an
    /// edge-attached sheet needs so a drag translates 1:1.
    BottomEdge,
}

impl SurfacePlacement {
    /// Vertical alignment this placement implies.
    ///
    /// A free function on the enum rather than a `match` inside the render
    /// path, so the mapping is reachable from a unit test — the render path
    /// is not.
    #[must_use]
    pub const fn vertical_alignment(self) -> iced::alignment::Vertical {
        match self {
            Self::Centred => iced::alignment::Vertical::Center,
            Self::BottomEdge => iced::alignment::Vertical::Bottom,
        }
    }
}

/// What the host reports. Proposals, never commands: the application decides
/// what to do with them, exactly as with every other controlled widget here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModalHostEvent {
    /// The user asked to dismiss, and the cause says how.
    Dismissed(DismissCause),
    /// Modal Tab: move focus to the next control, wrapping at the end.
    ///
    /// Reported rather than performed because moving focus is a runtime
    /// `Task`, which a widget cannot issue — see [`focus_wrap_operation`].
    FocusNext,
    /// Modal Shift+Tab: move focus to the previous control, wrapping at the start.
    FocusPrevious,
}

/// Decode a key press for an open modal. Escape + Tab only. Deliberately
/// [`navigation_from_key`](enzui_core::focus::navigation_from_key)-unbuilt (arrow-key-
/// collapsing increment/decrement vocabularies dialog-don't-have). Everything-
/// else-`None` returns keep dialog-own-content keys. Host-claimed Tabs:
/// focus-CONTAINING modals (stock traversals dialog-end-fall into backgrounds:
/// exactly closed leaks).
#[must_use]
pub fn event_for_key(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<ModalHostEvent> {
    match key {
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
            Some(ModalHostEvent::Dismissed(DismissCause::Escape))
        }
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab) => {
            // Only bare Tab and Shift+Tab are modal focus traversal.
            // Ctrl/Alt/Logo+Tab (and their +Shift chords) are application-
            // level shortcuts (document cycling, window switching) that the
            // host must not capture.  The core contract
            // (focus::traversal_from_key) reserves the same two-key set.
            if modifiers.control() || modifiers.alt() || modifiers.logo() {
                return None;
            }
            Some(if modifiers.shift() {
                ModalHostEvent::FocusPrevious
            } else {
                ModalHostEvent::FocusNext
            })
        }
        _ => None,
    }
}

/// The focus operation a [`ModalHostEvent`] focus event calls for.
///
/// The caller runs the returned `Operation` with `iced::widget::operate`;
/// widgets issue no `Task`s, so it is not performed here. It wraps within
/// the modal scope, so Tab cannot escape the scrim, using the same policy
/// as ordinary window traversal
/// ([`traversal_operation`](enzui_core::focus::traversal_operation));
/// only the scope it runs against differs. Keep the host's global Tab arms
/// gated while a modal is present, or each press advances the ring twice.
/// [`Dismissed`](ModalHostEvent::Dismissed) returns `None`, so every host
/// event can go through one call.
#[must_use]
pub fn focus_wrap_operation<T>(
    event: ModalHostEvent,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
where
    T: Send + 'static,
{
    match event {
        ModalHostEvent::FocusNext => Some(enzui_core::focus::traversal_operation(
            enzui_core::focus::Traversal::Next,
        )),
        ModalHostEvent::FocusPrevious => Some(enzui_core::focus::traversal_operation(
            enzui_core::focus::Traversal::Previous,
        )),
        ModalHostEvent::Dismissed(_) => None,
    }
}

/// The `Id` a presentation's surface container
/// carries, derived from the caller's key. Single render-path + operation-path
/// derivation functions: scope-id mutations otherwise silent (id-less-widget-
/// scoped operations match nothing: correctly-declined-covered-scope-
/// indistinguishable).
#[must_use]
pub fn presentation_scope_id(key: &str) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("modal-scope-{key}"))
}

/// The focus operation a [`ModalHostEvent`] calls for, **scoped to the
/// presentation that must receive it**.
///
/// # Why a scope id and not an `is_topmost: bool`
///
/// Ordering fixes. `update`-time-sampled `bool`s evaluate during-`update`
/// states. Iced drains whole message batches first (`winit/src/lib.rs`),
/// rebuilds all user interfaces (`:1253`), then runs `Action::Widget`
/// (`:1260`): traversed trees may no longer exist — batched presentation or
/// dismissal Tabs cycle whichever-scope-renders-after focuses. Traversal-time-
/// moved scoping decisions: `operation::scope` descends inner operations only
/// at `target`-carrying containers
/// (`iced/core/src/widget/operation.rs`); REBUILT trees render
/// topmost-presentation-only surfaces. Covered scopes structurally decline
/// (treeless ids match nothing). Topmost-earlier-remembering nothing needed:
/// batch immunity. [`Dismissed`](ModalHostEvent::Dismissed)-`None` returns
/// (non-focus-moves).
#[must_use]
pub fn scoped_focus_operation<T>(
    event: ModalHostEvent,
    scope: iced::advanced::widget::Id,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
where
    T: Send + 'static,
{
    let inner = focus_wrap_operation::<T>(event)?;
    Some(Box::new(iced::advanced::widget::operation::scope(
        scope, inner,
    )))
}

/// The focus operation that returns focus to the invoker after a modal is
/// dismissed. Run it with `iced::widget::operate`.
///
/// Returns `None` for a non-dismissal (see [`scoped_focus_operation`]), for a
/// pointer dismissal by scrim or drag (focus stays where the pointer left
/// it), and when no invoker was recorded. If the invoker has been removed,
/// focus stays where scoped traversal put it. [`initial_focus_operation`] is
/// the counterpart for opening.
#[must_use]
pub fn restore_focus_operation<T>(
    event: ModalHostEvent,
    invoker: Option<&iced::advanced::widget::Id>,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
where
    T: Send + 'static,
{
    let cause = match event {
        ModalHostEvent::Dismissed(cause) => cause,
        ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious => return None,
    };
    if cause.is_pointer_ambient() {
        return None;
    }
    let invoker = invoker?.clone();
    Some(Box::new(enzui_core::focus::RestoreFocus::new(invoker)))
}

/// The focus operation that moves focus to the first focusable control in a
/// freshly opened modal; with none, it lands on the Close button. It is
/// scoped to the presentation, so a covered or rebuilt one declines.
#[must_use]
pub fn initial_focus_operation<T>(
    scope: iced::advanced::widget::Id,
) -> Box<dyn iced::advanced::widget::Operation<T>>
where
    T: Send + 'static,
{
    let inner = Box::new(enzui_core::focus::focus_next_wrapping::<T>())
        as Box<dyn iced::advanced::widget::Operation<T>>;
    Box::new(iced::advanced::widget::operation::scope(scope, inner))
}

/// The focus lifecycle of one modal presentation, owned by the host.
///
/// Created at open with the invoker and scope, it answers every
/// [`ModalHostEvent`]: [`open`](ModalFocusLifecycle::open) moves focus into
/// the dialog; `FocusNext` and `FocusPrevious` traverse only this
/// presentation (the topmost, when modals nest); a keyboard or button
/// `Dismissed` returns focus to the invoker, while a scrim or drag dismissal
/// leaves it; [`close_for_host`](ModalFocusLifecycle::close_for_host) settles
/// focus in place when the host closes the modal itself. Route every event
/// through [`reduce`](ModalFocusLifecycle::reduce). [`ModalPresentation`]
/// composes one; the low-level operations remain for custom hosts.
#[derive(Debug, Clone)]
pub struct ModalFocusLifecycle {
    scope: iced::advanced::widget::Id,
    invoker: Option<iced::advanced::widget::Id>,
}

impl ModalFocusLifecycle {
    /// Open a presentation: capture the invoker, derive the scope from `key`,
    /// and return the lifecycle to store together with the initial-focus
    /// operation to run now.
    ///
    /// `invoker` is the id of the control that opened the dialog — the one
    /// focus returns to on dismissal. `None` for a dialog with no tracked
    /// invoker (a startup sheet), in which case dismissal restores nothing.
    #[must_use]
    pub fn open<T>(
        key: &str,
        invoker: Option<iced::advanced::widget::Id>,
    ) -> (Self, Box<dyn iced::advanced::widget::Operation<T>>)
    where
        T: Send + 'static,
    {
        let scope = presentation_scope_id(key);
        let op = initial_focus_operation::<T>(scope.clone());
        (Self { scope, invoker }, op)
    }

    /// Open a presentation whose initial focus belongs on a named control
    /// rather than on its first authored focusable, as [`Self::open`] does.
    ///
    /// The operation stays scoped to the presentation, so it declines when the
    /// surface is covered or rebuilt, exactly as [`initial_focus_operation`]
    /// does. A `target` that is not mounted focuses nothing rather than falling
    /// through to another control.
    pub fn open_at<T>(
        key: &str,
        invoker: Option<iced::advanced::widget::Id>,
        target: iced::advanced::widget::Id,
    ) -> (Self, Box<dyn iced::advanced::widget::Operation<T>>)
    where
        T: Send + 'static,
    {
        let scope = presentation_scope_id(key);
        let inner = Box::new(iced::advanced::widget::operation::focusable::focus(target))
            as Box<dyn iced::advanced::widget::Operation<T>>;
        let op = Box::new(iced::advanced::widget::operation::scope(
            scope.clone(),
            inner,
        ));
        (Self { scope, invoker }, op)
    }

    /// Reconstruct a lifecycle from an already-derived scope id and invoker,
    /// for a caller that opened the presentation itself and does not want the
    /// initial-focus operation (or already ran it).
    #[must_use]
    pub fn with_scope(
        scope: iced::advanced::widget::Id,
        invoker: Option<iced::advanced::widget::Id>,
    ) -> Self {
        Self { scope, invoker }
    }

    /// The presentation scope this lifecycle scopes its focus work to.
    #[must_use]
    pub fn scope(&self) -> &iced::advanced::widget::Id {
        &self.scope
    }

    /// The retained invoker, if any.
    #[must_use]
    pub fn invoker(&self) -> Option<&iced::advanced::widget::Id> {
        self.invoker.as_ref()
    }

    /// Apply a host event, returning the framework-owned focus work and whether
    /// the event logically dismissed the modal.
    ///
    /// `FocusNext`/`FocusPrevious` produce a traversal **scoped to this
    /// presentation** — never the unscoped app traversal that leaks behind the
    /// scrim. `Dismissed(cause)` produces the invoker restoration and reports
    /// the cause so the caller can tear down its own presentation state.
    pub fn reduce<T>(&self, event: ModalHostEvent) -> ModalFocusOutcome<T>
    where
        T: Send + 'static,
    {
        match event {
            ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious => ModalFocusOutcome {
                dismissed: None,
                operation: scoped_focus_operation::<T>(event, self.scope.clone()),
            },
            ModalHostEvent::Dismissed(cause) => ModalFocusOutcome {
                dismissed: Some(cause),
                operation: restore_focus_operation::<T>(event, self.invoker.as_ref()),
            },
        }
    }

    /// The outcome for a host-driven or model-unavailable closure: the modal is
    /// gone but focus stays exactly where it is.
    ///
    /// This is the deliberate non-restoring path. No [`ModalHostEvent`] is
    /// involved because no user input dismissed the modal — the backing model
    /// vanished (an item deleted out from under its own dialog), so there is no
    /// invoker to honour and stealing focus would be wrong.
    pub fn close_for_host<T>(&self) -> ModalFocusOutcome<T>
    where
        T: Send + 'static,
    {
        ModalFocusOutcome {
            dismissed: None,
            operation: None,
        }
    }
}

/// The result of reducing one [`ModalHostEvent`] through a
/// [`ModalFocusLifecycle`]: the framework-owned focus operation to run and
/// whether the modal was logically dismissed.
///
/// `#[must_use]` because dropping it silently discards both the focus move and
/// the dismissal signal — the exact silent omission this lifecycle exists to
/// prevent.
#[must_use]
pub struct ModalFocusOutcome<T> {
    dismissed: Option<DismissCause>,
    operation: Option<Box<dyn iced::advanced::widget::Operation<T>>>,
}

impl<T> ModalFocusOutcome<T>
where
    T: Send + 'static,
{
    /// The dismissal cause when this event logically closed the modal, else
    /// `None` (a focus traversal). The caller tears down its own presentation
    /// state when this is `Some`.
    #[must_use]
    pub fn dismissed(&self) -> Option<DismissCause> {
        self.dismissed
    }

    /// Take the focus operation, if any, to hand to `iced::widget::operate`.
    #[must_use]
    pub fn into_operation(self) -> Option<Box<dyn iced::advanced::widget::Operation<T>>> {
        self.operation
    }

    /// Convert the outcome into the `Task` to return from `update`: the focus
    /// operation issued through `iced::widget::operate`, or `Task::none` when
    /// there is nothing to run.
    pub fn into_task(self) -> iced::Task<T> {
        match self.operation {
            Some(op) => iced::advanced::widget::operate(op),
            None => iced::Task::none(),
        }
    }
}

/// Whether a policy lets Escape close this presentation, and the event to
/// report if so. Key-DECODING vs governing-POLICY-split ([`event_for_key`]):
/// insistent alerts still need Escape RECOGNITION (background-unreaching
/// swallows) without closing anything.
#[must_use]
pub fn escape_outcome(policy: DismissPolicy) -> Dismissal {
    dismissal::on_escape(policy)
}

/// The dismissal decision for a press at `point`, given where the surface is.
///
/// Wraps [`dismissal::on_press`] with the region set a modal has: the surface
/// only. A modal has no trigger region — the control that opened it is behind
/// the scrim and inert, so a press there is an outside press like any other.
#[must_use]
pub fn press_outcome(
    policy: DismissPolicy,
    surface: iced::Rectangle,
    point: iced::Point,
) -> Dismissal {
    dismissal::on_press(policy, &DismissRegions::new(surface), point)
}

/// Semantics for a dialog surface.
///
/// A free function so the role/modality mapping is testable without building
/// an element, and so the host and any preset dialog cannot describe the same
/// dialog differently.
pub fn dialog_semantics(label: Option<&str>, alert: bool, policy: DismissPolicy) -> Semantics {
    let role = if alert {
        Role::AlertDialog
    } else {
        Role::Dialog
    };
    Semantics::new(role)
        .label_opt(label)
        // Modality is read from the policy rather than passed separately, so
        // a dialog that shields input cannot advertise itself to AT as
        // non-modal (or the reverse).
        .modal(policy.modality.consumes_outside_input())
}

/// Whether an event is a scroll or paging gesture the modal must own, so it
/// cannot reach anything behind or beside the overlay: wheel, touch drag, and
/// the paging keys of [`ScrollKey`](crate::scroll_view::ScrollKey). Modifier
/// chords and pointer presses are not included; outside-click dismissal and
/// press swallowing handle presses.
fn is_owned_scroll_gesture(event: &iced::Event) -> bool {
    match event {
        iced::Event::Mouse(iced::mouse::Event::WheelScrolled { .. }) => true,
        iced::Event::Touch(iced::touch::Event::FingerMoved { .. }) => true,
        iced::Event::Keyboard(_) => crate::scroll_view::ScrollKey::from_event(event).is_some(),
        _ => false,
    }
}

/// Wraps the scrim and captures any scroll or paging gesture that reaches it,
/// so the page behind a modal never scrolls. An inert background is not
/// enough: scroll regions beside or in front of the host would still receive
/// it. The scrim is below the surface, so the dialog's own scrollable body
/// gets the gesture first; children receive events before the capture, so
/// outside-click dismissal still works. Non-modal popovers draw no scrim.
struct ModalScrollBarrier<'a, Message> {
    child: Element<'a, Message>,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for ModalScrollBarrier<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        // The scrim child runs first so outside-click dismissal keeps its
        // press/release, and (higher in the stack) the dialog's own scrollable
        // body already had first refusal on this gesture.
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if shell.is_event_captured() {
            return;
        }

        // A scroll/paging gesture that reached the scrim was declined by
        // everything above it: swallow it so it cannot reach a scroll region
        // behind or around the modal. The modality contract is exactly
        // "everything outside this surface is inert while it is open", and an
        // uncaptured scroll is the one hole left in it.
        if is_owned_scroll_gesture(event) {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

// Routes Escape for every active presentation and contains Tab only when the
// policy makes the background inert. The child receives each event first so an
// open control can consume Escape before the presentation handles it.
struct EscapeCapture<'a, Message> {
    child: Element<'a, Message>,
    active: bool,
    contain_focus: bool,
    outcome: Dismissal,
    on_escape: std::rc::Rc<dyn Fn() -> Message + 'a>,
    // A modal presentation still captures Tab when these publishers are absent;
    // a non-modal presentation leaves Tab to ordinary application traversal.
    on_focus_next: Option<std::rc::Rc<dyn Fn() -> Message + 'a>>,
    on_focus_previous: Option<std::rc::Rc<dyn Fn() -> Message + 'a>>,
    // Set while diffing: whether the host was closed on the previous view,
    // so a surface layer mounting now is an open and scales in.
    opening: std::rc::Rc<std::cell::Cell<bool>>,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for EscapeCapture<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        // This is the stack's tree: a closed host holds only the background,
        // and a tree built this frame holds nothing yet, so a dialog restored
        // with the window appears without motion.
        self.opening.set(tree.children.len() == 1);
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        // The child goes first, so a control inside the dialog that owns
        // Escape (an open combo box list) keeps it.
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if shell.is_event_captured() || !self.active {
            return;
        }

        let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event
        else {
            return;
        };

        let Some(decoded) = event_for_key(key, *modifiers) else {
            return;
        };

        match decoded {
            ModalHostEvent::FocusNext if self.contain_focus => {
                if let Some(publish) = self.on_focus_next.as_ref() {
                    shell.publish(publish());
                }
                shell.capture_event();
            }
            ModalHostEvent::FocusPrevious if self.contain_focus => {
                if let Some(publish) = self.on_focus_previous.as_ref() {
                    shell.publish(publish());
                }
                shell.capture_event();
            }
            ModalHostEvent::FocusNext | ModalHostEvent::FocusPrevious => {}
            ModalHostEvent::Dismissed(_) => {
                if self.outcome.closes() {
                    shell.publish((self.on_escape)());
                }
                if self.outcome.consumes() {
                    shell.capture_event();
                }
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

/// Releases focus held inside the background when a presentation makes it
/// inert, since the background keeps its identity and focus state across the
/// open. It runs in `layout` (inert subtrees are hidden from operations, and
/// this is before any press is handled) and only on the edge into inertness,
/// so an open sheet does not walk the subtree every frame.
struct WithdrawFocus<'a, Message> {
    child: Element<'a, Message>,
    withdrawn: bool,
}

/// The previous withdrawal state, so the release runs once per edge.
#[derive(Debug, Default)]
struct WithdrawFocusState {
    withdrawn: bool,
}

impl<'a, Message> WithdrawFocus<'a, Message> {
    fn new(child: Element<'a, Message>, withdrawn: bool) -> Self {
        Self { child, withdrawn }
    }
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for WithdrawFocus<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<WithdrawFocusState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(WithdrawFocusState::default())
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        // `Tree::new` does not build children; only `Tree::diff` does. Seed the
        // slot rather than indexing into an empty vector if a caller lays out
        // a freshly created tree.
        if tree.children.is_empty() {
            tree.diff_children(std::slice::from_mut(&mut self.child));
        }
        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);

        let entering = {
            let state = tree.state.downcast_mut::<WithdrawFocusState>();
            let entering = self.withdrawn && !state.withdrawn;
            state.withdrawn = self.withdrawn;
            entering
        };

        if entering {
            // Only the focus flag is touched: scroll offsets, text cursors,
            // selections and in-flight gestures behind the dialog are exactly
            // what stable slots are retaining, and clearing them here would be
            // a second, worse bug.
            let mut release = enzui_core::focus::release_all_focus();
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                iced::advanced::Layout::of(&node),
                &iced::advanced::Layout::of(&node).bounds(),
                renderer,
                &mut iced::advanced::widget::operation::black_box(&mut release),
            );
        }

        iced::advanced::layout::Node::new(node.size()).apply(tree);
    }

    // Tag-opaque single-child routing: this wrapper owns its own tree node, so
    // the background lives at `children[0]` and `Tree::new` may ask for
    // interest before `diff` has seeded it.
    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<WithdrawFocus<'a, Message>> for Element<'a, Message> {
    fn from(value: WithdrawFocus<'a, Message>) -> Self {
        Self::new(value)
    }
}

/// Hosts a dialog above an application background.
///
/// The background is always present in the tree — wrapped in
/// [`crate::inert::Inert`] rather than replaced — so opening a dialog does
/// not reset scroll offsets, text cursors or gestures behind it.
pub struct EnzModalHost<'a, Message> {
    background: Element<'a, Message>,
    dialog: Option<Element<'a, Message>>,
    on_event: std::rc::Rc<dyn Fn(ModalHostEvent) -> Message + 'a>,
    policy: DismissPolicy,
    label: Option<String>,
    alert: bool,
    scrim: Option<Color>,
    placement: SurfacePlacement,
    /// Active modality target. A retained outgoing dialog sets this false:
    /// it may still draw, but it is immediately absent from input, focus and AT.
    presentation_open: bool,
    /// Lazy completion publisher for the retained outgoing generation.
    on_exit_settled: Option<std::rc::Rc<dyn Fn() -> Message + 'a>>,
    /// The id this presentation's surface container carries, so a focus
    /// operation can be aimed at *this* scope and decline at traversal time
    /// if it is no longer the one rendered.
    scope: Option<iced::advanced::widget::Id>,
    // Modal Tab remains captured when these events are disabled; non-modal Tab
    // is always left to application traversal.
    emit_focus_events: bool,
}

/// `background`, hosted by `host` with `dialog` over it when there is one,
/// and in an empty host otherwise, so the page sits at the same place in the
/// widget tree whether or not a dialog shows.
///
/// Wrapping the page in a host only while a dialog is open moves it on every
/// open and close, and all of its widget state starts over: a scrolled page
/// jumps back to the top as the dialog appears. The dialogs' own
/// `*_dialog_host` functions call this with their `into_host`; `closed`
/// maps the events of the empty host, which publishes none in practice.
pub fn host_when_shown<'a, Message: Clone + 'a, D>(
    background: impl Into<Element<'a, Message>>,
    dialog: Option<D>,
    host: impl FnOnce(D, Element<'a, Message>) -> Element<'a, Message>,
    closed: impl Fn(ModalHostEvent) -> Message + 'a,
) -> Element<'a, Message> {
    let background = background.into();
    match dialog {
        Some(dialog) => host(dialog, background),
        None => enz_modal_host(background, None, closed).into_element(),
    }
}

/// Host `dialog` (when present) above `background`.
pub fn enz_modal_host<'a, Message>(
    background: impl Into<Element<'a, Message>>,
    dialog: Option<Element<'a, Message>>,
    on_event: impl Fn(ModalHostEvent) -> Message + 'a,
) -> EnzModalHost<'a, Message> {
    let presentation_open = dialog.is_some();
    EnzModalHost {
        background: background.into(),
        dialog,
        on_event: std::rc::Rc::new(on_event),
        policy: modal_dismiss_policy(),
        label: None,
        alert: false,
        scrim: None,
        placement: SurfacePlacement::Centred,
        presentation_open,
        on_exit_settled: None,
        scope: None,
        emit_focus_events: true,
    }
}

impl<'a, Message> EnzModalHost<'a, Message> {
    /// Stop publishing `FocusNext`/`FocusPrevious` on modal Tab presses; the
    /// Tab is captured and does nothing. For simple card dialogs that map
    /// every host event to close and cannot run an operation. Non-modal Tabs
    /// still reach ordinary application traversal. Richer modal callers leave
    /// this enabled and pair the events with [`scoped_focus_operation`] so Tab
    /// cycles inside the presentation.
    #[must_use]
    pub const fn no_focus_events(mut self) -> Self {
        self.emit_focus_events = false;
        self
    }
    /// Tags this presentation's surface with a scope id for
    /// [`scoped_focus_operation`]. Only the topmost presentation is in the
    /// tree, so operations aimed at a covered scope traverse nothing. A host
    /// with one dialog does not need it.
    #[must_use]
    pub fn scope(mut self, scope: iced::advanced::widget::Id) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Tag this presentation with the scope its [`ModalFocusLifecycle`] owns.
    /// Lifecycle-bound presentation APIs: host-surface scopes == every
    /// [`reduce`](ModalFocusLifecycle::reduce)-operation-aimed ids (or container-
    /// tree-unshared silently-moving-nothing Tab traversals + covered-scope
    /// arbitrations). Low-level [`scope`](Self::scope)s leave caller re-derived
    /// [`presentation_scope_id`]-identical host+lifecycle agreements ("two-
    /// place same-key" drifts removed here). Lifecycle-passing hosts read
    /// open-time-captured single-source ids.
    ///
    /// ```ignore
    /// let (focus, open_op) = ModalFocusLifecycle::open("settings", Some(invoker));
    /// let host = enz_modal_host(page, Some(dialog), Msg::Host).scoped_to(&focus);
    /// // host.scope == focus.scope(), by construction, from one source.
    /// ```
    #[must_use]
    pub fn scoped_to(self, lifecycle: &ModalFocusLifecycle) -> Self {
        self.scope(lifecycle.scope().clone())
    }

    /// Set whether the retained dialog is an active presentation.
    ///
    /// Set this to `false` immediately on dismissal while continuing to pass
    /// the element rebuilt from [`ModalPresentation::value`]. The outgoing
    /// surface then animates visually but is inert, absent from focus and AT,
    /// does not capture Escape, and no longer makes the background inert.
    #[must_use]
    pub const fn presentation_open(mut self, open: bool) -> Self {
        self.presentation_open = open;
        self
    }

    /// Publish once when a visible-to-hidden exit has settled.
    ///
    /// Capture the current [`ModalPresentationKey`] in this callback and pass
    /// it to [`ModalPresentation::finish_exit`]. The generation check makes a
    /// completion queued just before a reopen harmless.
    #[must_use]
    pub fn on_exit_settled_with(mut self, callback: impl Fn() -> Message + 'a) -> Self {
        self.on_exit_settled = Some(std::rc::Rc::new(callback));
        self
    }

    /// Use a specific dismissal policy (e.g. [`alert_dismiss_policy`]).
    #[must_use]
    pub const fn policy(mut self, policy: DismissPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Where the surface sits. Defaults to [`SurfacePlacement::Centred`];
    /// an edge-attached sheet wants [`SurfacePlacement::BottomEdge`].
    #[must_use]
    pub const fn placement(mut self, placement: SurfacePlacement) -> Self {
        self.placement = placement;
        self
    }

    /// The dialog's accessible name.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Announce this dialog as an alert that interrupts.
    ///
    /// Explicit, never inferred from a destructive button tone.
    #[must_use]
    pub const fn alert(mut self, alert: bool) -> Self {
        self.alert = alert;
        self
    }

    /// Override the scrim colour.
    #[must_use]
    pub const fn scrim(mut self, color: Color) -> Self {
        self.scrim = Some(color);
        self
    }

    /// The scrim colour this host paints over `palette`, or `None` when it
    /// draws no scrim. The render path uses this same value.
    #[must_use]
    pub fn resolved_scrim(&self, palette: &Palette) -> Option<Color> {
        if !self.renders_scrim() {
            return None;
        }
        Some(resolve_scrim(self.scrim, palette))
    }

    /// Whether a dialog is actually being presented.
    ///
    /// Availability, computed from what is here rather than stored as a
    /// second flag that could disagree with it. This
    /// is what decides whether the background goes inert, so a stale `true`
    /// would freeze the whole application behind an invisible scrim.
    #[must_use]
    pub const fn is_presenting(&self) -> bool {
        self.dialog.is_some() && self.presentation_open
    }

    /// Whether an outgoing dialog element is still supplied for visual exit.
    #[must_use]
    pub const fn retains_dialog(&self) -> bool {
        self.dialog.is_some()
    }

    /// Where the surface sits: [`SurfacePlacement::Centred`] or
    /// [`SurfacePlacement::BottomEdge`].
    #[must_use]
    pub const fn surface_placement(&self) -> SurfacePlacement {
        self.placement
    }

    /// Whether the background is currently withdrawn from input, focus and AT.
    ///
    /// Both conditions matter: a non-modal presentation (a popover hosted
    /// here) leaves the background live on purpose.
    #[must_use]
    pub const fn background_is_inert(&self) -> bool {
        self.is_presenting() && self.policy.modality.consumes_outside_input()
    }

    /// Whether a scrim is drawn over the background. Defined as exactly
    /// [`background_is_inert`](Self::background_is_inert), not merely
    /// alongside it: a full-bleed scrim over a live background would swallow
    /// every click while the background still looked reachable and stayed
    /// in traversal and the AT tree, a failure only an interaction test on
    /// the real widget tree catches.
    #[must_use]
    pub const fn renders_scrim(&self) -> bool {
        self.background_is_inert()
    }

    /// The semantics this host publishes for its dialog.
    ///
    /// Exposed because this is the call the render path makes: a test that
    /// builds a [`Semantics`] itself would prove only that
    /// [`dialog_semantics`] is right, leaving the host free to pass it the
    /// wrong arguments.
    #[must_use]
    pub fn semantics(&self) -> Option<Semantics> {
        self.is_presenting()
            .then(|| dialog_semantics(self.label.as_deref(), self.alert, self.policy))
    }

    /// Whether a press on the scrim would dismiss this presentation.
    ///
    /// Only meaningful while presenting: with no dialog there is no scrim to
    /// press, so this is `false` rather than "what the policy would have
    /// said".
    #[must_use]
    pub fn scrim_press_dismisses(&self) -> bool {
        self.renders_scrim() && self.scrim_dismissal().closes()
    }

    /// The decision a scrim press yields under this host's policy.
    ///
    /// Public so a host can ask without building an element, and so the
    /// render path's branch is the same call a test makes.
    #[must_use]
    pub fn scrim_dismissal(&self) -> Dismissal {
        // A press on the scrim is by construction outside the surface, so the
        // geometry is not in question -- only the policy is.
        dismissal::on_press(
            self.policy,
            &DismissRegions::new(iced::Rectangle::default()),
            iced::Point::new(-1.0, -1.0),
        )
    }
}

impl<'a, Message: Clone + 'a> EnzModalHost<'a, Message> {
    /// Build the element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        // Every decision that reads `self` is taken before the fields are
        // moved out, so the render path and the queryable methods above
        // cannot drift apart: these are the same calls a test makes.
        let inert = self.background_is_inert();
        let renders_scrim = self.renders_scrim();
        let scrim_dismisses = self.scrim_press_dismisses();
        let semantics = dialog_semantics(self.label.as_deref(), self.alert, self.policy);

        // `WithdrawFocus` sits *inside* `Inert`, not outside: `Inert::operate`
        // refuses to visit its subtree while inert, so a releaser above it
        // could never reach the stale owner it exists to clear.
        let background: Element<'a, Message> =
            crate::inert::Inert::new(WithdrawFocus::new(self.background, inert), inert).into();

        // Keep the stack root and background child 0 stable while closed.
        // Returning `background` directly would move that subtree to the root
        // whenever a modal closes, discarding retained state. Scrim and surface
        // may be appended without shifting the background.
        let dialog = self.dialog;
        let has_presentation = dialog.is_some() || renders_scrim;
        let pinned_scrim = self.scrim;
        // A dismissed modal dialog still on screen keeps a paint-only scrim
        // for its fade; the background is already live underneath it.
        let exiting_scrim =
            !renders_scrim && dialog.is_some() && self.policy.modality.consumes_outside_input();
        let scrim_fill = move |theme: &iced::Theme| {
            let color = resolve_scrim(pinned_scrim, &crate::theme_lookup::palette(theme));
            container::Style {
                background: Some(iced::Background::Color(color)),
                ..container::Style::default()
            }
        };
        // Keep this layer slot present on both sides of the edge. If removing
        // it shifted the surface from child 2 to child 1, iced's positional
        // diff would hand the surface the scrim's tree and reset the motion we
        // are trying to continue.
        let scrim_layer: Element<'a, Message> = if renders_scrim {
            // The scrim owns every scroll/paging gesture that reaches it, so a
            // modal never leaks a wheel/touchpad/paging gesture to whatever is
            // behind or around the host. Built here, inside the
            // `renders_scrim` (== background-inert) branch, so a non-modal
            // popover — which paints no scrim — keeps its background live.
            let base = container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(scrim_fill);

            // The scrim only takes presses when the policy says a press there
            // dismisses. An insistent alert gets an inert scrim -- but it is
            // still a scrim, so it still blocks what is behind it.
            let scrim_body: Element<'a, Message> = if scrim_dismisses {
                // Dismissal fires on a *completed* click — press and release
                // on the scrim, no drag-away — not on pointer-down. Binding it
                // to press-down tore the modal down on the "I changed my mind"
                // drag-off gesture, losing the dialog's contents. The
                // factory is still deferred to event time.
                let on_event = std::rc::Rc::clone(&self.on_event);
                crate::scrim_click::ScrimClick::new(base, move || {
                    (on_event)(ModalHostEvent::Dismissed(DismissCause::Scrim))
                })
                .into()
            } else {
                base.into()
            };
            Element::new(ModalScrollBarrier { child: scrim_body })
        } else if exiting_scrim {
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(scrim_fill)
                .into()
        } else {
            iced::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };

        let surface: Element<'a, Message> = match dialog {
            Some(dialog) => {
                crate::semantics_container::SemanticsContainer::new(dialog, semantics).into()
            }
            None => iced::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        };

        // Presses on the surface's non-interactive children stop here and
        // never reach the scrim. That makes `press_outcome`'s rule (a press
        // inside the surface never dismisses) true of the widget, not just
        // the free function: a blanket `mouse_area` scrim cannot tell a
        // press on the dialog's padding from one on the page behind it,
        // since the surface is just pixels above it when the event arrives.
        let surface: Element<'a, Message> = crate::swallow_press::SwallowPress::new(surface).into();
        // Dismissal withdraws the subtree from update, focus operations and
        // accessibility immediately, while leaving draw available to motion.
        // This also suppresses the Dialog role on the very first exit frame,
        // when geometric progress may still equal one.
        let surface: Element<'a, Message> =
            crate::inert::Inert::new(surface, !self.presentation_open).into();

        // The scope id goes on the container that wraps the *surface*, not on
        // the whole stack: the stack also holds the background, and a focus
        // operation scoped to it would descend into the background's controls
        // as well. Scoping to the surface is what makes "cycle inside this
        // dialog" mean the dialog.
        // A centred dialog keeps a margin from the window edge, as phone
        // alerts and adaptive desktop dialogs do; an edge sheet attaches.
        let margin = match self.placement {
            SurfacePlacement::Centred => DIALOG_WINDOW_MARGIN,
            SurfacePlacement::BottomEdge => 0.0,
        };
        let mut placed = container(surface)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(margin)
            .center_x(Length::Fill)
            .align_y(self.placement.vertical_alignment());

        if let Some(scope) = self.scope {
            placed = placed.id(scope);
        }

        let effect = match self.placement {
            SurfacePlacement::Centred => crate::motion::MotionEffect::Scale,
            SurfacePlacement::BottomEdge => {
                crate::motion::MotionEffect::Slide(crate::motion::SlideDirection::Down)
            }
        };
        let opening = std::rc::Rc::new(std::cell::Cell::new(false));
        let mut animated = crate::motion::motion(placed, self.presentation_open, effect)
            .enter_on_mount_if(std::rc::Rc::clone(&opening));
        if let Some(on_exit_settled) = self.on_exit_settled {
            animated = animated.on_hidden_with(move || on_exit_settled());
        }

        // Child 0 is always the background, so its state remains at the same
        // path across open/close. Closed hosts do not need two placeholder
        // children: appending the scrim and surface on open cannot displace
        // child 0, while omitting them removes inert nodes from every ordinary
        // application frame.
        let layers: Vec<Element<'a, Message>> = if has_presentation {
            // The scrim fades with the surface, so closing dissolves the dim
            // instead of dropping it in one frame after the surface has gone.
            let scrim_layer: Element<'a, Message> = crate::motion::motion(
                scrim_layer,
                self.presentation_open,
                crate::motion::MotionEffect::Fade,
            )
            .enter_on_mount_if(std::rc::Rc::clone(&opening))
            .into();
            vec![background, scrim_layer, animated.into()]
        } else {
            vec![background]
        };

        let stacked: Element<'a, Message> = stack(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // Escape is bound around the whole presentation because it must work
        // even when the surface holds no focus. The same wrapper contains Tab
        // only when the policy also withdraws the background.
        // The three factories are wrapped in `Rc<dyn Fn>` and invoked lazily
        // inside `update`, not at construction time; eager construction would
        // call `on_event` three times on every view pass without a key press.
        let ev = std::rc::Rc::clone(&self.on_event);
        let on_escape = {
            let ev = std::rc::Rc::clone(&ev);
            std::rc::Rc::new(move || (ev)(ModalHostEvent::Dismissed(DismissCause::Escape)))
                as std::rc::Rc<dyn Fn() -> Message + 'a>
        };
        // Card-dialog convenience paths disable these publishers because their
        // single callback maps every event to `on_close`; modal containment is
        // unchanged, while non-modal Tab bypasses this routing entirely.
        let (on_focus_next, on_focus_previous) = if self.emit_focus_events {
            let next = {
                let ev = std::rc::Rc::clone(&ev);
                std::rc::Rc::new(move || (ev)(ModalHostEvent::FocusNext))
                    as std::rc::Rc<dyn Fn() -> Message + 'a>
            };
            let prev = {
                std::rc::Rc::new(move || (ev)(ModalHostEvent::FocusPrevious))
                    as std::rc::Rc<dyn Fn() -> Message + 'a>
            };
            (Some(next), Some(prev))
        } else {
            // `ev` is only borrowed by `on_escape` above; dropping it here
            // keeps this branch reference-count-clean.
            let _ = ev;
            (None, None)
        };
        Element::new(EscapeCapture {
            child: stacked,
            active: self.presentation_open,
            contain_focus: inert,
            outcome: escape_outcome(self.policy),
            on_escape,
            on_focus_next,
            on_focus_previous,
            opening,
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzModalHost<'a, Message>> for Element<'a, Message> {
    fn from(host: EnzModalHost<'a, Message>) -> Self {
        host.into_element()
    }
}

include!("modal_host/dialog_stack.rs");
