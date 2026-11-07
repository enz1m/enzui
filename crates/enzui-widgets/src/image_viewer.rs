//! `EnzImageViewer`: an inspectable image region with a controlled transform.
//!
//! Host-controlled transform; the math/validation/clamping live in
//! [`zoom_pan`](enzui_core::zoom_pan) — this module is the iced transport
//! (events, focus, drawing, toolbar).
//!
//! # Why not wrap `iced::widget::image::Viewer`
//!
//! Rendering is reused: this draws through
//! `iced::advanced::image::Renderer::draw_image`, the native viewer's own
//! primitive — no `Canvas`, no shader, no second pipeline. Literal delegation
//! is impossible, precisely: the native `scale`/`current_offset` are private
//! fields of a private-constructor `State` (no setter/getter/message), so a
//! wrapper could never set/restore/reset/read the zoom — the
//! controlled-transform contract. And composing a native `Viewer` inside a
//! same-events parent forks two disagreeing transforms (the picture follows
//! the child's). Upstream `State` accessors would lift this (a reasonable
//! enzICED patch later); not here, because the required clamped-pan/re-clamp/
//! validated-range semantics are behaviour changes upstream should not inherit
//! from a getter. Layering on the same primitive is cheaper and reversible.
//! The fitting is shared: [`fit`](ContentFit::fit) via [`ViewportGeometry::new`].
//!
//! # Controlled, not internal
//!
//! The app owns the [`ViewTransform`] and receives
//! [`on_transform`](EnzImageViewer::on_transform) per would-be change; nothing
//! moves until fed back. The `text_input`-value contract — enabling zoom
//! persistence, external toolbars, deep links. No `on_transform` = inert and
//! inert-looking (dimmed toolbar, no grab cursor): chrome that cannot act must
//! not look actionable.
//!
//! # Keyboard
//!
//! Arrows pan, `Home` fits, `+`/`=` in, `-`/`_` out. Wheel anchors at the
//! pointer, keyboard zoom at the viewport centre. Ctrl/Alt/Logo-modified
//! controls bubble to app chords; Shift stays viewer-owned.
//!
//! # Deferred
//!
//! * Zoom/pan animation (transforms apply immediately).
//! * Rotation and flip (a separate feature).
//! * Async loading, decode limits, HTTP cache: this takes a resolved
//!   [`image::Handle`], so the two land independently.

use crate::value_action::{ValueIntent, ValueRequest};
use enzui_core::focus::{PressFocus, pointer_press_position, release_focus_if_inert};
// `OwnerId` is referenced inside `effective_id` by full path, not imported.
use enzui_core::zoom_pan::{
    DEFAULT_PAN_STEP, DEFAULT_ZOOM_STEP, GrabSession, ViewTransform, ViewportGeometry,
    ZoomDirection, ZoomRange, apply_zoom, can_zoom, clamp_transform, fit_control_semantics,
    fit_transform, is_pannable, pan_by, viewer_semantics, zoom_by_factor, zoom_by_step,
    zoom_control_semantics,
};
use enzui_theme::Palette;
use iced::advanced::image::Renderer as ImageRenderer;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::widget::image;
use iced::{
    Background, Border, Color, ContentFit, Element, Event, Length, Point, Radians, Rectangle,
    Shadow, Size, Vector, keyboard,
};

use enzui_core::collect::SemanticsNode;

/// Default id-namespace (the `kind` in [`OwnerId::of_kind`](enzui_core::identity::OwnerId::of_kind)) for a viewer
/// whose caller supplied no explicit [`id`](EnzImageViewer::id).
///
/// Identity derives from caller-supplied [`.owner_key()`](EnzImageViewer::owner_key),
/// never alt text: alt text is a DESCRIPTION ("Preview" shared by different
/// images) — deriving from it silently aliases state. No `.owner_key()` and
/// no `.id()` = ANONYMOUS: still collected and announced (AT reads the alt
/// text), but unroutable. Anonymous is honest; aliased is not. Used as the
/// `kind` in `OwnerId::of_kind`, hence injective and stable.
pub const DEFAULT_ID_PREFIX: &str = "enz-image-viewer";

