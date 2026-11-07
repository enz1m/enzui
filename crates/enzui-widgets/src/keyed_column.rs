//! [`KeyedColumn`] — a vertical collection reconciled by key, not by slot.
//!
//! # Reconciliation
//!
//! This is the vertical twin of [`KeyedRow`](crate::keyed_row::KeyedRow). The
//! motivating consumer is [`EnzChatList`](crate::chat_list::EnzChatList): a
//! virtualized transcript whose realised child set is a fixed-length window
//! that slides as the model mutates. Front-inserting or coalescing a message
//! shifts every realised index by one, so the window `[msg_5 .. msg_35]`
//! becomes `[msg_6 .. msg_36]` — the same length, every key shifted by one.
//!
//! iced ships `keyed::Column` (`iced/widget/src/keyed/column.rs`), but it
//! reconciles through [`tree::diff_children_custom_with_search`], which splices
//! retained state only on a length change (iced's `Tree::diff_children`). An
//! equal-length permutation — exactly the sliding-window case above — falls
//! through to the positional `zip` in iced's `Tree::diff_children`, so row *k*'s retained
//! `Tree` (a streaming bubble's caret animation, hover, focus) is handed to
//! whatever message now occupies slot *k*. The user watching a message stream
//! sees the caret jump to a different bubble the instant a row is inserted
//! above the viewport. Keyed reconciliation keeps state with each message.
//!
//! [`KeyedColumn`] instead reconciles through
//! [`keyed_diff::reconcile_children_by_key`](crate::keyed_diff::reconcile_children_by_key)
//! — the same key-indexed algorithm [`KeyedRow`](crate::keyed_row::KeyedRow)
//! and [`KeyedGrid`](crate::grid::KeyedGrid) share — which looks each new key up
//! in last frame's keys and handles insert, remove, reorder and any combination
//! uniformly. Layout delegates to `layout::flex::resolve` on
//! [`Axis::Vertical`](layout::flex::Axis::Vertical), the exact call
//! `iced::widget::Column::layout` makes, so geometry is byte-for-byte a
//! `Column`'s and only reconciliation differs.
//!
//! # Hazards
//!
//! Focus and sibling insertion. If a conditionally-shown sibling can
//! appear above (or below) a focused input in a `column![]`, use this
//! widget instead of `column![]`.
//!
//! iced reconciles `Column` children positionally: inserting a child shifts
//! every subsequent index by one, and the retained `Tree` state — including
//! focus — stays at the old index. The focused input's state is then
//! handed to the new sibling that slid into its slot. Symptom: typing a
//! trigger character (e.g. `/` to open a suggestion list) inserts a sibling,
//! and the next keystroke is swallowed — the input still draws a caret
//! and looks alive, but it no longer owns its retained state.
//!
//! `KeyedColumn` reconciles by key, so inserting or removing a sibling does
//! not disturb any other child's retained state. The swap is a one-line
//! change.
//!
//! This is a very common app-level layout — any chat composer with a
//! [`crate::suggestion_list`], any settings pane with a conditional banner,
//! any form with a toggleable helper — and the failure mode is silent and
//! confusing. See also the note in [`crate::text_area`] and
//! [`crate::text_field`] module docs.
//!
//! The framework's own chrome is a caller here:
//! [`EnzAppShell`](crate::app_shell::EnzAppShell) stacks its titlebar,
//! optional notifications band and content region in this widget, because the
//! band is exactly such a conditional sibling and the subtree under it is the
//! whole application view.
//!
//! # Moves
//!
//! With [`KeyedColumn::animate_moves`], children slide to their new places
//! when the children change: a row inserted above pushes the rows below down
//! on a spring instead of in one frame, and a removed or reordered row's
//! neighbours close the gap the same way. Layout is exact on every frame;
//! only the paint travels, carried by the render lane. Give an inserted row
//! its own entrance with [`motion::animated`](crate::motion::animated)'s
//! `enters_from`.
//!
//! # Keys
//!
//! Keys are `String` and must be logical, never positional — deriving a key
//! from an index reintroduces the bug this widget removes. Duplicate keys are
//! tolerated but degrade to positional behaviour among the duplicates and trip a
//! `debug_assert!`; [`KeyedColumn::allow_duplicate_keys`] opts out when they are
//! deliberate. See [`crate::keyed_diff`] for the reuse-across-widget-types rule.

