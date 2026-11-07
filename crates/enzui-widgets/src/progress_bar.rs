//! Determinate progress with optional captions and an explicit unsampled state.
//!
//! Sampled fractions clamp to 0–1, with non-finite inputs becoming measured
//! zero. Unsampled bars paint a dashed placeholder and announce no numeric
//! measurement. Caption text is host-supplied; unsampled captions replace
//! stale value text with an em dash. Unpinned ink follows the live palette.
//!
//! A new value glides the fill there over the theme's `motion-normal`
//! duration instead of jumping. A bar that stands for work under way
//! ([`EnzProgressBar::busy`]) also sweeps a soft highlight along its filled
//! part while it is between empty and full, so a bar waiting on slow work
//! still reads as alive, as Windows progress bars do; a bar that shows a
//! level (usage, a meter, how much was read or watched) stays still between
//! values and asks for no frames. Reduced motion, or
//! [`EnzProgressBar::animated`]`(false)`, keeps both still.

use crate::enz_text::enz_text;
use crate::gauge::{Sampled, UNSAMPLED_ANNOUNCEMENT, UNSAMPLED_VALUE_TEXT};
use enzui_core::semantics::{NumericValue, Role as SemRole, Semantics};
use enzui_theme::{Palette, SpacingScale, TypographyScale};
use iced::widget::progress_bar::Style;
use iced::widget::{Space, column, row};
use iced::{Color, Element};

const MIN_WIDTH: f32 = 80.0;

fn caption_size() -> f32 {
    TypographyScale::ambient().label_size
}

fn caption_gap() -> f32 {
    SpacingScale::default().space2
}
fn sanitize_fraction(v: f32) -> f32 {
    enzui_core::metrics::UnitInterval::clamped(v).get()
}
fn sanitize_dim(v: f32, fallback: f32) -> f32 {
    enzui_core::metrics::FiniteNonNeg::new_or(v, fallback).get()
}

/// Palette role for a sampled bar's fill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressTone {
    /// Accent fill; the default tone.
    #[default]
    Accent,
    /// Success status fill.
    Success,
    /// Warning status fill.
    Warning,
    /// Destructive status fill.
    Danger,
}

/// Track, sampled fill and caption ink overrides for a progress bar.
#[derive(Debug, Clone, Copy)]
pub struct ProgressBarTokens {
    /// Background of the sampled track.
    pub track: Color,
    /// Palette-derived outline colour retained in the token bundle; rendering paints no outline.
    pub track_border: Color,
    /// Fill for the accent tone.
    pub accent: Color,
    /// Fill for the success tone.
    pub success: Color,
    /// Fill for the warning tone.
    pub warning: Color,
    /// Fill for the danger tone.
    pub danger: Color,
    /// Leading caption ink.
    pub text_color: Color,
    /// Trailing sampled-value caption ink.
    pub value_text_color: Color,
    /// Ink shared by placeholder dashes and an unsampled value caption.
    pub unsampled: Color,
}

impl ProgressBarTokens {
    /// Resolve track and caption ink from this palette and fills from its status roles.
    /// Status colours within 1/255 per channel of the matching built-in preset snap
    /// to that preset; other colours remain as supplied.
    pub fn from_palette(p: &Palette) -> Self {
        let dark = p.background.r + p.background.g + p.background.b < 1.5;
        let base = if dark {
            Palette::dark()
        } else {
            Palette::light()
        };
        let success = tone_pick(p.success, base.success, base.success);
        let warning = tone_pick(p.warning, base.warning, base.warning);
        let danger = tone_pick(p.destructive, base.destructive, base.destructive);
        Self {
            track: p.meter_track(),
            track_border: p.meter_track_border(),
            accent: p.accent,
            success,
            warning,
            danger,
            text_color: p.text,
            value_text_color: p.text_muted,
            unsampled: p.text_muted,
        }
    }