/// Sanitise an f32 to a finite, non-negative value (non-finite -> 0).
///
/// `radius` is a plain `pub f32` (`.tokens(t)` routes anything in), reaching
/// the clip MASK (`with_layer_rounded`) and two `Quad` borders — and the
/// backends disagree on non-finite masks: wgpu's SDF rejects every pixel
/// (child masked away), tiny-skia's `min/max` keeps NaN (nothing masked). House
/// rule: NaN/negatives inexpressible, degrade to 0; `0` honoured literally.
fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerKey {
    PanLeft,
    PanRight,
    PanUp,
    PanDown,
    Fit,
    ZoomIn,
    ZoomOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ViewerKeyClaim {
    Named(keyboard::key::Named, ViewerKey),
    Text(&'static str, ViewerKey),
}

/// The decoder and its modifier matrix consume this single claim inventory.
/// Adding or removing a claimed key therefore changes the tested population.
const VIEWER_KEY_CLAIMS: [ViewerKeyClaim; 9] = [
    ViewerKeyClaim::Named(keyboard::key::Named::ArrowLeft, ViewerKey::PanLeft),
    ViewerKeyClaim::Named(keyboard::key::Named::ArrowRight, ViewerKey::PanRight),
    ViewerKeyClaim::Named(keyboard::key::Named::ArrowUp, ViewerKey::PanUp),
    ViewerKeyClaim::Named(keyboard::key::Named::ArrowDown, ViewerKey::PanDown),
    ViewerKeyClaim::Named(keyboard::key::Named::Home, ViewerKey::Fit),
    ViewerKeyClaim::Text("+", ViewerKey::ZoomIn),
    ViewerKeyClaim::Text("=", ViewerKey::ZoomIn),
    ViewerKeyClaim::Text("-", ViewerKey::ZoomOut),
    ViewerKeyClaim::Text("_", ViewerKey::ZoomOut),
];

/// Decode the focused viewer's keyboard controls without claiming application
/// chords. Ctrl, Alt, and Logo belong to the application/compositor, while
/// Shift remains available for the viewer's text aliases (`+` and `_`).
fn viewer_key(
    key: &keyboard::Key,
    text: Option<&str>,
    modifiers: keyboard::Modifiers,
) -> Option<ViewerKey> {
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }

    if let keyboard::Key::Named(named) = key
        && let Some(action) = VIEWER_KEY_CLAIMS.iter().find_map(|claim| match claim {
            ViewerKeyClaim::Named(claimed, action) if claimed == named => Some(*action),
            ViewerKeyClaim::Named(_, _) | ViewerKeyClaim::Text(_, _) => None,
        })
    {
        return Some(action);
    }

    text.and_then(|text| {
        VIEWER_KEY_CLAIMS.iter().find_map(|claim| match claim {
            ViewerKeyClaim::Text(claimed, action) if *claimed == text => Some(*action),
            ViewerKeyClaim::Named(_, _) | ViewerKeyClaim::Text(_, _) => None,
        })
    })
}

/// Focus-ring inset.
pub const FOCUS_RING_INSET: f32 = 2.0;

/// Alpha applied to inert chrome, matching the carousel's disabled chrome.
pub const DISABLED_ALPHA: f32 = 0.38;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Theme-derived colours and metrics for [`EnzImageViewer`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageViewerTokens {
    /// The backdrop behind the picture (`image-canvas`).
    pub canvas: Color,
    /// The viewer outline (`border`).
    pub border: Color,
    /// The keyboard focus ring (`accent`).
    pub focus_ring: Color,
    /// Corner radius.
    pub radius: f32,
}

impl ImageViewerTokens {
    /// Derive tokens from `theme`: its palette, and its `media` shape for the
    /// corners.
    #[must_use]
    pub fn from_theme(theme: &iced::Theme) -> Self {
        Self {
            radius: crate::theme_lookup::shape_radius(theme, |s| s.media),
            ..crate::theme_lookup::resolve(theme, Self::from_palette)
        }
    }

    /// Derive tokens from a runtime palette.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            canvas: p.image_canvas,
            border: p.border,
            focus_ring: p.accent,
            radius: enzui_theme::text_defaults::shape_radius(|s| s.media),
        }
    }

    /// Dark preset.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light preset.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// An inspectable image region with an application-owned zoom/pan transform.
pub struct EnzImageViewer<'a, Message> {
    handle: image::Handle,
    transform: ViewTransform,
    range: ZoomRange,
    fit: ContentFit,
    filter: image::FilterMethod,
    pan_step: f32,
    zoom_step: f32,
    alt_text: Option<String>,
    on_transform: Option<Box<dyn Fn(ViewTransform) -> Message + 'a>>,
    width: Length,
    height: Length,
    tokens: Option<ImageViewerTokens>,
    id: Option<iced::advanced::widget::Id>,
    /// A caller-supplied stable key for collision-safe identity.
    owner_key: Option<String>,
    /// A pending command the host wants resolved against live geometry.
    pending_command: Option<ViewerCommand>,
}

/// Construct a viewer over an image handle and an application-owned transform.
pub fn enz_image_viewer<'a, Message>(
    handle: impl Into<image::Handle>,
    transform: ViewTransform,
) -> EnzImageViewer<'a, Message> {
    EnzImageViewer {
        handle: handle.into(),
        transform,
        range: ZoomRange::default(),
        fit: ContentFit::Contain,
        filter: image::FilterMethod::default(),
        pan_step: DEFAULT_PAN_STEP,
        zoom_step: DEFAULT_ZOOM_STEP,
        alt_text: None,
        on_transform: None,
        width: Length::Fill,
        height: Length::Fill,
        tokens: None,
        id: None,
        owner_key: None,
        pending_command: None,
    }
}

impl<'a, Message> EnzImageViewer<'a, Message> {
    /// Publish transform changes. Without this the viewer is read-only.
    #[must_use]
    pub fn on_transform(mut self, f: impl Fn(ViewTransform) -> Message + 'a) -> Self {
        self.on_transform = Some(Box::new(f));
        self
    }

    /// Publish transform changes, or leave the viewer inert if `None`.
    #[must_use]
    pub fn on_transform_maybe(
        mut self,
        f: Option<Box<dyn Fn(ViewTransform) -> Message + 'a>>,
    ) -> Self {
        self.on_transform = f;
        self
    }

    /// Set the permitted zoom range. Invalid ranges fall back to the default
    /// rather than being silently repaired by swapping.
    #[must_use]
    pub fn zoom_range(mut self, range: ZoomRange) -> Self {
        self.range = range;
        self
    }

