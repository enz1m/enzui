//! Compositor-backed material over the desktop and windows behind the app.
//!
//! Bind each window, retain its [`WindowGlass`], and wrap its view with
//! [`enz_desktop_glass`]. [`Backdrop::Window`] surfaces or
//! [`crate::app_shell::PaneStyle::behind_window`] panes use this binding.
//! The window and application background must be transparent, with no paint
//! beneath those panes. Keep them outside scrollables: regions use laid-out
//! window coordinates without scroll clipping.
//!
//! Pending bindings hide window-backdrop panes; unavailable bindings use in-app
//! material. Reduced transparency uses an opaque fallback, and reduced motion
//! settles optics. The compositor receives the first 32 panes, each with its
//! largest corner radius. One shared shadow uses the highest pane shadow alpha
//! and its blur and vertical offset. On measuring compositors, any waiting pane
//! puts the entire window's glass into measurement-only mode.

use std::cell::RefCell;
use std::collections::HashMap;

use enzui_theme::MaterialTokens;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Color, Element, Event, Length, Rectangle, Shadow, Size, Subscription, Task, Vector};

#[cfg(target_os = "linux")]
pub use enzui_wayland::desktop_glass::DesktopGlass;

/// Never constructed: no platform but Wayland has compositor glass.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DesktopGlass {}

pub(crate) const COMPOSITOR_MAX_BLUR_SIGMA: f32 = 32.0;

/// What a material samples.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Backdrop {
    /// What the app drew beneath it, inside its window. The default.
    #[default]
    App,
    /// What lies behind the window: the desktop and other windows, painted by
    /// the compositor. Needs an [`enz_desktop_glass`] around the view.
    Window,
}

/// A window's compositor glass, as [`bind`] resolved it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowGlass(Binding);

#[derive(Debug, Clone, Default, PartialEq)]
enum Binding {
    #[default]
    Pending,
    Unavailable,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Bound(DesktopGlass),
}

impl WindowGlass {
    /// Before [`bind`] resolved, the state an app starts in.
    #[must_use]
    pub fn pending() -> Self {
        Self(Binding::Pending)
    }

    /// No compositor glass: every pane uses in-app glass.
    #[must_use]
    pub fn unavailable() -> Self {
        Self(Binding::Unavailable)
    }

    /// Whether this window holds a compositor glass binding, independent of pane visibility.
    #[must_use]
    pub fn is_bound(&self) -> bool {
        matches!(self.0, Binding::Bound(_))
    }

    #[cfg(target_os = "linux")]
    fn glass(&self) -> Option<&DesktopGlass> {
        match &self.0 {
            Binding::Bound(glass) => Some(glass),
            _ => None,
        }
    }
}

/// A window whose compositor glass the app bound itself.
#[cfg(target_os = "linux")]
impl From<DesktopGlass> for WindowGlass {
    fn from(glass: DesktopGlass) -> Self {
        Self(Binding::Bound(glass))
    }
}

/// Ask the compositor for the glass of `window`. Resolves to
/// [`WindowGlass::unavailable`] off Wayland or where the compositor offers
/// none, so every outcome ends the pending state.
#[cfg(target_os = "linux")]
pub fn bind(window: iced::window::Id) -> Task<WindowGlass> {
    iced::window::wayland_parent_export(window).then(|payload| {
        let payload = std::sync::Mutex::new(payload);
        Task::future(async move {
            let payload = payload.lock().ok().and_then(|mut slot| slot.take());
            let (sender, receiver) = iced::futures::channel::oneshot::channel();
            let _ = std::thread::Builder::new()
                .name("enzui-glass-bind".into())
                .spawn(move || {
                    let bound =
                        DesktopGlass::from_payload(payload, std::time::Duration::from_secs(2));
                    let _ = sender.send(bound);
                });
            match receiver.await {
                Ok(Ok(glass)) => WindowGlass(Binding::Bound(glass)),
                _ => WindowGlass::unavailable(),
            }
        })
    })
}

/// Ask the compositor for the glass of `window`: off Linux there is none.
#[cfg(not(target_os = "linux"))]
pub fn bind(_window: iced::window::Id) -> Task<WindowGlass> {
    Task::done(WindowGlass::unavailable())
}

/// Subscribe to backdrop brightness changes on a measuring compositor.
/// Map notifications to a message that redraws adaptive ink; pending, unavailable
/// and nonmeasuring bindings produce no subscription.
pub fn luminance_changes(glass: &WindowGlass) -> Subscription<()> {
    #[cfg(target_os = "linux")]
    if let Some(glass) = glass.glass().filter(|glass| glass.measures()) {
        return Subscription::run_with(glass.clone(), |glass| {
            let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
            glass.on_luminance(move || {
                let _ = sender.unbounded_send(());
            });
            receiver
        });
    }
    let _ = glass;
    Subscription::none()
}

