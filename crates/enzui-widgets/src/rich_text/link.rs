//! Inline-link actions and accessibility over shaped rich-text spans.
//!
//! Links expose keyboard stops and accessibility nodes; secondary actions use
//! the same paragraph geometry as primary hits.

use super::presentation::{LinkFocus, LinkPresentationTokens};
use crate::selection_scope::CompatibilityTextBoundary;
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, activation_from_key};
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::Renderer as _;
use iced::advanced::layout::Measure as _;
use iced::advanced::text::{Renderer as _, Span as CoreSpan};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer, text};
use iced::{Element, Event, Font, Length, Point, Rectangle, Size, Vector, keyboard};
use std::rc::Rc;

type ContextHandler<'a, Link, Message> = Rc<dyn Fn(Link, Point) -> Message + 'a>;

/// Typed primary and secondary actions for links in a rendered paragraph.
///
/// Each handler is optional. Secondary actions receive the link payload and
/// the press point in logical window coordinates, suitable for a context menu.
pub struct LinkHandlers<'a, Link, Message> {
    pub(crate) presentation: Option<LinkPresentationTokens>,
    pub(super) click: Option<Rc<dyn Fn(Link) -> Message + 'a>>,
    pub(super) context: Option<ContextHandler<'a, Link, Message>>,
}

impl<'a, Link, Message> LinkHandlers<'a, Link, Message> {
    /// Start with no activation handlers.
    pub fn new() -> Self {
        Self {
            presentation: None,
            click: None,
            context: None,
        }
    }

    /// Handle primary pointer activation and keyboard activation.
    #[must_use]
    pub fn on_click(mut self, handler: impl Fn(Link) -> Message + 'a) -> Self {
        self.click = Some(Rc::new(handler));
        self
    }

    /// Handle secondary pointer presses without also activating the link.
    #[must_use]
    pub fn on_context(mut self, handler: impl Fn(Link, Point) -> Message + 'a) -> Self {
        self.context = Some(Rc::new(handler));
        self
    }
}

impl<Link, Message> Default for LinkHandlers<'_, Link, Message> {
    fn default() -> Self {
        Self::new()
    }
}

/// The paragraph metrics that must match the wrapped `rich_text` element so
/// the wrapper's private paragraph reproduces the child's per-span geometry.
#[derive(Clone, Copy)]
pub(crate) struct RichMetrics {
    /// Base text size in px.
    pub size: f32,
    /// Optional base font (weight/family) applied to the whole paragraph.
    pub font: Option<Font>,
    /// Optional base line height.
    pub line_height: Option<iced::widget::text::LineHeight>,
    pub align_x: text::Alignment,
}

/// The concrete paragraph type of the app renderer.
type AppParagraph = <iced::Renderer as text::Renderer>::Paragraph;

/// Per-render state: the wrapper's private paragraph, rebuilt whenever the
/// spans or bounds change (mirrors `rich.rs::State`), plus one keyboard-focus
/// cell per link span, preserving keyboard reachability.
#[derive(Default)]
struct LinkSemanticsState {
    paragraph: AppParagraph,
    /// The spans the paragraph was last shaped from (static-lifetime copies).
    spans: Vec<CoreSpan<'static, ()>>,
    /// One [`FocusState`] per link span, in document order. `operate` registers
    /// each one with `operation.focusable`, so the N links of a single wrapper
    /// are N distinct Tab stops (`link.rs` gives one widget one focus identity;
    /// here one widget carries N). Sized on first `operate`/`layout`.
    focus: Vec<FocusState>,
    /// Space is held down on a focused link; the message fires on release
    /// (platform button behaviour, mirroring `focus_button`'s `space_held`).
    space_held: bool,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide a focused link's ring.
    window_key: u64,
    translation: Vector,
}

/// One link span the wrapper makes keyboard-reachable and AT-visible.
struct LinkEntry<'a, Message> {
    /// The indices into the child's iced span list this link covers (drives
    /// `span_bounds`). A single link can span several runs: `[**bold**rest]`
    /// parses to a bold run and a plain run that share one destination, and
    /// they must not become two Tab stops. The rects of every
    /// listed span are unioned to give the link's bounds.
    span_indices: Vec<usize>,
    /// The link's own run text — its accessible name. For a multi-run link this
    /// is the concatenation of its runs, so the announced name is the whole
    /// anchor text rather than just its first fragment.
    label: String,
    /// A stable focus identity, so `focus(id)`/`find_focused` can target this
    /// specific link. Derived per span so document order and identity agree.
    id: Id,
    /// Emit the link's typed message. This is the same call the pointer
    /// path makes: both are `on_link(payload.clone())` over the shared
    /// callback, so keyboard activation cannot drift from click activation.
    /// `None` for a document built with no link callback (a keyboard press is
    /// then a silent no-op, exactly as a pointer click is — see
    /// [`RichText::view_routed`](super::model::RichText::view_routed)).
    activate: Option<Box<dyn Fn() -> Message + 'a>>,
    context: Option<Box<dyn Fn(Point) -> Message + 'a>>,
}

