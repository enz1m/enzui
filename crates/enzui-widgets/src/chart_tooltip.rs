//! Chart data callouts with an optional title and owned, formatted series rows.
//!
//! Rows share label and value columns; constrained text wraps, and values move
//! below labels when both columns cannot fit. Unpinned colours follow the live
//! palette while geometry uses the constants below. Transparent swatches use
//! the accent. Text uses the ambient Body typography.
//!
//! Callers position the callout, optionally with [`position_chart_tooltip`].
//! Shadows may extend beyond its layout bounds.

use crate::chart_legend::{ChartLegendEntry, LABEL_GAP, SWATCH_RADIUS, SWATCH_SIZE};
use crate::enz_text::{TextSize, TextTokens, enz_text};
use enzui_core::metrics::FinitePositive;
use enzui_core::placement::place_at_point;
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::border::Radius;
use iced::widget::{Space, container};
use iced::{
    Background, Border, Color, Element, Event, Length, Padding, Point, Rectangle, Shadow, Size,
    Vector,
};

/// Default padding on each side in logical pixels: 8.
pub const TOOLTIP_PADDING: f32 = 8.0;
/// Default vertical spacing in logical pixels: 2.
pub const ROW_SPACING: f32 = 2.0;
/// Default minimum gap between label and value columns in logical pixels: 12.
pub const MIN_SPACER: f32 = 12.0;
/// Default shadow blur radius in logical pixels: 12.
pub const SHADOW_BLUR: f32 = 12.0;
/// Default border width in logical pixels: 1.
pub const BORDER_WIDTH: f32 = 1.0;
/// Default shadow ink: black at 20% alpha, independent of the palette.
pub const SHADOW_COLOR: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.20,
};
fn sane_dim(v: f32, default: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        default
    }
}
fn sane_size(v: f32, default: f32) -> f32 {
    FinitePositive::new_or(v, default).get()
}
#[derive(Debug, Clone, Copy)]
struct RowGeom {
    swatch_size: f32,
    swatch_gap: f32,
    label_col_w: f32,
    min_spacer: f32,
    value_col_w: f32,
}

impl RowGeom {
    fn swatch_x(&self) -> f32 {
        0.0
    }
    fn label_x(&self) -> f32 {
        self.swatch_size + self.swatch_gap
    }
    fn value_col_x(&self) -> f32 {
        self.label_x() + self.label_col_w + self.min_spacer
    }
    fn row_w(&self) -> f32 {
        self.value_col_x() + self.value_col_w
    }
}
type RowCells<'a, Message, Theme, Renderer> = (
    Element<'a, Message, Theme, Renderer>,
    Element<'a, Message, Theme, Renderer>,
    Element<'a, Message, Theme, Renderer>,
);
struct AlignedTooltipRows<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    title: Option<Element<'a, Message, Theme, Renderer>>,
    rows: Vec<RowCells<'a, Message, Theme, Renderer>>,
    swatch_size: f32,
    swatch_gap: f32,
    min_spacer: f32,
    row_spacing: f32,
}

