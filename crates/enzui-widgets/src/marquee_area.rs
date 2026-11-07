//! Rubber-band drag selection over positioned content.
//!
//! Item rectangles and reported bands use widget-local logical pixels, with
//! the origin at the top left. Children handle events before the area starts
//! an unclaimed background gesture. The application owns committed selection.
//!
//! Coverage previews publish only when the covered set changes; band previews
//! publish when the band changes. A real drag commits once on release, including
//! empty coverage. Cancellation emits no final callback; earlier previews remain
//! advisory. Only non-additive background clicks publish the click message.
//! When configured, release publishes band, index, then keyed commit messages.
//!
//! Highlights and bands paint over content within the area bounds intersected
//! with the viewport. The area does not autoscroll or provide its own focus stop.

use enzui_core::drag::ItemId;
use enzui_core::marquee::{
    MarqueeBand, MarqueeIndex, MarqueeOutcome, MarqueePhase, MarqueeSession, SelectionMode,
};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard;
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Shadow, Size, Vector};

/// Default band border thickness in logical pixels.
pub const BAND_BORDER_WIDTH: f32 = 1.0;

/// Default band corner radius in logical pixels.
pub const BAND_RADIUS: f32 = 2.0;

/// Band fill alpha over the accent colour.
pub const BAND_FILL_ALPHA: f32 = 0.18;

/// Alpha of the per-item covered highlight; the same as the band fill.
pub const HIGHLIGHT_ALPHA: f32 = BAND_FILL_ALPHA;

/// Band and highlight tokens for [`EnzMarqueeArea`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MarqueeTokens {
    /// Band interior fill (`band-fill`).
    pub band_fill: Color,
    /// Band outline (`band-border`).
    pub band_border: Color,
    /// Band outline thickness in logical pixels.
    pub band_border_width: f32,
    /// Band corner radius in logical pixels.
    pub band_radius: f32,
    /// Per-item covered highlight fill.
    pub highlight: Color,
    /// Radial movement in logical pixels required to activate a drag.
    pub drag_threshold: f32,
}

impl MarqueeTokens {
    /// Derive tokens from a runtime palette.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            band_fill: crate::color::with_alpha(p.accent, BAND_FILL_ALPHA),
            band_border: p.accent,
            band_border_width: BAND_BORDER_WIDTH,
            band_radius: BAND_RADIUS,
            highlight: crate::color::with_alpha(p.accent, HIGHLIGHT_ALPHA),
            drag_threshold: enzui_core::desktop_settings::input().drag_threshold,
        }
    }

    /// Dark preset.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light preset.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replaces non-finite or negative geometry with default border width, radius, and drag threshold.
    ///
    /// Zero geometry and colors are preserved.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        Self {
            band_border_width: FiniteNonNeg::new_or(self.band_border_width, BAND_BORDER_WIDTH)
                .get(),
            band_radius: FiniteNonNeg::new_or(self.band_radius, BAND_RADIUS).get(),
            drag_threshold: FiniteNonNeg::new_or(
                self.drag_threshold,
                enzui_core::desktop_settings::input().drag_threshold,
            )
            .get(),
            ..*self
        }
    }
}

impl Default for MarqueeTokens {
    fn default() -> Self {
        Self::dark()
    }
}

/// Compatibility alias for [`MarqueeTokens`].
pub type MarqueeAreaTokens = MarqueeTokens;

#[inline]
fn resolve_tokens(pinned: Option<MarqueeTokens>, theme: &iced::Theme) -> MarqueeTokens {
    crate::theme_lookup::resolve_pinned(
        crate::theme_lookup::PIN_SITE_MARQUEE_AREA,
        pinned,
        theme,
        MarqueeTokens::from_palette,
    )
}

/// Wraps content in an enabled marquee area that fills its available width and height.
pub fn enz_marquee_area<'a, Message>(
    content: impl Into<Element<'a, Message>>,
) -> EnzMarqueeArea<'a, Message>
where
    Message: 'a,
{
    EnzMarqueeArea::new(content)
}

