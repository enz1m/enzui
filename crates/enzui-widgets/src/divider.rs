//! Horizontal or vertical rules with optional captions and faded ends.
//!
//! Horizontal captions split the remaining width between equal rule segments.
//! Caption rows reserve a minimum of effective label size plus 6px and may grow;
//! vertical rules ignore captions. Labels use the ambient Medium weight, default
//! to scaled 11px size and publish one text label. Unlabelled rules are decorative.
//!
//! Line ink follows border_strong and caption ink follows text_muted unless
//! pinned. Explicit line colour wins over tokens. Geometry resolves at composition;
//! invalid sizes and gaps use stock fallbacks. Zero thickness paints no rule
//! while retaining any caption. Positive thickness snaps to physical pixels.
//! Fading changes end alpha over approximately the outer fifth of each segment.

use crate::enz_text::{TextTone, enz_text};
use crate::min_height::min_height;
use enzui_theme::Palette;
use iced::widget::rule::{self, FillMode};
use iced::widget::{container, row};
use iced::{Color, Element, Fill};
const DEFAULT_THICKNESS: f32 = 1.0;
const DEFAULT_LABEL_SIZE: f32 = 11.0;
const DEFAULT_LABEL_GAP: f32 = 12.0;

const LABEL_ROW_PADDING: f32 = 6.0;

fn sanitize_thickness(px: f32, fallback: f32) -> f32 {
    crate::hairline::sanitize_thickness(px, fallback)
}
fn effective_label_size(px: f32) -> f32 {
    if px.is_finite() && px > 0.0 {
        px
    } else {
        DEFAULT_LABEL_SIZE
    }
}
fn effective_label_gap(px: f32) -> f32 {
    if px.is_finite() && px >= 0.0 {
        px
    } else {
        DEFAULT_LABEL_GAP
    }
}
fn labeled_height(raw_label_size: f32) -> f32 {
    effective_label_size(raw_label_size) + LABEL_ROW_PADDING
}

fn rule_style(color: Color, thickness: f32) -> rule::Style {
    rule::Style {
        color,
        radius: 0.0.into(),
        fill_mode: FillMode::Full,
        // Physical snapping would resurrect a zero-thickness rule as a hairline.
        snap: crate::hairline::rule_snap(thickness),
    }
}
fn line_color_of(
    pinned: Option<DividerTokens>,
    color_override: Option<Color>,
    theme: &iced::Theme,
) -> Color {
    color_override
        .or_else(|| pinned.map(|t| t.line_color))
        .unwrap_or_else(|| {
            DividerTokens::from_palette(&crate::theme_lookup::palette(theme)).line_color
        })
}
fn label_color_of(pinned: Option<DividerTokens>, theme: &iced::Theme) -> Color {
    pinned.map_or_else(
        || DividerTokens::from_palette(&crate::theme_lookup::palette(theme)).label_color,
        |t| t.label_color,
    )
}

fn label_run(
    text: &str,
    label_size: f32,
    pinned: Option<DividerTokens>,
) -> crate::enz_text::EnzText {
    let run = enz_text(text)
        .size_px(label_size)
        .weight_rung(enzui_theme::WeightRung::Medium)
        .single_line_elided();
    match pinned {
        Some(t) => run.color(t.label_color),
        None => run.tone(TextTone::Muted),
    }
}
/// Direction of the rule and its fill axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DividerOrientation {
    /// Fill available width; this is the default.
    #[default]
    Horizontal,
    /// Fill available height and ignore any caption.
    Vertical,
}
/// Pinned divider ink, geometry and fading defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DividerTokens {
    /// Pinned rule ink.
    pub line_color: Color,
    /// Pinned caption ink for horizontal rules.
    pub label_color: Color,
    /// Caption size in logical pixels; invalid values use stock 11.
    pub label_size: f32,
    /// Gap on each side of the caption in logical pixels; invalid values use 12.
    pub label_gap: f32,
    /// Rule thickness in logical pixels; zero is preserved.
    pub thickness: f32,
    /// Fade both ends of each segment; defaults to false.
    pub fade_ends: bool,
}

impl DividerTokens {
    /// Pin border_strong and text_muted with scaled 11px caption size.
    /// Other defaults are 12px caption gaps, 1px thickness and no fading.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            line_color: p.border_strong,
            label_color: p.text_muted,
            label_size: enzui_theme::text_scale::ui_px(DEFAULT_LABEL_SIZE),
            label_gap: DEFAULT_LABEL_GAP,
            thickness: DEFAULT_THICKNESS,
            fade_ends: false,
        }
    }
    /// Pin defaults using the stock dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Pin defaults using the stock light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}
