//! Controlled pointer, keyboard and routed context menus with nested surfaces.
//!
//! The application owns the root model and [`ContextMenuState`] and applies
//! emitted proposals with [`apply_event`] or [`apply_event_restoring`]. Opening
//! resets submenu levels and stamps a fresh showing; stamped activations require
//! it to remain open and current. Unstamped events have weaker lifecycle checks.
//!
//! Invocations use host layout coordinates; overlay traversal translates both
//! pointer and keyboard anchors to window coordinates. Each surface is measured
//! within the viewport inset by the edge margin and placed at the shared Menu layer.
//! Keep optional hosts mounted to preserve child state across open and close.
//!
//! The default suppresses ordinary target events while presenting; point hosts
//! forward unclaimed events. Neither mode hides target semantics or child overlays.
//! Menu keys and plain Tab stay with the menu, and outside presses are consumed.
//! An optional [`EnzContextMenuHost::header`] adds caller-owned controls above
//! root commands; Tab then wraps within the deepest menu surface.
//! Routed requests drain during ordinary updates, after child dispatch; captured
//! child events can delay them. They are not drained by targeted frame dispatch.
//! Unavailable open state emits Host dismissal until the application applies it.

use std::rc::Rc;

use enzui_core::context_menu::{
    InvocationKind, MenuMove, Point as CorePoint, Rect as CoreRect, opens_context_menu,
};
use enzui_core::dismissal::{self, DismissPolicy, DismissRegions};
use enzui_core::menu_model::{
    Activation, CheckState, CloseCause, EntryId, MenuKey, MenuModel, key_for_open_menu,
    menu_semantics,
};
use enzui_core::menu_stack::{MenuLevel, MenuStack};
use enzui_core::metrics::FiniteNonNeg;
use enzui_core::placement::place_at_point;
use enzui_core::table::Generation;
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

#[derive(Debug, Default)]
struct State {
    context_menu_pending: bool,
    /// The menu fading and scaling in as it opens.
    reveal: crate::anchored_overlay::PopupReveal,
}

impl tree::Interactive for State {
    fn cancel_interaction(&mut self) {
        self.context_menu_pending = false;
    }
}

#[derive(Debug, Default)]
struct FocusProbe {
    focused: bool,
}

impl<T> Operation<T> for FocusProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if state.is_focused() {
            self.focused = true;
        }
    }
}

/// Default clearance from viewport edges in logical pixels: 8.
pub const EDGE_MARGIN: f32 = 8.0;

/// Default surface-width floor in logical pixels: 240; parent limits can compress it.
pub const CONTEXT_MENU_HUG_MIN_WIDTH: f32 = 240.0;

/// Default surface-width ceiling in logical pixels: 320; narrower viewports constrain layout.
pub const CONTEXT_MENU_HUG_MAX_WIDTH: f32 = 320.0;

/// App-owned invocation, submenu models and optional focus-restoration target.
/// Opening resets levels and advances the generation; closing removes levels
/// and the restoration target without advancing the generation. Mutable stack
/// access can create levels even while the root invocation is absent.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContextMenuState {
    invocation: Option<InvocationKind>,
    stack: MenuStack,
    restore_target: Option<FocusRestoreTarget>,
}

impl ContextMenuState {
    /// A closed menu.
    #[must_use]
    pub fn closed() -> Self {
        Self {
            invocation: None,
            stack: MenuStack::new(),
            restore_target: None,
        }
    }

    /// The open submenu levels of this showing.
    #[must_use]
    pub fn stack(&self) -> &MenuStack {
        &self.stack
    }

    /// Mutate the owned submenu models and levels.
    /// This access does not require the root menu to be open.
    pub fn stack_mut(&mut self) -> &mut MenuStack {
        &mut self.stack
    }