pub(crate) struct Pane {
    pub bounds: Rectangle,
    pub radius: f32,
    pub tokens: MaterialTokens,
    pub tint: Color,
    pub shadow: Shadow,
    pub probe: Option<u64>,
    pub waiting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    InApp,
    Hidden,
    Window { measures: bool },
}

struct Frame {
    route: Route,
    panes: Vec<Pane>,
    /// Opaque content over the window's glass: `(bounds, radius)`.
    cutouts: Vec<(Rectangle, f32)>,
}

thread_local! {
    // Nested hosts temporarily replace the active frame and restore it after drawing.
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
    #[cfg(target_os = "linux")]
    // Probe indices follow the region order sent for each window.
    static PROBES: RefCell<HashMap<u64, (DesktopGlass, usize)>> = RefCell::new(HashMap::new());
}

pub(crate) fn route() -> Route {
    FRAME.with(|frame| {
        frame
            .borrow()
            .as_ref()
            .map_or(Route::InApp, |frame| frame.route)
    })
}

pub(crate) fn place(pane: Pane) {
    FRAME.with(|frame| {
        if let Some(frame) = frame.borrow_mut().as_mut() {
            frame.panes.push(pane);
        }
    });
}

/// Notes opaque content drawn over the window's glass, so the compositor
/// spends nothing on glass there. Only while the window's glass is the
/// compositor's.
pub(crate) fn place_cutout(bounds: Rectangle, radius: f32) {
    FRAME.with(|frame| {
        if let Some(frame) = frame.borrow_mut().as_mut()
            && matches!(frame.route, Route::Window { .. })
        {
            frame.cutouts.push((bounds, radius));
        }
    });
}

pub(crate) fn luminance(probe: u64) -> Option<f32> {
    #[cfg(target_os = "linux")]
    return PROBES.with(|probes| {
        let probes = probes.borrow();
        let (glass, index) = probes.get(&probe)?;
        glass.luminance(*index)
    });
    #[cfg(not(target_os = "linux"))]
    {
        let _ = probe;
        None
    }
}

/// Collects this window's [`Backdrop::Window`] panes each frame and hands
/// them to the compositor before the frame is presented. See the
/// [module docs](self).
pub fn enz_desktop_glass<'a, Message, Theme, Renderer>(
    glass: &WindowGlass,
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> EnzDesktopGlass<'a, Message, Theme, Renderer> {
    EnzDesktopGlass {
        glass: glass.clone(),
        content: content.into(),
    }
}

/// The widget [`enz_desktop_glass`] builds.
pub struct EnzDesktopGlass<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    glass: WindowGlass,
    content: Element<'a, Message, Theme, Renderer>,
}

impl<Message, Theme, Renderer> EnzDesktopGlass<'_, Message, Theme, Renderer> {
    fn route(&self) -> Route {
        match &self.glass.0 {
            Binding::Pending => Route::Hidden,
            Binding::Unavailable => Route::InApp,
            #[cfg(target_os = "linux")]
            Binding::Bound(glass) => Route::Window {
                measures: glass.measures(),
            },
            #[cfg(not(target_os = "linux"))]
            Binding::Bound(glass) => match *glass {},
        }
    }

    #[cfg(target_os = "linux")]
    fn send(&self, panes: &[Pane], cutouts: &mut [(Rectangle, f32)]) {
        use enzui_wayland::desktop_glass::{GlassCutout, GlassRegion, GlassShadow, MAX_CUTOUTS};
        let Some(glass) = self.glass.glass() else {
            return;
        };
        let regions: Vec<GlassRegion> = panes
            .iter()
            .map(|pane| GlassRegion {
                x: pane.bounds.x,
                y: pane.bounds.y,
                width: pane.bounds.width,
                height: pane.bounds.height,
                radius: pane.radius,
                tokens: wire_tokens(&pane.tokens),
                tint: [pane.tint.r, pane.tint.g, pane.tint.b, pane.tint.a],
            })
            .collect();
        let shadow = panes
            .iter()
            .map(|pane| pane.shadow)
            .max_by(|a, b| a.color.a.total_cmp(&b.color.a))
            .filter(|shadow| shadow.color.a > 0.0)
            .map_or_else(GlassShadow::default, |shadow| GlassShadow {
                blur: shadow.blur_radius,
                offset_y: shadow.offset.y,
                opacity: shadow.color.a,
            });
        // The largest first: past the limit, the smallest are dropped, and
        // the glass under them is painted, unseen.
        cutouts.sort_by(|a, b| (b.0.width * b.0.height).total_cmp(&(a.0.width * a.0.height)));
        let cutouts: Vec<GlassCutout> = cutouts
            .iter()
            .take(MAX_CUTOUTS)
            .map(|(bounds, radius)| GlassCutout {
                x: bounds.x,
                y: bounds.y,
                width: bounds.width,
                height: bounds.height,
                radius: *radius,
            })
            .collect();
        glass.set_cutouts(&cutouts);
        glass.set_regions(&regions, shadow);
        glass.set_measure_only(panes.iter().any(|pane| pane.waiting));
        PROBES.with(|probes| {
            let mut probes = probes.borrow_mut();
            probes.retain(|_, (known, _)| known != glass);
            for (index, pane) in panes.iter().enumerate() {
                if let Some(probe) = pane.probe {
                    probes.insert(probe, (glass.clone(), index));
                }
            }
        });
    }
}

