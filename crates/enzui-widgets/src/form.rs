//! Forms with one measured label track shared across labelled rows.
//!
//! Finite hosts cap the label track by a fraction of available width and by the
//! room needed for the field track. Labels remain single-line and elide when
//! constrained. Narrow forms stack labels above controls unless wrapping is
//! forbidden; the breakpoint uses the form's allotted width, not the window.
//!
//! Relayout preserves child trees across stacking changes. Rows reconcile
//! positionally; this layout does not supply keyed reconciliation for row reorder.
//! Labelled rows publish accessibility groups named by their captions. Opaque
//! rows receive the full content-width offer without joining the shared track.
//!
//! [`crate::form_layout`] provides a spacing-only stack when labels need no shared track.

use crate::surface_stack::SpaceToken;
use enzui_core::collect::SemanticsNode;
use enzui_core::metrics::FiniteNonNeg;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{SpacingScale, TypographyScale};
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Default form inset token — `spacing-md` (12 px on the stock ladder). Matches
/// [`crate::form_layout::DEFAULT_FORM_INSET`].
pub const DEFAULT_INSET: SpaceToken = SpaceToken::Md;

/// Default vertical gap between rows — `spacing-sm` (8 px on the stock ladder).
pub const DEFAULT_ROW_GAP: SpaceToken = SpaceToken::Sm;

/// Default horizontal gap between the label track and the field track —
/// `spacing-sm` (8 px). Also the gap between fields in a
/// [`form_multi_field`] row.
pub const DEFAULT_LABEL_GAP: SpaceToken = SpaceToken::Sm;

/// Fallback label text size in logical pixels when the selected size is invalid.
/// Unpinned labels normally use forwarded or ambient typography instead.
pub const DEFAULT_LABEL_SIZE: f32 = 13.0;

/// Default allotted form width in logical pixels below which `FormWrap::Narrow` stacks rows.
pub const DEFAULT_BREAKPOINT: f32 = 320.0;

/// Default maximum fraction of the available content width reserved for labels.
/// A separate field-width cap can reduce the label track further.
pub const DEFAULT_LABEL_MAX_FRACTION: f32 = 0.5;

/// Target minimum inline field-track width in logical pixels.
/// Narrow and Always policies stack when there is insufficient room for this plus the label gap.
/// Never permits smaller fields when the host cannot fit them.
pub const MIN_FIELD_WIDTH: f32 = 64.0;

/// Policy for placing captions above controls or alongside them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FormWrap {
    /// Stacks below the allotted-width breakpoint or when the field floor plus gap cannot fit.
    /// This is the default policy.
    #[default]
    Narrow,
    /// Never stack: always keep the label|field row, whatever the width.
    Never,
    /// Always stack the label above its field, whatever the width.
    Always,
}

enum RowData<'a, Message> {
    Field {
        label: String,
        control: Element<'a, Message>,
    },
    Multi {
        label: String,
        controls: Vec<Element<'a, Message>>,
    },
    Full(Element<'a, Message>),
}

/// One row of an [`EnzForm`]. Build it with [`form_field`],
/// [`form_multi_field`] or [`form_row`].
pub struct FormRow<'a, Message>(RowData<'a, Message>);

/// A label paired with a single field. Establishes the shared label track.
pub fn form_field<'a, Message>(
    label: impl Into<String>,
    control: impl Into<Element<'a, Message>>,
) -> FormRow<'a, Message> {
    FormRow(RowData::Field {
        label: label.into(),
        control: control.into(),
    })
}

/// Pairs one label with controls in a horizontal field row.
/// Each control retains its width policy; equally weighted Fill controls share remaining space equally.
/// Compose a custom row through `form_field` when different proportions are needed.
pub fn form_multi_field<'a, Message>(
    label: impl Into<String>,
    controls: Vec<Element<'a, Message>>,
) -> FormRow<'a, Message> {
    FormRow(RowData::Multi {
        label: label.into(),
        controls,
    })
}

/// Offers opaque content the full width inside the form inset, without a label track or group.
/// The content retains its own width policy and may hug rather than fill that offer.
pub fn form_row<'a, Message>(content: impl Into<Element<'a, Message>>) -> FormRow<'a, Message> {
    FormRow(RowData::Full(content.into()))
}

