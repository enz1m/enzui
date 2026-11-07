//! `HoverControl` — a glyph-box + label row that paints its own box quad so the
//! box chrome can react to hover over the whole control.
//!
//! ## What this fixes
//!
//! An iced `container`'s style closure only ever receives `&Theme` — never any
//! interaction status. A composed [`crate::focus_button::FocusButton`] computes
//! `Hovered`, but hands it only to its own `style_focus` closure for the outer
//! quad, so no status can reach a nested container. That is why both
//! `CheckboxTokens::hover_border` and `RadioTokens::hover_border`
//! were dead production configuration: the box/ring was a plain container and
//! could not respond to hover at all.
//!
//! `HoverControl` owns `draw`, so it can read `cursor.is_over(bounds)` over its
//! entire row (box + label + description) — the hit area covers the whole
//! control, not just the 14/16/20px box — and paint the box quad accordingly.
//!
//! Checkbox and radio share this machinery; the only real difference is
//! *what is painted*
//! inside the box, which the caller supplies as a
//! [`ControlPaint`]-returning closure.

use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Size};

/// What [`HoverControl`] paints for one render of the box.
///
/// The `dot` is an optional concentric filled circle (the radio's inner dot);
/// widgets that only need a single quad (the checkbox) leave it `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ControlPaint {
    /// Box background fill.
    pub background: Color,
    /// Box border colour.
    pub border_color: Color,
    /// Box border width in logical px.
    pub border_width: f32,
    /// Box corner radius in logical px (`box_size / 2.0` gives a circle).
    pub radius: f32,
    /// Optional concentric filled circle: `(diameter, colour)`.
    pub dot: Option<(f32, Color)>,
}

impl crate::motion::Lerp for ControlPaint {
    /// A dot appearing or leaving fades at its own size.
    fn lerp(&self, to: &Self, t: f32) -> Self {
        let clear = |(size, color): (f32, Color)| (size, Color { a: 0.0, ..color });
        let dot = match (self.dot, to.dot) {
            (Some(from), Some(target)) => Some((from, target)),
            (Some(from), None) => Some((from, clear(from))),
            (None, Some(target)) => Some((clear(target), target)),
            (None, None) => None,
        }
        .filter(|_| t < 1.0 || to.dot.is_some())
        .map(|((from_size, from), (size, color))| (from_size.lerp(&size, t), from.lerp(&color, t)));
        Self {
            background: self.background.lerp(&to.background, t),
            border_color: self.border_color.lerp(&to.border_color, t),
            border_width: self.border_width.lerp(&to.border_width, t),
            radius: self.radius.lerp(&to.radius, t),
            dot,
        }
    }
}

/// Runtime-theme + hover-aware paint resolver for [`HoverControl`].
///
/// Called once per `draw` with the live theme and whether the cursor is over
/// the whole control.
type PaintFn<'a> = Box<dyn Fn(&iced::Theme, bool) -> ControlPaint + 'a>;

/// A two-child widget — the box glyph (child 0, drawn over a fixed square) and
/// the label/description column (child 1) — that paints the box quad itself so
/// the box chrome can react to hover over the whole control.
///
/// See the module docs for why a plain `container` cannot do this.
pub(crate) struct HoverControl<'a, Message> {
    /// Drawn over the box square (a check glyph, or a zero-size `Space` when
    /// the box paints everything itself, as the radio's dot does).
    pub glyph: Element<'a, Message>,
    /// Label (+ optional description) column, laid out beside the box.
    pub text_col: Element<'a, Message>,
    /// Side length of the box square.
    pub box_size: f32,
    /// Resolves the painted box chrome for `(theme, hovered)`.
    pub paint: PaintFn<'a>,
    /// Whether the control is checked or selected: turning on pops the
    /// tick or dot in from smaller, as GNOME, Apple and Material controls do,
    /// and turning off shrinks it away. The glyph is drawn only while on or
    /// shrinking, so a checkbox builds its tick in both states.
    pub on: bool,
}

/// How long a tick or dot takes to pop in, in milliseconds.
const POP_MS: f32 = 160.0;
/// The scale a popping tick or dot starts from.
const POP_FROM: f32 = 0.55;

/// The pop's progress, the box paint fading between states, and the latest
/// presented frame.
#[derive(Debug, Default)]
struct PopState {
    pop: crate::motion::Swell,
    frame_at: Option<iced::time::Instant>,
    fade: crate::motion::StateFade<ControlPaint>,
    /// Whether the pointer was over the control at the last frame.
    hovered: Option<bool>,
}