impl<'a, Message, Theme, Renderer> AlignedTooltipRows<'a, Message, Theme, Renderer> {
    fn children_iter(&self) -> impl Iterator<Item = &Element<'a, Message, Theme, Renderer>> {
        self.title
            .iter()
            .chain(self.rows.iter().flat_map(|(s, l, v)| [s, l, v]))
    }
    fn children_iter_mut(
        &mut self,
    ) -> impl Iterator<Item = &mut Element<'a, Message, Theme, Renderer>> {
        self.title
            .iter_mut()
            .chain(self.rows.iter_mut().flat_map(|(s, l, v)| [s, l, v]))
    }
    fn child_count(&self) -> usize {
        usize::from(self.title.is_some()) + self.rows.len() * 3
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for AlignedTooltipRows<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        let mut flat: Vec<&mut Element<'_, Message, Theme, Renderer>> =
            Vec::with_capacity(self.child_count());
        if let Some(t) = self.title.as_mut() {
            flat.push(t);
        }
        for (s, l, v) in &mut self.rows {
            flat.push(s);
            flat.push(l);
            flat.push(v);
        }
        // Title changes shift flat slots; reconcile tags before diffing replaced cells.
        tree.diff_children_custom(
            &mut flat,
            |state, el| state.diff(el.as_widget_mut()),
            |el| Tree::new(el.as_widget()),
        );
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let max = limits.max();
        let measure = layout::Limits::new(Size::ZERO, max);
        if tree.children.len() != self.child_count() {
            self.diff(tree);
        }

        let has_title = self.title.is_some();
        let title_start = 0usize;
        let rows_start = usize::from(has_title);

        let mut label_col_w: f32 = 0.0;
        let mut value_col_w: f32 = 0.0;
        let mut line_height = f32::INFINITY;

        for (i, (_, label, value)) in self.rows.iter_mut().enumerate() {
            let ti = rows_start + i * 3;
            let ln = label
                .as_widget_mut()
                .measure(&mut tree.children[ti + 1], renderer, &measure);
            let vn = value
                .as_widget_mut()
                .measure(&mut tree.children[ti + 2], renderer, &measure);
            label_col_w = label_col_w.max(ln.size().width);
            value_col_w = value_col_w.max(vn.size().width);
            line_height = line_height.min(ln.size().height).min(vn.size().height);
        }

        let fixed_w = self.swatch_size + self.swatch_gap + self.min_spacer;
        let lane_budget = (max.width - fixed_w).max(0.0);
        let natural_lanes = label_col_w + value_col_w;
        let stacked =
            natural_lanes > lane_budget && value_col_w + label_col_w.min(line_height) > lane_budget;
        if stacked {
            label_col_w = (max.width - self.swatch_size - self.swatch_gap).max(0.0);
            value_col_w = label_col_w;
        } else if natural_lanes > lane_budget {
            label_col_w = (lane_budget - value_col_w).max(0.0);
        }

        let geom = RowGeom {
            swatch_size: self.swatch_size,
            swatch_gap: self.swatch_gap,
            label_col_w,
            min_spacer: self.min_spacer,
            value_col_w,
        };
        let rows_w = if self.rows.is_empty() {
            0.0
        } else {
            if stacked {
                geom.label_x() + label_col_w
            } else {
                geom.row_w()
            }
        };
        // Measure against the parent first so a title can widen short or empty rows.
        let mut title_w = 0.0;
        let mut title_h = 0.0;
        if let Some(title) = self.title.as_mut() {
            let tl = layout::Limits::new(Size::ZERO, Size::new(max.width.max(0.0), max.height));
            let tn = title
                .as_widget_mut()
                .measure(&mut tree.children[title_start], renderer, &tl);
            title_w = tn.size().width;
            title_h = tn.size().height;
        }
        let row_w = rows_w.max(title_w).max(0.0).min(max.width.max(0.0));
        let mut nodes: Vec<layout::Node> = Vec::with_capacity(self.child_count());
        let mut y = 0.0;
        if let Some(title) = self.title.as_mut() {
            let tl = layout::Limits::new(Size::ZERO, Size::new(row_w.max(0.0), max.height));
            let mut tn =
                title
                    .as_widget_mut()
                    .measure(&mut tree.children[title_start], renderer, &tl);
            tn.move_to_mut(iced::Point::new(0.0, y));
            nodes.push(tn);
            y += title_h;
        }

        for (i, (swatch, label, value)) in self.rows.iter_mut().enumerate() {
            if has_title || i > 0 {
                y += self.row_spacing;
            }
            let ti = rows_start + i * 3;

            let mut sn = swatch.as_widget_mut().measure(
                &mut tree.children[ti],
                renderer,
                &layout::Limits::new(
                    Size::new(self.swatch_size, self.swatch_size),
                    Size::new(self.swatch_size, self.swatch_size),
                ),
            );
            let sh = sn.size().height;

            let mut ln = label.as_widget_mut().measure(
                &mut tree.children[ti + 1],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::new(label_col_w.max(0.0), max.height)),
            );
            let lh = ln.size().height;

            let mut vn = value.as_widget_mut().measure(
                &mut tree.children[ti + 2],
                renderer,
                &layout::Limits::new(Size::ZERO, Size::new(value_col_w.max(0.0), max.height)),
            );
            let vh = vn.size().height;
            let row_h = if stacked {
                let label_h = sh.max(lh);
                sn.move_to_mut(Point::new(geom.swatch_x(), y + (label_h - sh) / 2.0));
                ln.move_to_mut(Point::new(geom.label_x(), y + (label_h - lh) / 2.0));
                vn.move_to_mut(Point::new(geom.label_x(), y + label_h + self.row_spacing));
                label_h + self.row_spacing + vh
            } else {
                let row_h = sh.max(lh).max(vh);
                sn.move_to_mut(Point::new(geom.swatch_x(), y + (row_h - sh) / 2.0));
                ln.move_to_mut(Point::new(geom.label_x(), y + (row_h - lh) / 2.0));
                vn.move_to_mut(Point::new(geom.value_col_x(), y + (row_h - vh) / 2.0));
                row_h
            };

            nodes.push(sn);
            nodes.push(ln);
            nodes.push(vn);
            y += row_h;
        }

        layout::Node::with_children(Size::new(row_w.max(0.0), y), nodes).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (layout, state)) in self
                .children_iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, layout, viewport, renderer, operation);
            }
        });
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
        for (child, (layout, state)) in self
            .children_iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, layout, cursor, renderer, shell, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.children_iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (layout, state))| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
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
        for (child, (layout, state)) in self.children_iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, layout, cursor, viewport);
        }
    }
}