/// Resolved divider dimensions after override precedence and validation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedDividerLayout {
    /// Rule thickness in logical pixels; zero is preserved.
    pub thickness: f32,
    /// Caption size in logical pixels; invalid values use stock 11.
    pub label_size: f32,
    /// Gap on each side of the caption in logical pixels; invalid values use 12.
    pub label_gap: f32,
    /// Caption-row height floor: effective label size plus 6px.
    pub labeled_height: f32,
}
/// Rule builder with an optional horizontal caption.
#[must_use]
pub struct EnzDivider {
    orientation: DividerOrientation,
    label: Option<String>,
    tokens: Option<DividerTokens>,
    thickness: Option<f32>,
    label_size: Option<f32>,
    label_gap: Option<f32>,
    color: Option<Color>,
    fade_ends: Option<bool>,
}
/// Construct an unlabelled horizontal rule with live ink and default geometry.
pub fn enz_divider() -> EnzDivider {
    EnzDivider {
        orientation: DividerOrientation::Horizontal,
        label: None,
        tokens: None,
        thickness: None,
        label_size: None,
        label_gap: None,
        color: None,
        fade_ends: None,
    }
}

impl EnzDivider {
    /// Select a vertical rule; captions are ignored.
    pub fn vertical(mut self) -> Self {
        self.orientation = DividerOrientation::Vertical;
        self
    }
    /// Select a horizontal rule; this is the default.
    pub fn horizontal(mut self) -> Self {
        self.orientation = DividerOrientation::Horizontal;
        self
    }
    /// Set a horizontal caption; empty text removes it. Vertical rules ignore it.
    pub fn label(mut self, label: &str) -> Self {
        if label.is_empty() {
            self.label = None;
        } else {
            self.label = Some(label.to_owned());
        }
        self
    }
    /// Pin ink, geometry and fading defaults; explicit setters take precedence.
    pub fn tokens(mut self, tokens: DividerTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Set logical rule thickness; zero paints no rule.
    /// Negative or non-finite values fall back to valid pinned thickness, then stock 1px.
    pub fn thickness(mut self, px: f32) -> Self {
        self.thickness = Some(px);
        self
    }
    /// Set caption size in logical pixels and its size-plus-6 height floor.
    /// Nonpositive or non-finite input uses stock 11px, without pinning ink.
    pub fn label_size(mut self, px: f32) -> Self {
        self.label_size = Some(px);
        self
    }
    /// Set caption gaps in logical pixels; zero is allowed.
    /// Negative or non-finite input uses stock 12px.
    pub fn label_gap(mut self, px: f32) -> Self {
        self.label_gap = Some(px);
        self
    }
    /// Override end fading for both axes and captioned segments; defaults to false.
    pub fn fade_ends(mut self, fade: bool) -> Self {
        self.fade_ends = Some(fade);
        self
    }
    /// Pin rule ink, overriding tokens while leaving caption ink unaffected.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
    /// Resolve geometry using explicit overrides, pinned tokens, then defaults.
    /// Invalid thickness may use a valid pin; invalid caption size or gaps use stock fallbacks.
    pub fn resolved_layout(&self) -> ResolvedDividerLayout {
        let pinned = self.tokens;
        let thickness = match self.thickness {
            Some(px) => sanitize_thickness(px, pinned.map_or(DEFAULT_THICKNESS, |t| t.thickness)),
            None => sanitize_thickness(
                pinned.map_or(DEFAULT_THICKNESS, |t| t.thickness),
                DEFAULT_THICKNESS,
            ),
        };
        let raw_label_size = self
            .label_size
            .or_else(|| pinned.map(|t| t.label_size))
            .unwrap_or_else(|| enzui_theme::text_scale::ui_px(DEFAULT_LABEL_SIZE));
        let raw_label_gap = self
            .label_gap
            .or_else(|| pinned.map(|t| t.label_gap))
            .unwrap_or(DEFAULT_LABEL_GAP);

        ResolvedDividerLayout {
            thickness,
            label_size: effective_label_size(raw_label_size),
            label_gap: effective_label_gap(raw_label_gap),
            labeled_height: labeled_height(raw_label_size),
        }
    }
    /// Resolve rule ink: explicit colour, pinned tokens, then live border_strong.
    pub fn line_color(&self, theme: &iced::Theme) -> Color {
        line_color_of(self.tokens, self.color, theme)
    }
    /// Resolve caption ink: pinned tokens, then live text_muted.
    pub fn label_color(&self, theme: &iced::Theme) -> Color {
        label_color_of(self.tokens, theme)
    }
    /// Compose the rule or caption row, resolving unpinned ink during painting.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let geo = self.resolved_layout();
        let pinned = self.tokens;
        let color_override = self.color;

        let faded = self
            .fade_ends
            .or_else(|| pinned.map(|t| t.fade_ends))
            .unwrap_or(false);
        // Which ends of a segment fade: both for a plain rule; beside a
        // caption, only the outer ends, so the line runs solid up to it.
        let segment =
            move |vertical: bool, fade_start: bool, fade_end: bool| -> Element<'a, Message> {
                if faded {
                    Element::new(FadedRule {
                        vertical,
                        thickness: geo.thickness,
                        pinned,
                        color_override,
                        fade_start,
                        fade_end,
                    })
                } else {
                    let style = move |theme: &iced::Theme| {
                        rule_style(line_color_of(pinned, color_override, theme), geo.thickness)
                    };
                    if vertical {
                        rule::vertical(geo.thickness).style(style).into()
                    } else {
                        rule::horizontal(geo.thickness).style(style).into()
                    }
                }
            };