    /// Use fixed dark-palette colours.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Use fixed light-palette colours.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Resolve a sampled fill role from these tokens.
    pub fn fill_for_tone(&self, tone: ProgressTone) -> Color {
        match tone {
            ProgressTone::Accent => self.accent,
            ProgressTone::Success => self.success,
            ProgressTone::Warning => self.warning,
            ProgressTone::Danger => self.danger,
        }
    }
}
fn tone_pick(role: Color, base_role: Color, literal: Color) -> Color {
    let same = (role.r - base_role.r).abs() < 1.0 / 255.0
        && (role.g - base_role.g).abs() < 1.0 / 255.0
        && (role.b - base_role.b).abs() < 1.0 / 255.0
        && (role.a - base_role.a).abs() < 1.0 / 255.0;
    if same { literal } else { role }
}
const UNSAMPLED_DASHES: usize = 8;
const DASH_WEIGHT: u16 = 3;
const GAP_WEIGHT: u16 = 2;
fn unsampled_track<'a, Message: 'a>(
    height: f32,
    radius: f32,
    pinned: Option<ProgressBarTokens>,
) -> Element<'a, Message> {
    let mut dashes = row![];
    for i in 0..UNSAMPLED_DASHES {
        if i > 0 {
            dashes = dashes.push(Space::new().width(iced::Length::FillPortion(GAP_WEIGHT)));
        }
        dashes = dashes.push(
            iced::widget::container(Space::new())
                .width(iced::Length::FillPortion(DASH_WEIGHT))
                .height(iced::Length::Fixed(height))
                .style(move |theme: &iced::Theme| {
                    let tokens = pinned.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, ProgressBarTokens::from_palette)
                    });
                    iced::widget::container::Style {
                        background: Some(tokens.unsampled.into()),
                        border: iced::Border {
                            radius: radius.into(),
                            width: 0.0,
                            color: Color::TRANSPARENT,
                        },
                        ..iced::widget::container::Style::default()
                    }
                }),
        );
    }
    dashes.height(iced::Length::Fixed(height)).into()
}

/// Builder for measured or unsampled progress with optional captions.
#[must_use]
pub struct EnzProgressBar {
    sample: Sampled<f32>,
    tone: ProgressTone,
    fill_override: Option<Color>,
    /// Theme-resolved fill role; see [`EnzProgressBar::fill_role`].
    fill_role: Option<fn(&enzui_theme::Palette) -> Color>,
    label: String,
    caption_typography: Option<TypographyScale>,
    caption_spacing: Option<SpacingScale>,
    value_text: String,
    bar_height: f32,
    bar_radius: f32,
    tokens: Option<ProgressBarTokens>,
    animated: bool,
    busy: bool,
}

/// Create measured progress, including a literal measured zero.
/// Finite fractions clamp to 0–1; non-finite values become zero. Defaults to
/// accent fill, a 6 px bar, a 3 px radius and no captions. The 80 px preferred
/// minimum width yields to a narrower parent offer.
pub fn enz_progress_bar(value: f32) -> EnzProgressBar {
    enz_progress_bar_sampled(Sampled::Sampled(value))
}

/// Create progress with an explicit distinction between missing and measured values.
/// Unsampled bars have no numeric accessibility value and use dashed placeholder
/// ink; captions show an em dash only when a label or value caption was supplied.
/// Styling defaults match [`enz_progress_bar`].
pub fn enz_progress_bar_sampled(sample: Sampled<f32>) -> EnzProgressBar {
    EnzProgressBar {
        sample: sample.map(sanitize_fraction),
        tone: ProgressTone::default(),
        fill_override: None,
        fill_role: None,
        label: String::new(),
        caption_typography: None,
        caption_spacing: None,
        value_text: String::new(),
        bar_height: 6.0,
        bar_radius: 3.0,
        tokens: None,
        animated: true,
        busy: false,
    }
}

impl EnzProgressBar {
    /// Whether the bar moves at all: the fill's glide to a new value and,
    /// when [`busy`](Self::busy), the sweep (default `true`; reduced motion
    /// always keeps it still).
    pub fn animated(mut self, animated: bool) -> Self {
        self.animated = animated;
        self
    }

    /// Whether the bar stands for work under way, such as a running transfer:
    /// a highlight then sweeps along the fill while it is between empty and
    /// full. Default `false`, for a bar that shows a level; the sweep keeps
    /// the render lane presenting every frame, so leave it off for bars that
    /// stay on screen at rest.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Set measured or missing progress.
    /// Sampled fractions clamp to 0–1 and non-finite samples become measured zero.
    /// Missing samples use placeholder paint and omit the numeric accessibility value.
    pub fn sampled(mut self, sample: Sampled<f32>) -> Self {
        self.sample = sample.map(sanitize_fraction);
        self
    }