    /// The current showing stamp; opening advances it and closing leaves it unchanged.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.stack.generation()
    }

    /// Number of owned submenu levels; zero means no child level.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.depth()
    }

    /// Whether the menu is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.invocation.is_some()
    }

    /// The invocation that opened this menu, if it is open.
    #[must_use]
    pub const fn invocation(&self) -> Option<InvocationKind> {
        self.invocation
    }

    /// Invocation anchor before the host's overlay translation is applied.
    #[must_use]
    pub fn anchor(&self) -> Option<CorePoint> {
        self.invocation.map(InvocationKind::anchor)
    }

    /// Begin a fresh showing at `invocation`, resetting levels and restoration target.
    /// Returns whether the invocation changed. Even an identical invocation advances
    /// the generation and resets the stack.
    pub fn open(&mut self, invocation: InvocationKind) -> bool {
        let changed = self.invocation != Some(invocation);
        self.invocation = Some(invocation);
        self.stack.reset();
        self.restore_target = None;
        changed
    }

    /// Begin a fresh showing and retain a focus-restoration target.
    /// A later `open` or applied `Invoked` event clears this target.
    pub fn open_with_restore(
        &mut self,
        invocation: InvocationKind,
        target: FocusRestoreTarget,
    ) -> bool {
        let changed = self.open(invocation);
        self.restore_target = Some(target);
        changed
    }

    /// The target retained for this showing.
    #[must_use]
    pub fn restore_target(&self) -> Option<&FocusRestoreTarget> {
        self.restore_target.as_ref()
    }

    /// Remove the invocation, submenu levels and restoration target.
    /// Returns whether an invocation was present; the generation is unchanged.
    pub fn close(&mut self) -> bool {
        self.stack.close_all();
        self.restore_target = None;
        self.invocation.take().is_some()
    }
}

/// Proposed menu changes that the application applies to its owned model and state.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextMenuEvent {
    /// Open at an anchor in the host's layout coordinate space.
    /// The host adds overlay translation to both pointer and keyboard invocations
    /// when presenting them; a point supplied manually must use that same space.
    Invoked(InvocationKind),
    /// The menu should close, for this reason.
    Dismissed(CloseCause),
    /// Move the highlight on the deepest stored surface.
    Navigate(MenuMove),
    /// The highlight should jump to this row (hover) on the root surface.
    ///
    /// Equivalent to [`ContextMenuEvent::HighlightAt`] with `depth: 0`.
    Highlight(EntryId),
    /// Highlight a row on the named surface; zero selects the root.
    /// This variant is not generation-stamped.
    HighlightAt {
        /// Surface depth: zero is the root.
        depth: usize,
        /// Command or highlighted row identity.
        id: EntryId,
    },
    /// Request an owned child model from the resolver for this generation and depth.
    /// The reducer checks generation but does not require the root to remain open.
    SubmenuOpened {
        /// Showing stamp captured when the event was produced.
        generation: Generation,
        /// Surface depth: zero is the root.
        depth: usize,
        /// Row that requests a child surface.
        parent: EntryId,
    },
    /// Close the deepest stored level if its generation matches.
    SubmenuClosed {
        /// Showing stamp captured when the event was produced.
        generation: Generation,
    },
    /// Resolve an unstamped activation against the deepest stored surface.
    /// This compatibility variant has no open or generation check; the host emits
    /// [`Self::ActivatedAt`] instead. The supplied check state is informational.
    Activated {
        /// Command or highlighted row identity.
        id: EntryId,
        /// Informational row state; the reducer does not apply it.
        check: CheckState,
    },
    /// Resolve an activation only while the root is open with this generation.
    /// The live deepest model decides availability; the supplied check state is
    /// ignored by the reducer and checked rows are not toggled there.
    ActivatedAt {
        /// Showing stamp captured when the event was produced.
        generation: Generation,
        /// Command or highlighted row identity.
        id: EntryId,
        /// Informational row state; the reducer does not apply it.
        check: CheckState,
    },
}

/// Dismiss on Escape or an outside press and consume the dismissing press.
/// The policy does not itself install a scrim or make background widgets inert.
#[must_use]
pub const fn context_menu_dismiss_policy() -> DismissPolicy {
    DismissPolicy::popover().modality(enzui_core::dismissal::Modality::Modal)
}

/// Add a target origin and translation to a local point.
/// All inputs must share logical pixel units; invalid coordinates pass through.
#[must_use]
pub fn to_viewport(local: Point, origin: Point, translation: Vector) -> CorePoint {
    CorePoint::new(
        local.x + origin.x + translation.x,
        local.y + origin.y + translation.y,
    )
}

/// Translate a logical-pixel rectangle, preserving its dimensions.
#[must_use]
pub fn bounds_to_viewport(bounds: Rectangle, translation: Vector) -> CoreRect {
    CoreRect::new(
        bounds.x + translation.x,
        bounds.y + translation.y,
        bounds.width,
        bounds.height,
    )
}