/// Default placement clearance from plot edges in logical pixels: 4.
pub const TOOLTIP_EDGE_MARGIN: f32 = 4.0;
/// Return the callout top-left point using [`enzui_core::placement::place_at_point`].
/// Anchor and plot must share a coordinate space. Each axis flips when needed
/// and clamps within the plot and margin; an oversized callout uses the near
/// margin. Invalid sizes normalize to zero and invalid margins to zero, following
/// the shared placement policy. This helper does not position a widget itself.
#[must_use]
pub fn position_chart_tooltip(
    anchor: Point,
    tooltip_size: Size,
    plot: Rectangle,
    edge_margin: f32,
) -> Point {
    let resolved = place_at_point(anchor, tooltip_size, plot, edge_margin);
    Point::new(resolved.bounds.x, resolved.bounds.y)
}

/// An owned series entry paired with its host-formatted value.
#[derive(Debug, Clone)]
pub struct ChartTooltipRow {
    /// Series label and swatch colour.
    pub entry: ChartLegendEntry,
    /// Host-formatted value text; narrow callouts may wrap it below the label.
    pub value: String,
}

impl ChartTooltipRow {
    /// Pair a series entry with owned, formatted value text.
    pub fn new(entry: ChartLegendEntry, value: impl Into<String>) -> Self {
        Self {
            entry,
            value: value.into(),
        }
    }
}

/// Callout colour and geometry overrides.
#[derive(Debug, Clone, Copy)]
pub struct ChartTooltipTokens {
    /// Surface fill; defaults to the palette elevated surface.
    pub background: Color,
    /// Outline ink derived from the elevated surface with the shared edge-depth policy.
    pub border: Color,
    /// Outline width in logical pixels; defaults to 1.
    pub border_width: f32,
    /// Callout corner radius in logical pixels; defaults to 5.
    pub radius: f32,
    /// Padding on each side in logical pixels; defaults to 8.
    pub padding: f32,
    /// Vertical gap between rows and stacked label/value bands; defaults to 2 px.
    pub row_spacing: f32,
    /// Swatch edge length in logical pixels; defaults to 12 and must be positive.
    pub swatch_size: f32,
    /// Swatch corner radius in logical pixels; defaults to 3.
    pub swatch_radius: f32,
    /// Swatch-to-label gap in logical pixels; defaults to 4.
    pub swatch_gap: f32,
    /// Minimum label-to-value column gap in logical pixels; defaults to 12.
    pub min_spacer: f32,
    /// Shadow blur radius in logical pixels; defaults to 12 with no offset.
    pub shadow_blur: f32,
    /// Shadow ink; defaults to black at 20% alpha.
    pub shadow_color: Color,
    /// Title and value ink; defaults to the palette primary text colour.
    pub text_color: Color,
    /// Series-label ink; defaults to the palette muted text colour.
    pub muted_color: Color,
    /// Fallback swatch ink when entry alpha is not positive; defaults to palette accent.
    pub accent: Color,
}