    /// Return the stored normalized reading, including whether it exists.
    #[must_use]
    pub fn sample(&self) -> Sampled<f32> {
        self.sample
    }

    /// Whether a measurement is absent.
    #[must_use]
    pub fn is_unsampled(&self) -> bool {
        self.sample.is_unsampled()
    }

    /// Construct the accessibility descriptor for the stored reading.
    /// Rendering attaches it as a terminal ProgressIndicator leaf. A label supplies
    /// the name, otherwise the announced value text does; an uncaptioned sampled
    /// bar has no name. Unsampled bars announce [`UNSAMPLED_ANNOUNCEMENT`].
    pub fn semantics_for_test(&self) -> Semantics {
        self.semantics(self.sample)
    }

    /// Choose sampled fill ink; defaults to Accent and yields to an explicit fill colour.
    pub fn tone(mut self, tone: ProgressTone) -> Self {
        self.tone = tone;
        self
    }

    /// Pin sampled fill ink, overriding the tone.
    /// Unsampled placeholder ink is unaffected.
    pub fn fill_color(mut self, color: Color) -> Self {
        self.fill_override = Some(color);
        self
    }

    /// Resolve the sampled fill from the live palette at draw time, so a quiet
    /// fill (paused or queued work) follows theme switches. An explicit
    /// [`fill_color`](Self::fill_color) still wins.
    pub(crate) fn fill_role(mut self, role: fn(&enzui_theme::Palette) -> Color) -> Self {
        self.fill_role = Some(role);
        self
    }

    /// Set the leading caption and accessible name.
    /// An empty label permits the announced value text to supply the name.
    pub fn label(mut self, label: &str) -> Self {
        label.clone_into(&mut self.label);
        self
    }

    /// Set host-formatted trailing caption text without changing the reading.
    /// Sampled bars announce it verbatim; unsampled bars replace it with an em dash
    /// visually and a missing-measurement announcement for accessibility.
    pub fn value_text(mut self, vt: &str) -> Self {
        vt.clone_into(&mut self.value_text);
        self
    }

    /// Set bar height in logical pixels and reset the radius to half that height.
    /// Defaults to 6; non-positive or non-finite values fall back to 6.
    /// This overrides an earlier radius pin.
    pub fn bar_height(mut self, h: f32) -> Self {
        self.bar_height = enzui_core::metrics::FinitePositive::new_or(h, 6.0).get();
        self.bar_radius = self.bar_height / 2.0;
        self
    }

    /// Pin corner radius in logical pixels; defaults to 3.
    /// Negative or non-finite values fall back to 3, while zero is allowed. A later
    /// height setter replaces this pin with half the stored height.
    pub fn bar_radius(mut self, r: f32) -> Self {
        self.bar_radius = enzui_core::metrics::FiniteNonNeg::new_or(r, 3.0).get();
        self
    }

    /// Supply caption typography at construction time.
    /// Captions use label_size and the supplied text weights; defaults use an 11 px
    /// label size. Non-positive or non-finite sizes fall back to ambient label size.
    /// Rebuild the configuration after a theme change.
    pub fn typography(mut self, scale: TypographyScale) -> Self {
        self.caption_typography = Some(scale);
        self
    }

    /// Supply caption spacing at construction time.
    /// space2 controls the caption/control gap and the gap between caption cells;
    /// defaults to 4 logical pixels. Negative or non-finite values fall back to 4,
    /// while zero is allowed.
    pub fn spacing_scale(mut self, scale: SpacingScale) -> Self {
        self.caption_spacing = Some(scale);
        self
    }

