//! Wrapping rows and columns with semantic gaps and padding.
//!
//! Defaults to Row direction, wrapping, Md gaps, no inset, Start alignment and
//! content-sized main extent. Cross gaps reuse the main gap unless overridden.
//! Spacing resolves against the stock scale or an explicitly forwarded scale.
//! `fill` expands the main axis: width for rows, height for columns.
//!
//! Reverse directions reverse logical children, including focus and accessibility
//! traversal. Disabling wrap keeps one potentially overflowing row or column.

use crate::cross_fluid_guard::{Axis, hug_cross_fluid};
use crate::surface_stack::SpaceToken;
use iced::widget::{column, row};
use iced::{Element, Length, Padding};

/// Main-axis direction; reverse variants also reverse logical traversal order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlowDirection {
    /// Lay children out left-to-right (the default).
    #[default]
    Row,
    /// Lay children out top-to-bottom.
    Column,
    /// Lay children out left-to-right with the child order reversed — the
    /// last pushed child is leftmost.
    RowReverse,
    /// Lay children out top-to-bottom with the child order reversed — the
    /// last pushed child is topmost.
    ColumnReverse,
}

impl FlowDirection {
    /// Whether the main axis is horizontal (`Row`/`RowReverse`).
    pub fn is_horizontal(self) -> bool {
        matches!(self, FlowDirection::Row | FlowDirection::RowReverse)
    }

    /// Whether children are laid out in reverse order.
    pub fn is_reversed(self) -> bool {
        matches!(
            self,
            FlowDirection::RowReverse | FlowDirection::ColumnReverse
        )
    }
}

/// Builder for a wrapping flow layout. See [`enz_flow`].
#[must_use]
pub struct EnzFlow<'a, Message> {
    children: Vec<Element<'a, Message>>,
    gap: SpaceToken,
    cross_gap: Option<SpaceToken>,
    inset: SpaceToken,
    direction: FlowDirection,
    wrap: bool,
    fill: bool,
    align_items: iced::Alignment,
    spacing_scale: Option<enzui_theme::SpacingScale>,
}

/// Construct a flow layout.
pub fn enz_flow<'a, Message: 'a>() -> EnzFlow<'a, Message> {
    EnzFlow {
        children: Vec::new(),
        gap: SpaceToken::Md,
        cross_gap: None,
        inset: SpaceToken::None,
        direction: FlowDirection::Row,
        wrap: true,
        fill: false,
        align_items: iced::Alignment::Start,
        spacing_scale: None,
    }
}

impl<'a, Message: 'a> EnzFlow<'a, Message> {
    /// Main-axis gap between items. Also the cross-axis (between-lines) gap
    /// unless [`Self::cross_gap`] overrides it. Defaults to
    /// [`SpaceToken::Md`].
    pub fn gap(mut self, g: SpaceToken) -> Self {
        self.gap = g;
        self
    }
    /// Set spacing between wrapped rows or columns independently of the item gap.
    /// Unset by default: wrapped lines reuse the main gap. Ignored when wrapping is disabled.
    pub fn cross_gap(mut self, g: SpaceToken) -> Self {
        self.cross_gap = Some(g);
        self
    }
    /// Set cross-axis alignment within each line; Start by default.
    /// Aligns vertically in rows and horizontally in columns without resizing children.
    pub fn align_items(mut self, a: iced::Alignment) -> Self {
        self.align_items = a;
        self
    }
    /// Uniform padding around the whole flow. Defaults to [`SpaceToken::None`].
    pub fn inset(mut self, i: SpaceToken) -> Self {
        self.inset = i;
        self
    }

    /// Resolve item gaps, cross gaps and inset against this scale during composition.
    /// Without an override, all three use the stock spacing scale.
    pub fn spacing_scale(mut self, spacing: enzui_theme::SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
    /// Main-axis direction. Defaults to [`FlowDirection::Row`].
    pub fn direction(mut self, d: FlowDirection) -> Self {
        self.direction = d;
        self
    }
    /// Whether items wrap onto new lines when they overflow. Defaults to
    /// `true`; `false` keeps a single (possibly overflowing) line.
    pub fn wrap(mut self, w: bool) -> Self {
        self.wrap = w;
        self
    }
    /// Fill the available main-axis extent instead of hugging content.
    /// Sets row width or column height to Fill; false by default.
    pub fn fill(mut self, f: bool) -> Self {
        self.fill = f;
        self
    }
    /// Append one child (accumulates). No children by default.
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let scale = self.spacing_scale;
        let px = |t: SpaceToken| scale.map_or_else(|| t.px(), |s| t.resolve(&s));

        let gap = px(self.gap);
        let cross_gap = self.cross_gap.map(px);
        let inset = px(self.inset);
        let pad = Padding::from(inset);
        let align = self.align_items;

        let mut children = self.children;
        if self.direction.is_reversed() {
            children.reverse();
        }
        // Non-wrapping flex lines need cross-fluid children to hug under cross compression.
        if !self.wrap {
            let axis = if self.direction.is_horizontal() {
                Axis::Horizontal
            } else {
                Axis::Vertical
            };
            children = children
                .into_iter()
                .map(|c| hug_cross_fluid(c, axis))
                .collect();
        }

        if self.direction.is_horizontal() {
            let r = row(children)
                .spacing(gap)
                .align_y(iced::alignment::Vertical::from(align));
            let r = if self.fill { r.width(Length::Fill) } else { r };
            if self.wrap {
                let mut w = r.wrap();
                if let Some(g) = cross_gap {
                    w = w.vertical_spacing(g);
                }
                iced::widget::container(w).padding(pad).into()
            } else {
                iced::widget::container(r).padding(pad).into()
            }
        } else {
            let c = column(children)
                .spacing(gap)
                .align_x(iced::alignment::Horizontal::from(align));
            let c = if self.fill { c.height(Length::Fill) } else { c };
            if self.wrap {
                let mut w = c.wrap();
                if let Some(g) = cross_gap {
                    w = w.horizontal_spacing(g);
                }
                iced::widget::container(w).padding(pad).into()
            } else {
                iced::widget::container(c).padding(pad).into()
            }
        }
    }
}

impl<'a, Message: 'a> From<EnzFlow<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzFlow<'a, Message>) -> Self {
        w.into_element()
    }
}