    /// Set the content fit applied before zooming.
    #[must_use]
    pub fn fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }

    /// Set the sampling filter.
    #[must_use]
    pub fn filter_method(mut self, filter: image::FilterMethod) -> Self {
        self.filter = filter;
        self
    }

    /// Distance one arrow-key press pans.
    #[must_use]
    pub fn pan_step(mut self, step: f32) -> Self {
        self.pan_step = if step.is_finite() && step > 0.0 {
            step
        } else {
            DEFAULT_PAN_STEP
        };
        self
    }

    /// Multiplicative step for one wheel notch or `+`/`-` press.
    #[must_use]
    pub fn zoom_step(mut self, step: f32) -> Self {
        self.zoom_step = if step.is_finite() && step > 0.0 {
            step
        } else {
            DEFAULT_ZOOM_STEP
        };
        self
    }

    /// Accessible name for the picture.
    #[must_use]
    pub fn alt_text(mut self, text: impl Into<String>) -> Self {
        self.alt_text = Some(text.into());
        self
    }

    /// Override the theme-derived tokens.
    #[must_use]
    pub fn tokens(mut self, tokens: ImageViewerTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Stable widget id, for focus targeting.
    #[must_use]
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Set the width.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the height.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Caller-supplied stable key for collision-safe identity.
    ///
    /// Two viewers with the same alt text but different keys get different
    /// widget ids; two viewers with no key are anonymous — collected
    /// and announced, but not routable for AT actions. This follows the
    /// `OwnerId` contract: descriptions are not identity.
    #[must_use]
    pub fn owner_key(mut self, key: impl Into<String>) -> Self {
        self.owner_key = Some(key.into());
        self
    }

    /// Queue a command to be resolved against the widget's own geometry.
    ///
    /// The host sets this once; the widget applies it at the next layout
    /// cycle and publishes the result through `on_transform`.  Prefer
    /// this over calling [`apply_toolbar_action`] in the host, which
    /// requires the host to duplicate the renderer geometry measurement.
    #[must_use]
    pub fn command(mut self, cmd: ViewerCommand) -> Self {
        self.pending_command = Some(cmd);
        self
    }

    /// The transform currently being displayed.
    #[must_use]
    pub fn transform_value(&self) -> ViewTransform {
        self.transform
    }

    /// The effective zoom range.
    #[must_use]
    pub fn range_value(&self) -> ZoomRange {
        self.range
    }

    /// Whether the viewer publishes changes.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        self.on_transform.is_some()
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ViewerState {
    focus: enzui_core::focus::FocusState,
    grab: Option<ActiveGrab>,
    /// A routed AT value change (`Increment`/`Decrement`/`SetValue`) latched by
    /// [`operate`](EnzImageViewer::operate) and applied by
    /// [`update`](EnzImageViewer::update) on the next event pass, through the
    /// viewer's own zoom arithmetic — the same `value_action` mechanism
    /// `stepper` uses. Latching rather than publishing here keeps the
    /// caller's `on_transform` closure off the per-frame operate sweep.
    pending_value: Option<ValueIntent>,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide this viewer's ring.
    window_key: u64,
    /// Monotone counter incremented every time the *authoritative* transform
    /// changes externally: source replacement, range change, or a host reset
    /// A grab started under generation `g` is stale as soon as the
    /// current generation advances past `g`, and is cancelled before it can
    /// overwrite the newer state.
    transform_generation: u64,
    /// The transform the widget last saw from the builder, used to detect
    /// external resets (host changed the transform outside of a gesture).
    last_seen_transform: Option<ViewTransform>,
    /// The transform most recently published by this widget via
    /// `on_transform`.  Compared against the builder's transform in
    /// `diff()` to distinguish a normal feedback loop (host feeding back
    /// what this widget published) from an external reset (host set a completely
    /// different transform).  Only an external reset advances the
    /// generation and cancels a grab.
    last_published_transform: Option<ViewTransform>,
    /// A command latched from the builder during `diff`, consumed by
    /// the next `update` pass.
    latched_command: Option<ViewerCommand>,
}

/// A grab session paired with the generation it was started under.
#[derive(Debug, Clone, Copy)]
struct ActiveGrab {
    session: GrabSession,
    generation: u64,
}

// ---------------------------------------------------------------------------
// Widget impl
// ---------------------------------------------------------------------------

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzImageViewer<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ViewerState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ViewerState::default())
    }

    /// release stale focus at the rebuild, not at the next `operate`.
    ///
    /// See `stepper.rs`'s `diff`. "Can take input" = `on_transform.is_some()`
    /// (the same gate `operate` hangs `focusable(..)` on): a read-only picture
    /// is no Tab stop, so a frame-ago-pannable viewer must not keep painting
    /// the ring `draw` reads. Also detects an EXTERNAL transform reset (host
    /// set outside a gesture — source swap, range change, toolbar command)
    /// and cancels grabs whose generation went stale.
    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<ViewerState>();
        let _ = release_focus_if_inert(&mut state.focus, self.on_transform.is_some());

        // detect an *external* transform change — the host set a
        // transform that is neither what we last saw nor what we last
        // published.  The normal feedback loop (host echoing our published
        // transform back) does not count as external.
        let current = self.transform;
        let is_external_reset = state
            .last_seen_transform
            .is_some_and(|prev| prev != current && state.last_published_transform != Some(current));
        if is_external_reset {
            state.transform_generation = state.transform_generation.wrapping_add(1);
            // A grab started under an earlier generation is stale: its
            // snapshot records the old transform, and continuing it would
            // overwrite the host's newer state.  Cancel it.
            if let Some(active) = &state.grab
                && active.generation != state.transform_generation
            {
                state.grab = None;
            }
        }
        state.last_seen_transform = Some(current);

        // Latch a pending command from the builder so `update` can
        // resolve it against live geometry on the next event pass.
        if let Some(cmd) = self.pending_command.take() {
            state.latched_command = Some(cmd);
        }
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        layout::Node::new(limits.resolve(self.width, self.height, Size::ZERO)).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<ViewerState>();

        // The routable identity: the caller's explicit id, a stable
        // `OwnerId` from the caller's key, or `None` (anonymous).  An
        // anonymous node is collected and announced but absent from
        // `NodeIdMap`, hence unroutable.
        let id = self.effective_id();
        let id_ref = id.as_ref();

        // Only a viewer that can actually move is a Tab stop. A read-only
        // picture that swallows focus and then ignores every arrow key is a
        // dead stop, which is exactly the trap `focus_button` avoids.
        if self.on_transform.is_some() {
            operation.focusable(id_ref, layout.bounds(), &mut state.focus);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }

        // The AT tree publishes zoom percent, numeric scale and the
        // steppable/adjustable contract from the transform — routed through
        // the same clamp render uses, so AT never hears a scale outside the
        // advertised range (the semantics half of the choke point). Even
        // unmeasurable (`geometry.is_empty()`), `clamp_transform` still clamps
        // SCALE — the field semantics read.
        let geometry = self.geometry(renderer, layout.bounds().size());
        // `interactive` is the CAPABILITY half — an `on_transform` handler
        // wired at all — not the focus flag. An inert-but-focusable viewer
        // stays readable (alt text) while advertising no unserviceable
        // `adjustable`/step actions. The two once shared a parameter: inert+
        // focused announced adjustable, interactive+unfocused did not. Both
        // wrong.
        let semantics = viewer_semantics(
            self.alt_text.as_deref(),
            self.effective_transform(geometry),
            self.range,
            self.on_transform.is_some(),
            state.focus.is_focused(),
        );
        let mut node = SemanticsNode::new(semantics);
        operation.custom(id_ref, layout.bounds(), &mut node);
        operation.container(id_ref, layout.bounds(), viewport);

        // Offer the viewer's real zoom arithmetic to routed
        // Increment/Decrement/SetValue (pointer- and key-free AT zoom). The
        // viewer advertises `adjustable`; without this every announced step
        // dropped — AT reporting success for a zoom that never happened.
        // Gated on `on_transform` (inert = `adjustable(false)`: advertises
        // nothing, services nothing) and on a routable id (id-less
        // `ValueRequest` matches nothing). Only OFFERED: latched and applied
        // by `update` next event pass, never on the per-frame operate sweep.
        if self.on_transform.is_some()
            && let Some(id) = &id
        {
            let mut request = ValueRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if let Some(intent) = request.take() {
                tree.state.downcast_mut::<ViewerState>().pending_value = Some(intent);
            }
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        // feed the global input modality from every raw event, so a
        // pointer-last frame suppresses the keyboard ring even in an app with
        // no `focus_button` to keep the modality current. Runs before the
        // no-`on_transform` early-return so an inert viewer still keeps the
        // this window's modality current. Keyed per window and
        // stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<ViewerState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        let Some(on_transform) = self.on_transform.as_ref() else {
            return;
        };

        let bounds = layout.bounds();

        // Exclusive press-focus needs a different "inside" than the
        // paint/hit mask: it reads the RAW press position, computed before
        // the cursor masks below. The masked cursor answers "visible takeable
        // pixel" (declines clipped corner slivers — right for Take/grab/
        // wheel), but focus-RELEASE asks "somewhere this control does not
        // own": a (600,600) press must DROP focus, yet `masked_cursor`
        // collapses "outside frame" + "corner sliver" + "no cursor" into one
        // `Unavailable` (shared `press_focus` reads: `Leave`, do nothing) —
        // breaking `outside_press_still_unfocuses`. So the focus decision uses
        // raw position + rounded hit-test (`press_focus_rounded` three-way).
        let raw_press = pointer_press_position(event, cursor).flatten();

        // Radius-aware hit exclusion: a cursor in a clipped corner sliver is
        // invisible (nothing painted) — no focus, no grab, no wheel notch.
        // `Unavailable` makes `position_over`/`is_over`/`pointer_press_position`
        // all decline (as `glow.rs` for its delegated child). A LIVE grab keeps
        // panning: `CursorMoved` reads the raw event `position`, not this
        // cursor — a legally-begun gesture owns the drag across corners
        // (the grab-commit rule below).
        let cursor = self.masked_cursor(bounds, cursor);

        let geometry = self.geometry(renderer, bounds.size());

        // Every gesture computes from the transform as it is *observed*, i.e.
        // clamped against the live geometry and range. If the caller
        // supplied an out-of-range transform, the first gesture then publishes
        // a legal value (`next != transform`, below) rather than nudging an
        // illegal one, and gesture math is never seeded from an off-limit base.
        let transform = self.effective_transform(geometry);

        // A single publish path: every branch computes a candidate and hands
        // it here, so "no-op publishes nothing" and "everything clamped" are
        // widget properties, not per-branch discipline. Baseline is the clamped
        // `transform`: no-ops judge against what is drawn. Returns whether it
        // published: the wheel arm gates `capture_event` on it, so a limit-hit
        // notch (no change) does not swallow the event — a viewer in a
        // scrollable page hands the wheel back at min/max zoom, else it traps
        // page-scroll silently (the carousel's boundary rule). Other captures
        // stay unconditional, and message-only no-op tests cannot see the trap.
        // Real-gesture callers (grab-began press, live-grab move) capture
        // regardless: capture tracks "I acted", not "I published" — a live
        // gesture owns the event through clamped no-op frames. Tracks the last
        // published transform so `diff()` tells host echo (fed-back output)
        // from external reset.
        let published_cell = std::cell::Cell::new(None::<ViewTransform>);
        let publish = |next: ViewTransform, shell: &mut Shell<'_, Message>| -> bool {
            if next != transform {
                shell.publish(on_transform(next));
                shell.request_redraw();
                published_cell.set(Some(next));
                true
            } else {
                false
            }
        };

        // apply a routed AT value change latched by `operate`, through
        // the same `zoom_by_step`/`apply_zoom` the keyboard and toolbar paths
        // use (see `routed_transform`). Taken unconditionally so an armed viewer
        // rebuilt without geometry still discards a stale latch; `routed_transform`
        // and `publish` both suppress a no-op, so a routed zoom already at the
        // range end publishes nothing.
        if let Some(intent) = tree
            .state
            .downcast_mut::<ViewerState>()
            .pending_value
            .take()
            && let Some(next) = self.routed_transform(&intent, geometry)
        {
            // A routed AT value change is not a propagating input event, so
            // there is nothing to capture; the publish result is unused.
            let _ = publish(next, shell);
        }

        // apply a latched command against the widget's own geometry.
        // The host set `.command(ViewerCommand::Toolbar(..))` on the
        // builder; `diff()` latched it into state; here we resolve it
        // against the live geometry and publish the result.
        if let Some(cmd) = tree
            .state
            .downcast_mut::<ViewerState>()
            .latched_command
            .take()
        {
            let next = match cmd {
                ViewerCommand::Toolbar(action) => {
                    use iced::advanced::Renderer as _;
                    // Resolve `Actual` to a true physical
                    // one-image-pixel-per-screen-pixel scale: divide out the
                    // renderer's total scale (window * application).
                    // `renderer.scale()` is `None` where unreported (tiny-skia,
                    // headless test renderer): fall back to 1.0, exactly the
                    // logical-pixel path. Every non-`Actual` action ignores it.
                    let scale_factor = renderer
                        .scale()
                        .map_or(1.0, iced::advanced::renderer::Scale::total);
                    apply_toolbar_action_scaled(
                        action,
                        transform,
                        geometry,
                        self.range,
                        self.zoom_step,
                        scale_factor,
                    )
                }
            };
            let _ = publish(next, shell);
        }

        let state = tree.state.downcast_mut::<ViewerState>();

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };

                let y = match *delta {
                    mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => y,
                };

                let Some(direction) = ZoomDirection::from_scroll(y) else {
                    return;
                };

                let local = Point::new(position.x - bounds.x, position.y - bounds.y);
                let next = zoom_by_step(
                    transform,
                    geometry,
                    self.range,
                    self.zoom_step,
                    direction,
                    local,
                );

                // A wheel notch mid-drag must not make the content jump: the
                // grab is rebased onto the new scale at the current pointer.
                let mid_drag = state.grab.is_some();
                if let Some(active) = state.grab {
                    state.grab = Some(ActiveGrab {
                        session: active.session.rebase(local, next),
                        generation: state.transform_generation,
                    });
                }

                // Capture only when the wheel actually did something: it
                // changed the transform (`published`), or it is part of a live
                // pan gesture (`mid_drag` — the button is held, so the wheel is
                // the user's, not the page's). A bare wheel notch at a zoom
                // limit changes nothing and belongs to the scrollable page
                // behind the viewer, so it must propagate.
                let published = publish(next, shell);
                if published || mid_drag {
                    shell.capture_event();
                }
            }

            // A touchpad pinch zooms continuously about the pointer.
            Event::Gesture(iced::gesture::Event::Pinch { scale, .. }) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                let local = Point::new(position.x - bounds.x, position.y - bounds.y);
                let next = zoom_by_factor(transform, geometry, self.range, 1.0 + *scale, local);
                if let Some(active) = state.grab {
                    state.grab = Some(ActiveGrab {
                        session: active.session.rebase(local, next),
                        generation: state.transform_generation,
                    });
                }
                let _ = publish(next, shell);
                shell.capture_event();
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Position rule lives in `enzui_core::focus` — one
                // definition. `Drop` (elsewhere-press takes focus: arrows
                // stop panning the moved-on-from viewer), `Take`
                // (picture-press focuses: the only reason the keyboard map is
                // reachable without Tab) and `Leave` (unresolvable position
                // holds focus, since touch-only devices report `Unavailable`
                // and unfocusing there would kill their keyboard map). Uses `raw_press` + ROUNDED frame
                // (corner/outside must Drop, only visible pixels Take); the
                // shared `hit_radius` paints and hit-tests, so no drift.
                match self.press_focus_rounded(raw_press, bounds) {
                    PressFocus::Take => state.focus.focus(),
                    PressFocus::Drop => {
                        state.focus.unfocus();
                        return;
                    }
                    PressFocus::Leave => return,
                }

                // The grab needs the press position, which `Take` guarantees is
                // inside `bounds`.
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };

                // The grab starts even when nothing is pannable, so press,
                // wheel-zoom in, drag still pans; fitted drags clamp to a
                // no-op. Inertness is shown by the cursor: `mouse_interaction`
                // consults `is_pannable`.

                let local = Point::new(position.x - bounds.x, position.y - bounds.y);
                state.grab = Some(ActiveGrab {
                    session: GrabSession::begin(local, transform),
                    generation: state.transform_generation,
                });
                shell.capture_event();
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let handles_event = state.grab.take().is_some();
                if handles_event {
                    shell.request_redraw();
                }
            }

            // Losing pointer/window mid-pan ends the grab — else immortal:
            // `ButtonReleased` is the only other clearer, so a `CursorLeft` /
            // `Unfocused` (alt-tab, modal) grab stays live and the next
            // `CursorMoved` resumes from the stale origin, teleporting by the
            // unseen travel (`EnzMarqueeArea` closes the same hole). COMMIT,
            // never cancel: a pan applies incrementally — every move already
            // published a clamped legal transform the app adopted — so the
            // reached state is real, not provisional; restoring pre-grab would
            // discard app-owned pan with no user action. Nothing published
            // (no transform change) — only the cursor-affordance redraw as
            // `Grabbing` leaves.
            Event::Mouse(mouse::Event::CursorLeft)
            | Event::Window(iced::window::Event::Unfocused) => {
                let handles_event = state.grab.take().is_some();
                if handles_event {
                    shell.request_redraw();
                }
            }

            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                let Some(active) = state.grab else {
                    return;
                };

                let local = Point::new(position.x - bounds.x, position.y - bounds.y);
                // A move during a live grab always captures, even on a frame
                // where the clamp makes it a no-op (dragging into the pannable
                // edge): the button is held, the gesture owns the event, and
                // releasing it to the page mid-drag would be wrong. So the
                // publish result is intentionally not gated on here.
                let _ = publish(
                    active.session.transform_at(local, geometry, self.range),
                    shell,
                );
                shell.capture_event();
            }

            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                text,
                modifiers,
                ..
            }) => {
                if !state.focus.is_focused() {
                    return;
                }

                let centre = Point::new(bounds.width / 2.0, bounds.height / 2.0);
                let step = self.pan_step;

                let next =
                    viewer_key(key, text.as_deref(), *modifiers).map(|action| match action {
                        ViewerKey::PanLeft => {
                            pan_by(transform, geometry, self.range, Vector::new(-step, 0.0))
                        }
                        ViewerKey::PanRight => {
                            pan_by(transform, geometry, self.range, Vector::new(step, 0.0))
                        }
                        ViewerKey::PanUp => {
                            pan_by(transform, geometry, self.range, Vector::new(0.0, -step))
                        }
                        ViewerKey::PanDown => {
                            pan_by(transform, geometry, self.range, Vector::new(0.0, step))
                        }
                        ViewerKey::Fit => fit_transform(self.range),
                        ViewerKey::ZoomIn => zoom_by_step(
                            transform,
                            geometry,
                            self.range,
                            self.zoom_step,
                            ZoomDirection::In,
                            centre,
                        ),
                        ViewerKey::ZoomOut => zoom_by_step(
                            transform,
                            geometry,
                            self.range,
                            self.zoom_step,
                            ZoomDirection::Out,
                            centre,
                        ),
                    });

                if let Some(next) = next {
                    // The `if let Some(next)` gate = recognised viewer control
                    // (arrow/Home/+-): acted, so captured even when the clamp
                    // no-ops (edge arrow, max-zoom `+`) — a focused viewer's
                    // documented keys must not leak to the page behind.
                    // Unrecognised keys fall to `None` above and propagate:
                    // capture tracks "handled", not "published".
                    let _ = publish(next, shell);
                    shell.capture_event();
                }
            }

            _ => {}
        }

        // record the last published transform so `diff()` can
        // distinguish a host echo from an external reset.
        if let Some(pub_t) = published_cell.get() {
            tree.state
                .downcast_mut::<ViewerState>()
                .last_published_transform = Some(pub_t);
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;

        let tokens = self
            .tokens
            .unwrap_or_else(|| ImageViewerTokens::from_theme(theme));
        // `radius` is a plain `pub f32` (`.tokens(t)` the only path): sanitize
        // once at the resolve site, use `radius` below (three quad borders +
        // one clip mask). Skipping one panics tiny-skia building a rounded-rect
        // path from NaN/INFINITY (`expect("Build rounded rectangle path")`).
        let radius = finite_nonneg(tokens.radius);
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<ViewerState>();

        // Backdrop. Drawn even when the image cannot be measured, so a
        // still-loading or broken source leaves a tokenized frame rather than
        // a hole.
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    radius: radius.into(),
                    ..Border::default()
                },
                shadow: Shadow::default(),
                snap: true,
            },
            Background::Color(tokens.canvas),
        );

        let geometry = self.geometry(renderer, bounds.size());
        if !geometry.is_empty() {
            // Clamp against the live geometry so a caller transform that is
            // out of range (or off-limit for this image/viewport/fit) can never
            // be *drawn* out of range. This is the render half of the
            // single choke point; see `effective_transform`.
            let transform = self.effective_transform(geometry);
            let scaled = geometry.scaled(transform.scale());

            // Placement (centre scaled content, displace by scroll-sign pan)
            // is `ImageProjection`'s origin — the same value
            // `display_to_image`/`image_to_display` invert: painted pixels and
            // annotation-overlay pixels cannot drift. The letterbox term: under
            // `Contain` a non-square source leaves slack on exactly one axis
            // (vertical/landscape, horizontal/portrait) — what a width-only
            // scale drops. `None` is unreachable under `!is_empty()` yet
            // HANDLED, not unwrapped: a `draw` panic takes the window; one
            // skipped frame keeps tokenized frame + focus ring. Chained into
            // the `if let Some(cull)` below for the same reason (no early
            // `return` skipping the ring).
            let placement = enzui_core::image_space::ImageProjection::from_geometry(
                bounds.position(),
                geometry,
                transform,
            )
            .map(enzui_core::image_space::ImageProjection::origin);

            // One rounded mask, at the FRAME's real corners — never
            // `bounds ∩ viewport`. The intersection CULLS, never shapes:
            // rounding it invents an arc where the viewport truncates the frame
            // (a scrolled viewer grows a mid-image corner on a scroll-cut
            // straight edge). Frame bounds carry the arc; the intersection is
            // RECTANGULAR cull only (`RoundedClip::draw`'s split: rounded layer
            // = full `clip_bounds(bounds)`, child handed the narrowed rect for
            // straight-edge cuts). The layer reaches the image pipeline on both
            // backends (wgpu shader `clip_radius`, tiny-skia `_force_mask`):
            // one mask, no second. So `Image.border_radius` stays 0 — setting
            // it double-rounds on wgpu (its coverage MULTIPLIES the layer's;
            // `image.wgsl:122-126`) while tiny-skia fringes once. One resolved
            // radius, one mask, sanitised at `draw` top (non-finite into a mask
            // is materially worse than into a `Quad`). Fully-offscreen
            // (`intersection` None): picture skipped, ring below still runs.
            if let Some(cull) = bounds.intersection(viewport)
                && let Some(placement) = placement
            {
                // `scaled`, not `projection.image_rect(geometry.image)`: the
                // two agree mathematically, but the latter is
                // `image * (scaled / image)`, which is not bit-identical to
                // `scaled` in `f32`, and every existing pixel test measures
                // the rectangle this expression produces.
                let drawing = Rectangle {
                    x: placement.x,
                    y: placement.y,
                    width: scaled.width,
                    height: scaled.height,
                };

                // Two nested layers: one rectangle cannot say "round at FRAME
                // corners" and "cull to VIEWPORT". OUTER rectangular
                // `with_layer(cull)` — viewport intersection, scissor for
                // scrolled-off content (tiny-skia's rounded inner sets
                // `force_mask`, making `draw_image` skip its own rectangular
                // `clip_bounds`: without this a scrolled frame paints past the
                // edge). INNER `with_layer_rounded(bounds, radius)` — the
                // FRAME's real corners, never the intersection (no invented
                // arcs on viewport-truncated straight edges).
                renderer.with_layer(cull, |renderer| {
                    renderer.with_layer_rounded(bounds, radius.into(), |renderer| {
                        renderer.draw_image(
                            iced::advanced::image::Image {
                                handle: self.handle.clone(),
                                filter_method: self.filter,
                                rotation: Radians(0.0),
                                opacity: 1.0,
                                border_radius: iced::border::Radius::default(),
                            },
                            drawing,
                            cull,
                        );
                    });
                });
            }
        }

        // The outline goes over the image: an image that reaches the frame
        // would otherwise cover it there and leave it broken.
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: tokens.border,
                    width: 1.0,
                    radius: radius.into(),
                },
                shadow: Shadow::default(),
                snap: true,
            },
            Background::Color(iced::Color::TRANSPARENT),
        );

        // Use the shared focus-visible policy rather than raw focus so
        // a pointer click focuses the viewer without painting the keyboard ring.
        if enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key) {
            // shared ring painter. This site insets (negative
            // outset) so the ring lands inside the viewport edge instead of
            // outside it — the image is clipped to `bounds`, and an outset
            // ring would paint over whatever is laid out beside the viewer.
            crate::focus_button::paint_focus_ring(
                renderer,
                bounds,
                crate::focus_button::FocusRingQuad {
                    ink: tokens.focus_ring,
                    width: crate::theme_lookup::focus_ring_width(theme),
                    radius: radius.into(),
                    outset: -FOCUS_RING_INSET,
                    snap: true,
                },
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<ViewerState>();

        if state.grab.is_some() {
            return mouse::Interaction::Grabbing;
        }

        let bounds = layout.bounds();
        // Same corner exclusion as `update`: the grab affordance must
        // not be offered in a clipped corner the picture does not fill.
        let cursor = self.masked_cursor(bounds, cursor);
        if self.on_transform.is_none() || !cursor.is_over(bounds) {
            return mouse::Interaction::None;
        }

        // Offer the grab affordance only when a drag could move something —
        // the rule that inert chrome must look inert, applied to the
        // cursor.
        let geometry = self.geometry(renderer, bounds.size());
        if is_pannable(geometry, self.effective_transform(geometry).scale()) {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }
}

impl<Message> EnzImageViewer<'_, Message> {
    /// Resolve the viewport geometry against the renderer's measurement of the
    /// image.
    ///
    /// `measure_image` returning `None` (undecodable, still loading, missing
    /// file) yields an empty geometry, which every downstream operation
    /// already treats as "nothing to inspect".
    fn geometry(&self, renderer: &iced::Renderer, bounds: Size) -> ViewportGeometry {
        let measured = renderer.measure_image(&self.handle).unwrap_or_default();
        let image = Size::new(measured.width as f32, measured.height as f32);
        ViewportGeometry::new(image, bounds, self.fit)
    }

    /// The single resolved frame radius governing both the rounded
    /// paint mask and the rounded hit test.
    ///
    /// `draw` resolves the same value from theme tokens
    /// (`finite_nonneg(tokens.radius)`); `update`/`mouse_interaction` have no
    /// `theme`, and `from_palette` takes `radius` from the published `media`
    /// shape family, so the default matches paint exactly unless custom
    /// `.tokens(t)` (which wins here too: paint and hit-test never disagree).
    /// Sanitised identically (`finite_nonneg`): non-finite/negative degrades
    /// to square hit region, matching the square paint mask.
    fn hit_radius(&self) -> f32 {
        finite_nonneg(self.tokens.map_or_else(
            || enzui_theme::text_defaults::shape_radius(|s| s.media),
            |t| t.radius,
        ))
    }

    /// Present a cursor sitting in a clipped corner as
    /// [`Unavailable`](mouse::Cursor::Unavailable): wheel/press/pan and the
    /// grab cursor decline the four rounded-corner slivers the frame never
    /// paints (the `glow`/`rounded_clip` rule). Predicate is the shared
    /// [`point_in_rounded_rect`](enzui_core::rounded_geometry::point_in_rounded_rect)
    /// (also `glow.rs`'s) on frame `bounds` + the one [`hit_radius`](Self::hit_radius):
    /// the excluded region is the EXACT complement of the painted mask.
    /// Outside-the-box or unavailable passes through unchanged.
    fn masked_cursor(&self, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Cursor {
        let radius = iced::border::Radius::from(self.hit_radius());
        match cursor.position() {
            Some(p) if !enzui_core::rounded_geometry::point_in_rounded_rect(p, bounds, radius) => {
                mouse::Cursor::Unavailable
            }
            _ => cursor,
        }
    }

    /// The exclusive press-focus decision for a press at `press`, against
    /// the *rounded* frame — deliberately not [`masked_cursor`](Self::masked_cursor)'s
    /// "inside" (take/grab/wheel) nor the plain-rectangle
    /// [`press_focus`](enzui_core::focus::press_focus).
    ///
    /// Three-way, mirroring `press_focus`: `None` (touch-only `Unavailable`)
    /// → [`Leave`](PressFocus::Leave) (sweeping there kills touch hardware's
    /// keyboard map); inside the rounded frame → [`Take`](PressFocus::Take)
    /// (visible pixel focuses); anywhere else — corner sliver or fully outside
    /// → [`Drop`](PressFocus::Drop) (releases focus like empty page space).
    /// Rounded frame uses [`hit_radius`](Self::hit_radius) — the one radius
    /// `draw` paints — so no drift. Inexpressible as
    /// `press_focus(masked, ..)`: masking fuses "outside" (Drop) with "no
    /// cursor" (Leave) into one `Unavailable` that can never Drop.
    fn press_focus_rounded(&self, press: Option<Point>, bounds: Rectangle) -> PressFocus {
        let Some(p) = press else {
            return PressFocus::Leave;
        };
        let radius = iced::border::Radius::from(self.hit_radius());
        if enzui_core::rounded_geometry::point_in_rounded_rect(p, bounds, radius) {
            PressFocus::Take
        } else {
            PressFocus::Drop
        }
    }

    /// The transform as anything downstream is allowed to observe it.
    ///
    /// # Why clamping is lazy, at the point of use
    ///
    /// `enz_image_viewer(handle, transform)` stores the caller's value VERBATIM
    /// (no `.transform(..)` setter exists) — before `.zoom_range(..)`/`.fit(..)`
    /// chain and before measurement. A caller can construct an out-of-range /
    /// illegal-offset transform. Construction-clamping cannot see the eventual
    /// range; per-setter re-clamping reintroduces builder-order dependence
    /// (the `icon_label` `sane_font_size` shape: `.a(x).b(y)` vs `.b(y).a(x)`
    /// diverging). So the stored value stays as-given (`transform_value()`
    /// reports the supply) and every render/derive funnels through this one
    /// method — a pure clamp vs range + live geometry:
    ///
    /// * render, semantics, `ToolbarAvailability`, cursor affordance never see
    ///   out-of-range scale or off-limit offset;
    /// * recomputed vs live geometry: resize/source/fit re-clamps free — no
    ///   stale cached transform goes illegal;
    /// * reads `self.range` at use: `.transform(x).range(r)` ≡
    ///   `.range(r).transform(x)` by construction (one choke point, no
    ///   orderable per-setter state).
    fn effective_transform(&self, geometry: ViewportGeometry) -> ViewTransform {
        clamp_transform(self.transform, geometry, self.range)
    }

    /// The widget `Id` this viewer publishes for
    /// focus targeting and AT value-action routing.
    ///
    /// Resolution order:
    ///
    /// 1. Caller-set [`.id()`](EnzImageViewer::id) verbatim.
    /// 2. Stable [`OwnerId`](enzui_core::identity::OwnerId) from
    ///    [`DEFAULT_ID_PREFIX`] + caller [`.owner_key()`](EnzImageViewer::owner_key).
    /// 3. **`None`** — ANONYMOUS. Still collected and announced (alt text
    ///    reaches AT), but unroutable: AT Increment/Decrement resolves to
    ///    nothing. Honest; alt-text-derived identity would alias.
    fn effective_id(&self) -> Option<iced::advanced::widget::Id> {
        if let Some(id) = &self.id {
            return Some(id.clone());
        }
        if let Some(key) = &self.owner_key {
            let owner = enzui_core::identity::OwnerId::of_kind(DEFAULT_ID_PREFIX, key);
            return Some(owner.id("viewer"));
        }
        None
    }

    /// The transform a routed AT value change produces, or `None` when it would
    /// not change anything.
    ///
    /// Through the same `zoom_by_step`/`apply_zoom` the keyboard `+`/`-` and
    /// toolbar call, viewport-centre-anchored: a routed zoom is a keyed one,
    /// another trigger — never drift. Clamp/range stays widget-owned: AT
    /// cannot push scale out of range.
    fn routed_transform(
        &self,
        intent: &ValueIntent,
        geometry: ViewportGeometry,
    ) -> Option<ViewTransform> {
        let current = self.effective_transform(geometry);
        let centre = Point::new(geometry.bounds.width / 2.0, geometry.bounds.height / 2.0);

        let next = match intent {
            ValueIntent::Increment => zoom_by_step(
                current,
                geometry,
                self.range,
                self.zoom_step,
                ZoomDirection::In,
                centre,
            ),
            ValueIntent::Decrement => zoom_by_step(
                current,
                geometry,
                self.range,
                self.zoom_step,
                ZoomDirection::Out,
                centre,
            ),
            // Unvalidated AT text: a percentage such as "150" or "150%". Parsed
            // defensively into a target scale, then clamped by `apply_zoom`
            // against the range — an unparseable or empty payload does nothing.
            ValueIntent::Set(text) => {
                let target = parse_zoom_percent(text)?;
                apply_zoom(current, geometry, self.range, target, centre)
            }
        };

        // Suppress a no-op (already at the range end, or the parsed target
        // equals the current scale): a routed action that changes nothing must
        // not publish, matching the pointer/keyboard no-op suppression.
        (next != current).then_some(next)
    }
}

/// Parse an AT `SetValue` payload into a target scale.
///
/// A zoom PERCENTAGE string (the published `"{percent}%"`), `%` optional:
/// `"150"`/`"150%"` → `1.5`. Non-finite, non-positive, empty, unparseable →
/// `None`: hostile input refused, never panicked or illegal.
fn parse_zoom_percent(text: &str) -> Option<f32> {
    let percent: f32 = text.trim().trim_end_matches('%').trim().parse().ok()?;
    let scale = percent / 100.0;
    (scale.is_finite() && scale > 0.0).then_some(scale)
}

impl<'a, Message> EnzImageViewer<'a, Message>
where
    Message: Clone + 'a,
{
    /// Finish the builder and hand back the [`Element`]: step 3 of the
    /// build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// Forwards to the `From<Self> for Element` impl below — one
    /// implementation, no drift. Prefer inside `row![]`/`column![]`, where
    /// `.into()` has no inferable target.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzImageViewer<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(viewer: EnzImageViewer<'a, Message>) -> Self {
        Element::new(viewer)
    }
}

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

