//! Ordered optical composition and opaque fallbacks for one material surface.
//!
//! Foreground paint stays outside its own backdrop sample; nested filtering is
//! refused. Adaptive ink follows rendered measurements and retains visible ink
//! while changed geometry is sampled again; a new adaptive surface stays
//! hidden (for a few frames at most) until its first reading, so it appears
//! in its measured ink. Child lifecycle routing preserves
//! the surface's focus, accessibility and event behavior.

use enzui_core::material::{
    self, FallbackReason, MaterialCapability, MaterialDecision, MaterialGeometry, MaterialRect,
    MaterialRegionRequest,
};
use enzui_theme::{GlassInk, MaterialPolicy, MaterialStyle, MaterialTokens};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer};
use iced::border::Radius;
use iced::{
    Background, Border, Color, Element, Event, Length, Padding, Rectangle, Shadow, Size, Vector,
};
use std::cell::Cell;
use std::rc::Rc;

use crate::desktop_glass::{Backdrop, Route};

/// The panel chrome a material surface hands to its boundary.
///
/// Both colours are resolved per draw from the live theme, exactly as the
/// container's own style closure resolves them.
pub(crate) struct Chrome {
    /// The opaque fill painted when no material can be filtered.
    pub fill: Rc<dyn Fn(&iced::Theme) -> Color>,
    /// The panel's border colour.
    pub border_color: Rc<dyn Fn(&iced::Theme) -> Color>,
    /// The panel's border width, in logical pixels.
    pub border_width: f32,
    /// The panel's drop shadow.
    pub shadow: Rc<dyn Fn(&iced::Theme) -> Shadow>,
    // Explicit fills suppress automatic ink washes.
    pub fill_pinned: bool,
}

/// Opens the material region the surface around it paints.
pub(crate) struct MaterialBoundary<'a, Message> {
    chrome_visible: bool,
    child: Element<'a, Message>,
    tokens: MaterialTokens,
    radii: Rc<Cell<Radius>>,
    chrome: Chrome,
    /// A caller-supplied policy overriding the live one.
    policy: Option<MaterialPolicy>,
    /// Grows the recorded geometry back out to the composing surface's own
    /// bounds, which lie outside this wrapper by exactly that container's
    /// padding.
    outset: Padding,
    /// The style the child sits on. When the glass renders, the child is
    /// drawn with [`crate::theme_lookup::on_material`], so its controls
    /// default to glass.
    style: Option<MaterialStyle>,
    ink: Option<GlassInk>,
    /// What the glass samples: the app's own drawing, or the desktop behind
    /// the window.
    backdrop: Backdrop,
    /// A window's frame: glass only where the compositor paints it, opaque
    /// otherwise, and its content keeps its own look on the glass.
    frame: bool,
}

impl<'a, Message> MaterialBoundary<'a, Message> {
    pub(crate) fn new(
        child: impl Into<Element<'a, Message>>,
        tokens: MaterialTokens,
        radii: Rc<Cell<Radius>>,
        chrome: Chrome,
    ) -> Self {
        Self {
            chrome_visible: chrome_visible(),
            child: child.into(),
            tokens,
            radii,
            chrome,
            policy: None,
            outset: Padding::from(0),
            style: None,
            ink: None,
            backdrop: Backdrop::App,
            frame: false,
        }
    }

    /// See [`Self::frame`](MaterialBoundary).
    pub(crate) fn window_frame(mut self, frame: bool) -> Self {
        self.frame = frame;
        self
    }

    pub(crate) fn backdrop(mut self, backdrop: Backdrop) -> Self {
        self.backdrop = backdrop;
        self
    }

    /// Draw the child as content on `style` glass whenever the glass renders.
    pub(crate) fn style(mut self, style: MaterialStyle) -> Self {
        self.style = Some(style);
        self
    }

    pub(crate) fn ink(mut self, ink: Option<GlassInk>) -> Self {
        self.ink = ink;
        self
    }

    pub(crate) fn policy(mut self, policy: Option<MaterialPolicy>) -> Self {
        self.policy = policy;
        self
    }

    pub(crate) fn outset(mut self, outset: Padding) -> Self {
        self.outset = outset;
        self
    }

    fn region_bounds(&self, bounds: Rectangle) -> Rectangle {
        layout::clip::bounds(bounds, self.outset)
    }

