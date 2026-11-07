//! Loading placeholders with text bars, a circle or a rounded card and opacity pulsing.
//!
//! The default loading state drives a 1.2 s eased pulse from frame timestamps.
//! Supply [`EnzSkeleton::phase`] for an external sample instead of the internal clock.
//!
//! Text renders at least one 14 px bar with 8 px spacing and a final bar at
//! 60% width. Circle defaults to 40 px; Card defaults to 120 px height and
//! 8 px corners. Text and Card fill width with an advisory 140 px minimum.
//! [`EnzSkeleton::fill_height`] makes only Card fill its parent's height.
//!
//! Unpinned fill uses the draw theme's alternate surface. A shared memo follows
//! enzUI theme payload identity; stock iced themes are distinguished only by
//! darkness, so switching between stock themes of the same darkness requires
//! rebuilding the element to refresh its fill. Pulse opacity multiplies fill alpha.
//!
//! Theme reduced motion holds the loading crest and uses low-rate restart
//! probes while visible. An explicit reduced-motion override wins in either
//! direction; explicit stillness, idle state and external phase schedule no
//! clock frames. Offscreen placeholders schedule none. The pulse is painted
//! once and faded by the render lane, so pulsing placeholders cost replays,
//! not redraws.
//!
//! Each mounted skeleton publishes a named ProgressIndicator leaf, defaulting
//! to `Loading`, including when `.loading(false)` selects idle opacity.
//! Remove the placeholder when its busy announcement is no longer appropriate.

use crate::min_width::min_width;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::widget::{column, container};
use iced::{Color, Length};
use std::cell::RefCell;
use std::rc::Rc;
const PULSE_PERIOD_SECS: f32 = 1.2;

#[cfg(test)]
pub(crate) static THEME_RESOLVE_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
#[derive(Clone, Copy)]
struct ThemeResolution {
    base: Color,
    reduced: bool,
}
#[derive(Clone, Copy, PartialEq)]
enum ThemeKey {
    Enz(*const enzui_theme::EnzTheme),
    Stock(bool),
}

fn theme_key(theme: &iced::Theme) -> ThemeKey {
    match theme.extension::<enzui_theme::EnzTheme>() {
        Some(arc) => ThemeKey::Enz(std::sync::Arc::as_ptr(&arc)),
        None => ThemeKey::Stock(theme.palette().is_dark),
    }
}
// The memo spans draws; invalidate on payload identity or stock darkness changes.
type ThemeMemo = Rc<RefCell<Option<(ThemeKey, ThemeResolution)>>>;
fn resolve_shared(
    memo: &ThemeMemo,
    theme: &iced::Theme,
    pinned: Option<SkeletonTokens>,
    reduced_override: Option<bool>,
) -> ThemeResolution {
    let key = theme_key(theme);
    if let Some((cached_key, cached)) = *memo.borrow()
        && cached_key == key
    {
        return cached;
    }
    #[cfg(test)]
    THEME_RESOLVE_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let base = pinned.map_or_else(
        || crate::theme_lookup::palette(theme).surface_alt,
        |p| p.base_color,
    );
    let reduced = reduced_override.unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme));
    let resolution = ThemeResolution { base, reduced };
    *memo.borrow_mut() = Some((key, resolution));
    resolution
}

/// Placeholder fill, geometry in logical pixels and opacity endpoints.
#[derive(Debug, Clone, Copy)]
pub struct SkeletonTokens {
    /// Fill ink; pulse opacity multiplies its alpha. Defaults to translucent text.
    pub base_color: Color,
    /// Text-bar height in logical pixels; defaults to 14 and must be positive.
    pub line_height: f32,
    /// Text-bar gap in logical pixels; defaults to 8 and allows zero.
    pub line_spacing: f32,
    /// Circle diameter in logical pixels; defaults to 40 and must be positive.
    pub circle_size: f32,
    /// Card height in logical pixels; defaults to 120 and must be positive.
    pub card_height: f32,
    /// Card corner radius in logical pixels; defaults to 8 and allows zero.
    pub card_radius: f32,
    /// Advisory Text/Card minimum width in logical pixels; defaults to 140.
    pub text_min_width: f32,
    /// Pulse crest and reduced-motion opacity multiplier; defaults to 0.6.
    pub loading_opacity: f32,
    /// Pulse trough and idle opacity multiplier; defaults to 0.3.
    pub idle_opacity: f32,
}