/// Place a menu at the invocation anchor within an origin-zero viewport.
/// Uses [`place_at_point`] for per-axis flipping and clamping. Menu size,
/// viewport and margin use logical pixels.
#[must_use]
pub fn place_for(
    invocation: InvocationKind,
    menu_size: Size,
    viewport: Size,
    edge_margin: f32,
) -> CorePoint {
    let anchor = invocation.anchor();
    let resolved = place_at_point(
        Point::new(anchor.x, anchor.y),
        menu_size,
        Rectangle::new(Point::ORIGIN, viewport),
        edge_margin,
    );
    CorePoint::new(resolved.bounds.x, resolved.bounds.y)
}

/// Treat the optional menu rectangle as the only inside region.
/// The invocation target is not an inside region.
#[must_use]
pub fn dismiss_regions(menu: Option<Rectangle>) -> DismissRegions {
    match menu {
        Some(menu) => DismissRegions::new(menu),
        None => DismissRegions::empty(),
    }
}

/// Resolve the model at `depth`, with zero selecting the root.
/// The root is returned even while closed; higher depths require stored levels.
#[must_use]
pub fn surface_model<'a>(
    root: &'a MenuModel,
    state: &'a ContextMenuState,
    depth: usize,
) -> Option<&'a MenuModel> {
    match depth.checked_sub(1) {
        None => Some(root),
        Some(level) => state.stack().levels().get(level).map(MenuLevel::model),
    }
}

/// Mutably resolve a stored surface model; zero selects the root even while closed.
pub fn surface_model_mut<'a>(
    root: &'a mut MenuModel,
    state: &'a mut ContextMenuState,
    depth: usize,
) -> Option<&'a mut MenuModel> {
    match depth.checked_sub(1) {
        None => Some(root),
        Some(level) => state
            .stack_mut()
            .levels_mut()
            .get_mut(level)
            .map(MenuLevel::model_mut),
    }
}

/// The model of the deepest open surface — the one keyboard input applies
/// to.
#[must_use]
pub fn deepest_model<'a>(root: &'a MenuModel, state: &'a ContextMenuState) -> &'a MenuModel {
    state.stack().deepest().map_or(root, MenuLevel::model)
}

/// Decode menu navigation and activation against the deepest stored surface.
/// Escape closes one submenu before the root. Enter without a choice is
/// swallowed. Right opens a highlighted submenu; Left closes a child level.
/// Other horizontal cases and unrecognised keys return None. This helper does
/// not check that the root is open or swallow Tab; the host handles those guards.
#[must_use]
pub fn event_for_open_key(
    root: &MenuModel,
    state: &ContextMenuState,
    event: &iced::keyboard::Event,
) -> Option<MenuEvent> {
    let model = deepest_model(root, state);
    let depth = state.depth();
    let generation = state.generation();
    match key_for_open_menu(event)? {
        MenuKey::Move(movement) => Some(MenuEvent::Claimed(ContextMenuEvent::Navigate(movement))),
        MenuKey::Dismiss => Some(MenuEvent::Claimed(if depth > 0 {
            ContextMenuEvent::SubmenuClosed { generation }
        } else {
            ContextMenuEvent::Dismissed(CloseCause::Escape)
        })),
        MenuKey::Activate => match model.activate_active() {
            Activation::Dispatch { id, check } => {
                Some(MenuEvent::Claimed(ContextMenuEvent::ActivatedAt {
                    generation,
                    id,
                    check,
                }))
            }
            Activation::OpenSubmenu(parent) => {
                Some(MenuEvent::Claimed(ContextMenuEvent::SubmenuOpened {
                    generation,
                    depth,
                    parent,
                }))
            }
            Activation::None => Some(MenuEvent::Swallowed),
        },
        MenuKey::OpenSubmenu => match model.activate_active() {
            Activation::OpenSubmenu(parent) => {
                Some(MenuEvent::Claimed(ContextMenuEvent::SubmenuOpened {
                    generation,
                    depth,
                    parent,
                }))
            }
            _ => None,
        },
        MenuKey::CloseSubmenu => {
            (depth > 0).then_some(MenuEvent::Claimed(ContextMenuEvent::SubmenuClosed {
                generation,
            }))
        }
    }
}

/// A claimed key either publishes a menu event or is consumed without one.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuEvent {
    /// Publish this event, and consume the key.
    Claimed(ContextMenuEvent),
    /// Publish nothing, but consume the key anyway.
    Swallowed,
}

/// Supply an owned submenu model for storage in the context-menu state.
/// Return None to leave the level unopened. An empty supplied model still
/// creates a level; availability is not checked by the submenu reducer.
pub trait SubmenuResolver {
    /// Supply the child model for a row on the named parent surface.
    fn submenu(&self, depth: usize, parent: &EntryId) -> Option<MenuModel>;
}