// Field order is fixed by the enzui-material-v1 region payload.
pub(crate) fn wire_tokens(tokens: &MaterialTokens) -> [f32; 16] {
    [
        tokens.blur_sigma,
        tokens.refraction,
        tokens.bevel_width,
        tokens.index_of_refraction,
        tokens.dispersion,
        tokens.color_diffusion,
        tokens.color_reach,
        tokens.optical_depth,
        tokens.tint_opacity,
        tokens.saturation,
        tokens.highlight,
        tokens.brightness,
        tokens.light_angle,
        tokens.profile_curvature,
        tokens.contrast_protection,
        tokens.protected_inset,
    ]
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EnzDesktopGlass<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
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
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let outer = FRAME.with(|frame| {
            frame.replace(Some(Frame {
                route: self.route(),
                panes: Vec::new(),
                cutouts: Vec::new(),
            }))
        });
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        let frame = FRAME.with(|frame| frame.replace(outer));
        // Region changes must reach the surface before the commit presenting this frame.
        #[cfg(target_os = "linux")]
        if let Some(mut frame) = frame {
            self.send(&frame.panes, &mut frame.cutouts);
        }
        #[cfg(not(target_os = "linux"))]
        let _ = frame;
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a, Theme: 'a, Renderer: iced::advanced::Renderer + 'a>
    EnzDesktopGlass<'a, Message, Theme, Renderer>
{
    /// This host as an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer> {
        Element::from(self)
    }
}

impl<'a, Message: 'a, Theme: 'a, Renderer: iced::advanced::Renderer + 'a>
    From<EnzDesktopGlass<'a, Message, Theme, Renderer>> for Element<'a, Message, Theme, Renderer>
{
    fn from(host: EnzDesktopGlass<'a, Message, Theme, Renderer>) -> Self {
        Element::new(host)
    }
}

/// Marks `content` as opaque over the window's glass (see [`place_cutout`]):
/// a pane drawn over a glass window frame, whose glass beneath it no one
/// sees. Draws `content` unchanged.
/// Only an `opaque` one is noted; the wrapper is there either way, so a pane
/// that turns opaque keeps its place in the tree.
pub(crate) fn cutout<'a, Message: 'a>(
    content: Element<'a, Message>,
    radius: f32,
    opaque: bool,
) -> Element<'a, Message> {
    Element::new(Cutout {
        content,
        radius,
        opaque,
    })
}