impl<Message> HoverControl<'_, Message> {
    /// Gap between the box and the text column (`label-gap` = spacing-sm).
    const GAP: f32 = 8.0;
    /// Vertical padding around the row.
    const V_PAD: f32 = 2.0;
    /// Fallback box side length for a non-finite / non-positive `box_size`.
    ///
    /// A control box must be paintable and non-degenerate, so — unlike a
    /// border width or radius — `0.0` is not honoured literally here (a
    /// zero-size box paints nothing and carries no meaning; that is a defect,
    /// not caller intent. The fallback is to the medium checkbox /
    /// radio ring side length (`RadioGroupSize::Md.ring_size()` = 16px), the
    /// default control size, so a poisoned value degrades to the ordinary
    /// box rather than an invisible or panicking one.
    const BOX_SIZE_FALLBACK: f32 = 16.0;

    /// Finite, strictly-positive box side length. NaN / `±inf` / `<= 0` all
    /// degrade to [`Self::BOX_SIZE_FALLBACK`]; valid values pass through.
    fn safe_box_size(&self) -> f32 {
        FinitePositive::new_or(self.box_size, Self::BOX_SIZE_FALLBACK).get()
    }
}

impl ControlPaint {
    /// Fallback border width for a non-finite / negative `border_width`. A
    /// poisoned width is a programmer error, not "no border"; degrade to the
    /// 1px checkbox box border so the control still renders a visible edge.
    /// (A *literal* `0.0` is preserved — see [`Self::box_border`].)
    const BORDER_WIDTH_FALLBACK: f32 = 1.0;
    /// Fallback corner radius for a non-finite / negative `radius`. Degrades to
    /// the 2px default; a *literal* `0.0`
    /// is preserved as a square control.
    const RADIUS_FALLBACK: f32 = 2.0;