impl<F> SubmenuResolver for F
where
    F: Fn(usize, &EntryId) -> Option<MenuModel>,
{
    fn submenu(&self, depth: usize, parent: &EntryId) -> Option<MenuModel> {
        self(depth, parent)
    }
}

/// A resolver that never has a submenu — the behaviour of a flat menu.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoSubmenus;

impl SubmenuResolver for NoSubmenus {
    fn submenu(&self, _depth: usize, _parent: &EntryId) -> Option<MenuModel> {
        None
    }
}

/// Apply a flat-menu event and return a resolved command entry, if any.
/// Submenu opening uses [`NoSubmenus`]; nested menus use [`apply_event_with`].
/// The application executes commands and updates checked values.
pub fn apply_event(
    model: &mut MenuModel,
    state: &mut ContextMenuState,
    event: &ContextMenuEvent,
) -> Option<EntryId> {
    apply_event_with(model, state, event, &NoSubmenus)
}

/// Apply a flat-menu event and prepare restoration from the retained target.
/// Use [`ContextMenuState::open_with_restore`] to retain that target. Restoration
/// is selected before activation validation, so a rejected stale activation can
/// still produce a focus operation. [`ContextMenuUpdate::finish`] batches it
/// with the caller's activation task without guaranteeing their order.
pub fn apply_event_restoring<T>(
    model: &mut MenuModel,
    state: &mut ContextMenuState,
    event: &ContextMenuEvent,
) -> ContextMenuUpdate<T>
where
    T: Send + 'static,
{
    apply_event_restoring_with(model, state, event, &NoSubmenus)
}

/// Submenu-aware form of [`apply_event_restoring`].
pub fn apply_event_restoring_with<T>(
    model: &mut MenuModel,
    state: &mut ContextMenuState,
    event: &ContextMenuEvent,
    submenus: &dyn SubmenuResolver,
) -> ContextMenuUpdate<T>
where
    T: Send + 'static,
{
    // Focus goes back only for a transition that really closed the menu: a
    // refused stale activation leaves a newer menu open, and its focus with it.
    let was_open = state.is_open();
    let restore = focus_restore_operation_to(event, state.restore_target());
    let activated = apply_event_with(model, state, event, submenus);
    let closed = was_open && !state.is_open();
    ContextMenuUpdate {
        activated,
        restore: restore.filter(|_| closed),
    }
}

/// Apply an event with owned child models supplied by `submenus`.
/// Stamped activations require an open root and matching generation. Submenu
/// changes check generation only; a delayed submenu open can add stored levels
/// after dismissal, although the root remains closed. Invoked resets levels,
/// clears the root highlight and clears any restoration target. Highlight and
/// navigation events are unstamped. Command activation closes the state and
/// clears the root highlight, but does not execute a command or toggle checks.
pub fn apply_event_with(
    model: &mut MenuModel,
    state: &mut ContextMenuState,
    event: &ContextMenuEvent,
    submenus: &dyn SubmenuResolver,
) -> Option<EntryId> {
    match event {
        ContextMenuEvent::Invoked(invocation) => {
            state.open(*invocation);
            model.clear_active();
            None
        }
        ContextMenuEvent::Dismissed(_) => {
            if state.close() {
                model.clear_active();
            }
            None
        }
        ContextMenuEvent::Navigate(movement) => {
            match state.stack_mut().navigate_deepest(*movement) {
                Some(_) => {}
                None => {
                    model.navigate(*movement);
                }
            }
            None
        }
        ContextMenuEvent::Highlight(id) => {
            model.set_active(id);
            None
        }
        ContextMenuEvent::HighlightAt { depth, id } => {
            let opens_submenu = surface_model_mut(model, state, *depth).is_some_and(|surface| {
                surface.set_active(id);
                surface
                    .entry(id)
                    .is_some_and(|entry| matches!(entry.activation(), Activation::OpenSubmenu(_)))
            });
            // Resting on a submenu row opens its submenu; resting on any
            // other row closes submenus opened below this surface.
            if opens_submenu {
                if let Some(child) = submenus.submenu(*depth, id) {
                    let generation = state.generation();
                    let _ = state.stack_mut().open_submenu_checked(
                        generation,
                        *depth,
                        id.clone(),
                        child,
                    );
                }
            } else {
                while state.depth() > *depth {
                    let _ = state.stack_mut().close_submenu();
                }
            }
            None
        }
        ContextMenuEvent::SubmenuOpened {
            generation,
            depth,
            parent,
        } => {
            // Reject old showings before calling the application resolver.
            if !state.stack().is_current(*generation) {
                return None;
            }
            if let Some(child) = submenus.submenu(*depth, parent) {
                let _ = state.stack_mut().open_submenu_checked(
                    *generation,
                    *depth,
                    parent.clone(),
                    child,
                );
            }
            None
        }
        ContextMenuEvent::SubmenuClosed { generation } => {
            let _ = state.stack_mut().close_submenu_checked(*generation);
            None
        }
        ContextMenuEvent::Activated { id, .. } => resolve_activation(model, state, id, submenus),
        ContextMenuEvent::ActivatedAt { generation, id, .. } => {
            // Closing retains generation, so current stamps still require an open root.
            if !activation_is_current(state, *generation) {
                return None;
            }
            resolve_activation(model, state, id, submenus)
        }
    }
}