        match self.orientation {
            DividerOrientation::Horizontal => {
                let Some(label_text) = self.label else {
                    return segment(false, true, true);
                };

                let label_el = label_run(&label_text, geo.label_size, pinned);
                let label_el: Element<'a, Message> = label_el.into();
                min_height(
                    row![
                        segment(false, true, false),
                        label_el,
                        segment(false, false, true)
                    ]
                    .spacing(geo.label_gap)
                    .align_y(iced::Center)
                    .width(Fill),
                    geo.labeled_height,
                )
                .into()
            }
            DividerOrientation::Vertical => {
                // Pin cross-axis width so a Fill-width parent cannot widen the vertical rule.
                container(segment(true, true, true))
                    .width(iced::Length::Fixed(geo.thickness))
                    .height(Fill)
                    .into()
            }
        }
    }
}

impl<'a, Message: 'a> From<EnzDivider> for Element<'a, Message> {
    fn from(div: EnzDivider) -> Self {
        div.into_element()
    }
}

struct FadedRule {
    vertical: bool,
    thickness: f32,
    pinned: Option<DividerTokens>,
    color_override: Option<Color>,
    /// Fade the leading end (left or top).
    fade_start: bool,
    /// Fade the trailing end (right or bottom).
    fade_end: bool,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer> for FadedRule {
    fn size(&self) -> iced::Size<iced::Length> {
        if self.vertical {
            iced::Size::new(iced::Length::Fixed(self.thickness), iced::Length::Fill)
        } else {
            iced::Size::new(iced::Length::Fill, iced::Length::Fixed(self.thickness))
        }
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        _renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        let size =
            <Self as iced::advanced::Widget<Message, iced::Theme, iced::Renderer>>::size(self);
        iced::advanced::layout::paint_box(limits, size.width, size.height).apply(tree);
    }

    fn draw(
        &self,
        _tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        _cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let scale = renderer.hint_factor().unwrap_or(1.0);
        let bounds = iced::advanced::layout::rule::Paint {
            vertical: self.vertical,
            fill: FillMode::Full,
            snap: crate::hairline::rule_snap(self.thickness),
        }
        .bounds(layout.bounds(), scale)
        .hint(scale);
        let full_thickness = if self.vertical {
            bounds.width
        } else {
            bounds.height
        };
        if self.thickness == 0.0 || bounds.intersection(viewport).is_none() {
            return;
        }
        let length = if self.vertical {
            bounds.height
        } else {
            bounds.width
        };
        let fade = (length * 0.2 * scale).round() / scale;
        let color = line_color_of(self.pinned, self.color_override, theme);
        if fade <= 0.0 {
            renderer.fill_quad(
                iced::advanced::renderer::Quad {
                    bounds,
                    snap: false,
                    ..Default::default()
                },
                color,
            );
            return;
        }
        // Each fading end is one quad whose colour runs linearly from
        // transparent to the line's: the line keeps its thickness and only
        // its ink fades out.
        let solid_from = if self.fade_start { fade } else { 0.0 };
        let solid_to = if self.fade_end { length - fade } else { length };
        let rect = |from: f32, extent: f32| {
            let thickness = full_thickness;
            if self.vertical {
                iced::Rectangle {
                    x: bounds.center().x - thickness / 2.0,
                    y: bounds.y + from,
                    width: thickness,
                    height: extent,
                }
            } else {
                iced::Rectangle {
                    x: bounds.x + from,
                    y: bounds.center().y - thickness / 2.0,
                    width: extent,
                    height: thickness,
                }
            }
        };
        // `Degrees(90)` runs left to right, `Degrees(180)` top to bottom.
        let along = if self.vertical { 180.0 } else { 90.0 };
        let mut paint = |rect: iced::Rectangle, background: iced::Background| {
            if rect.width > 0.0 && rect.height > 0.0 && rect.intersects(viewport) {
                renderer.fill_quad(
                    iced::advanced::renderer::Quad {
                        bounds: rect,
                        snap: false,
                        ..Default::default()
                    },
                    background,
                );
            }
        };
        // Eased out of the clear end: the line keeps nearly its full ink
        // until close to its tip and then dissolves. A straight ramp leaves a
        // long faint tail that reads as the line narrowing to a point.
        let ramp = |clear_first: bool| {
            let mut linear = iced::gradient::Linear::new(iced::Degrees(along));
            // Stops in ascending offset; `t` runs from the clear tip.
            for offset in [0.0_f32, 0.12, 0.28, 0.5, 0.72, 0.88, 1.0] {
                let t = if clear_first { offset } else { 1.0 - offset };
                let ink = Color {
                    a: color.a * (1.0 - (1.0 - t).powi(3)),
                    ..color
                };
                linear = linear.add_stop(offset, ink);
            }
            iced::Background::Gradient(iced::Gradient::Linear(linear))
        };
        paint(rect(solid_from, solid_to - solid_from), color.into());
        if self.fade_start {
            paint(rect(0.0, fade), ramp(true));
        }
        if self.fade_end {
            paint(rect(length - fade, fade), ramp(false));
        }
    }
}