/// Alpha of the text colour a placeholder is filled with, before the pulse.
const PLACEHOLDER_ALPHA: f32 = 0.18;

impl SkeletonTokens {
    /// Use a translucent text-colour fill with fixed stock geometry and
    /// opacity endpoints: a placeholder reads on the window and on cards in
    /// both tones, the way redacted placeholders tint with the label colour.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            base_color: Color {
                a: PLACEHOLDER_ALPHA,
                ..p.text
            },
            line_height: 14.0,
            line_spacing: 8.0,
            circle_size: 40.0,
            card_height: 120.0,
            card_radius: 8.0,
            text_min_width: 140.0,
            loading_opacity: 0.60,
            idle_opacity: 0.30,
        }
    }
    /// Use the built-in dark palette with stock geometry and opacity endpoints.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Use the built-in light palette with stock geometry and opacity endpoints.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    fn sanitized(self) -> Self {
        let d = Self::from_palette(&Palette::dark());
        let pos = |v: f32, fb: f32| if v.is_finite() && v > 0.0 { v } else { fb };
        let alpha = |v: f32, fb: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { fb };
        Self {
            base_color: self.base_color,
            line_height: pos(self.line_height, d.line_height),
            line_spacing: if self.line_spacing.is_finite() && self.line_spacing >= 0.0 {
                self.line_spacing
            } else {
                d.line_spacing
            },
            circle_size: pos(self.circle_size, d.circle_size),
            card_height: pos(self.card_height, d.card_height),
            card_radius: if self.card_radius.is_finite() && self.card_radius >= 0.0 {
                self.card_radius
            } else {
                d.card_radius
            },
            text_min_width: pos(self.text_min_width, d.text_min_width),
            loading_opacity: alpha(self.loading_opacity, d.loading_opacity),
            idle_opacity: alpha(self.idle_opacity, d.idle_opacity),
        }
    }
}

/// Placeholder shape; defaults to three text bars.
#[derive(Debug, Clone, Copy)]
pub enum SkeletonKind {
    /// Stack rounded bars, shortening the final bar to 60% width even for one line.
    Text {
        /// Number of bars; zero renders one bar.
        lines: usize,
    },
    /// One circular placeholder.
    Circle,
    /// One rounded panel.
    Card,
}

impl Default for SkeletonKind {
    fn default() -> Self {
        Self::Text { lines: 3 }
    }
}

/// Sample a 1.2 s smoothstep-eased triangle pulse between `idle` and `loading`.
/// Reduced motion returns the clamped loading endpoint. See
/// [`pulse_alpha_with_period`] for normalization and endpoint rules.
pub fn pulse_alpha(phase_secs: f32, idle: f32, loading: f32, reduced_motion: bool) -> f32 {
    pulse_alpha_with_period(phase_secs, PULSE_PERIOD_SECS, idle, loading, reduced_motion)
}

/// Sample an eased triangle pulse with a period in seconds.
/// Negative phases wrap; non-finite phases become zero. Non-finite or
/// nonpositive periods use 1.2 s. Half a period reaches `loading`; zero and
/// a whole period reach `idle`. Reduced motion returns `loading`, independent
/// of phase and period. The result clamps to `[0, 1]`; endpoints themselves
/// are not repaired, so non-finite endpoints can produce NaN.
pub fn pulse_alpha_with_period(
    phase_secs: f32,
    period_secs: f32,
    idle: f32,
    loading: f32,
    reduced_motion: bool,
) -> f32 {
    if reduced_motion {
        return loading.clamp(0.0, 1.0);
    }
    let period = if period_secs.is_finite() && period_secs > 0.0 {
        period_secs
    } else {
        PULSE_PERIOD_SECS
    };
    let phase = if phase_secs.is_finite() {
        phase_secs.rem_euclid(period) / period
    } else {
        0.0
    };
    let tri = 1.0 - (2.0 * phase - 1.0).abs();
    let eased = tri * tri * (3.0 - 2.0 * tri);
    (idle + (loading - idle) * eased).clamp(0.0, 1.0)
}