/// A transparent wrapper that publishes one `Role::Link` AT node per link span
/// in the wrapped `rich_text` element and makes each link a keyboard Tab
/// stop with a visible focus ring. See the module-level `` note.
///
/// Concrete over the app's `iced::Renderer`/`iced::Font` (the only renderer
/// the crate targets): the wrapper shapes a private paragraph, which requires
/// naming the renderer's `Paragraph` type.
pub(crate) struct LinkSemantics<'a, Message> {
    child: Element<'a, Message>,
    /// Every link span, in span (document) order.
    links: Vec<LinkEntry<'a, Message>>,
    /// Geometry-only spans (payload erased to `()`), used to shape the private
    /// paragraph. Kept in step with the child's spans.
    geometry_spans: Vec<CoreSpan<'a, ()>>,
    metrics: RichMetrics,
    link_focus: Option<LinkFocus>,
}

impl<'a, Message> LinkSemantics<'a, Message> {
    /// Build the wrapper from the child element, the resolved iced spans (used
    /// to recover link indices/labels and paragraph geometry), the base
    /// metrics, and a per-link activation callback list aligned to the link
    /// spans in document order. Returns the child unchanged when there are
    /// no links, so link-free documents pay nothing.
    ///
    /// `activations` is consumed in link order: the *i*-th `Some(_)` is the
    /// activation for the *i*-th link span. It is built by the view methods
    /// from the same callback they hand the child `rich_text`, so keyboard and
    /// pointer activation are the identical call. Pass an all-`None` (or empty)
    /// vector for a link-free-callback document.
    pub(super) fn wrap<L: Clone + PartialEq + 'static>(
        child: Element<'a, Message>,
        spans: &[CoreSpan<'a, L>],
        metrics: RichMetrics,
        mut activations: Vec<Option<Box<dyn Fn() -> Message + 'a>>>,
        on_context: Option<ContextHandler<'a, L, Message>>,
        link_focus: Option<LinkFocus>,
    ) -> Element<'a, Message>
    where
        Message: 'a,
    {
        // group adjacent runs that share the same link
        // target into one entry.
        //
        // A styled fragment inside an anchor (`[**bold**rest](url)`) parses to
        // two runs — bold and plain — carrying the same destination. Treating
        // each run as its own link would produce two AT nodes and two Tab stops
        // for one link: a keyboard user would tab through it twice and a screen
        // reader would announce it twice.
        //
        // The discriminator is the link target, not merely "has a link":
        // two genuinely different adjacent links (`[a](u1)[b](u2)`) differ in
        // payload and must stay separate, which is the case
        // `two_adjacent_links_with_distinct_targets_stay_two_nodes` pins.
        // Adjacency is also required, so an `a … b … a` pattern does not fold
        // the two `a`s into one node across the intervening text.
        //
        // This is deliberately not done in `coalesced()`: that merges only runs
        // whose *style* is identical, so merging there would flatten the bold
        // run into plain text — a styling regression. Grouping here keeps the
        // rendered spans (and thus the bold) exactly as they were and changes
        // only the semantics layer.
        let mut link_groups: Vec<(Vec<usize>, String)> = Vec::new();
        let mut prev_link: Option<&L> = None;
        for (i, s) in spans.iter().enumerate() {
            let Some(payload) = s.link.as_ref() else {
                // A run with no link breaks adjacency.
                prev_link = None;
                continue;
            };
            let same_as_prev = prev_link.is_some_and(|p| p == payload);
            if same_as_prev && let Some(last) = link_groups.last_mut() {
                last.0.push(i);
                last.1.push_str(&s.text);
            } else {
                link_groups.push((vec![i], s.text.to_string()));
            }
            prev_link = s.link.as_ref();
        }

        if link_groups.is_empty() {
            return child;
        }

        // Pad/truncate the activation list to exactly one slot per link so a
        // caller that supplied none (or fewer) still gets a well-formed set;
        // extra activations are ignored.
        activations.resize_with(link_groups.len(), || None);

        let links: Vec<LinkEntry<'a, Message>> = link_groups
            .into_iter()
            .zip(activations)
            .enumerate()
            .map(|(link_ix, ((span_indices, label), activate))| {
                // A per-link id derived from the link's ordinal and label, so
                // two links with identical visible text still get distinct
                // focus identities (the defect one level up). Document
                // order is baked into the ordinal, so identity and Tab order
                // agree.
                let id = enzui_core::identity::derive_id(
                    "enz-inline-link",
                    &format!("{link_ix}:{label}"),
                );
                let context = on_context.as_ref().map(|handler| {
                    let handler = Rc::clone(handler);
                    let payload = spans[span_indices[0]]
                        .link
                        .clone()
                        .expect("link group payload");
                    Box::new(move |point| handler(payload.clone(), point))
                        as Box<dyn Fn(Point) -> Message + 'a>
                });
                LinkEntry {
                    span_indices,
                    label,
                    id,
                    activate,
                    context,
                }
            })
            .collect();

        // Erase the link payload to `()` for the geometry-only paragraph: span
        // bounds depend on text + style, never on the payload type.
        let geometry_spans: Vec<CoreSpan<'a, ()>> = spans
            .iter()
            .map(|s| {
                let mut g: CoreSpan<'a, ()> = CoreSpan::new(s.text.clone());
                g.size = s.size;
                g.line_height = s.line_height;
                g.font = s.font;
                g.color = s.color;
                g.highlight = s.highlight;
                g.padding = s.padding;
                g.underline = s.underline;
                g.strikethrough = s.strikethrough;
                g
            })
            .collect();

        Element::new(LinkSemantics {
            child,
            links,
            geometry_spans,
            metrics,
            link_focus,
        })
    }