/// Create a label-aligned form with a form-measured shared label track.
pub fn enz_form<'a, Message>(rows: Vec<FormRow<'a, Message>>) -> EnzForm<'a, Message>
where
    Message: 'a,
{
    EnzForm {
        rows,
        inset: DEFAULT_INSET,
        row_gap: DEFAULT_ROW_GAP,
        label_gap: DEFAULT_LABEL_GAP,
        wrap: FormWrap::Narrow,
        breakpoint: DEFAULT_BREAKPOINT,
        label_max_fraction: DEFAULT_LABEL_MAX_FRACTION,
        label_size: DEFAULT_LABEL_SIZE,
        label_size_explicit: false,
        typography: None,
        spacing_scale: None,
        width: Length::Fill,
        id_prefix: None,
    }
}

/// Builder returned by [`enz_form`].
#[must_use]
pub struct EnzForm<'a, Message> {
    rows: Vec<FormRow<'a, Message>>,
    inset: SpaceToken,
    row_gap: SpaceToken,
    label_gap: SpaceToken,
    wrap: FormWrap,
    breakpoint: f32,
    label_max_fraction: f32,
    label_size: f32,
    label_size_explicit: bool,
    typography: Option<TypographyScale>,
    spacing_scale: Option<SpacingScale>,
    width: Length,
    id_prefix: Option<String>,
}

impl<'a, Message> EnzForm<'a, Message>
where
    Message: 'a,
{
    /// Give each row's caption a widget id under `prefix`, so the caption
    /// labels its row and the first control in it (`labelled_by`), as GTK's
    /// label-for does. Use a prefix unique in the window: two forms with the
    /// same prefix and caption would publish one id twice.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// Set the form inset with a semantic [`SpaceToken`] (default
    /// [`SpaceToken::Md`]).
    pub fn inset(mut self, token: SpaceToken) -> Self {
        self.inset = token;
        self
    }

    /// Set the vertical gap between rows with a semantic [`SpaceToken`]
    /// (default [`SpaceToken::Sm`]).
    pub fn row_gap(mut self, token: SpaceToken) -> Self {
        self.row_gap = token;
        self
    }

    /// Set the horizontal gap between the label track and the field track (and
    /// between multi-fields) with a semantic [`SpaceToken`] (default
    /// [`SpaceToken::Sm`]).
    pub fn label_gap(mut self, token: SpaceToken) -> Self {
        self.label_gap = token;
        self
    }

    /// Set the narrow-stacking policy (default [`FormWrap::Narrow`]).
    pub fn wrap(mut self, wrap: FormWrap) -> Self {
        self.wrap = wrap;
        self
    }

    /// Set the width breakpoint (px) used by [`FormWrap::Narrow`]. A
    /// non-finite / negative value is ignored.
    pub fn breakpoint(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.breakpoint = px;
        }
        self
    }

    /// Cap the shared label track at `fraction` of the field-area width
    /// (default `0.5`). Past the cap a long label elides rather than starving
    /// the fields. A value `<= 0` or non-finite is ignored; values above `1.0`
    /// are clamped to `1.0`.
    pub fn label_max_fraction(mut self, fraction: f32) -> Self {
        if fraction.is_finite() && fraction > 0.0 {
            self.label_max_fraction = fraction.min(1.0);
        }
        self
    }

    /// Pins label size in logical pixels, overriding forwarded and ambient typography.
    /// Non-finite or non-positive pins resolve to `DEFAULT_LABEL_SIZE`.
    pub fn label_size(mut self, px: f32) -> Self {
        self.label_size = px;
        self.label_size_explicit = true;
        self
    }

    /// Supplies body_size for unpinned labels; otherwise ambient typography provides it.
    /// An explicit label_size takes precedence, including its invalid-value fallback.
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }

    /// Forward the active runtime [`SpacingScale`] so the inset / row gap /
    /// label gap tokens resolve against a rescaled theme's ladder. Same pattern
    /// as [`EnzFormLayout::spacing_scale`](crate::form_layout::EnzFormLayout::spacing_scale).
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Override the outer width (default [`Length::Fill`]).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Finish the builder and hand back the [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

fn resolve_label_size(explicit: bool, pinned: f32, typography: Option<&TypographyScale>) -> f32 {
    let px = if explicit {
        pinned
    } else {
        typography.map_or_else(|| TypographyScale::ambient().body_size, |t| t.body_size)
    };
    if px.is_finite() && px > 0.0 {
        px
    } else {
        DEFAULT_LABEL_SIZE
    }
}

fn label_element<'a, Message: 'a>(label: String, size: f32) -> Element<'a, Message> {
    crate::enz_text::enz_text(label)
        .size_px(size)
        .weight_rung(enzui_theme::WeightRung::Medium)
        .tone(crate::enz_text::TextTone::Muted)
        .single_line_elided()
        .into()
}