/// Builder for a named progress placeholder, self-animating while loading by default.
#[must_use]
pub struct EnzSkeleton {
    kind: SkeletonKind,
    phase: Option<f32>,
    loading: bool,
    reduced_motion: Option<bool>,
    label: String,
    tokens: Option<SkeletonTokens>,
    fill_height: bool,
}

impl EnzSkeleton {
    /// Enable the internal pulse when no external phase is supplied; defaults to `true`.
    /// False removes the clock and selects idle opacity unless reduced motion
    /// selects the loading crest. An explicit phase still controls the pulse.
    /// The accessibility progress indicator remains published in either state.
    pub fn loading(mut self, on: bool) -> Self {
        self.loading = on;
        self
    }

    /// Use an external pulse sample in elapsed seconds and disable internal scheduling.
    /// The sample wraps over 1.2 s; non-finite values use phase zero. It takes
    /// precedence over `loading` but still follows the effective reduced-motion policy.
    pub fn phase(mut self, elapsed_secs: f32) -> Self {
        self.phase = Some(elapsed_secs);
        self
    }

    /// Override theme motion policy in either direction.
    /// True holds loading opacity and stops internal frame requests; false permits
    /// animation even under a reduced-motion theme. Otherwise theme policy applies.
    pub fn reduced_motion(mut self, on: bool) -> Self {
        self.reduced_motion = Some(on);
        self
    }

    /// Make Card fill the offered height instead of using `card_height`; defaults to `false`.
    /// Text and Circle ignore this option. Use a bounded parent to control the
    /// space a filling card consumes.
    pub fn fill_height(mut self, on: bool) -> Self {
        self.fill_height = on;
        self
    }