    fn radii(&self) -> [f32; 4] {
        let radii = self.radii.get();
        [
            radii.top_left,
            radii.top_right,
            radii.bottom_left,
            radii.bottom_right,
        ]
    }

    fn shadow(&self, theme: &iced::Theme, light_ink: Option<f32>) -> Shadow {
        let own = (self.chrome.shadow)(theme);
        if self.tokens.bevel_width == 0.0 {
            return own;
        }
        match light_ink {
            Some(t) if t < 1.0 => {
                let faint = Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.16 * (1.0 - t)),
                    offset: Vector::new(0.0, 1.0),
                    blur_radius: 10.0,
                };
                if own.color.a >= faint.color.a {
                    own
                } else {
                    faint
                }
            }
            _ => own,
        }
    }

    /// Draws the child as content on the glass, in its ink.
    #[expect(
        clippy::too_many_arguments,
        reason = "mirrors Widget::draw plus the ink"
    )]
    fn draw_on_glass(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        light_ink: Option<f32>,
    ) {
        let on_glass = self.style.map(|material| match light_ink {
            Some(t) => crate::theme_lookup::on_material_ink(theme, material, t),
            None => crate::theme_lookup::on_material(theme, material),
        });
        // Plain text inherits its colour from the style passed down, not the
        // theme: a chosen ink must reach it too.
        let inked = match (light_ink, on_glass.as_ref()) {
            (Some(_), Some(glass_theme)) => renderer::Style {
                text_color: crate::theme_lookup::palette(glass_theme).text,
            },
            _ => *style,
        };
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            on_glass.as_ref().unwrap_or(theme),
            &inked,
            layout,
            cursor,
            viewport,
        );
    }

    /// The panel border, painted over a composited material and around an
    /// opaque fallback alike.
    fn border(&self, theme: &iced::Theme) -> Border {
        Border {
            color: (self.chrome.border_color)(theme),
            width: self.chrome.border_width,
            radius: self.radii.get(),
        }
    }
}

/// What the live renderer can do, in the vocabulary the material plan decides
/// with.
fn capability(support: iced::advanced::material::Support) -> MaterialCapability {
    match support {
        iced::advanced::material::Support::Optical { max_blur_sigma } => {
            MaterialCapability::Optical { max_blur_sigma }
        }
        iced::advanced::material::Support::Unsupported => {
            MaterialCapability::Fallback(FallbackReason::RendererUnsupported)
        }
    }
}

/// Backdrop brightness (perceptual, `0..1`) above which adaptive glass with
/// light ink switches to dark ink. Black and white ink read equally well
/// near 0.46; the band either side keeps the current ink.
const TO_DARK_INK: f32 = 0.56;
/// Where black and white ink read about equally well; the first measurement
/// decides by it.
const INK_MIDPOINT: f32 = 0.46;
/// Backdrop brightness below which adaptive glass with dark ink switches back
/// to light ink. The gap between the two absorbs most mixed scenes.
const TO_LIGHT_INK: f32 = 0.38;
/// How long the backdrop must stay past a threshold before the ink switches:
/// a flash or a scene cut does not flip it.
const SWITCH_DWELL: std::time::Duration = std::time::Duration::from_millis(900);
/// How long a new surface's backdrop must hold steady before its ink commits
/// to the hysteresis band.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(600);
/// The crossfade from one ink to the other, in seconds.
const CROSSFADE: f32 = 0.3;
/// Frames the boundary keeps redrawing after one it did not ask for, so a
/// measurement that lands a frame or two later is still seen.
const MEASURE_TAIL: u8 = 3;

/// The adaptive ink of one glass surface, kept across frames.
struct Adaptive {
    /// The renderer's measurement key for this surface.
    probe: u64,
    /// Where the ink is: `1.0` light, `0.0` dark.
    light_ink: f32,
    /// Where it is heading.
    target: f32,
    measured: bool,
    fallback_ink: Option<f32>,
    // Settling keeps late image/video arrival from fixing ink to a loading backdrop.
    settling_since: Option<std::time::Instant>,