/// A rubber-band selection surface wrapped around positioned content.
pub struct EnzMarqueeArea<'a, Message> {
    content: Element<'a, Message>,
    items: Vec<[f32; 4]>,
    ids: Vec<ItemId>,
    on_preview: Option<Box<dyn Fn(Vec<usize>) -> Message + 'a>>,
    on_commit: Option<Box<dyn Fn(Vec<usize>) -> Message + 'a>>,
    on_band_preview: Option<Box<dyn Fn(MarqueeBand) -> Message + 'a>>,
    on_band_commit: Option<Box<dyn Fn(MarqueeBand) -> Message + 'a>>,
    on_preview_keyed: Option<Box<dyn Fn(Vec<ItemId>) -> Message + 'a>>,
    on_commit_keyed: Option<Box<dyn Fn(Vec<ItemId>) -> Message + 'a>>,
    on_background_click: Option<Message>,
    selected: Vec<usize>,
    selected_ids: Vec<ItemId>,
    generation: Option<u64>,
    width: Length,
    height: Length,
    enabled: bool,
    highlight_covered: bool,
    tokens: Option<MarqueeTokens>,
    id: Option<Id>,
}

impl<'a, Message> EnzMarqueeArea<'a, Message> {
    /// Wraps content with selection and highlights enabled, no items, and fill sizing.
    pub fn new(content: impl Into<Element<'a, Message>>) -> Self {
        Self {
            content: content.into(),
            items: Vec::new(),
            ids: Vec::new(),
            on_preview: None,
            on_commit: None,
            on_band_preview: None,
            on_band_commit: None,
            on_preview_keyed: None,
            on_commit_keyed: None,
            on_background_click: None,
            selected: Vec::new(),
            selected_ids: Vec::new(),
            generation: None,
            width: Length::Fill,
            height: Length::Fill,
            enabled: true,
            highlight_covered: true,
            tokens: None,
            id: None,
        }
    }

    /// Sets rectangles as `[x, y, width, height]` in widget-local logical pixels.
    /// Reported indices follow this order; callers must apply any content transforms.
    #[must_use]
    pub fn items(mut self, items: Vec<[f32; 4]>) -> Self {
        self.items = items;
        self
    }

    /// Sets the item-geometry cache stamp, including zero.
    ///
    /// Change the stamp whenever rectangle geometry, order, or count changes; reusing
    /// a stamp can serve stale geometry. ID-only changes need no bump. Without a stamp,
    /// each hit-test hashes all rectangles.
    #[must_use]
    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = Some(generation);
        self
    }

    /// Sets stable identities parallel to item rectangles.
    ///
    /// The first occurrence of an ID keeps its position; duplicates and missing IDs
    /// map to no identity, and surplus IDs are ignored. Index callbacks remain positional.
    ///
    /// # Panics
    ///
    /// In debug builds, an unclaimed background press panics on duplicate or surplus IDs.
    #[must_use]
    pub fn ids(mut self, ids: Vec<ItemId>) -> Self {
        self.ids = ids;
        self
    }

    /// Publishes resolved IDs when live coverage changes and updates keyed live highlights.
    ///
    /// Resolution applies the press-time selection mode to the current [`Self::selected_ids`].
    /// Without item IDs, covered IDs are empty but the callback can still fire.
    #[must_use]
    pub fn on_preview_keyed(mut self, f: impl Fn(Vec<ItemId>) -> Message + 'a) -> Self {
        self.on_preview_keyed = Some(Box::new(f));
        self
    }

    /// Publishes resolved IDs once on release of a real drag, including empty coverage.
    ///
    /// Uses the press-time mode and current [`Self::selected_ids`], even without item IDs.
    /// Clicks and cancellation emit no keyed commit.
    #[must_use]
    pub fn on_commit_keyed(mut self, f: impl Fn(Vec<ItemId>) -> Message + 'a) -> Self {
        self.on_commit_keyed = Some(Box::new(f));
        self
    }

    /// Sets the committed keyed selection used for idle highlights and Add/Toggle resolution.
    #[must_use]
    pub fn selected_ids(mut self, selected: Vec<ItemId>) -> Self {
        self.selected_ids = selected;
        self
    }

    fn id_map(&self) -> Vec<Option<ItemId>> {
        dedup_ids(&self.ids, self.items.len()).0
    }

    fn assert_ids_sane(&self) {
        #[cfg(debug_assertions)]
        {
            let dropped = dedup_ids(&self.ids, self.items.len()).1;
            debug_assert!(
                dropped == 0,
                "EnzMarqueeArea: {dropped} item id(s) dropped — ids must be unique \
                 and parallel to items"
            );
        }
    }

    fn resolve_selection(&self, mode: SelectionMode, covered: &[usize]) -> Vec<ItemId> {
        let map = self.id_map();
        let covered_ids: Vec<ItemId> = covered
            .iter()
            .filter_map(|&i| map.get(i).and_then(Clone::clone))
            .collect();
        mode.apply(&self.selected_ids, &covered_ids)
    }

    /// Publishes positional coverage only when it changes during an active drag.
    /// The band and positional highlights still draw without this callback.
    #[must_use]
    pub fn on_preview(mut self, f: impl Fn(Vec<usize>) -> Message + 'a) -> Self {
        self.on_preview = Some(Box::new(f));
        self
    }

    /// Publishes positional coverage once on release of a real drag, including an empty set.
    /// Clicks and cancellation emit no commit.
    #[must_use]
    pub fn on_commit(mut self, f: impl Fn(Vec<usize>) -> Message + 'a) -> Self {
        self.on_commit = Some(Box::new(f));
        self
    }

    /// Publishes the normalized widget-local band when it changes during an active drag.
    /// Coverage need not change; clicks emit no band, and cancellation emits no final band.
    #[must_use]
    pub fn on_band_preview(mut self, f: impl Fn(MarqueeBand) -> Message + 'a) -> Self {
        self.on_band_preview = Some(Box::new(f));
        self
    }

    /// Publishes the last moved widget-local band once on release of a real drag.
    /// Empty coverage still commits the band; clicks and cancellation do not.
    #[must_use]
    pub fn on_band_commit(mut self, f: impl Fn(MarqueeBand) -> Message + 'a) -> Self {
        self.on_band_commit = Some(Box::new(f));
        self
    }

    /// Registers the area’s own bounds for container operations under this identity.
    /// The area does not expose its own focusable state.
    #[must_use]
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Sets the message published on release of a background click in Replace mode.
    /// Shift-add and Control/Command-toggle clicks do not publish it.
    #[must_use]
    pub fn on_background_click(mut self, message: Message) -> Self {
        self.on_background_click = Some(message);
        self
    }

    /// Sets positional idle highlights; ignored when item IDs are supplied.
    #[must_use]
    pub fn selected(mut self, selected: Vec<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// Sets width, defaulting to [`Length::Fill`].
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Height. Defaults to [`Length::Fill`].
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Enables the area’s event handling, defaulting to true; child events always run.
    /// Disabling retains gesture state and its visuals.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Enables item highlights, defaulting to true; the drag band draws independently.
    /// Keyed live highlights require [`Self::on_preview_keyed`].
    #[must_use]
    pub fn highlight_covered(mut self, highlight: bool) -> Self {
        self.highlight_covered = highlight;
        self
    }

    /// Pins colors instead of tracking the runtime theme and sanitizes geometry.
    #[must_use]
    pub fn tokens(mut self, tokens: MarqueeTokens) -> Self {
        self.tokens = Some(tokens.sanitized());
        self
    }
}