    /// Set the accessible progress-indicator name; an empty string falls back to `Loading`.
    /// Whitespace-only labels are retained.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Pin fill, geometry and opacity endpoints. Invalid geometry uses stock defaults.
    /// Line height, circle size, card height and minimum width must be finite and
    /// positive; gaps and corner radius allow zero. Finite opacities clamp to
    /// `[0, 1]`; non-finite opacities use stock defaults. Fill colour is not repaired.
    pub fn tokens(mut self, t: SkeletonTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }
    fn resolved_reduced_motion(&self, theme: &iced::Theme) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme))
    }
    fn alpha_for(&self, t: &SkeletonTokens, reduced_motion: bool) -> f32 {
        match self.phase {
            Some(p) => pulse_alpha(p, t.idle_opacity, t.loading_opacity, reduced_motion),
            None => {
                if reduced_motion || self.loading {
                    t.loading_opacity
                } else {
                    t.idle_opacity
                }
            }
        }
    }

    /// Evaluate the stored external phase or loading/idle endpoint under this theme.
    /// This helper does not observe the internal frame clock or multiply fill alpha;
    /// it can differ from the opacity currently painted by a self-animating element.
    /// The supplied tokens are used directly without builder sanitization.
    pub fn current_alpha_for_theme(&self, t: &SkeletonTokens, theme: &iced::Theme) -> f32 {
        self.alpha_for(t, self.resolved_reduced_motion(theme))
    }

    /// Evaluate [`Self::current_alpha_for_theme`] against stock iced Dark.
    /// This helper does not observe internal clock state or the runtime draw theme.
    pub fn current_alpha(&self, t: &SkeletonTokens) -> f32 {
        self.current_alpha_for_theme(t, &iced::Theme::Dark)
    }

    /// Build the placeholder, optional internal clock and named accessibility leaf.
    pub fn into_element<'a, Message: 'a>(self) -> iced::Element<'a, Message> {
        let t = self.tokens.unwrap_or_else(SkeletonTokens::dark);
        let pinned = self.tokens;
        let phase = self.phase;
        let loading = self.loading;
        let reduced_override = self.reduced_motion;
        let label = if self.label.is_empty() {
            enzui_core::localization::text("enzui-skeleton-loading")
        } else {
            self.label.clone()
        };
        let self_animate = phase.is_none() && loading;
        let theme_memo: ThemeMemo = Rc::new(RefCell::new(None));
        let fill_for = std::rc::Rc::new(
            move |theme: &iced::Theme, radius: f32| -> container::Style {
                let ThemeResolution { base, reduced } =
                    resolve_shared(&theme_memo, theme, pinned, reduced_override);
                // A self-animating placeholder paints at full strength; the
                // pulse fades it as a whole.
                let alpha = match phase {
                    None if self_animate => 1.0,
                    Some(p) => pulse_alpha(p, t.idle_opacity, t.loading_opacity, reduced),
                    None => {
                        if reduced || loading {
                            t.loading_opacity
                        } else {
                            t.idle_opacity
                        }
                    }
                };
                let fill = Color {
                    a: base.a * alpha,
                    ..base
                };
                container::Style {
                    background: Some(fill.into()),
                    border: iced::Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: radius.into(),
                    },
                    ..Default::default()
                }
            },
        );

        let body: iced::Element<'a, Message> = match self.kind {
            SkeletonKind::Text { lines } => {
                let n = lines.max(1);
                let radius = (t.line_height / 2.0).floor();
                let mut bars: Vec<iced::Element<'a, Message>> = Vec::with_capacity(n);
                for i in 0..n {
                    let is_last = i == n - 1;
                    let fill_for = Rc::clone(&fill_for);
                    let bar = container(iced::widget::text(""))
                        .width(Length::Fill)
                        .height(Length::Fixed(t.line_height))
                        .style(move |theme: &iced::Theme| fill_for(theme, radius));
                    if is_last {
                        bars.push(
                            iced::widget::row![
                                container(bar).width(Length::FillPortion(60)),
                                container(iced::widget::text("")).width(Length::FillPortion(40)),
                            ]
                            .into(),
                        );
                    } else {
                        bars.push(bar.into());
                    }
                }
                min_width(
                    container(column(bars).spacing(t.line_spacing)).width(Length::Fill),
                    t.text_min_width,
                )
                .into()
            }
            SkeletonKind::Circle => {
                let radius = (t.circle_size / 2.0).floor();
                let fill_for = Rc::clone(&fill_for);
                container(iced::widget::text(""))
                    .width(Length::Fixed(t.circle_size))
                    .height(Length::Fixed(t.circle_size))
                    .style(move |theme: &iced::Theme| fill_for(theme, radius))
                    .into()
            }
            SkeletonKind::Card => {
                let radius = t.card_radius;
                let fill_for = Rc::clone(&fill_for);
                let card_h = if self.fill_height {
                    Length::Fill
                } else {
                    Length::Fixed(t.card_height)
                };
                min_width(
                    container(iced::widget::text(""))
                        .width(Length::Fill)
                        .height(card_h)
                        .style(move |theme: &iced::Theme| fill_for(theme, radius)),
                    t.text_min_width,
                )
                .into()
            }
        };
        let body: iced::Element<'a, Message> = if self_animate {
            crate::opacity_pulse::OpacityPulse::new(
                body,
                PULSE_PERIOD_SECS,
                t.idle_opacity,
                t.loading_opacity,
                reduced_override,
            )
            .into()
        } else {
            body
        };
        crate::semantics_container::SemanticsContainer::leaf(
            body,
            Semantics::new(Role::ProgressIndicator).label(label),
        )
        .into()
    }
}

impl<'a, Message: 'a> From<EnzSkeleton> for iced::Element<'a, Message> {
    fn from(s: EnzSkeleton) -> Self {
        s.into_element()
    }
}

/// Build a loading placeholder with unpinned fill, stock geometry and theme motion policy.
pub fn enz_skeleton(kind: SkeletonKind) -> EnzSkeleton {
    EnzSkeleton {
        kind,
        phase: None,
        loading: true,
        reduced_motion: None,
        label: String::new(),
        tokens: None,
        fill_height: false,
    }
}