    /// Pin track, fill and caption colours instead of resolving the live palette.
    pub fn tokens(mut self, tokens: ProgressBarTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    fn semantics(&self, sample: Sampled<f32>) -> Semantics {
        let announced = self.announced_value_text();
        let a11y_label = if !self.label.is_empty() {
            Some(self.label.clone())
        } else {
            announced.clone()
        };
        let base = Semantics::new(SemRole::ProgressIndicator)
            .label_opt(a11y_label)
            .value_opt(announced);
        match sample {
            Sampled::Unsampled => base,
            Sampled::Sampled(v) => base.numeric(NumericValue {
                value: f64::from(v),
                min: 0.0,
                max: 1.0,
                step: None,
            }),
        }
    }
    fn announced_value_text(&self) -> Option<String> {
        if self.sample.is_unsampled() {
            return Some(UNSAMPLED_ANNOUNCEMENT.to_owned());
        }
        if self.value_text.is_empty() {
            None
        } else {
            Some(self.value_text.clone())
        }
    }
    fn painted_value_text(&self) -> String {
        let has_caption = !self.label.is_empty() || !self.value_text.is_empty();
        if self.sample.is_unsampled() && has_caption {
            UNSAMPLED_VALUE_TEXT.to_owned()
        } else if self.sample.is_unsampled() {
            String::new()
        } else {
            self.value_text.clone()
        }
    }

    /// Build the bar, optional caption row and terminal progress accessibility leaf.
    /// Descendant caption nodes are suppressed to avoid duplicate announcements.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let typography = self.caption_typography.unwrap_or_default();
        let caption_size =
            enzui_core::metrics::FinitePositive::new_or(typography.label_size, caption_size())
                .get();
        let caption_gap = enzui_core::metrics::FiniteNonNeg::new_or(
            self.caption_spacing.unwrap_or_default().space2,
            caption_gap(),
        )
        .get();
        let pinned_tokens = self.tokens;
        let tone = self.tone;
        let fill_override = self.fill_override;
        let fill_role = self.fill_role;
        let height = sanitize_dim(self.bar_height, 6.0);
        let radius = sanitize_dim(self.bar_radius, height / 2.0);
        // One normalized state drives both placeholder paint and numeric accessibility.
        let sample = self.sample.map(sanitize_fraction);
        let value = sample.unwrap_or(0.0);
        let unsampled = sample.is_unsampled();
        let painted_value_text = self.painted_value_text();
        let progress_semantics = self.semantics(sample);
        let wrap = move |el: Element<'a, Message>| -> Element<'a, Message> {
            crate::semantics_container::SemanticsContainer::leaf(el, progress_semantics.clone())
                .into()
        };

        let bar_style_tokens = pinned_tokens;
        let sweeping = self.animated && self.busy && value > 0.0 && value < 1.0;
        let animated = self.animated;
        let bar: Element<'a, Message> = if unsampled {
            unsampled_track(height, radius, pinned_tokens)
        } else {
            let painted = std::rc::Rc::new(std::cell::Cell::new(value));
            let gliding = std::rc::Rc::new(std::cell::Cell::new(false));
            let plain: Element<'a, Message> = Element::new(Fill {
                value,
                height,
                animated,
                painted: std::rc::Rc::clone(&painted),
                gliding: std::rc::Rc::clone(&gliding),
                style: Box::new(move |theme: &iced::Theme| {
                    let tokens = bar_style_tokens.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, ProgressBarTokens::from_palette)
                    });
                    let fill = fill_override.unwrap_or_else(|| match fill_role {
                        Some(role) => role(&crate::theme_lookup::palette(theme)),
                        None => tokens.fill_for_tone(tone),
                    });
                    Style {
                        background: tokens.track.into(),
                        bar: fill.into(),
                        border: iced::Border {
                            radius: radius.into(),
                            ..iced::Border::default()
                        },
                    }
                }),
            });
            // Wrapped whether or not it sweeps, so the fill keeps its glide
            // when the bar reaches full.
            Element::new(Sheen {
                child: plain,
                painted,
                gliding,
                sweeping,
                radius,
            })
        };
        if self.label.is_empty() && painted_value_text.is_empty() {
            return wrap(min_width_floor(bar, height));
        }
        let mut caption = row![].spacing(caption_gap);

        if !self.label.is_empty() {
            let label_tokens = pinned_tokens;
            caption = caption.push(
                enz_text(self.label)
                    .typography(typography)
                    .size_px(caption_size)
                    .single_line_elided()
                    .color_resolved(move |theme: &iced::Theme| {
                        label_tokens
                            .unwrap_or_else(|| {
                                crate::theme_lookup::resolve(theme, ProgressBarTokens::from_palette)
                            })
                            .text_color
                    })
                    .width(iced::Fill)
                    .into_element(),
            );
        } else if !painted_value_text.is_empty() {
            caption = caption.push(Space::new().width(iced::Fill));
        }

        if !painted_value_text.is_empty() {
            let val_tokens = pinned_tokens;
            caption = caption.push(
                enz_text(painted_value_text)
                    .typography(typography)
                    .size_px(caption_size)
                    .single_line_elided()
                    .color_resolved(move |theme: &iced::Theme| {
                        let t = val_tokens.unwrap_or_else(|| {
                            crate::theme_lookup::resolve(theme, ProgressBarTokens::from_palette)
                        });
                        if unsampled {
                            t.unsampled
                        } else {
                            t.value_text_color
                        }
                    })
                    .into_element(),
            );
        }

        let stacked = column![caption, bar].spacing(caption_gap);
        wrap(min_width_floor(stacked.into(), height))
    }
}