impl ChartTooltipTokens {
    /// Resolve palette colours with fixed default geometry.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_elevated,
            border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            border_width: BORDER_WIDTH,
            radius: enzui_theme::text_defaults::shape_radius(|s| s.tooltip),
            padding: TOOLTIP_PADDING,
            row_spacing: ROW_SPACING,
            swatch_size: SWATCH_SIZE,
            swatch_radius: SWATCH_RADIUS,
            swatch_gap: LABEL_GAP,
            min_spacer: MIN_SPACER,
            shadow_blur: SHADOW_BLUR,
            shadow_color: SHADOW_COLOR,
            text_color: p.text,
            muted_color: p.text_muted,
            accent: p.accent,
        }
    }

    /// Use the built-in dark palette and default geometry.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Use the built-in light palette and default geometry.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Keep entry ink when alpha is positive, including partial alpha; otherwise use accent.
    pub fn resolve_fill(&self, entry_color: Color) -> Color {
        if entry_color.a > 0.0 {
            entry_color
        } else {
            self.accent
        }
    }
}

/// Builder for a visual chart callout without event callbacks or automatic positioning.
#[must_use]
pub struct EnzChartTooltip {
    title: Option<String>,
    rows: Vec<ChartTooltipRow>,
    tokens: Option<ChartTooltipTokens>,
}