struct Cutout<'a, Message> {
    content: Element<'a, Message>,
    radius: f32,
    opaque: bool,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Cutout<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.content.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.content.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content.as_widget_mut().layout(tree, renderer, limits);
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
        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    // The wrapper shares the content's tree node, so frame interest passes through.
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.content.as_widget().frame_interest(tree)
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
        self.content
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        if self.opaque {
            place_cutout(layout.bounds(), self.radius);
        }
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
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
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_shell::{ShellLayout, ShellTokens, enz_app_shell};
    use iced::advanced::renderer::Headless as _;
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    const SIZE: Size = Size::new(1100.0, 700.0);

    struct Drawn {
        pixels: Vec<u8>,
        panes: Vec<(Rectangle, f32)>,
        cutouts: Vec<(Rectangle, f32)>,
    }

    /// A shell with a glass frame (sidebar and content panes), drawn while
    /// the window's glass takes `route`.
    fn shell(route: Route) -> Drawn {
        shell_with(route, ShellTokens::inset_panes())
    }

    /// [`shell`] with `tokens` (a glass frame is added) and a titlebar.
    fn shell_with(route: Route, tokens: ShellTokens) -> Drawn {
        let mut renderer = iced::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let view: Element<'_, ()> = enz_app_shell(|| iced::widget::text("content").into())
            .layout(ShellLayout::FloatingTitlebarSidebar)
            .tokens(tokens.frame_material(enzui_theme::MaterialStyle::FlatFrosted))
            .titlebar(|| iced::widget::text("title").into())
            .sidebar(|| iced::widget::text("sidebar").into())
            .on_sidebar_state(|_| ())
            .into();
        let mut ui = UserInterface::build(view, SIZE, Cache::default(), &mut renderer);
        ui = ui.relayout(SIZE, &mut renderer);
        let outer = FRAME.with(|frame| {
            frame.replace(Some(Frame {
                route,
                panes: Vec::new(),
                cutouts: Vec::new(),
            }))
        });
        ui.draw(
            &mut renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        let frame = FRAME
            .with(|frame| frame.replace(outer))
            .expect("the frame drawn");
        let pixels = renderer.screenshot(
            Size::new(SIZE.width as u32, SIZE.height as u32),
            1.0,
            Color::TRANSPARENT,
        );
        Drawn {
            pixels,
            panes: frame
                .panes
                .iter()
                .map(|pane| (pane.bounds, pane.radius))
                .collect(),
            cutouts: frame.cutouts,
        }
    }

    fn alpha(pixels: &[u8], x: u32, y: u32) -> u8 {
        pixels[((y * SIZE.width as u32 + x) * 4 + 3) as usize]
    }

    /// On the compositor's glass, the frame is one region over the whole
    /// window, left clear for the compositor in the gaps around the panes,
    /// and each opaque pane is a cutout; the panes keep their fill.
    ///
    /// Sabotage: register no cutout in `Cutout::draw`. No cutouts are
    /// collected.
    #[test]
    fn a_glass_frame_is_one_region_with_its_panes_cut_out() {
        let drawn = shell(Route::Window { measures: false });
        assert_eq!(drawn.panes.len(), 1, "{:?}", drawn.panes);
        let (frame, radius) = drawn.panes[0];
        assert_eq!(frame, Rectangle::with_size(SIZE));
        assert!(radius > 0.0, "the window's rounded corners");
        assert!(
            drawn.cutouts.len() >= 2,
            "sidebar and content: {:?}",
            drawn.cutouts
        );
        for (cutout, _) in &drawn.cutouts {
            assert!(
                cutout.x >= 1.0 && cutout.x + cutout.width <= SIZE.width - 1.0,
                "{cutout:?}"
            );
        }
        // The edge gap shows the compositor's glass; the panes are filled.
        assert_eq!(alpha(&drawn.pixels, 3, 200), 0, "the frame is left clear");
        let (pane, _) = drawn.cutouts[0];
        let centre = pane.center();
        assert_eq!(
            alpha(&drawn.pixels, centre.x as u32, centre.y as u32),
            255,
            "{pane:?}"
        );
    }

    /// Over a glass frame a blended content pane stays filled, cut out of
    /// the glass, while the blended titlebar floating over it stays clear,
    /// so the content continues beneath the bar as over an opaque frame.
    ///
    /// Sabotage: back every blended pane in `pane_on_frame`, whatever
    /// `filled` says. The titlebar becomes an opaque cutout.
    #[test]
    fn a_blended_titlebar_over_glass_stays_clear_over_its_content() {
        let drawn = shell_with(
            Route::Window { measures: false },
            ShellTokens {
                content: crate::app_shell::PaneStyle::blended(),
                ..ShellTokens::inset_panes()
            },
        );
        let heights: Vec<f32> = drawn.cutouts.iter().map(|(b, _)| b.height).collect();
        let full = SIZE.height - 16.0;
        assert_eq!(
            heights,
            [full, full],
            "sidebar and content only: {:?}",
            drawn.cutouts
        );
    }

    /// Where the compositor paints no glass, or before it is known, the frame
    /// is the opaque shell tone, never in-app glass over nothing.
    ///
    /// Sabotage: drop the `Route::Hidden | Route::InApp if self.frame` arm in
    /// `MaterialBoundary::draw`. Pending hides the window; in-app glass
    /// leaves the frame translucent.
    #[test]
    fn a_frame_without_the_compositors_glass_is_opaque() {
        for route in [Route::InApp, Route::Hidden] {
            let drawn = shell(route);
            assert!(
                drawn.panes.is_empty() && drawn.cutouts.is_empty(),
                "{route:?}"
            );
            assert_eq!(
                alpha(&drawn.pixels, 3, 200),
                255,
                "{route:?}: an opaque frame"
            );
        }
    }
}