fn min_width_floor<'a, Message: 'a>(
    content: Element<'a, Message>,
    _bar_height: f32,
) -> Element<'a, Message> {
    crate::min_width::min_width(content, MIN_WIDTH).into()
}

impl<'a, Message: 'a> From<EnzProgressBar> for Element<'a, Message> {
    fn from(bar: EnzProgressBar) -> Self {
        bar.into_element()
    }
}

/// Seconds for one highlight pass along the fill, pause included.
const SHEEN_PERIOD_S: f32 = 1.8;
/// The highlight band's width as a share of the bar's width.
const SHEEN_BAND: f32 = 0.35;

/// A progress bar with a soft highlight sweeping along its filled part.
///
/// The band is painted once and moved by a track, so the render lane sweeps
/// it without the UI thread drawing: a page of bars costs nothing per frame.
/// Between passes, and under reduced motion, it asks for no frames at all.
struct Sheen<'a, Message> {
    child: Element<'a, Message>,
    /// The fill's painted share, as it glides.
    painted: std::rc::Rc<std::cell::Cell<f32>>,
    /// Whether the fill below is gliding: the band rests until it lands.
    gliding: std::rc::Rc<std::cell::Cell<bool>>,
    sweeping: bool,
    radius: f32,
}

/// A track and its fill, gliding to a new value.
struct Fill {
    value: f32,
    height: f32,
    animated: bool,
    /// The share last painted, for the sheen above it.
    painted: std::rc::Rc<std::cell::Cell<f32>>,
    /// Whether the fill is gliding; the sheen rests meanwhile.
    gliding: std::rc::Rc<std::cell::Cell<bool>>,
    style: Box<dyn Fn(&iced::Theme) -> Style>,
}

/// The fill's glide, and the node its bar is recorded in: the bar is painted
/// at full width inside the track's rounded clip and slid into place, so a
/// glide is a track the render lane moves, not a redraw.
struct FillState {
    fade: crate::motion::StateFade<f32>,
    node: iced::advanced::renderer::NodeId,
    recorded: std::cell::Cell<bool>,
}

impl Fill {
    /// Where the full-width bar sits for `value` of a track `width` wide.
    fn slide(value: f32, width: f32) -> iced::Vector {
        iced::Vector::new(-(1.0 - value.clamp(0.0, 1.0)) * width, 0.0)
    }
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer> for Fill {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<FillState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(FillState {
            fade: crate::motion::StateFade::default(),
            node: crate::motion::next_scene_node(),
            recorded: std::cell::Cell::new(false),
        })
    }