    /// The union bounds of link `link_ix`'s span(s), translated to screen
    /// space. Falls back to the whole widget bounds if the paragraph has no
    /// geometry for that span yet.
    fn link_bounds(&self, state: &LinkSemanticsState, layout: Layout, link_ix: usize) -> Rectangle {
        let translation = layout.bounds().position() - Point::ORIGIN;
        // A link may cover several runs; union every one of
        // them so the focus ring encloses the whole anchor, not just its first
        // fragment.
        let regions: Vec<Rectangle> = self.links[link_ix]
            .span_indices
            .iter()
            .flat_map(|&ix| <AppParagraph as text::Paragraph>::span_bounds(&state.paragraph, ix))
            .collect();
        union_rect(&regions).map_or_else(
            || layout.bounds(),
            |r| r + Vector::new(translation.x, translation.y),
        )
    }

    /// Assemble the `core::Text` describing the private paragraph, matching the
    /// defaults `rich.rs::layout` uses (`Shaping::Advanced`, the renderer's
    /// default size/font when unset).
    fn text_config<'s>(
        &'s self,
        renderer: &iced::Renderer,
        bounds: Size,
    ) -> text::Text<&'s [CoreSpan<'a, ()>]> {
        let size = iced::Pixels(self.metrics.size);
        let font = self.metrics.font.unwrap_or_else(|| renderer.font());
        let line_height = self.metrics.line_height.unwrap_or_default();
        text::Text {
            content: self.geometry_spans.as_slice(),
            bounds,
            size,
            line_height,
            font,
            align_x: self.metrics.align_x,
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::advanced::text::Shaping::Advanced,
            wrapping: iced::advanced::text::Wrapping::WordOrGlyph,
            ellipsis: iced::advanced::text::Ellipsis::default(),
            hint_factor: renderer.hint_factor(),
        }
    }
}

/// Build one keyboard-activation closure per link span, aligned to the link
/// spans in document order, sharing `on_link` with the child `rich_text`.
///
/// Each closure captures the link's own payload and calls the same `on_link`
/// the pointer path uses, so keyboard activation and click activation are the
/// identical call over the identical callback — they cannot drift.
pub(super) fn link_activations<'a, L, Message>(
    spans: &[CoreSpan<'a, L>],
    on_link: &std::rc::Rc<impl Fn(L) -> Message + 'a + ?Sized>,
) -> Vec<Option<Box<dyn Fn() -> Message + 'a>>>
where
    L: Clone + PartialEq + 'a,
    Message: 'a,
{
    // emit one activation per link group, not per
    // link-bearing span, so this list stays index-aligned with the grouped
    // `LinkEntry` list `wrap` builds. The grouping rule must match `wrap`'s
    // exactly: adjacent runs sharing the same target are one link.
    let mut out: Vec<Option<Box<dyn Fn() -> Message + 'a>>> = Vec::new();
    let mut prev: Option<&L> = None;
    for s in spans {
        let Some(payload) = s.link.as_ref() else {
            prev = None;
            continue;
        };
        if prev.is_some_and(|p| p == payload) {
            // Same link continuing across another run — already has its slot.
            continue;
        }
        let cb = Rc::clone(on_link);
        let owned = payload.clone();
        out.push(Some(
            Box::new(move || cb(owned.clone())) as Box<dyn Fn() -> Message + 'a>
        ));
        prev = s.link.as_ref();
    }
    out
}