/// Require both an open root invocation and a matching showing stamp.
/// Closing alone retains the stamp; reopening advances it.
#[must_use]
pub fn activation_is_current(state: &ContextMenuState, generation: Generation) -> bool {
    state.is_open() && state.stack().is_current(generation)
}

fn resolve_activation(
    model: &mut MenuModel,
    state: &mut ContextMenuState,
    id: &EntryId,
    submenus: &dyn SubmenuResolver,
) -> Option<EntryId> {
    let depth = state.depth();
    // Recheck the live model; a queued row can become disabled before application.
    let resolved = surface_model(model, state, depth).map(|surface| surface.activate(id));
    match resolved {
        Some(Activation::Dispatch { id, .. }) => {
            state.close();
            model.clear_active();
            Some(id)
        }
        Some(Activation::OpenSubmenu(parent)) => {
            let generation = state.generation();
            if let Some(child) = submenus.submenu(depth, &parent) {
                let _ = state
                    .stack_mut()
                    .open_submenu_checked(generation, depth, parent, child);
            }
            None
        }
        Some(Activation::None) | None => None,
    }
}

/// Prepare widget focus restoration for dismissal and activation event shapes.
/// Missing invokers and Host dismissal return None. The returned operation must
/// be executed by the runtime and leaves focus unchanged when its widget target
/// is absent. Event staleness and whether an activation actually closes the menu
/// are not checked here; tree rows use [`focus_restore_operation_to`].
#[must_use]
pub fn focus_restore_operation<T>(
    event: &ContextMenuEvent,
    invoker: Option<&Id>,
) -> Option<Box<dyn Operation<T>>>
where
    T: Send + 'static,
{
    let target = FocusRestoreTarget::Widget(invoker?.clone());
    focus_restore_operation_to(event, Some(&target))
}

/// Hosts a context menu over a target.
///
/// Build with [`enz_context_menu_host`].
/// Presenting a model with duplicate entry IDs panics in debug builds;
/// release builds keep the first command occurrence.
#[must_use]
pub struct EnzContextMenuHost<'a, Message> {
    target: Element<'a, Message>,
    model: &'a MenuModel,
    state: &'a ContextMenuState,
    on_event: Rc<dyn Fn(ContextMenuEvent) -> Message + 'a>,
    enabled: bool,
    label: Option<String>,
    invoker: Option<Id>,
    at_id: Option<Id>,
    menu_width: Option<f32>,
    menu_material: Option<enzui_theme::MaterialStyle>,
    menu_row_tokens: Option<crate::menu_row::MenuRowTokens>,
    edge_margin: f32,
    id_prefix: Option<String>,
    live_target: bool,
    header: Option<Rc<dyn Fn(Generation) -> Element<'a, Message> + 'a>>,
}

/// Suffix used to derive an AT host identity distinct from the invoker: context-menu.
pub const CONTEXT_MENU_ID_SUFFIX: &str = "context-menu";

/// Derive a host AT identity from the invoker's Debug representation.
/// The result is stable for the same invoker and distinct from its field identity.
#[must_use]
pub fn derive_context_menu_id(invoker: &Id) -> Id {
    enzui_core::identity::derive_id(&format!("{invoker:?}"), CONTEXT_MENU_ID_SUFFIX)
}