impl EnzChartTooltip {
    /// Set an optional title; blank or whitespace-only text removes its row.
    /// Nonblank text retains its original whitespace.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        let t = title.into();
        self.title = if t.trim().is_empty() { None } else { Some(t) };
        self
    }

    /// Pin colours and geometry instead of resolving live palette colours.
    /// Negative or non-finite dimensions fall back to their defaults. Zero is allowed
    /// except for swatch size, which falls back to 12 px unless finite and positive.
    pub fn tokens(mut self, tokens: ChartTooltipTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Build the callout with ambient Body typography and wrapping text.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let pinned = self.tokens;
        let geo = pinned.unwrap_or_else(ChartTooltipTokens::dark);
        let radius = sane_dim(
            geo.radius,
            enzui_theme::text_defaults::shape_radius(|s| s.tooltip),
        );
        let padding = sane_dim(geo.padding, TOOLTIP_PADDING);
        let row_spacing = sane_dim(geo.row_spacing, ROW_SPACING);
        let swatch_size = sane_size(geo.swatch_size, SWATCH_SIZE);
        let swatch_radius = sane_dim(geo.swatch_radius, SWATCH_RADIUS);
        let swatch_gap = sane_dim(geo.swatch_gap, LABEL_GAP);
        let min_spacer = sane_dim(geo.min_spacer, MIN_SPACER);
        let border_width = sane_dim(geo.border_width, BORDER_WIDTH);
        let shadow_blur = sane_dim(geo.shadow_blur, SHADOW_BLUR);
        let title_el: Option<Element<'a, Message>> = self.title.map(|title| {
            let mut t = enz_text(title)
                .size(TextSize::Body)
                .weight_rung(enzui_theme::WeightRung::Semibold)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
            if let Some(tok) = pinned {
                t = t.tokens(fixed_text_tokens(tok.text_color));
            }
            t.into()
        });

        let mut row_cells: Vec<RowCells<'a, Message, iced::Theme, iced::Renderer>> =
            Vec::with_capacity(self.rows.len());

        for r in self.rows {
            let entry_color = r.entry.color;
            let swatch = container(Space::new())
                .width(Length::Fixed(swatch_size))
                .height(Length::Fixed(swatch_size))
                .style(move |theme: &iced::Theme| {
                    let accent = match pinned {
                        Some(t) => t.accent,
                        None => crate::theme_lookup::palette(theme).accent,
                    };
                    let fill = if entry_color.a > 0.0 {
                        entry_color
                    } else {
                        accent
                    };
                    container::Style {
                        background: Some(Background::Color(fill)),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: Radius::from(swatch_radius),
                        },
                        ..Default::default()
                    }
                });
            let mut label = enz_text(r.entry.label)
                .size(TextSize::Body)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
            let mut value = enz_text(r.value)
                .size(TextSize::Body)
                .weight_rung(enzui_theme::WeightRung::Medium)
                .wrapping(iced::widget::text::Wrapping::WordOrGlyph);
            if let Some(tok) = pinned {
                label = label.tokens(fixed_text_tokens(tok.muted_color));
                value = value.tokens(fixed_text_tokens(tok.text_color));
            } else {
                label = label.tone(crate::enz_text::TextTone::Muted);
            }

            row_cells.push((swatch.into(), label.into(), value.into()));
        }

        let rows_col: Element<'a, Message> = Element::new(AlignedTooltipRows {
            title: title_el,
            rows: row_cells,
            swatch_size,
            swatch_gap,
            min_spacer,
            row_spacing,
        });

        container(rows_col)
            .padding(Padding::from(padding))
            .width(Length::Shrink)
            .style(move |theme: &iced::Theme| {
                let (bg, border_c) = match pinned {
                    Some(t) => (t.background, t.border),
                    None => {
                        let p = crate::theme_lookup::palette(theme);
                        (
                            p.surface_elevated,
                            crate::surface_edge::edge_color(
                                p.surface_elevated,
                                crate::surface_edge::DEFAULT_EDGE_DEPTH,
                            ),
                        )
                    }
                };
                let shadow_color = pinned.map_or(SHADOW_COLOR, |t| t.shadow_color);
                container::Style {
                    background: Some(Background::Color(bg)),
                    border: Border {
                        color: border_c,
                        width: border_width,
                        radius: Radius::from(radius),
                    },
                    shadow: Shadow {
                        color: shadow_color,
                        offset: Vector::new(0.0, 0.0),
                        blur_radius: shadow_blur,
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}
fn fixed_text_tokens(color: Color) -> TextTokens {
    TextTokens {
        default_color: color,
        muted_color: color,
        subtle_color: color,
        accent_color: color,
        success_color: color,
        warning_color: color,
        danger_color: color,
        disabled_color: color,
    }
}

impl<'a, Message: 'a> From<EnzChartTooltip> for Element<'a, Message> {
    fn from(t: EnzChartTooltip) -> Self {
        t.into_element()
    }
}

/// Own the supplied rows with no title, default geometry and live palette colours.
pub fn enz_chart_tooltip_rows(rows: impl IntoIterator<Item = ChartTooltipRow>) -> EnzChartTooltip {
    EnzChartTooltip {
        title: None,
        rows: rows.into_iter().collect(),
        tokens: None,
    }
}

/// Pair entries and values in iterator order with no title and default styling.
/// Entries without a value receive empty text; excess values are ignored.
/// [`enz_chart_tooltip_rows`] keeps each entry and value in one owned row.
pub fn enz_chart_tooltip(
    rows: impl IntoIterator<Item = ChartLegendEntry>,
    values: impl IntoIterator<Item = String>,
) -> EnzChartTooltip {
    let mut values = values.into_iter();
    let rows = rows
        .into_iter()
        .map(|entry| ChartTooltipRow {
            entry,
            value: values.next().unwrap_or_default(),
        })
        .collect();
    EnzChartTooltip {
        title: None,
        rows,
        tokens: None,
    }
}