    crossing_since: Option<std::time::Instant>,
    last_frame: Option<std::time::Instant>,
    tail: u8,
    // Paint bounds the wait even when an ancestor forwards no events.
    unmeasured_frames: Cell<u8>,
    last_seen: Option<f32>,
    last_seen_changed: bool,
    bounds: Option<Rectangle>,
    sampling: Cell<bool>,
    refresh: Cell<bool>,
    awaiting_frame: Cell<Option<u64>>,
}

// Glass bounds its first-measurement wait.
const MAX_UNMEASURED_FRAMES: u8 = 6;

/// How far past its bounds an unmeasured surface's hidden group reaches, to
/// take in its shadow.
const UNMEASURED_REACH: f32 = 32.0;

/// How far a surface may move in one frame and still be the same surface,
/// in logical pixels: a sliding toast keeps its ink, a different widget that
/// inherited this tree slot does not.
const SAME_SURFACE_JUMP: f32 = 48.0;

fn next_probe() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Adaptive {
    // A new probe prevents a resized surface from reading its old footprint.
    fn track(&mut self, bounds: Rectangle) {
        let same = self.bounds.is_none_or(|old| {
            (old.width - bounds.width).abs() < 1.0
                && (old.height - bounds.height).abs() < 1.0
                && (old.x - bounds.x).abs() <= SAME_SURFACE_JUMP
                && (old.y - bounds.y).abs() <= SAME_SURFACE_JUMP
        });
        if !same {
            let fallback_ink = if self.measured {
                Some(self.light_ink)
            } else {
                self.fallback_ink
            };
            *self = Self::new();
            // A pending replacement measurement must not unpaint a retained surface.
            self.fallback_ink = fallback_ink;
        }
        self.bounds = Some(bounds);
    }

    fn ink(&self, renderer: &iced::Renderer) -> Option<f32> {
        if self.measured {
            return Some(self.light_ink);
        }
        self.luminance(renderer)
            .map(|luminance| if luminance > INK_MIDPOINT { 0.0 } else { 1.0 })
            .or(self.fallback_ink)
    }

    /// The latest backdrop brightness: the compositor's for glass behind the
    /// window, the renderer's otherwise.
    fn luminance(&self, renderer: &iced::Renderer) -> Option<f32> {
        crate::desktop_glass::luminance(self.probe)
            .or_else(|| renderer.material_luminance(self.probe))
    }

    // A compositor that never answers must not leave its surface hidden indefinitely.
    fn wait_one_more(&self) -> bool {
        let waited = self.unmeasured_frames.get();
        if waited >= MAX_UNMEASURED_FRAMES {
            return false;
        }
        self.unmeasured_frames.set(waited + 1);
        true
    }

    fn new() -> Self {
        Self {
            probe: next_probe(),
            light_ink: 1.0,
            target: 1.0,
            measured: false,
            fallback_ink: None,
            settling_since: None,
            crossing_since: None,
            last_frame: None,
            tail: MEASURE_TAIL,
            unmeasured_frames: Cell::new(0),
            last_seen: None,
            last_seen_changed: false,
            bounds: None,
            sampling: Cell::new(false),
            refresh: Cell::new(true),
            awaiting_frame: Cell::new(None),
        }
    }

    /// Advances the ink for one frame at `now`, given the latest backdrop
    /// brightness. Returns whether another frame is needed.
    fn advance(&mut self, now: std::time::Instant, luminance: Option<f32>) -> bool {
        // A frame this surface did not ask for may have changed the backdrop;
        // its measurement lands a frame or two later.
        if self.tail == 0 {
            self.tail = MEASURE_TAIL;
        } else {
            self.tail -= 1;
        }
        self.last_seen_changed = luminance.is_some() && luminance != self.last_seen;
        if self.last_seen_changed {
            self.tail = self.tail.max(1);
        }
        self.last_seen = luminance;

        if let Some(luminance) = luminance {
            let midpoint = if luminance > INK_MIDPOINT { 0.0 } else { 1.0 };
            if !self.measured {
                // The first reading decides at once: no fade from a guess.
                self.measured = true;
                self.fallback_ink = None;
                self.target = midpoint;
                self.light_ink = midpoint;
                self.settling_since = Some(now);
            }
            if let Some(since) = self.settling_since {
                // Settling: the ink follows the midpoint once a reading has
                // held for a quarter of the window, so a flash while the page
                // loads does not move it; the band takes over once the
                // backdrop has held for the whole window.
                if self.last_seen_changed {
                    self.settling_since = Some(now);
                } else {
                    let held = now.duration_since(since);
                    if held >= SETTLE / 4 {
                        self.target = midpoint;
                    }
                    if held >= SETTLE {
                        self.settling_since = None;
                    }
                }
            }
            // Past the band on the other side, held for the dwell: switch.
            let wanted = if self.settling_since.is_some() {
                self.target
            } else if self.target >= 0.5 {
                if luminance > TO_DARK_INK { 0.0 } else { 1.0 }
            } else if luminance < TO_LIGHT_INK {
                1.0
            } else {
                0.0
            };
            if wanted != self.target {
                let since = *self.crossing_since.get_or_insert(now);
                if now.duration_since(since) >= SWITCH_DWELL {
                    self.target = wanted;
                    self.crossing_since = None;
                }
            } else {
                self.crossing_since = None;
            }
        }

        let step = self.last_frame.map_or(0.0, |last| {
            now.duration_since(last).as_secs_f32() / CROSSFADE
        });
        self.last_frame = Some(now);
        if self.light_ink < self.target {
            self.light_ink = (self.light_ink + step).min(self.target);
        } else if self.light_ink > self.target {
            self.light_ink = (self.light_ink - step).max(self.target);
        }

        self.tail > 0
            || self.crossing_since.is_some()
            || self.settling_since.is_some()
            || (self.light_ink - self.target).abs() > f32::EPSILON
            || !self.measured
    }
}