/// Host a controlled menu over a target, suppressing its ordinary events while presenting.
/// Secondary press invokes within the target; Menu or Shift+F10 requires focus
/// inside it. An advertised ID enables routed requests. The caller applies
/// published events to model and state.
pub fn enz_context_menu_host<'a, Message: Clone + 'a>(
    target: impl Into<Element<'a, Message>>,
    model: &'a MenuModel,
    state: &'a ContextMenuState,
    on_event: impl Fn(ContextMenuEvent) -> Message + 'a,
) -> EnzContextMenuHost<'a, Message> {
    EnzContextMenuHost {
        target: target.into(),
        model,
        state,
        on_event: Rc::new(on_event),
        enabled: true,
        label: None,
        invoker: None,
        at_id: None,
        menu_width: None,
        menu_material: None,
        menu_row_tokens: None,
        edge_margin: EDGE_MARGIN,
        id_prefix: None,
        live_target: false,
        header: None,
    }
}

/// Host the same controlled menu while keeping unclaimed page events live.
/// Open with an app-supplied invocation in the host's layout coordinate space;
/// this constructor changes target event policy, not the coordinate convention.
pub fn enz_point_context_menu_host<'a, Message: Clone + 'a>(
    page: impl Into<Element<'a, Message>>,
    model: &'a MenuModel,
    state: &'a ContextMenuState,
    on_event: impl Fn(ContextMenuEvent) -> Message + 'a,
) -> EnzContextMenuHost<'a, Message> {
    enz_context_menu_host(page, model, state, on_event).live_target(true)
}

static DORMANT_MODEL: std::sync::LazyLock<MenuModel> =
    std::sync::LazyLock::new(|| MenuModel::new(Vec::new()));

static DORMANT_STATE: std::sync::LazyLock<ContextMenuState> =
    std::sync::LazyLock::new(ContextMenuState::closed);

/// Keep a live-page host mounted while its menu model and state are optional.
/// None supplies an empty model and closed state: no menu invocation or surface,
/// while the child tree remains mounted. Keep the wrapper present across views
/// to preserve child state.
pub fn enz_optional_point_context_menu_host<'a, Message: Clone + 'a>(
    page: impl Into<Element<'a, Message>>,
    menu: Option<(&'a MenuModel, &'a ContextMenuState)>,
    on_event: impl Fn(ContextMenuEvent) -> Message + 'a,
) -> EnzContextMenuHost<'a, Message> {
    let (model, state) = menu.unwrap_or((&DORMANT_MODEL, &DORMANT_STATE));
    enz_point_context_menu_host(page, model, state, on_event)
}

