//! Application-wide font families, semantic weights and control metrics for
//! widgets built without a theme in hand.
//!
//! [`publish`] installs the resolved theme after font precedence has been
//! applied. `DesktopFontFeed::renderer_sync` publishes it synchronously while
//! preparing iced's renderer-default task. Hosts managing their own theme
//! publish on boot and after a font, weight, sizing or scrollbar change, then
//! rebuild their views. Text scale travels separately through [`crate::text_scale`].
//! Unscoped readers and publishers panic if the shared defaults lock is poisoned.

use std::cell::RefCell;
use std::sync::{Arc, LazyLock, RwLock};

use crate::{
    EnzTheme, FontCatalog, FontFamily, FontRole, MotionScale, OpacityScale, RadiusScale,
    RadiusStep, ScrollbarTokens, ShapeScale, SizingTokens, SpacingScale, StrokeScale, StrokeToken,
    TypographyScale, WeightRung, WindowButtons,
};
use iced_core::font::Weight;

#[derive(Debug, Clone, PartialEq)]
struct Defaults {
    fonts: FontCatalog,
    weights: [u16; 5],
    font_size_factor: f32,
    sizing: SizingTokens,
    spacing: SpacingScale,
    opacity: OpacityScale,
    scrollbar: ScrollbarTokens,
    radius: RadiusScale,
    shape: ShapeScale,
    stroke: StrokeScale,
    motion: MotionScale,
    window_buttons: WindowButtons,
    window_frame: crate::WindowFrame,
    window_radius: f32,
    window_shadow: crate::WindowShadow,
}

impl Defaults {
    fn from_theme(theme: &EnzTheme) -> Self {
        Self {
            fonts: theme.font_catalog.clone(),
            font_size_factor: theme.typography.base_ramp().body_size
                / TypographyScale::default().body_size,
            weights: WeightRung::ALL.map(|rung| theme.typography.weight(rung)),
            sizing: theme.sizing,
            spacing: theme.spacing,
            opacity: theme.opacity,
            scrollbar: theme.scrollbar.normalized(),
            radius: theme.radius,
            shape: theme.shape,
            stroke: theme.stroke,
            motion: theme.motion,
            window_buttons: theme.window_buttons,
            window_frame: theme.window_frame,
            window_radius: theme.window_radius,
            window_shadow: theme.window_shadow.normalized(),
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            fonts: FontCatalog::default(),
            font_size_factor: 1.0,
            weights: WeightRung::ALL.map(|rung| TypographyScale::default().weight(rung)),
            sizing: SizingTokens::default(),
            spacing: SpacingScale::default(),
            opacity: OpacityScale::default(),
            scrollbar: ScrollbarTokens::default(),
            radius: RadiusScale::default(),
            shape: ShapeScale::default(),
            stroke: StrokeScale::default(),
            motion: MotionScale::default(),
            window_buttons: WindowButtons::default(),
            window_frame: crate::WindowFrame::default(),
            window_radius: crate::DEFAULT_WINDOW_RADIUS,
            window_shadow: crate::WindowShadow::default(),
        }
    }
}

static PUBLISHED: LazyLock<RwLock<Arc<Defaults>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Defaults::default())));

thread_local! {
    static SCOPED: RefCell<Option<Arc<Defaults>>> = const { RefCell::new(None) };
}

/// Read the defaults in effect on this thread: its scope, else the published
/// ones, read in place (widgets read these every frame).
fn with_effective<R>(read: impl FnOnce(&Defaults) -> R) -> R {
    match SCOPED.with(|cell| cell.borrow().clone()) {
        Some(scoped) => read(&scoped),
        None => read(&PUBLISHED.read().expect("read text defaults")),
    }
}

/// Publish resolved families, weight rungs, sizing, scrollbar, radius, shape,
/// stroke and motion tokens and the window buttons and radius.
/// Returns whether any stored default changed.
///
/// Changes affect subsequent view builds in every window and thread. Existing
/// font/weight pins and forwarded typography scales retain precedence.
/// This does not rebuild views or dispatch window invalidation.
///
/// # Panics
///
/// Panics if the shared defaults lock is poisoned.
pub fn publish(theme: &EnzTheme) -> bool {
    let next = Defaults::from_theme(theme);
    let mut current = PUBLISHED.write().expect("publish text defaults");
    if current.as_ref() == &next {
        return false;
    }
    *current = Arc::new(next);
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    true
}

/// Bumped by every [`publish`] that changes the defaults. Caches that retain
/// built elements (fonts, weights and sizes are read at build time) compare
/// it to know when to rebuild.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The published defaults' generation; see [`publish`].
#[must_use]
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

/// The current family for a run with no explicit theme-family pin.
#[must_use]
pub fn family(role: FontRole) -> FontFamily {
    with_effective(|defaults| defaults.fonts.family(role).clone())
}

/// The desktop UI base size relative to the framework baseline, before accessibility scaling.
#[must_use]
pub fn font_size_factor() -> f32 {
    with_effective(|defaults| defaults.font_size_factor)
}