fn rect(bounds: Rectangle) -> MaterialRect {
    MaterialRect::new(bounds.x, bounds.y, bounds.width, bounds.height)
}

impl<Message> MaterialBoundary<'_, Message> {
    fn adapts(&self) -> bool {
        self.style
            .is_some_and(|style| self.ink.unwrap_or_else(|| style.ink()) == GlassInk::Adaptive)
    }

    // Frame events can arrive through update and update_frame; advance once per instant.
    fn follow_backdrop(
        &self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return;
        };
        if !self.adapts() {
            return;
        }
        let state = tree.state.downcast_mut::<Adaptive>();
        if state.last_frame == Some(*now) {
            return;
        }
        if self.backdrop == Backdrop::App && !state.sampling.get() {
            return;
        }
        if state.tail == 0 && state.awaiting_frame.get().is_none() {
            state.refresh.set(true);
        }
        state.track(layout.bounds());
        if let Some(expected) = state.awaiting_frame.get()
            && renderer
                .material_luminance_with_frame(state.probe)
                .is_some_and(|(_, sampled)| sampled >= expected)
        {
            state.awaiting_frame.set(None);
        }
        let luminance = state.luminance(renderer);
        if state.advance(*now, luminance) || state.awaiting_frame.get().is_some() {
            shell.request_redraw();
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MaterialBoundary<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<Adaptive>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(Adaptive::new())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
        if self.adapts() {
            tree.state.downcast_ref::<Adaptive>().refresh.set(true);
        }
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    /// Adaptive glass takes every frame, to follow its backdrop; otherwise the
    /// child's interest is the boundary's.
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.adapts() || crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.follow_backdrop(tree, event, layout, renderer, shell);
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
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
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
        self.follow_backdrop(tree, event, layout, renderer, shell);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
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
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = self.region_bounds(layout.bounds());
        let adaptive = tree.state.downcast_ref::<Adaptive>();
        adaptive.sampling.set(false);
        if !self.chrome_visible {
            adaptive.awaiting_frame.set(None);
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
            return;
        }

        // The ink the glass's content takes, `1.0` light to `0.0` dark, or
        // `None` for the app's own.
        let light_ink =
            self.style
                .and_then(|style| match self.ink.unwrap_or_else(|| style.ink()) {
                    GlassInk::Adaptive => adaptive.ink(renderer),
                    GlassInk::Theme => None,
                    GlassInk::Light => Some(1.0),
                });

        // A material's panel colour is a silhouette, not a wash: every refused
        // route paints it instead of a filter, so it must hide the backdrop
        // whatever alpha the chrome carries. The optics read only its channels.
        // Under a chosen ink it is that ink's glass: dark behind light ink,
        // light behind dark, so the shader protects for the ink in use.
        let chrome_fill = Color {
            a: 1.0,
            ..(self.chrome.fill)(theme)
        };
        let fill = match light_ink {
            _ if self.chrome.fill_pinned => chrome_fill,
            None => chrome_fill,
            Some(t) => {
                let dark_mode = crate::theme_lookup::palette(theme).is_dark();
                let dark = if dark_mode {
                    chrome_fill
                } else {
                    enzui_theme::Palette::dark().surface_elevated
                };
                let light = if dark_mode {
                    enzui_theme::Palette::light().surface_elevated
                } else {
                    chrome_fill
                };
                Color {
                    r: light.r + (dark.r - light.r) * t,
                    g: light.g + (dark.g - light.g) * t,
                    b: light.b + (dark.b - light.b) * t,
                    a: 1.0,
                }
            }
        };

        // An open planning frame is the declared capability authority; with no
        // frame the live renderer answers, which is the production path.
        let policy = self
            .policy
            .unwrap_or_else(|| crate::theme_lookup::material_policy(theme));
        let capability = material::under_policy(
            material::frame_capability().unwrap_or_else(|| capability(renderer.material_support())),
            policy,
        );

        // Reduced motion settles the optics rather than removing them:
        // displacement and dispersion are what warp content moving beneath.
        let mut requested = policy.settle(self.tokens);
        // A window's frame is darker glass in a dark theme.
        if self.frame && crate::theme_lookup::palette(theme).is_dark() {
            requested = enzui_theme::window_frame::in_dark(requested);
        }

        // A window frame the compositor does not paint is a plain opaque
        // frame: in-app glass would sample nothing beneath the window.
        let mut opaque_frame = false;
        if self.backdrop == Backdrop::Window {
            match crate::desktop_glass::route() {
                Route::Hidden | Route::InApp if self.frame => opaque_frame = true,
                // The window does not know yet whether the compositor paints
                // it: show nothing rather than another look first.
                Route::Hidden => return,
                Route::Window { measures } => {
                    let compositor = material::under_policy(
                        MaterialCapability::Optical {
                            max_blur_sigma: crate::desktop_glass::COMPOSITOR_MAX_BLUR_SIGMA,
                        },
                        policy,
                    );
                    if let MaterialDecision::Optical(tokens) =
                        material::decide_with(compositor, &requested)
                    {
                        let waiting = self.adapts()
                            && measures
                            && light_ink.is_none()
                            // The compositor's reading wakes the window
                            // itself, so here waiting needs no frame.
                            && adaptive.wait_one_more();
                        crate::desktop_glass::place(crate::desktop_glass::Pane {
                            bounds,
                            radius: self.radii().into_iter().fold(0.0, f32::max),
                            tokens,
                            tint: fill,
                            shadow: self.shadow(theme, light_ink),
                            probe: self.adapts().then_some(adaptive.probe),
                            waiting,
                        });
                        // Like in-app glass, adaptive glass appears only in
                        // its measured ink.
                        if !waiting {
                            renderer.fill_quad(
                                renderer::Quad {
                                    bounds,
                                    border: self.border(theme),
                                    ..Default::default()
                                },
                                Background::Color(Color::TRANSPARENT),
                            );
                            if self.frame {
                                self.child.as_widget().draw(
                                    &tree.children[0],
                                    renderer,
                                    theme,
                                    style,
                                    layout,
                                    cursor,
                                    viewport,
                                );
                            } else {
                                self.draw_on_glass(
                                    tree, renderer, theme, style, layout, cursor, viewport,
                                    light_ink,
                                );
                            }
                        }
                        return;
                    }
                    opaque_frame = self.frame;
                }
                Route::InApp => {}
            }
        }

        // The decision is taken before the scope opens, or this material would
        // read its own scope as an enclosing one and refuse itself.
        let filtered = match material::decide_with(capability, &requested) {
            _ if opaque_frame => None,
            MaterialDecision::Optical(tokens) => material::record_with(
                capability,
                MaterialRegionRequest {
                    geometry: MaterialGeometry::new(rect(bounds), self.radii()),
                    clip: rect(*viewport),
                    tokens: requested,
                    tint: [fill.r, fill.g, fill.b],
                },
            )
            .ok()
            .map(|scope| (tokens, scope)),
            MaterialDecision::Opaque(_) => None,
        };

        match filtered {
            Some((tokens, scope)) => {
                // Until its first reading lands (a frame or two), adaptive glass draws
                // invisibly, so its backdrop is still sampled.
                let unmeasured = self.adapts()
                    && light_ink.is_none()
                    && renderer.material_support().is_optical()
                    && adaptive.wait_one_more();
                if unmeasured {
                    renderer.start_layer_alpha(bounds.expand(UNMEASURED_REACH), 0.0);
                }
                // Ordered: the region is stated first, so the composite reads
                // only what is already painted, and the child paints into the
                // layer the boundary opened.
                let mut material = tokens.to_material(self.radii(), fill);
                if self.adapts() {
                    material.probe = adaptive.probe;
                    if renderer.material_support().is_optical() {
                        adaptive.sampling.set(true);
                        if adaptive.refresh.replace(false) {
                            // A late result from the old backdrop cannot satisfy a new source-frame refresh.
                            adaptive
                                .awaiting_frame
                                .set(Some(renderer.material_frame().saturating_add(1)));
                            renderer.request_material_redraw();
                        }
                    }
                }
                renderer.start_layer_material(bounds, material);
                renderer.fill_quad(
                    renderer::Quad {
                        bounds,
                        border: self.border(theme),
                        ..Default::default()
                    },
                    Background::Color(
                        if (self.ink == Some(GlassInk::Theme)
                            || self.adapts() && light_ink.is_none())
                            && !self.chrome.fill_pinned
                        {
                            // Theme ink needs a neutral wash when the backdrop has the opposite brightness.
                            crate::color::with_alpha(
                                crate::theme_lookup::palette(theme).background,
                                0.36,
                            )
                        } else {
                            Color::TRANSPARENT
                        },
                    ),
                );

                self.draw_on_glass(
                    tree, renderer, theme, style, layout, cursor, viewport, light_ink,
                );

                renderer.end_layer();

                // The shadow is painted after the composite, in a layer of its
                // own so it orders after the material: a shadow already in the
                // frame is backdrop, and the blur would pull it across the rim.
                // A shadow quad over a transparent fill paints only outside
                // the silhouette, so it never covers the glass.
                let shadow = self.shadow(theme, light_ink);
                if shadow.color.a > 0.0 {
                    let reach = shadow.blur_radius.max(0.0)
                        + shadow.offset.x.abs().max(shadow.offset.y.abs())
                        + 1.0;
                    renderer.with_layer(bounds.expand(reach), |renderer| {
                        renderer.fill_quad(
                            renderer::Quad {
                                bounds,
                                border: Border {
                                    radius: self.radii.get(),
                                    ..Border::default()
                                },
                                shadow,
                                ..Default::default()
                            },
                            Background::Color(Color::TRANSPARENT),
                        );
                    });
                }
                if unmeasured {
                    renderer.end_layer();
                }

                // The foreground is only excluded from the backdrop while the
                // scope is open, so it closes after the child, never before.
                drop(scope);
            }
            None => {
                adaptive.awaiting_frame.set(None);
                // An explicit opaque panel: the composing container painted no
                // chrome, so this is the only fill, and it is never a filter
                // that did not run.
                renderer.fill_quad(
                    renderer::Quad {
                        bounds,
                        border: self.border(theme),
                        shadow: (self.chrome.shadow)(theme),
                        ..Default::default()
                    },
                    Background::Color(fill),
                );

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
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message> From<MaterialBoundary<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(widget: MaterialBoundary<'a, Message>) -> Self {
        Self::new(widget)
    }
}

thread_local! {
    static CHROME_VISIBLE: Cell<bool> = const { Cell::new(true) };
}

struct ChromeVisibilityGuard(bool);
impl Drop for ChromeVisibilityGuard {
    fn drop(&mut self) {
        CHROME_VISIBLE.with(|value| value.set(self.0));
    }
}

fn chrome_visible() -> bool {
    CHROME_VISIBLE.with(Cell::get)
}

pub(crate) fn with_chrome_visibility<R>(visible: bool, build: impl FnOnce() -> R) -> R {
    let previous = CHROME_VISIBLE.with(|value| value.replace(value.get() && visible));
    let _guard = ChromeVisibilityGuard(previous);
    build()
}