impl<'a, Message: Clone + 'a> EnzContextMenuHost<'a, Message> {
    /// Build interactive content above the root commands inside the menu surface.
    /// The factory can run repeatedly during tree and overlay rebuilds; it must
    /// not dispatch actions. Its showing stamp lets caller-owned accessory
    /// messages reject obsolete activations with [`activation_is_current`]. It is
    /// not called while closed. Header content uses the menu's resolved width;
    /// use [`Self::menu_width`] for a wider accessory.
    ///
    /// Tab and Shift+Tab wrap through the deepest surface's controls; header
    /// controls retain their own keys and semantics. Escape and unclaimed menu
    /// keys outside a focused header keep the usual nested-menu behavior.
    /// Keep this builder configured across views to retain header state.
    pub fn header(mut self, build: impl Fn(Generation) -> Element<'a, Message> + 'a) -> Self {
        self.header = Some(Rc::new(build));
        self
    }

    /// Allow menu invocation and presentation; defaults to true.
    /// False delegates child events and hides menu surfaces. An app state still open
    /// while unavailable emits Host dismissal until the application closes it.
    /// Previously armed routed requests are not cleared by this builder.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Accessible name for the menu surface ("Text actions").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the identity used to derive the host's default AT address.
    /// This does not itself retain a focus-restoration target in app-owned state;
    /// use [`ContextMenuState::open_with_restore`] or an explicit restoration helper.
    pub fn invoker(mut self, id: impl Into<Id>) -> Self {
        self.invoker = Some(id.into());
        self
    }

    /// Override the invoker-derived AT address.
    /// With neither an explicit ID nor an invoker, the host advertises no routed
    /// context-menu affordance.
    pub fn at_id(mut self, id: impl Into<Id>) -> Self {
        self.at_id = Some(id.into());
        self
    }

    /// Return the explicit AT ID, or derive one from the invoker; otherwise return None.
    #[must_use]
    pub fn advertised_id(&self) -> Option<Id> {
        self.at_id
            .clone()
            .or_else(|| self.invoker.as_ref().map(derive_context_menu_id))
    }

    /// Return the configured invoker identity; it does not identify the AT host node.
    #[must_use]
    pub fn invoker_id(&self) -> Option<&Id> {
        self.invoker.as_ref()
    }

    /// Pin the surface width instead of measuring rows within the default 240–320 px band.
    /// Non-finite or non-positive widths fall back to 240; parent limits still apply.
    pub fn menu_width(mut self, width: f32) -> Self {
        self.menu_width = Some(width);
        self
    }

    /// Request the menu's shared material style; renderer capability and policy may use fallback.
    pub fn menu_material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.menu_material = Some(style);
        self
    }

    /// Override every command row and separator's geometry/colour tokens.
    /// This allows a dense, app-specific command set to fit a compact viewport
    /// without dropping actions from the menu model.
    pub fn menu_row_tokens(mut self, tokens: crate::menu_row::MenuRowTokens) -> Self {
        self.menu_row_tokens = Some(tokens.sanitized());
        self
    }

    /// Set logical-pixel viewport clearance; defaults to [`EDGE_MARGIN`].
    /// Zero is retained; negative or non-finite values use the default.
    pub fn edge_margin(mut self, margin: f32) -> Self {
        self.edge_margin = FiniteNonNeg::new_or(margin, EDGE_MARGIN).get();
        self
    }

    /// Namespace for derived row ids, so two hosts on one screen sharing a row
    /// key do not derive the same widget id.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// Forward unclaimed target events while the menu presents; defaults to false.
    /// Menu keys are handled first, and mouse/touch presses remain reserved for
    /// invocation or dismissal. Child operations, drawing, mouse feedback and
    /// overlays are still forwarded in both modes; this is not a full inert wrapper.
    pub fn live_target(mut self, live: bool) -> Self {
        self.live_target = live;
        self
    }

    /// Whether unclaimed target events are forwarded while a menu is presented.
    #[must_use]
    pub const fn target_is_live(&self) -> bool {
        self.live_target
    }

    /// Whether the host is enabled and the root model has a focusable row.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.enabled && self.model.has_focusable()
    }

    fn target_holds_focus(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
    ) -> bool {
        let mut probe = FocusProbe::default();
        {
            let mut boxed = iced::advanced::widget::operation::black_box::<(), ()>(
                &mut probe as &mut dyn Operation<()>,
            );
            self.target.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                &layout.bounds(),
                renderer,
                &mut boxed,
            );
        }
        probe.focused
    }

    /// Whether app state is open and the root model is currently available.
    #[must_use]
    pub fn is_presenting(&self) -> bool {
        self.state.is_open() && self.is_available()
    }

    /// Return the untranslated invocation anchor when presenting; otherwise None.
    #[must_use]
    pub fn anchor(&self) -> Option<CorePoint> {
        self.is_presenting().then(|| self.state.anchor()).flatten()
    }

    /// Semantics for the menu surface.
    #[must_use]
    pub fn menu_semantics(&self) -> Option<enzui_core::semantics::Semantics> {
        self.is_presenting()
            .then(|| menu_semantics(self.label.as_deref(), self.model))
    }

    fn menu(&self) -> Option<Element<'a, Message>> {
        Some(self.build_menu()?.into_element())
    }

    fn surfaces(&self) -> Vec<Element<'a, Message>> {
        let Some(root) = self.menu() else {
            return Vec::new();
        };
        let mut surfaces = vec![root];
        for (level_index, level) in self.state.stack().levels().iter().enumerate() {
            surfaces.push(
                self.build_surface(level_index + 1, level.model())
                    .into_element(),
            );
        }
        surfaces
    }

    fn build_menu(&self) -> Option<crate::menu::EnzMenu<'a, Message>> {
        if !self.is_presenting() {
            return None;
        }

        debug_assert!(
            self.model.duplicate_ids().is_empty(),
            "context-menu entries must have unique ids; duplicates: {:?}",
            self.model.duplicate_ids()
        );

        Some(self.build_surface(0, self.model))
    }

    /// The id prefix of the surface at `depth`: equal row keys on different
    /// submenu levels need separate widget identities.
    fn surface_prefix(&self, depth: usize) -> String {
        let base = self.id_prefix.as_deref().unwrap_or(crate::menu::ID_PREFIX);
        if depth == 0 {
            base.to_owned()
        } else {
            format!("{base}-{depth}")
        }
    }

    /// For each open submenu level: the id of the row that opened it, in the
    /// surface above, and of its own first row, so the overlay can line the
    /// two rows up.
    fn submenu_anchors(&self) -> Vec<SubmenuAnchor> {
        self.state
            .stack()
            .levels()
            .iter()
            .enumerate()
            .map(|(index, level)| SubmenuAnchor {
                parent_row: enzui_core::identity::derive_id(
                    &self.surface_prefix(index),
                    level.parent().as_str(),
                ),
                first_row: level
                    .model()
                    .entries()
                    .iter()
                    .find(|entry| entry.is_focusable())
                    .map(|entry| {
                        enzui_core::identity::derive_id(
                            &self.surface_prefix(index + 1),
                            entry.id().as_str(),
                        )
                    }),
            })
            .collect()
    }

    fn build_surface(&self, depth: usize, model: &MenuModel) -> crate::menu::EnzMenu<'a, Message> {
        let mut menu = crate::menu::enz_menu();
        if let Some(style) = self.menu_material {
            menu = menu.material(style);
        }
        if let Some(width) = self.menu_width {
            menu = menu.width(width);
        } else {
            menu = menu
                .hug_content(true)
                .hug_min_width(CONTEXT_MENU_HUG_MIN_WIDTH)
                .hug_max_width(CONTEXT_MENU_HUG_MAX_WIDTH);
        }
        if let Some(tokens) = self.menu_row_tokens {
            menu = menu.row_tokens(tokens);
        }
        menu = menu.id_prefix(self.surface_prefix(depth));
        if let Some(label) = &self.label {
            menu = menu.label(label.clone());
        }
        if depth == 0
            && let Some(header) = &self.header
        {
            menu = menu.push(Element::new(MenuHeader {
                child: header(self.state.generation()),
            }));
        }
        menu = menu.active_maybe(model.active().cloned());
        {
            let on_event = Rc::clone(&self.on_event);
            menu = menu.on_hover(move |id| on_event(ContextMenuEvent::HighlightAt { depth, id }));
        }
        // Keep-first dedup is required in release; activation stays lazy to avoid build-time messages.
        for entry in model.dedup_entries() {
            let factory: Option<Rc<dyn Fn() -> Message + 'a>> = match entry.activation() {
                Activation::Dispatch { id, check } => {
                    let on_event = Rc::clone(&self.on_event);
                    // Capture the showing when building, not when the delayed factory fires.
                    let generation = self.state.generation();
                    Some(Rc::new(move || {
                        on_event(ContextMenuEvent::ActivatedAt {
                            generation,
                            id: id.clone(),
                            check,
                        })
                    }))
                }
                Activation::OpenSubmenu(id) => {
                    let on_event = Rc::clone(&self.on_event);
                    // Capture the showing when building, not when the delayed factory fires.
                    let generation = self.state.generation();
                    Some(Rc::new(move || {
                        on_event(ContextMenuEvent::SubmenuOpened {
                            generation,
                            depth,
                            parent: id.clone(),
                        })
                    }))
                }
                Activation::None => None,
            };
            menu = menu.push_item(crate::menu::EnzMenuItem::from_entry_lazy(
                entry.clone(),
                factory,
            ));
        }

        menu
    }

    /// Consume the builder and produce the widget tree.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::new(self)
    }
}
include!("context_menu_host/widget.rs");
include!("context_menu_host/header.rs");