#[derive(Debug, Default)]
struct MarqueeState {
    session: Option<MarqueeSession>,
    // Press events carry no modifiers, so retain the last modifier update.
    modifiers: keyboard::Modifiers,
    index: Option<MarqueeIndex>,
    fingerprint: u64,
    indexed_generation: Option<u64>,
    hits: Vec<usize>,
    covered: Vec<usize>,
    resolved: Vec<ItemId>,
    last_band: Option<MarqueeBand>,
}

fn fingerprint(items: &[[f32; 4]]) -> u64 {
    #[cfg(test)]
    test_probe::note_fingerprint(items.len());
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for r in items {
        for v in r {
            h ^= u64::from(v.to_bits());
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h ^= items.len() as u64;
    h
}

// Mask in place so duplicates cannot shift later identities onto different rectangles.
fn dedup_ids(ids: &[ItemId], item_count: usize) -> (Vec<Option<ItemId>>, usize) {
    let mut seen = std::collections::HashSet::with_capacity(ids.len());
    let mut out: Vec<Option<ItemId>> = Vec::with_capacity(item_count);
    let mut dropped = 0usize;
    for i in 0..item_count {
        match ids.get(i) {
            Some(id) if seen.insert(id.clone()) => out.push(Some(id.clone())),
            Some(_) => {
                out.push(None);
                dropped += 1;
            }
            None => out.push(None),
        }
    }
    dropped += ids.len().saturating_sub(item_count);
    (out, dropped)
}

impl MarqueeState {
    fn ensure_index(&mut self, items: &[[f32; 4]], generation: Option<u64>) {
        let stale = match (generation, self.indexed_generation) {
            (Some(g), Some(prev)) if g == prev && self.index.is_some() => false,
            (Some(_), _) => true,
            (None, _) => {
                let fp = fingerprint(items);
                let stale = self.index.is_none() || self.fingerprint != fp;
                if stale {
                    self.fingerprint = fp;
                }
                // Clear the stamp so returning to generation mode cannot reuse a stale index.
                self.indexed_generation = None;
                stale
            }
        };
        if stale {
            #[cfg(test)]
            test_probe::note_rebuild();
            self.index = Some(MarqueeIndex::build(items));
            self.indexed_generation = generation;
        }
    }

    fn hit_test(&mut self, band: MarqueeBand, items: &[[f32; 4]], generation: Option<u64>) {
        self.ensure_index(items, generation);
        let (index, hits) = (&mut self.index, &mut self.hits);
        index.as_mut().expect("just built").query_into(band, hits);
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzMarqueeArea<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<MarqueeState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(MarqueeState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn is_void(&self) -> bool {
        self.content.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::frame(limits, Size::new(self.width, self.height), |limits| {
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits)
        })
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
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            if let Some(child) = layout.children(&tree.children).next() {
                self.content.as_widget_mut().operate(
                    &mut tree.children[0],
                    child,
                    viewport,
                    renderer,
                    operation,
                );
            }
        });
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
        // Child capture prevents a content press from becoming a background gesture.
        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }

        if !self.enabled {
            return;
        }

        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<MarqueeState>();

        match event {
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if shell.is_event_captured() {
                    return;
                }
                let Some(pos) = cursor.position_in(bounds) else {
                    return;
                };
                self.assert_ids_sane();
                let mode = SelectionMode::from_modifiers(
                    state.modifiers.shift(),
                    state.modifiers.command() || state.modifiers.control(),
                );
                state.session = Some(MarqueeSession::begin(
                    (pos.x, pos.y),
                    mode,
                    self.tokens.map_or_else(
                        || enzui_core::desktop_settings::input().drag_threshold,
                        |t| t.drag_threshold,
                    ),
                ));
                state.covered.clear();
                state.resolved.clear();
                state.last_band = None;
                // Leave the press uncaptured until movement activates the gesture.
                shell.request_redraw();
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(session) = state.session.as_mut() else {
                    return;
                };
                // Raw coordinates keep an active band moving beyond the area bounds.
                let Some(raw) = cursor.position() else {
                    return;
                };
                let band = session.pointer_moved((raw.x - bounds.x, raw.y - bounds.y));
                let Some(band) = band else {
                    return;
                };

                if state.last_band != Some(band) {
                    state.last_band = Some(band);
                    if let Some(f) = &self.on_band_preview {
                        shell.publish(f(band));
                    }
                }

                let mode = session.mode();
                state.hit_test(band, &self.items, self.generation);
                if state.hits != state.covered {
                    state.covered.clear();
                    state.covered.extend_from_slice(&state.hits);
                    if let Some(f) = &self.on_preview {
                        shell.publish(f(state.covered.clone()));
                    }
                    if self.on_preview_keyed.is_some() {
                        let resolved = self.resolve_selection(mode, &state.covered);
                        state.resolved.clone_from(&resolved);
                        if let Some(f) = &self.on_preview_keyed {
                            shell.publish(f(resolved));
                        }
                    }
                }
                shell.request_redraw();
                shell.capture_event();
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some(mut session) = state.session.take() else {
                    return;
                };
                let was_active = session.is_active();
                match session.release() {
                    MarqueeOutcome::Committed { band, mode } => {
                        state.hit_test(band, &self.items, self.generation);
                        if let Some(f) = &self.on_band_commit {
                            shell.publish(f(band));
                        }
                        if let Some(f) = &self.on_commit {
                            shell.publish(f(state.hits.clone()));
                        }
                        if let Some(f) = &self.on_commit_keyed {
                            let resolved = self.resolve_selection(mode, &state.hits);
                            shell.publish(f(resolved));
                        }
                    }
                    MarqueeOutcome::Clicked { mode } => {
                        if !mode.is_additive()
                            && let Some(m) = &self.on_background_click
                        {
                            shell.publish(m.clone());
                        }
                    }
                    MarqueeOutcome::Cancelled => {}
                }
                state.covered.clear();
                state.resolved.clear();
                state.last_band = None;
                shell.request_redraw();
                if was_active {
                    shell.capture_event();
                }
            }

            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => {
                if let Some(session) = state.session.as_mut() {
                    let active = session.is_active();
                    session.cancel();
                    state.session = None;
                    state.covered.clear();
                    state.last_band = None;
                    shell.request_redraw();
                    if active {
                        shell.capture_event();
                    }
                }
            }

            Event::Mouse(mouse::Event::CursorLeft)
            | Event::Window(iced::window::Event::Unfocused) => {
                if let Some(session) = state.session.as_mut() {
                    session.cancel();
                    state.session = None;
                    state.covered.clear();
                    state.resolved.clear();
                    state.last_band = None;
                    shell.request_redraw();
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
        use iced::advanced::Renderer as _;

        let tokens = resolve_tokens(self.tokens, theme);
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<MarqueeState>();
        let dragging = state
            .session
            .as_ref()
            .is_some_and(|s| s.phase() == MarqueePhase::Active);

        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child,
                cursor,
                viewport,
            );
        }

        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };

        // Clip partial highlights and out-of-bounds bands together, above content.
        renderer.with_layer(clip, |renderer| {
            if self.highlight_covered {
                let keyed = !self.ids.is_empty();
                let id_map = keyed.then(|| self.id_map());
                let shown: Vec<usize> = if keyed {
                    let map = id_map.as_deref().unwrap_or_default();
                    let wanted: &[ItemId] = if dragging {
                        &state.resolved
                    } else {
                        &self.selected_ids
                    };
                    wanted
                        .iter()
                        .filter_map(|id| map.iter().position(|x| x.as_ref() == Some(id)))
                        .collect()
                } else if dragging {
                    state.covered.clone()
                } else {
                    self.selected.clone()
                };
                for &i in &shown {
                    let Some(r) = self.items.get(i) else {
                        continue;
                    };
                    let rect = Rectangle {
                        x: bounds.x + r[0],
                        y: bounds.y + r[1],
                        width: r[2],
                        height: r[3],
                    };
                    if rect.intersection(viewport).is_none() {
                        continue;
                    }
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: rect,
                            border: Border {
                                color: tokens.band_border,
                                width: tokens.band_border_width,
                                radius: tokens.band_radius.into(),
                            },
                            shadow: Shadow::default(),
                            snap: true,
                        },
                        Background::Color(tokens.highlight),
                    );
                }
            }

            if dragging && let Some(session) = state.session.as_ref() {
                let band = session.band();
                let rect = Rectangle {
                    x: bounds.x + band.x,
                    y: bounds.y + band.y,
                    width: band.width,
                    height: band.height,
                };
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: rect,
                        border: Border {
                            color: tokens.band_border,
                            width: tokens.band_border_width,
                            radius: tokens.band_radius.into(),
                        },
                        shadow: Shadow::default(),
                        snap: true,
                    },
                    Background::Color(tokens.band_fill),
                );
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let child = layout
            .children(&tree.children)
            .next()
            .map(|c| {
                self.content.as_widget().mouse_interaction(
                    &tree.children[0],
                    c,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .unwrap_or_default();
        let state = tree.state.downcast_ref::<MarqueeState>();
        if state
            .session
            .as_ref()
            .is_some_and(|s| s.phase() == MarqueePhase::Active)
        {
            mouse::Interaction::Crosshair
        } else {
            child
        }
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
        let Some(child) = layout.children(&tree.children).next() else {
            return Vec::new();
        };
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> EnzMarqueeArea<'a, Message>
where
    Message: 'a + Clone,
{
    /// Converts the configured area into an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzMarqueeArea<'a, Message>> for Element<'a, Message>
where
    Message: 'a + Clone,
{
    fn from(area: EnzMarqueeArea<'a, Message>) -> Self {
        Self::new(area)
    }
}

#[cfg(test)]
pub(crate) mod test_probe {
    use std::cell::Cell;

    // Measurements are isolated per test thread, including their enable gate.
    thread_local! {
        static FINGERPRINT_CALLS: Cell<usize> = const { Cell::new(0) };
        static RECTS_HASHED: Cell<usize> = const { Cell::new(0) };
        static INDEX_REBUILDS: Cell<usize> = const { Cell::new(0) };
        static MEASURING: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn note_fingerprint(len: usize) {
        if MEASURING.with(Cell::get) {
            FINGERPRINT_CALLS.with(|c| c.set(c.get() + 1));
            RECTS_HASHED.with(|c| c.set(c.get() + len));
        }
    }

    pub(super) fn note_rebuild() {
        if MEASURING.with(Cell::get) {
            INDEX_REBUILDS.with(|c| c.set(c.get() + 1));
        }
    }
}