    fn size(&self) -> iced::Size<iced::Length> {
        iced::Size::new(iced::Length::Fill, iced::Length::Fixed(self.height))
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        _renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        iced::advanced::layout::atomic_node(limits, iced::Length::Fill, self.height).apply(tree);
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        let fade = &tree.state.downcast_ref::<FillState>().fade;
        fade.is_moving() || fade.needs_frame(&self.value)
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        _cursor: iced::mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &iced::Rectangle,
    ) {
        if let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            let value = self.value;
            let state = tree.state.downcast_mut::<FillState>();
            // A new value the edge would cross in under two pixels lands at
            // once: gliding it moves the edge a fraction of a pixel a frame.
            let small = state.fade.needs_frame(&value)
                && state.fade.value_at(*now).is_some_and(|painted| {
                    (value.clamp(0.0, 1.0) - painted.clamp(0.0, 1.0)).abs() * layout.bounds().width
                        < 2.0
                });
            // A value change glides over `motion-normal`, decelerating.
            let moving = state.fade.frame_over(
                *now,
                (!self.animated).then_some(true),
                small,
                enzui_theme::MotionToken::Normal,
                |_: &iced::Theme| value,
            );
            self.gliding.set(moving);
            if moving {
                if !state.recorded.get() {
                    // The next draw records the node the glide moves.
                    shell.request_redraw();
                } else if let Some(painted) = state.fade.value_at(*now) {
                    let width = layout.bounds().width;
                    let track = |painted: f32| iced::advanced::renderer::Track {
                        translation: Self::slide(painted, width),
                        ..iced::advanced::renderer::Track::identity(state.node)
                    };
                    shell.publish_track(track(painted));
                    // The lane carries the rest of the glide; the UI thread
                    // wakes where it lands to record the bar in place.
                    state.fade.publish(shell, state.node, *now, track);
                    if let Some(lands) = state.fade.lands_at() {
                        shell.schedule_redraw_at(lands);
                    }
                }
            }
        }
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        _cursor: iced::mouse::Cursor,
        _viewport: &iced::Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let style = (self.style)(theme);
        let state = tree.state.downcast_ref::<FillState>();
        let value = state.fade.paint(theme, self.value).clamp(0.0, 1.0);
        self.painted.set(value);
        renderer.fill_quad(
            iced::advanced::renderer::Quad {
                bounds,
                border: style.border,
                ..iced::advanced::renderer::Quad::default()
            },
            style.background,
        );
        // The bar's left end is the track's rounded start, its right end
        // the bar's own rounding.
        renderer.with_layer_rounded(bounds, style.border.radius, |renderer| {
            renderer.with_motion(state.node, Self::slide(value, bounds.width), |renderer| {
                renderer.fill_quad(
                    iced::advanced::renderer::Quad {
                        bounds,
                        border: iced::Border {
                            color: iced::Color::TRANSPARENT,
                            ..style.border
                        },
                        ..iced::advanced::renderer::Quad::default()
                    },
                    style.bar,
                );
            });
        });
        state.recorded.set(true);
    }
}

/// When the sweep started, the latest presented frame, and the motion
/// policy the last draw read.
#[derive(Debug)]
struct SheenState {
    node: iced::advanced::renderer::NodeId,
    epoch: Option<iced::time::Instant>,
    frame_at: Option<iced::time::Instant>,
    reduced: std::cell::Cell<bool>,
    /// Whether the last draw painted the band, which a replay moves.
    recorded: std::cell::Cell<bool>,
}

/// Where the band is in the pass at `now`: `0` entering the fill, `1` past
/// it, `None` resting between passes.
fn sheen_travel(epoch: iced::time::Instant, now: iced::time::Instant) -> Option<f32> {
    let phase =
        (now.saturating_duration_since(epoch).as_secs_f32() % SHEEN_PERIOD_S) / SHEEN_PERIOD_S;
    // The pass crosses the fill in the first two thirds of the period and
    // rests for the last third.
    let travel = phase / SHEEN_TRAVEL;
    (travel < 1.0).then_some(travel)
}

/// The share of the period the band spends crossing the fill.
const SHEEN_TRAVEL: f32 = 0.66;