/// Recognise Tab and Shift+Tab presses without Control, Alt or Logo.
/// Modified application chords are left unclaimed.
#[must_use]
pub fn is_focus_traversal(event: &iced::keyboard::Event) -> bool {
    matches!(
        event,
        iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Tab),
            modifiers,
            ..
        } if !modifiers.control() && !modifiers.alt() && !modifiers.logo()
    )
}

/// Translate pointer coordinates or keyboard bounds without changing the invocation kind.
#[must_use]
pub fn translate_invocation(invocation: InvocationKind, translation: Vector) -> InvocationKind {
    match invocation {
        InvocationKind::Pointer(point) => InvocationKind::Pointer(CorePoint::new(
            point.x + translation.x,
            point.y + translation.y,
        )),
        InvocationKind::Keyboard(rect) => InvocationKind::Keyboard(CoreRect::new(
            rect.x + translation.x,
            rect.y + translation.y,
            rect.width,
            rect.height,
        )),
    }
}

impl<'a, Message: Clone + 'a> From<EnzContextMenuHost<'a, Message>> for Element<'a, Message> {
    fn from(host: EnzContextMenuHost<'a, Message>) -> Self {
        Element::new(host)
    }
}
include!("context_menu_host/overlay.rs");
include!("context_menu_host/focus_restore.rs");
include!("context_menu_host/typed.rs");