    /// The box [`Border`] with `border_width` and `radius` sanitized at the
    /// point they reach the renderer quad.
    ///
    /// Both are [`FiniteNonNeg`]: NaN / `±inf` / negatives degrade, but zero
    /// is preserved — a zero `border_width` means "no border" and a zero
    /// `radius` means "square control", both legitimate caller intent
    /// The fallbacks are only ever substituted for values
    /// that are not expressible at all: a NaN / `±inf` / negative input is a
    /// programmer error, not "draw no border", so it degrades to a visible
    /// hairline (`1.0`, the checkbox box border) rather than silently
    /// vanishing: invalid input falls back to the `from_palette` default rather
    /// than zero. Only a *literal* zero yields no border.
    fn box_border(self) -> Border {
        Border {
            radius: FiniteNonNeg::new_or(self.radius, Self::RADIUS_FALLBACK)
                .get()
                .into(),
            width: FiniteNonNeg::new_or(self.border_width, Self::BORDER_WIDTH_FALLBACK).get(),
            color: self.border_color,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for HoverControl<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut [&mut self.glyph, &mut self.text_col]);
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<PopState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(PopState {
            pop: crate::motion::Swell::snap(self.on),
            ..PopState::default()
        })
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Shrink,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // The box is a fixed square; the text column is laid out in the
        // remaining width, then both are vertically centred and offset by the
        // row's vertical padding.
        let box_size = self.safe_box_size();
        let text_limits = limits.shrink(Size::new(box_size + Self::GAP, 2.0 * Self::V_PAD));
        let text_node =
            self.text_col
                .as_widget_mut()
                .measure(&mut tree.children[1], renderer, &text_limits);
        let text_size = text_node.size();

        // The gap sits *between* the box and a label, so it only exists when
        // there is a label to separate. An empty label column lays out to zero
        // width (`enz_checkbox(v, "", ..)`, or a radio with an empty string);
        // reserving `GAP` beside it would pad every unlabelled control by 8px
        // and shift the label offset, which callers then double up on with
        // their own row spacing. Gate the gap on a non-empty label.
        let gap = if text_size.width > 0.0 {
            Self::GAP
        } else {
            0.0
        };

        let content_height = box_size.max(text_size.height);
        let total_height = content_height + 2.0 * Self::V_PAD;
        let total_width = box_size + gap + text_size.width;

        let box_node = layout::Node::new(Size::new(box_size, box_size)).move_to(iced::Point::new(
            0.0,
            Self::V_PAD + (content_height - box_size) / 2.0,
        ));
        let text_node = text_node.move_to(iced::Point::new(
            box_size + gap,
            Self::V_PAD + (content_height - text_size.height) / 2.0,
        ));

        layout::Node::with_children(
            Size::new(total_width, total_height),
            vec![box_node, text_node],
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut children = layout.children(&tree.children);
        let glyph_layout = children
            .next()
            .expect("the layout has glyph and text children");
        let text_layout = children
            .next()
            .expect("the layout has glyph and text children");
        self.glyph.as_widget_mut().operate(
            &mut tree.children[0],
            glyph_layout,
            viewport,
            renderer,
            operation,
        );
        self.text_col.as_widget_mut().operate(
            &mut tree.children[1],
            text_layout,
            viewport,
            renderer,
            operation,
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
        {
            let hovered = cursor.is_over(layout.bounds());
            let state = tree.state.downcast_mut::<PopState>();
            state
                .pop
                .track(&mut state.frame_at, event, self.on, POP_MS, shell);
            // The box fill, border and dot fade with the tick (checking,
            // hovering), over the theme's motion-fast duration.
            if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
                state.hovered = Some(hovered);
                let paint = &self.paint;
                if state.fade.frame(*now, None, false, |theme: &iced::Theme| {
                    paint(theme, hovered)
                }) {
                    shell.request_redraw();
                }
            } else if state.hovered.is_some_and(|was| was != hovered) {
                shell.request_redraw();
            }
        }
        let mut children = layout.children(&tree.children);
        let glyph_layout = children
            .next()
            .expect("the layout has glyph and text children");
        let text_layout = children
            .next()
            .expect("the layout has glyph and text children");
        self.glyph.as_widget_mut().update(
            &mut tree.children[0],
            event,
            glyph_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        self.text_col.as_widget_mut().update(
            &mut tree.children[1],
            event,
            text_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let mut children = layout.children(&tree.children);
        let glyph_layout = children
            .next()
            .expect("the layout has glyph and text children");
        let text_layout = children
            .next()
            .expect("the layout has glyph and text children");

        // Whole-control hover: `*-touch.has-hover`, where the
        // touch area covers the entire control, not just the box.
        let hovered = cursor.is_over(bounds);
        let pop_state = tree.state.downcast_ref::<PopState>();
        let paint = pop_state.fade.paint(theme, (self.paint)(theme, hovered));

        let box_bounds = glyph_layout.bounds();
        renderer.fill_quad(
            renderer::Quad {
                bounds: box_bounds,
                border: paint.box_border(),
                shadow: iced::Shadow::default(),
                snap: false,
            },
            Background::Color(paint.background),
        );

        let pop = pop_state.pop.paint(pop_state.frame_at, POP_MS, theme);
        // In from smaller; out all the way.
        let pop_scale = if self.on {
            POP_FROM + (1.0 - POP_FROM) * pop
        } else {
            pop
        };

        // Concentric dot (radio): centred inside the box, fully round.
        if let Some((dot_size, dot_color)) = paint.dot
            && dot_size > 0.0
            && dot_color.a > 0.0
        {
            let dot_size = dot_size * pop_scale;
            let dot_bounds = Rectangle {
                x: box_bounds.x + (box_bounds.width - dot_size) / 2.0,
                y: box_bounds.y + (box_bounds.height - dot_size) / 2.0,
                width: dot_size,
                height: dot_size,
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds: dot_bounds,
                    border: Border {
                        radius: (dot_size / 2.0).into(),
                        width: 0.0,
                        color: Color::TRANSPARENT,
                    },
                    shadow: iced::Shadow::default(),
                    snap: false,
                },
                Background::Color(dot_color),
            );
        }

        // The tick pops in about the box's centre.
        let centre = box_bounds.center();
        if pop > 0.0 {
            renderer.with_transformation(
                iced::Transformation::translate(centre.x, centre.y)
                    * iced::Transformation::scale(pop_scale)
                    * iced::Transformation::translate(-centre.x, -centre.y),
                |renderer| {
                    self.glyph.as_widget().draw(
                        &tree.children[0],
                        renderer,
                        theme,
                        style,
                        glyph_layout,
                        cursor,
                        viewport,
                    );
                },
            );
        }
        self.text_col.as_widget().draw(
            &tree.children[1],
            renderer,
            theme,
            style,
            text_layout,
            cursor,
            viewport,
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
        let mut children = layout.children(&tree.children);
        let glyph_layout = children
            .next()
            .expect("the layout has glyph and text children");
        let text_layout = children
            .next()
            .expect("the layout has glyph and text children");
        self.glyph
            .as_widget()
            .mouse_interaction(&tree.children[0], glyph_layout, cursor, viewport, renderer)
            .max(self.text_col.as_widget().mouse_interaction(
                &tree.children[1],
                text_layout,
                cursor,
                viewport,
                renderer,
            ))
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let mut children = layout.children(&tree.children);
        let glyph_layout = children
            .next()
            .expect("the layout has glyph and text children");
        let text_layout = children
            .next()
            .expect("the layout has glyph and text children");
        let mut trees = tree.children.iter_mut();
        let glyph_tree = trees.next().expect("the tree has glyph and text children");
        let text_tree = trees.next().expect("the tree has glyph and text children");
        let glyph_overlay = self.glyph.as_widget_mut().overlay(
            glyph_tree,
            glyph_layout,
            renderer,
            viewport,
            translation,
            window,
        );
        let text_overlay = self.text_col.as_widget_mut().overlay(
            text_tree,
            text_layout,
            renderer,
            viewport,
            translation,
            window,
        );
        if glyph_overlay.is_empty() {
            text_overlay
        } else {
            glyph_overlay
        }
    }
}

impl<'a, Message: 'a> From<HoverControl<'a, Message>> for Element<'a, Message> {
    fn from(w: HoverControl<'a, Message>) -> Self {
        Element::new(w)
    }
}