use iced::advanced::layout;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::Operation;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Layout, Shell, Widget};
use iced::{Alignment, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// A vertical collection whose children are reconciled by key.
///
/// Layout is identical to `iced::widget::Column` (both call
/// [`layout::flex::resolve`] on the vertical axis); only child-state
/// reconciliation differs. See the module docs.
#[must_use]
pub struct KeyedColumn<'a, Message> {
    keys: Vec<String>,
    children: Vec<Element<'a, Message>>,
    spacing: f32,
    padding: Padding,
    align_x: Alignment,
    width: Length,
    height: Length,
    allow_duplicate_keys: bool,
    clip: bool,
    moves: Option<enzui_theme::Spring>,
}

/// Last frame's keys, retained in the widget tree so `diff` can compare.
struct KeyedColumnState {
    keys: Vec<String>,
    moves: Option<crate::motion::Moves>,
}

/// Build a [`KeyedColumn`] from `(key, child)` pairs in document order.
///
/// Keys must be logical identifiers (a message key, a path segment), never a
/// position — see the module docs.
pub fn keyed_column<'a, Message>(
    keyed_children: Vec<(String, Element<'a, Message>)>,
) -> KeyedColumn<'a, Message>
where
    Message: 'a,
{
    let (keys, children): (Vec<String>, Vec<Element<'a, Message>>) =
        keyed_children.into_iter().unzip();
    KeyedColumn {
        keys,
        children,
        spacing: 0.0,
        padding: Padding::ZERO,
        align_x: Alignment::Start,
        // Match `Column::from_vec`: `Fit` is important here, rather than
        // `Shrink`. During `diff`, a fit axis inherits child constraints (for
        // example, a `Fill` text area makes its enclosing column fill too).
        // `Shrink` is unmergeable and would silently discard that constraint.
        width: Length::Fit,
        height: Length::Fit,
        allow_duplicate_keys: false,
        clip: false,
        moves: None,
    }
}

impl<'a, Message> KeyedColumn<'a, Message>
where
    Message: 'a,
{
    /// Vertical gap between adjacent children, in px. Non-finite and negative
    /// values are rejected to `0.0`.
    pub fn spacing(mut self, px: f32) -> Self {
        self.spacing = if px.is_finite() && px >= 0.0 { px } else { 0.0 };
        self
    }

    /// Padding around the whole column.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Cross-axis (horizontal) alignment of the children.
    pub fn align_x(mut self, align: impl Into<Alignment>) -> Self {
        self.align_x = align.into();
        self
    }

    /// Whether to clip the children to this column's bounds when drawing.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Column width. Defaults to [`Length::Fit`], matching
    /// `iced::widget::Column`.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Column height. Defaults to [`Length::Fit`], matching
    /// `iced::widget::Column`.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Slide children to their new places when the children change, on the
    /// theme's default spatial spring; see the module docs. The theme's
    /// motion speed applies, and under reduced motion they move at once.
    pub fn animate_moves(self) -> Self {
        self.animate_moves_on(enzui_theme::ExpressiveMotion::DefaultSpatial.spring())
    }

    /// [`animate_moves`](Self::animate_moves) on `spring`.
    pub fn animate_moves_on(mut self, spring: enzui_theme::Spring) -> Self {
        self.moves = Some(spring);
        self
    }

    /// Accept duplicate reconciliation keys without tripping the debug
    /// assertion. Changes no reconciliation behaviour; silences a
    /// development diagnostic only.
    pub fn allow_duplicate_keys(mut self) -> Self {
        self.allow_duplicate_keys = true;
        self
    }

    /// This column's reconciliation keys, in document order.
    ///
    /// Exposed so a composing widget's tests can assert the key scheme it
    /// generates directly. Asserting keys is not a substitute for asserting
    /// retained state across a real mutation — use both.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for KeyedColumn<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyedColumnState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyedColumnState {
            keys: self.keys.clone(),
            moves: self.moves.map(crate::motion::Moves::new),
        })
    }

    /// Key-based reconciliation — see the module docs and
    /// [`crate::keyed_row::KeyedRow::diff`] for why this does not use
    /// `diff_children_custom_with_search`.
    fn diff(&mut self, tree: &mut Tree) {
        debug_assert!(
            self.allow_duplicate_keys || crate::keyed_diff::duplicate_keys(&self.keys).is_empty(),
            "KeyedColumn: duplicate reconciliation key(s) {:?} in {:?} — duplicates \
             reconcile positionally among themselves, which is the bug keying \
             removes. Disambiguate with enzui_core::identity::KeyedIds, or \
             call .allow_duplicate_keys() if this is deliberate.",
            crate::keyed_diff::duplicate_keys(&self.keys),
            self.keys,
        );

        let state = tree.state.downcast_mut::<KeyedColumnState>();

        crate::keyed_diff::reconcile_children_by_key(
            &mut tree.children,
            &state.keys,
            &self.keys,
            &mut self.children,
            // `Tree::diff` rather than a bare `Widget::diff`, matching
            // `iced::widget::Column`: it refreshes the child's own frame-route
            // cache, so a subtree that only became animated in this rebuild is
            // routed the frame it is owed without waiting for an event. Its
            // tag check is redundant here (reconciliation already matched tags
            // or built a fresh tree) and costs one comparison.
            |child_tree, child| child_tree.diff(child.as_widget_mut()),
            |child| child.as_widget().tag(),
            |child| Tree::new(child.as_widget()),
        );

        match (self.moves, &mut state.moves) {
            (Some(spring), Some(moves)) => {
                moves.set_spring(spring);
                if state.keys != self.keys {
                    moves.change(&self.keys);
                }
            }
            (Some(spring), moves @ None) => *moves = Some(crate::motion::Moves::new(spring)),
            (None, moves) => *moves = None,
        }
        if state.keys != self.keys {
            state.keys.clone_from(&self.keys);
        }

        // Preserve `Column`'s sizing propagation exactly. A fit axis inherits
        // the constraints of its children during reconciliation; without this
        // pass, a `Fill` child is hidden behind the column's outer `Fit` and a
        // parent allocates only intrinsic height. This is load-bearing for
        // `EnzTextArea::fill_height()` inside `FieldColumn`.
        if self.width.is_fit() || self.height.is_fit() {
            for child in &self.children {
                let size = child.as_widget().size();

                self.width = self.width.cross(size.width);
                self.height = self.height.stack(size.height);
            }
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    /// Interested in presented-frame events only while a child is, the answer
    /// `iced::widget::Column` and [`KeyedRow`](crate::keyed_row::KeyedRow)
    /// give. The defaulted `true` would keep every keyed column on every frame
    /// route and fan each redraw across its whole subtree.
    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
            || tree
                .state
                .downcast_ref::<KeyedColumnState>()
                .moves
                .as_ref()
                .is_some_and(crate::motion::Moves::frame_interest)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // The very same call `iced::widget::Column::layout` makes, so geometry
        // is indistinguishable from a plain `Column`.
        layout::flex::resolve(
            layout::flex::Axis::Vertical,
            renderer,
            limits,
            self.width,
            self.height,
            self.padding,
            self.spacing,
            self.align_x,
            &mut self.children,
            &mut tree.children,
        )
        .with_drawing_cull(self.clip)
        .with_child_drawing_culls()
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
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (lay, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, lay, viewport, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.advance_moves(tree, event, layout, shell);
        let moves = tree.state.downcast_ref::<KeyedColumnState>().moves.as_ref();
        let offsets: Vec<Vector> = self
            .keys
            .iter()
            .map(|key| moves.map_or(Vector::ZERO, |moves| moves.offset(key)))
            .collect();
        for ((child, (lay, state)), offset) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .zip(offsets)
        {
            child.as_widget_mut().update(
                state,
                event,
                lay,
                shifted(cursor, offset),
                renderer,
                shell,
                viewport,
            );
            // An ordinary event is what starts an animation, so a child's
            // cached interest is refreshed here; refreshing only in
            // `update_frame` would leave the route missing and never deliver
            // that animation's first frame.
            state.refresh_frame_interest(child.as_widget());
        }
        tree.refresh_frame_children();
    }

    /// Deliver a presented-frame event only to the children the cached route
    /// names, pruning any child that settles during the dispatch. This is the
    /// walk `iced::widget::Column::update_frame` performs, shared with
    /// [`KeyedRow`](crate::keyed_row::KeyedRow) through
    /// [`crate::frame_routing::forward_frame_children`].
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.advance_moves(tree, event, layout, shell);
        crate::frame_routing::forward_frame_children(
            &mut self.children,
            tree,
            event,
            layout,
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
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let moves = tree.state.downcast_ref::<KeyedColumnState>().moves.as_ref();
        if let Some(moves) = moves {
            let origin = layout.bounds().position();
            moves.note(
                crate::theme_lookup::motion(theme),
                self.keys
                    .iter()
                    .zip(layout.children(&tree.children))
                    .map(|(key, child)| (key, child.bounds().position() - origin)),
            );
        }
        layout.draw_culled(tree, renderer, viewport, |renderer, viewport| {
            for ((child, key), (lay, state)) in self
                .children
                .iter()
                .zip(&self.keys)
                .zip(layout.iter(&tree.children))
            {
                let offset = moves.map_or(Vector::ZERO, |moves| moves.offset(key));
                if offset == Vector::ZERO {
                    if let Some(moves) = moves {
                        moves.drawn_in_place(key);
                    }
                    lay.draw_culled(state, renderer, viewport, |renderer, viewport| {
                        child
                            .as_widget()
                            .draw(state, renderer, theme, style, lay, cursor, viewport);
                    });
                    continue;
                }
                // A moving child is shown away from its layout, so it is
                // culled on its way, and its clips stay put.
                let Some(moves) = moves else { continue };
                moves.draw(key, renderer, lay.bounds(), viewport, |renderer, offset| {
                    child.as_widget().draw(
                        state,
                        renderer,
                        theme,
                        style,
                        lay,
                        shifted(cursor, offset),
                        viewport,
                    );
                });
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        let moves = tree.state.downcast_ref::<KeyedColumnState>().moves.as_ref();
        self.children
            .iter()
            .zip(&self.keys)
            .zip(layout.iter(&tree.children))
            .map(|((child, key), (lay, state))| {
                let offset = moves.map_or(Vector::ZERO, |moves| moves.offset(key));
                child.as_widget().mouse_interaction(
                    state,
                    lay,
                    shifted(cursor, offset),
                    viewport,
                    renderer,
                )
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

impl<Message> KeyedColumn<'_, Message> {
    /// On a presented frame, starts the moves a change of children caused
    /// and publishes the moving children's tracks.
    fn advance_moves(
        &self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        shell: &mut Shell<'_, Message>,
    ) {
        let Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let origin = layout.bounds().position();
        let places: Vec<Vector> = layout
            .children(&tree.children)
            .map(|child| child.bounds().position() - origin)
            .collect();
        if let Some(moves) = &mut tree.state.downcast_mut::<KeyedColumnState>().moves {
            moves.frame(*now, self.keys.iter().zip(places), shell);
        }
    }
}

/// `cursor` as a child shown `offset` from its layout sees it.
fn shifted(cursor: iced::advanced::mouse::Cursor, offset: Vector) -> iced::advanced::mouse::Cursor {
    match cursor {
        iced::advanced::mouse::Cursor::Available(position) => {
            iced::advanced::mouse::Cursor::Available(position - offset)
        }
        other => other,
    }
}

impl<'a, Message> From<KeyedColumn<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(column: KeyedColumn<'a, Message>) -> Self {
        Element::new(column)
    }
}