/// The baseline semantic ramp with the published UI size and weight choices.
/// Accessibility scaling is applied by [`TypographyScale::ambient`].
#[must_use]
pub fn typography() -> TypographyScale {
    with_effective(|defaults| TypographyScale {
        weight_regular: defaults.weights[0],
        weight_medium: defaults.weights[1],
        weight_semibold: defaults.weights[2],
        weight_bold: defaults.weights[3],
        weight_extrabold: defaults.weights[4],
        ..TypographyScale::default()
            .with_base_body_size(TypographyScale::default().body_size * defaults.font_size_factor)
    })
}

/// The current discrete face for an unpinned semantic weight rung.
#[must_use]
pub fn weight(rung: WeightRung) -> Weight {
    with_effective(|defaults| {
        crate::typography::css_weight_to_iced(defaults.weights[rung as usize])
    })
}

/// The theme's control sizing: the density a theme file sets.
#[must_use]
pub fn sizing() -> SizingTokens {
    with_effective(|defaults| defaults.sizing)
}

/// The theme's spacing ladder for layout built without a theme argument.
#[must_use]
pub fn spacing() -> SpacingScale {
    with_effective(|defaults| defaults.spacing)
}

/// The theme's whole-control and surface state opacities.
#[must_use]
pub fn opacity() -> OpacityScale {
    with_effective(|defaults| defaults.opacity)
}

/// The theme's scrollbar geometry and opacity.
#[must_use]
pub fn scrollbar() -> ScrollbarTokens {
    with_effective(|defaults| defaults.scrollbar)
}

/// The radius the theme gives a component family's corners, for widgets that
/// fix their corners while the view is built (a draw-time style reads the
/// theme instead).
#[must_use]
pub fn shape_radius(family: fn(&ShapeScale) -> RadiusStep) -> f32 {
    with_effective(|defaults| match family(&defaults.shape) {
        RadiusStep::Window => defaults.window_radius,
        step => defaults.radius.step(step),
    })
}

/// The theme's keyboard focus ring width, for widgets that fix it while the
/// view is built or laid out (a draw-time style reads the theme instead).
#[must_use]
pub fn focus_ring_width() -> f32 {
    with_effective(|defaults| StrokeToken::FocusRing.resolve(&defaults.stroke))
}

/// The theme's motion pace: its `normal` duration over the unscaled 180 ms, so
/// a widget's own fixed-length motion runs at the theme's speed (2.0 is half
/// as fast, and `ENZUI_MOTION_SCALE` reaches it through the default scale).
/// Reduced motion is not folded in; readers check it themselves.
#[must_use]
pub fn motion_pace() -> f32 {
    let normal = with_effective(|defaults| defaults.motion.normal_ms);
    if normal.is_finite() && normal >= 0.0 {
        normal / crate::motion::DEFAULT_NORMAL_MS
    } else {
        1.0
    }
}

/// The window controls the theme asks for; [`WindowButtons::Desktop`] leaves
/// them to the desktop's button layout.
#[must_use]
pub fn window_buttons() -> WindowButtons {
    with_effective(|defaults| defaults.window_buttons)
}

/// What the theme makes every self-drawn window frame of; see
/// [`crate::WindowFrame`].
#[must_use]
pub fn window_frame() -> crate::WindowFrame {
    with_effective(|defaults| defaults.window_frame)
}

/// The corner radius the theme gives a self-drawn window frame, in logical px.
#[must_use]
pub fn window_radius() -> f32 {
    with_effective(|defaults| defaults.window_radius)
}

/// The theme's shadow and edge lines for a self-drawn window frame.
#[must_use]
pub fn window_shadow() -> crate::WindowShadow {
    with_effective(|defaults| defaults.window_shadow)
}

/// Resolve a role's family and semantic weight from the current defaults.
#[must_use]
pub fn font(role: FontRole, rung: WeightRung) -> iced_core::Font {
    with_effective(|defaults| iced_core::Font {
        family: defaults.fonts.family(role).to_iced(),
        weight: crate::typography::css_weight_to_iced(defaults.weights[rung as usize]),
        ..iced_core::Font::DEFAULT
    })
}

/// A nested, thread-local theme override for view construction and measurement.
///
/// Other threads continue to read their own scope or the published defaults.
/// Drop nested guards in reverse order on the thread that entered them.
/// Out-of-order drops can leave an earlier override active.
#[derive(Debug)]
pub struct TextDefaultsScope {
    previous: Option<Arc<Defaults>>,
    thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl TextDefaultsScope {
    /// Override families, semantic weights, sizing, scrollbar, radius, shape,
    /// stroke and motion tokens until this guard is dropped.
    #[must_use]
    pub fn enter(theme: &EnzTheme) -> Self {
        let next = Arc::new(Defaults::from_theme(theme));
        Self {
            previous: SCOPED.with(|cell| cell.replace(Some(next))),
            thread_bound: std::marker::PhantomData,
        }
    }
}

impl Drop for TextDefaultsScope {
    fn drop(&mut self) {
        SCOPED.with(|cell| {
            cell.replace(self.previous.take());
        });
    }
}