/// Whether each toolbar control can currently do anything.
///
/// Computed from the transform and range alone, so the toolbar's enabled state
/// and the viewer's actual behaviour cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolbarAvailability {
    /// Whether zooming in would change the scale.
    pub can_zoom_in: bool,
    /// Whether zooming out would change the scale.
    pub can_zoom_out: bool,
    /// Whether the fit control would change anything.
    pub can_fit: bool,
}

impl ToolbarAvailability {
    /// Derive availability for a transform.
    #[must_use]
    pub fn of(transform: ViewTransform, range: ZoomRange, interactive: bool) -> Self {
        if !interactive {
            return Self {
                can_zoom_in: false,
                can_zoom_out: false,
                can_fit: false,
            };
        }

        Self {
            can_zoom_in: can_zoom(transform, range, ZoomDirection::In),
            can_zoom_out: can_zoom(transform, range, ZoomDirection::Out),
            can_fit: transform != fit_transform(range),
        }
    }
}

/// Semantics for the three toolbar controls, in display order.
///
/// Exposed so a host composing its own toolbar publishes the same names and
/// disabled states this module would.
pub fn toolbar_semantics(
    availability: ToolbarAvailability,
) -> [enzui_core::semantics::Semantics; 3] {
    [
        zoom_control_semantics(ZoomDirection::Out, availability.can_zoom_out),
        zoom_control_semantics(ZoomDirection::In, availability.can_zoom_in),
        fit_control_semantics(availability.can_fit),
    ]
}