impl<Message> Sheen<'_, Message> {
    /// The fill, the band's width and how far the band travels across it.
    fn geometry(&self, bounds: iced::Rectangle) -> (iced::Rectangle, f32, f32) {
        let fill = iced::Rectangle {
            width: bounds.width * self.painted.get(),
            ..bounds
        };
        let band = (bounds.width * SHEEN_BAND).max(24.0);
        (fill, band, fill.width + band)
    }

    fn track(node: iced::advanced::renderer::NodeId, x: f32) -> iced::advanced::renderer::Track {
        iced::advanced::renderer::Track {
            translation: iced::Vector::new(x, 0.0),
            ..iced::advanced::renderer::Track::identity(node)
        }
    }
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer> for Sheen<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<SheenState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(SheenState {
            node: crate::motion::next_scene_node(),
            epoch: None,
            frame_at: None,
            reduced: std::cell::Cell::new(false),
            recorded: std::cell::Cell::new(false),
        })
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> iced::Size<iced::Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        _cursor: iced::mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &iced::Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            _cursor,
            _renderer,
            shell,
            _viewport,
        );
        let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return;
        };
        if !self.sweeping || self.gliding.get() {
            return;
        }
        let state = tree.state.downcast_mut::<SheenState>();
        state.frame_at = Some(*now);
        let epoch = *state.epoch.get_or_insert(*now);
        let (fill, _, distance) = self.geometry(layout.bounds());
        if state.reduced.get() || fill.width <= 1.0 {
            return;
        }
        let period = iced::time::Duration::from_secs_f32(SHEEN_PERIOD_S);
        let elapsed = now.saturating_duration_since(epoch).as_secs_f32();
        let pass_start = epoch + period.mul_f32((elapsed / SHEEN_PERIOD_S).floor());
        let Some(travel) = sheen_travel(epoch, *now) else {
            // Resting: nothing moves until the next pass begins.
            shell.schedule_redraw_at(pass_start + period);
            return;
        };
        crate::motion::request_motion_frame(shell, state.recorded.get());
        if !state.recorded.get() {
            return;
        }
        let node = state.node;
        shell.publish_track(Self::track(node, travel * distance));
        let pass_end = pass_start + period.mul_f32(SHEEN_TRAVEL);
        let remaining = pass_end.saturating_duration_since(*now);
        if !remaining.is_zero() {
            shell.publish_track_animation(iced::advanced::renderer::TrackAnimation {
                node,
                start: *now,
                duration: remaining,
                keyframes: vec![
                    (0.0, Self::track(node, travel * distance)),
                    (1.0, Self::track(node, distance)),
                ],
                generation: (elapsed / SHEEN_PERIOD_S) as u64,
            });
        }
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        let state = tree.state.downcast_ref::<SheenState>();
        let reduced = crate::theme_lookup::motion(theme).reduced_motion;
        state.reduced.set(reduced);
        state.recorded.set(false);
        if !self.sweeping || self.gliding.get() {
            return;
        }
        let (Some(epoch), Some(now)) = (state.epoch, state.frame_at) else {
            return;
        };
        let (fill, band, distance) = self.geometry(layout.bounds());
        if reduced || fill.width <= 1.0 {
            return;
        }
        // Resting, the band waits where the next pass starts: left of the
        // fill and clipped away, but recorded, so that pass moves it by track
        // from its first frame.
        let travel = sheen_travel(epoch, now).unwrap_or(0.0);
        let dark = crate::theme_lookup::palette(theme).is_dark();
        let peak = iced::Color::from_rgba(1.0, 1.0, 1.0, if dark { 0.22 } else { 0.38 });
        let clear = iced::Color { a: 0.0, ..peak };
        let gradient = iced::gradient::Linear::new(std::f32::consts::FRAC_PI_2)
            .add_stop(0.0, clear)
            .add_stop(0.5, peak)
            .add_stop(1.0, clear);
        // The band is painted where the pass starts and moved to where it is:
        // the same paint a replay moves along the pass.
        renderer.with_layer(fill, |renderer| {
            renderer.with_motion(
                state.node,
                iced::Vector::new(travel * distance, 0.0),
                |renderer| {
                    renderer.fill_quad(
                        iced::advanced::renderer::Quad {
                            bounds: iced::Rectangle {
                                x: fill.x - band,
                                width: band,
                                ..fill
                            },
                            border: iced::Border {
                                radius: self.radius.into(),
                                ..iced::Border::default()
                            },
                            ..iced::advanced::renderer::Quad::default()
                        },
                        iced::Background::Gradient(gradient.into()),
                    );
                },
            );
        });
        state.recorded.set(true);
    }
}