/// Union of a set of rects (the enclosing rect). Empty input yields `None`.
fn union_rect(rects: &[Rectangle]) -> Option<Rectangle> {
    let mut it = rects.iter();
    let first = *it.next()?;
    let (mut x0, mut y0) = (first.x, first.y);
    let (mut x1, mut y1) = (first.x + first.width, first.y + first.height);
    for r in it {
        x0 = x0.min(r.x);
        y0 = y0.min(r.y);
        x1 = x1.max(r.x + r.width);
        y1 = y1.max(r.y + r.height);
    }
    Some(Rectangle {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    })
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for LinkSemantics<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<LinkSemanticsState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(LinkSemanticsState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut [self.child.as_widget_mut()]);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // Lay the child out first (this is the geometry that is drawn and the
        // bounds AT nodes hang from).
        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);

        // (Re)shape our private paragraph over the *same* max bounds the child
        // received, so `span_bounds` reproduces the child's per-span geometry.
        let state = tree.state.downcast_mut::<LinkSemanticsState>();
        // One focus cell per link, in document order. Never shrink an existing
        // set on a rebuild that keeps the same link count (that would clear a
        // live focus); only grow/reset when the link count actually changes.
        if state.focus.len() != self.links.len() {
            state.focus = vec![FocusState::new(); self.links.len()];
        }
        let bounds = limits.max();
        let want: Vec<CoreSpan<'static, ()>> = self
            .geometry_spans
            .iter()
            .map(|s| s.clone().to_static())
            .collect();
        let config = self.text_config(renderer, bounds);
        let difference =
            <AppParagraph as text::Paragraph>::compare(&state.paragraph, config.with_content(()));
        if state.spans != want || difference == text::Difference::Shape {
            state.paragraph = <AppParagraph as text::Paragraph>::with_spans(config);
            state.spans = want;
        } else if difference == text::Difference::Bounds {
            <AppParagraph as text::Paragraph>::resize(&mut state.paragraph, bounds);
        }
        layout::Node::new(node.size()).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<LinkSemanticsState>();
        // Operations can run before the first layout.
        if state.focus.len() != self.links.len() {
            state.focus = vec![FocusState::new(); self.links.len()];
        }
        let translation = layout.bounds().position() - Point::ORIGIN;

        let mut boundary = CompatibilityTextBoundary::Begin;
        operation.custom(None, layout.bounds(), &mut boundary);

        for (link_ix, entry) in self.links.iter().enumerate() {
            let regions: Vec<Rectangle> = entry
                .span_indices
                .iter()
                .flat_map(|&ix| {
                    <AppParagraph as text::Paragraph>::span_bounds(&state.paragraph, ix)
                })
                .collect();
            let bounds = union_rect(&regions).map_or_else(
                || layout.bounds(),
                |r| r + Vector::new(translation.x, translation.y),
            );
            let mut node = SemanticsNode::new(
                Semantics::new(Role::Link)
                    .label(entry.label.clone())
                    .focused(state.focus[link_ix].is_focused())
                    .default_action(),
            );
            // Focus IDs are paragraph-local; anonymous AT nodes avoid cross-paragraph collisions.
            operation.custom(None, bounds, &mut node);
            operation.focusable(Some(&entry.id), bounds, &mut state.focus[link_ix]);
        }

        boundary = CompatibilityTextBoundary::End;
        operation.custom(None, layout.bounds(), &mut boundary);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
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
        // Keep the crate-wide input modality current so the focus ring is
        // focus-visible (keyboard-last) rather than painted after a click —
        // same rule `focus_button` follows. Keyed per window and
        // stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<LinkSemanticsState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        if self.links.is_empty() {
            return;
        }

        if !shell.is_event_captured()
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
            )
            && let Some(position) = cursor
                .position()
                .filter(|point| viewport.contains(*point) && layout.bounds().contains(*point))
        {
            let state = tree.state.downcast_ref::<LinkSemanticsState>();
            let local = position - (layout.position() - Point::ORIGIN);
            if let Some(span) = <AppParagraph as text::Paragraph>::hit_span(&state.paragraph, local)
                && let Some(handler) = self
                    .links
                    .iter()
                    .find(|entry| entry.span_indices.contains(&span))
                    .and_then(|entry| entry.context.as_ref())
            {
                shell.publish(handler(position + state.translation));
                shell.capture_event();
                return;
            }
        }

        // -style exclusive pointer focus: a press drops keyboard focus from
        // every link (the child `rich_text` already published the click). This
        // stops a stale keyboard ring lingering after the user clicks away.
        if enzui_core::focus::pointer_press_position(event, cursor).is_some() {
            let state = tree.state.downcast_mut::<LinkSemanticsState>();
            let mut changed = false;
            for f in &mut state.focus {
                if f.is_focused() {
                    f.unfocus();
                    changed = true;
                }
            }
            state.space_held = false;
            if changed {
                shell.request_redraw();
            }
        }

        // Keyboard activation of the focused link: Enter fires immediately,
        // Space arms on press and fires on release — the platform button
        // contract `focus_button` implements. Both publish through the link's
        // own activation closure, which is the *same* `on_link(payload)` call
        // the pointer path makes.
        //
        // `activation_from_key` drops Ctrl/Alt/Logo chords: a focused link
        // must not claim Ctrl+Enter or Alt+Space, which belong to the
        // application (see `enzui_core::focus::activation_from_key`).
        match event {
            Event::Keyboard(kb @ keyboard::Event::KeyPressed { key, .. })
                if activation_from_key(kb) =>
            {
                use keyboard::key::Named;
                let focused = {
                    let state = tree.state.downcast_ref::<LinkSemanticsState>();
                    state.focus.iter().position(FocusState::is_focused)
                };
                if let Some(ix) = focused {
                    match key {
                        keyboard::Key::Named(Named::Enter) => {
                            if let Some(activate) = &self.links[ix].activate {
                                shell.publish(activate());
                            }
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        keyboard::Key::Named(Named::Space) => {
                            let state = tree.state.downcast_mut::<LinkSemanticsState>();
                            state.space_held = true;
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        _ => {}
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) => {
                if matches!(key, keyboard::Key::Named(keyboard::key::Named::Space)) {
                    let (fire, ix) = {
                        let state = tree.state.downcast_mut::<LinkSemanticsState>();
                        let held = state.space_held;
                        state.space_held = false;
                        (held, state.focus.iter().position(FocusState::is_focused))
                    };
                    if fire && let Some(ix) = ix {
                        if let Some(activate) = &self.links[ix].activate {
                            shell.publish(activate());
                        }
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            _ => {}
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
        let state = tree.state.downcast_ref::<LinkSemanticsState>();
        if let Some(focus) = &self.link_focus {
            let mut focus = focus.borrow_mut();
            focus.fill(false);
            for (link, state) in self.links.iter().zip(&state.focus) {
                if state.is_focused() {
                    for index in &link.span_indices {
                        if let Some(focused) = focus.get_mut(*index) {
                            *focused = true;
                        }
                    }
                }
            }
        }
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        // paint a focus-visible ring around the focused link. Drawn
        // after the child so the child's text cannot paint over it. Gated on
        // `focus_ring_visible` (keyboard-last modality) so a pointer click does
        // not leave a lingering ring — the rule `focus_button` follows.
        let state = tree.state.downcast_ref::<LinkSemanticsState>();
        for (link_ix, f) in state.focus.iter().enumerate() {
            if !enzui_core::focus::focus_ring_visible_for(f.is_focused(), state.window_key) {
                continue;
            }
            let link_bounds = self.link_bounds(state, layout, link_ix);
            // A 1px padding so the ring sits just outside the underlined link
            // text rather than cropping its glyphs.
            let color = <iced::Theme as crate::focus_button::FocusRingTheme>::enz_focus_ring_color(
                theme, None,
            );
            let width =
                <iced::Theme as crate::focus_button::FocusRingTheme>::enz_focus_ring_width(theme);
            // shared ring painter. `snap: false` is deliberate and
            // must stay false here — a link ring tracks glyph metrics that are
            // not on the pixel grid, and snapping it walks the ring off the
            // underline it is supposed to enclose.
            crate::focus_button::paint_focus_ring(
                renderer,
                link_bounds,
                crate::focus_button::FocusRingQuad {
                    ink: color,
                    width,
                    radius: 2.0.into(),
                    outset: 1.0,
                    snap: false,
                },
            );
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
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        // Overlay traversal supplies the accumulated content-to-window scroll offset.
        tree.state.downcast_mut::<LinkSemanticsState>().translation = translation;
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