impl<'a, Message: 'a> From<EnzForm<'a, Message>> for Element<'a, Message> {
    fn from(form: EnzForm<'a, Message>) -> Self {
        // Layout has no runtime theme; spacing resolves against the forwarded or stock scale at conversion.
        let default = SpacingScale::default();
        let scale = form.spacing_scale.as_ref().unwrap_or(&default);
        let inset = sane(form.inset.resolve(scale));
        let row_gap = sane(form.row_gap.resolve(scale));
        let label_gap = sane(form.label_gap.resolve(scale));
        let label_size = resolve_label_size(
            form.label_size_explicit,
            form.label_size,
            form.typography.as_ref(),
        );

        let mut children: Vec<Element<'a, Message>> = Vec::new();
        let mut specs: Vec<RowSpec> = Vec::with_capacity(form.rows.len());
        let mut caption_ids = form
            .id_prefix
            .map(|prefix| enzui_core::identity::KeyedIds::new(format!("{prefix}-caption")));
        let mut caption = |label: &str| {
            let id = caption_ids.as_mut().map(|ids| ids.id_for(label));
            let element = label_element(label.to_owned(), label_size);
            let element = match &id {
                // The caption is a label node with an id the row and its
                // control point at.
                Some(id) => crate::semantics_container::SemanticsContainer::leaf(
                    element,
                    Semantics::new(Role::Label).label(label.to_owned()),
                )
                .id(id.clone())
                .into(),
                None => element,
            };
            (element, id)
        };
        for FormRow(data) in form.rows {
            match data {
                RowData::Field { label, control } => {
                    let (element, caption_id) = caption(&label);
                    children.push(element);
                    children.push(control);
                    specs.push(RowSpec {
                        label: Some(label),
                        caption_id,
                    });
                }
                RowData::Multi { label, controls } => {
                    let (element, caption_id) = caption(&label);
                    children.push(element);
                    let composed: Element<'a, Message> = iced::widget::Row::with_children(controls)
                        .spacing(label_gap)
                        .width(Length::Fill)
                        .into();
                    children.push(composed);
                    specs.push(RowSpec {
                        label: Some(label),
                        caption_id,
                    });
                }
                RowData::Full(content) => {
                    children.push(content);
                    specs.push(RowSpec {
                        label: None,
                        caption_id: None,
                    });
                }
            }
        }

        Element::new(FormWidget {
            children,
            specs,
            inset,
            row_gap,
            label_gap,
            wrap: form.wrap,
            breakpoint: if form.breakpoint.is_finite() && form.breakpoint > 0.0 {
                form.breakpoint
            } else {
                0.0
            },
            label_max_fraction: form.label_max_fraction.clamp(0.0, 1.0),
            width: form.width,
        })
    }
}

#[inline]
fn sane(px: f32) -> f32 {
    FiniteNonNeg::new_or(px, 0.0).get()
}

fn place_cell<Message>(
    child: &mut Element<'_, Message>,
    tree: &mut Tree,
    renderer: &iced::Renderer,
    avail: f32,
    content_h: f32,
    hug: bool,
) -> layout::Node {
    let bounds = Size::new(avail, content_h);
    let limits = if hug {
        layout::Limits::new(Size::ZERO, bounds).width(Length::Shrink)
    } else {
        layout::Limits::new(Size::ZERO, bounds)
    };
    child.as_widget_mut().measure(tree, renderer, &limits)
}

struct RowSpec {
    label: Option<String>,
    /// The caption's widget id, when the form has an id prefix.
    caption_id: Option<iced::advanced::widget::Id>,
}

impl RowSpec {
    fn child_count(&self) -> usize {
        if self.label.is_some() { 2 } else { 1 }
    }
}