/// Apply a toolbar action to a transform.
///
/// Toolbar shares [`apply_zoom`] and [`fit_transform`] with pointer/keyboard:
/// a button and a keypress meaning one thing produce one transform, by
/// construction.
///
/// # `Actual` — one image pixel per logical pixel
///
/// DPI-agnostic: resolves `Actual` to
/// [`actual_scale`](ViewportGeometry::actual_scale) (image / fitted-content) —
/// one image px per LOGICAL px. On HiDPI a logical px is several physical, so
/// not screen-1:1. For true 1:1 use [`apply_toolbar_action_scaled`] with the
/// renderer factor. This wrapper is `apply_toolbar_action_scaled(.., 1.0)` —
/// for geometry-less callers (host previews, tests) and the 1.0-scale case.
#[must_use]
pub fn apply_toolbar_action(
    action: ToolbarAction,
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
    zoom_step: f32,
) -> ViewTransform {
    apply_toolbar_action_scaled(action, transform, geometry, range, zoom_step, 1.0)
}

/// Apply a toolbar action, resolving `Actual` to a true physical
/// one-image-pixel-per-screen-pixel scale.
///
/// `scale_factor` = renderer total (`window * application`): physical per
/// logical. iced draws in LOGICAL space, so `actual_scale` (image/content)
/// fills `image_px` logical = `image_px * scale_factor` physical (`scale`:1,
/// not 1:1). Dividing targets `image_px / scale_factor` logical = `image_px`
/// physical — the 1:1 "100%" promises. E.g. 480×320 Contain-fit into 360×240:
/// `actual_scale ≈ 1.333`, 2×-display physical-1:1 ≈ `0.667`. Unknown raw size
/// (loading): `Actual` → 1 (then ÷ factor). Non-finite/non-positive factor →
/// `1.0` (NaN/negatives inexpressible). Non-`Actual` actions DPI-free,
/// unchanged.
#[must_use]
pub fn apply_toolbar_action_scaled(
    action: ToolbarAction,
    transform: ViewTransform,
    geometry: ViewportGeometry,
    range: ZoomRange,
    zoom_step: f32,
    scale_factor: f32,
) -> ViewTransform {
    let centre = Point::new(geometry.bounds.width / 2.0, geometry.bounds.height / 2.0);

    match action {
        ToolbarAction::ZoomIn => zoom_by_step(
            transform,
            geometry,
            range,
            zoom_step,
            ZoomDirection::In,
            centre,
        ),
        ToolbarAction::ZoomOut => zoom_by_step(
            transform,
            geometry,
            range,
            zoom_step,
            ZoomDirection::Out,
            centre,
        ),
        ToolbarAction::Fit => fit_transform(range),
        ToolbarAction::Actual => {
            let dpi = if scale_factor.is_finite() && scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            };
            let target = geometry.actual_scale().unwrap_or(1.0) / dpi;
            apply_zoom(transform, geometry, range, target, centre)
        }
    }
}

/// A toolbar control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolbarAction {
    /// Step the scale up.
    ZoomIn,
    /// Step the scale down.
    ZoomOut,
    /// Return to the fitted, centred transform.
    Fit,
    /// Set to actual size: one image pixel = one logical pixel.
    Actual,
}

/// A safe command the host can hand to the viewer **without knowing the
/// renderer geometry**.
///
/// The host sets [`.command()`](EnzImageViewer::command) on its next rebuild;
/// the widget resolves vs its own measured geometry in `diff`, publishing via
/// `on_transform`. Closes the "toolbar-requires-geometry" gap: the host sends
/// INTENT, never a resolved transform — no duplicated measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewerCommand {
    /// Apply a toolbar action.  Equivalent to calling
    /// [`apply_toolbar_action`] with the widget's own geometry.
    Toolbar(ToolbarAction),
}