struct FormWidget<'a, Message> {
    children: Vec<Element<'a, Message>>,
    specs: Vec<RowSpec>,
    inset: f32,
    row_gap: f32,
    label_gap: f32,
    wrap: FormWrap,
    breakpoint: f32,
    label_max_fraction: f32,
    width: Length,
}

impl<'a, Message> FormWidget<'a, Message> {
    fn stacked_at(&self, outer_w: f32) -> bool {
        match self.wrap {
            FormWrap::Never => false,
            FormWrap::Always => true,
            FormWrap::Narrow => outer_w.is_finite() && outer_w > 0.0 && outer_w < self.breakpoint,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FormWidget<'_, Message> {
    // Stacking changes layout without replacing child trees or their input state.
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width);
        let max = limits.max();
        let outer_w = max.width;
        let content_h = max.height;
        let bounded_w = outer_w.is_finite();

        // Hugging remains constrained by a finite host; only unbounded offers release label and field caps.
        let hug = matches!(self.width, Length::Shrink | Length::Fit) || !bounded_w;
        let avail_w = if bounded_w {
            (outer_w - 2.0 * self.inset).max(0.0)
        } else {
            f32::INFINITY
        };
        // Fill under an unbounded offer must resolve from intrinsic content rather than infinity.
        let width_policy = if hug { Length::Shrink } else { self.width };

        if self.children.is_empty() {
            let inner = Size::new(2.0 * self.inset, 2.0 * self.inset);
            let size = limits.resolve(width_policy, Length::Shrink, inner);
            return layout::Node::with_children(size, vec![]).apply(tree);
        }

        let policy_stacked = self.stacked_at(outer_w);
        let width_forced_stack =
            self.wrap != FormWrap::Never && bounded_w && avail_w < MIN_FIELD_WIDTH + self.label_gap;
        let stacked = policy_stacked || width_forced_stack;

        let track = if stacked {
            0.0
        } else {
            let mut widest = 0.0f32;
            let mut ci = 0usize;
            for spec in &self.specs {
                if spec.label.is_some() {
                    let w = crate::grid::measure_intrinsic_size(
                        &mut self.children[ci],
                        &mut tree.children[ci],
                        renderer,
                        Size::new(avail_w, content_h),
                    )
                    .width;
                    if w.is_finite() && w > widest {
                        widest = w;
                    }
                }
                ci += spec.child_count();
            }
            if bounded_w {
                // Apply both caps to bounded Shrink forms as well as Fill forms.
                let cap_fraction = avail_w * self.label_max_fraction;
                let cap_min_field = (avail_w - self.label_gap - MIN_FIELD_WIDTH).max(0.0);
                widest.min(cap_fraction).min(cap_min_field).max(0.0)
            } else {
                widest.max(0.0)
            }
        };

        let mut nodes: Vec<layout::Node> = Vec::with_capacity(self.children.len());
        let mut ci = 0usize;
        let mut y = self.inset;
        let mut widest_inline_field = 0.0f32;
        let mut widest_stacked_cell = 0.0f32;
        let mut widest_full = 0.0f32;
        let n_rows = self.specs.len();
        for (row_i, spec) in self.specs.iter().enumerate() {
            let row_h = if spec.label.is_some() {
                let (label_i, field_i) = (ci, ci + 1);
                if stacked {
                    let label_node = place_cell(
                        &mut self.children[label_i],
                        &mut tree.children[label_i],
                        renderer,
                        avail_w,
                        content_h,
                        hug,
                    );
                    let label_h = label_node.size().height;
                    let field_node = place_cell(
                        &mut self.children[field_i],
                        &mut tree.children[field_i],
                        renderer,
                        avail_w,
                        content_h,
                        hug,
                    );
                    let field_h = field_node.size().height;
                    widest_stacked_cell = widest_stacked_cell
                        .max(label_node.size().width)
                        .max(field_node.size().width);
                    nodes.push(label_node.move_to(Point::new(self.inset, y)));
                    nodes.push(
                        field_node.move_to(Point::new(self.inset, y + label_h + self.label_gap)),
                    );
                    label_h + self.label_gap + field_h
                } else {
                    let label_node = self.children[label_i].as_widget_mut().measure(
                        &mut tree.children[label_i],
                        renderer,
                        &layout::Limits::new(Size::ZERO, Size::new(track, content_h)),
                    );
                    let field_avail = if bounded_w {
                        (avail_w - track - self.label_gap).max(0.0)
                    } else {
                        f32::INFINITY
                    };
                    let field_node = place_cell(
                        &mut self.children[field_i],
                        &mut tree.children[field_i],
                        renderer,
                        field_avail,
                        content_h,
                        hug,
                    );
                    let label_h = label_node.size().height;
                    let field_h = field_node.size().height;
                    widest_inline_field = widest_inline_field.max(field_node.size().width);
                    let row_h = label_h.max(field_h);
                    nodes.push(
                        label_node.move_to(Point::new(self.inset, y + (row_h - label_h) / 2.0)),
                    );
                    nodes.push(field_node.move_to(Point::new(
                        self.inset + track + self.label_gap,
                        y + (row_h - field_h) / 2.0,
                    )));
                    row_h
                }
            } else {
                let node = place_cell(
                    &mut self.children[ci],
                    &mut tree.children[ci],
                    renderer,
                    avail_w,
                    content_h,
                    hug,
                );
                let h = node.size().height;
                widest_full = widest_full.max(node.size().width);
                nodes.push(node.move_to(Point::new(self.inset, y)));
                h
            };

            y += row_h;
            if row_i + 1 < n_rows {
                y += self.row_gap;
            }
            ci += spec.child_count();
        }

        let total_h = y + self.inset;
        let content_inner = if stacked {
            widest_stacked_cell.max(widest_full)
        } else {
            (track + self.label_gap + widest_inline_field).max(widest_full)
        };
        let content_w = content_inner + 2.0 * self.inset;
        let size = limits.resolve(width_policy, Length::Shrink, Size::new(content_w, total_h));
        layout::Node::with_children(size, nodes).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layouts: Vec<Layout> = layout.children(&tree.children).collect();
        let mut ci = 0usize;
        for spec in &self.specs {
            let count = spec.child_count();
            if let Some(name) = &spec.label {
                let bounds = union_bounds(&child_layouts[ci..ci + count]);
                let semantics = Semantics::new(Role::Group).label(name.clone());
                let semantics = match &spec.caption_id {
                    Some(caption) => semantics.labelled_by(caption.clone()),
                    None => semantics,
                };
                let mut node = SemanticsNode::group(semantics);
                operation.custom(None, bounds, &mut node);
                let start = ci;
                operation.container(None, bounds, viewport);
                operation.traverse(&mut |operation| {
                    for ((child, child_tree), child_layout) in self.children[start..start + count]
                        .iter_mut()
                        .zip(&mut tree.children[start..start + count])
                        .zip(child_layouts[start..start + count].iter().copied())
                    {
                        child.as_widget_mut().operate(
                            child_tree,
                            child_layout,
                            viewport,
                            renderer,
                            operation,
                        );
                    }
                });
            } else {
                for ((child, child_tree), child_layout) in self.children[ci..ci + count]
                    .iter_mut()
                    .zip(&mut tree.children[ci..ci + count])
                    .zip(child_layouts[ci..ci + count].iter().copied())
                {
                    child.as_widget_mut().operate(
                        child_tree,
                        child_layout,
                        viewport,
                        renderer,
                        operation,
                    );
                }
            }
            ci += count;
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
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, lay, cursor, renderer, shell, viewport);
        }
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
        for (child, (lay, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, lay, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (lay, state))| {
                child
                    .as_widget()
                    .mouse_interaction(state, lay, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
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
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

fn union_bounds(layouts: &[Layout]) -> Rectangle {
    let mut it = layouts.iter().map(iced_core::Layout::bounds);
    let Some(first) = it.next() else {
        return Rectangle::new(Point::ORIGIN, Size::ZERO);
    };
    it.fold(first, |acc, b| {
        let x0 = acc.x.min(b.x);
        let y0 = acc.y.min(b.y);
        let x1 = (acc.x + acc.width).max(b.x + b.width);
        let y1 = (acc.y + acc.height).max(b.y + b.height);
        Rectangle::new(Point::new(x0, y0), Size::new(x1 - x0, y1 - y0))
    })
}

#[cfg(test)]
mod tests;
