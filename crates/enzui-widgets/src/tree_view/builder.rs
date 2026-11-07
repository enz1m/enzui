//! `tree_view`: builder.
//!
//! Owns the public builder and its assembly logic for the tree-view implementation.

use super::*;

/// Row-body closure over realized rows, named to keep `clippy::type_complexity`
/// quiet: one boxed `Fn`, not an abstraction. Same shape as
/// [`virtual_list`](crate::virtual_list)'s row builder.
///
/// The index is logical (into the full list), not a realized-window offset:
/// anything keyed off it must survive scrolling.
pub type TreeRowContentFn<'a, Message> =
    Box<dyn Fn(usize, &EnzTreeItem) -> Element<'a, Message> + 'a>;

/// A virtualized row action factory. `None` leaves that row with no slot.
pub type TreeRowTrailingFn<'a, Message> =
    Box<dyn Fn(usize, &EnzTreeItem) -> Option<Element<'a, Message>> + 'a>;

/// Builder for a tree view.
#[must_use]
pub struct EnzTreeView<'a, Message: Clone> {
    pub(crate) items: Vec<EnzTreeItem>,
    /// App-owned row body, once per realized row; `None` draws the built-in
    /// single-line label (existing call sites unchanged).
    pub(crate) row_content: Option<TreeRowContentFn<'a, Message>>,
    /// Optional per-row trailing action, built only for realized rows.
    pub(crate) row_trailing: Option<TreeRowTrailingFn<'a, Message>>,
    pub(crate) row_trailing_visibility: crate::list_row_surface::TrailingVisibility,
    pub(crate) selected: Option<usize>,
    /// Keyed selection: stable key, not position. Wins over [`Self::selected`]
    /// and resolves each frame, so selection follows its node across
    /// insert/remove/reorder and expand/collapse instead of transferring to
    /// whatever drifted into the slot.
    pub(crate) selected_key: Option<SequenceKey>,
    pub(crate) aria_label: String,
    pub(crate) tree_row_height: f32,
    /// Vertical separation between adjacent row surfaces, in px; `0.0` by
    /// default.
    pub(crate) tree_row_gap: f32,
    pub(crate) indent_width: f32,
    /// Writing direction override; `None` follows the locale.
    pub(crate) direction: Option<enzui_core::placement::Direction>,
    pub(crate) disclosure_size: f32,
    pub(crate) row_padding_x: f32,
    pub(crate) body_size: f32,
    pub(crate) panel_radius: f32,
    pub(crate) row_radius: Option<f32>,
    pub(crate) viewport_radius: Option<f32>,
    pub(crate) panel_border_width: f32,
    pub(crate) empty_text: String,
    pub(crate) text_color: Option<Color>,
    pub(crate) muted_color: Option<Color>,
    pub(crate) selected_background: Option<Color>,
    /// Built in the shell's sidebar: select with the sidebar's tint.
    pub(crate) in_sidebar: bool,
    pub(crate) item_background: Option<Color>,
    pub(crate) hover_background: Option<Color>,
    pub(crate) surface_background: Option<Color>,
    pub(crate) tree_border: Option<Color>,
    pub(crate) id: Option<iced::advanced::widget::Id>,
    /// Per-row id namespace; defaults to [`ID_PREFIX`].
    pub(crate) id_prefix: String,
    /// Per-instance reduced-motion override forwarded to every row's
    /// background transition; `None` follows the runtime theme.
    pub(crate) reduced_motion: Option<bool>,
    /// An explicitly pinned `active_descendant` overriding the derived cursor
    /// row. Mirrors [`crate::menu::EnzMenu::active_descendant`].
    pub(crate) active_descendant: Option<iced::advanced::widget::Id>,
    pub(crate) on_select: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    pub(crate) on_toggle: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    /// Row right-click hook, fed the logical index and window-space point.
    pub(crate) row_on_context: Option<Box<dyn Fn(usize, iced::Point) -> Message + 'a>>,
    /// Name announced for every row's context-menu affordance.
    pub(crate) row_context_label: Option<String>,

    // ---- Virtualization inputs: viewport, scroll and overscan ------
    /// Viewport height; `None` keeps the eager path, `Some(h)` realizes
    /// visible + overscan rows only.
    pub(crate) viewport_height: Option<f32>,
    /// Scroll offset (viewport top) in px: the realization-window input on
    /// the virtualized path; the first-layout seed on the eager path, so an
    /// unmounted-then-reopened subtree restores its offset. Fed back through
    /// [`Self::on_scroll`] on both paths.
    pub(crate) scroll_offset: f32,
    /// Caller-stable identity for the internal scrollable. `None` mints a
    /// fresh id per build (fine inside a stable parent, lost across unmount);
    /// a stable id keeps `scroll_to` and the live offset across reopen.
    /// Named `scroll_id` because [`Self::id`] already targets row focus — one
    /// bare `id()` routing to either part would be a coin toss.
    pub(crate) scroll_id: Option<iced::advanced::widget::Id>,
    /// Overscan rows per side; `0` honoured literally.
    pub(crate) overscan: usize,
    /// Live viewport reporter feeding [`Self::scroll_offset`] back; the eager
    /// path's window is the whole list anyway.
    pub(crate) on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    /// Invocation probe (`#[doc(hidden)]`): bumped once per materialized row,
    /// so "only visible rows are built" is an assertable number.
    pub(crate) row_builder_probe: Option<Rc<Cell<usize>>>,
}

/// Build a tree over a pre-flattened item list.
pub fn enz_tree_view<Message: Clone>(items: Vec<EnzTreeItem>) -> EnzTreeView<'static, Message> {
    EnzTreeView {
        items,
        row_content: None,
        row_trailing: None,
        row_trailing_visibility: crate::list_row_surface::TrailingVisibility::Always,
        selected: None,
        selected_key: None,
        aria_label: enzui_core::localization::text("enzui-tree-view-tree"),
        tree_row_height: enzui_core::metrics::FinitePositive::new_or(
            enzui_theme::text_defaults::sizing().row_height,
            ROW_HEIGHT,
        )
        .get(),
        tree_row_gap: crate::list_row_surface::DEFAULT_ROW_GAP,
        indent_width: INDENT_WIDTH,
        direction: None,
        disclosure_size: DISCLOSURE_SIZE,
        row_padding_x: ROW_PADDING_X,
        body_size: BODY_SIZE,
        panel_radius: PANEL_RADIUS,
        row_radius: None,
        viewport_radius: None,
        // Flat by default: the usual host (a sidebar rail) already paints the
        // surface, so our own fill adds no separation and its 1px stroke reads
        // as a stray card edge. Opt in with `.surface_background(color)`
        // and/or `.panel_border_width(w)`; both setters still normalize
        // garbage, so an explicit request keeps its finite fallback.
        panel_border_width: 0.0,
        empty_text: String::new(),
        text_color: None,
        muted_color: None,
        selected_background: None,
        in_sidebar: crate::title_bar::in_sidebar_pane(),
        item_background: None,
        hover_background: None,
        surface_background: None,
        tree_border: None,
        id: None,
        id_prefix: ID_PREFIX.into(),
        reduced_motion: None,
        active_descendant: None,
        on_select: None,
        on_toggle: None,
        row_on_context: None,
        row_context_label: None,
        viewport_height: None,
        scroll_offset: 0.0,
        scroll_id: None,
        overscan: DEFAULT_OVERSCAN,
        on_scroll: None,
        row_builder_probe: None,
    }
}

impl<'a, Message: Clone + 'a> EnzTreeView<'a, Message> {
    /// App-supplied row body keeping this widget's keyboard, virtualization
    /// and accessibility machinery. Called with the logical index and item;
    /// unset keeps the built-in label, so this is additive.
    ///
    /// # A closure, not per-item `Element`, because of virtualization
    ///
    /// Owned elements per item would be O(rows) host work before the widget
    /// can decide which ~25 rows it realizes, and would stop `EnzTreeItem`
    /// being `Clone`/`Debug`. The closure runs only for realized rows (pinned
    /// by `row_content_is_invoked_only_for_realized_rows`), like
    /// [`virtual_list`](crate::virtual_list) and [`enz_table`](crate::enz_table()).
    ///
    /// # The widget keeps the chrome
    ///
    /// Indent, chevron, surface, `Role::TreeItem` node and its state stay
    /// owned: hand-rolled indents get the arithmetic wrong and hand-rolled
    /// chevrons add a second Tab stop per row. Custom bodies lay out after
    /// them, in the default label's slot; the four custom-body traps live
    /// under `# Hazards` in [`crate::tree_view`].
    pub fn row_content<F: Fn(usize, &EnzTreeItem) -> Element<'a, Message> + 'a>(
        mut self,
        f: F,
    ) -> Self {
        self.row_content = Some(Box::new(f));
        self
    }

    /// Optional right-pinned per-realized-row action. `None` for action-less
    /// rows; the shared surface keeps slot and reveal policy, and the closure
    /// runs only for materialized rows like `row_content`.
    pub fn row_trailing<F: Fn(usize, &EnzTreeItem) -> Option<Element<'a, Message>> + 'a>(
        mut self,
        f: F,
    ) -> Self {
        self.row_trailing = Some(Box::new(f));
        self
    }

    /// Choose when actions supplied by [`Self::row_trailing`] are painted.
    pub fn row_trailing_visibility(
        mut self,
        visibility: crate::list_row_surface::TrailingVisibility,
    ) -> Self {
        self.row_trailing_visibility = visibility;
        self
    }

    /// Right-click a row: logical index plus window-space point. One hook for the whole tree
    /// (per-item closures would rebuild O(rows) work virtualization removes);
    /// the callback reports branch and leaf alike through the hit index —
    /// tell them apart via `items[index].has_children`. Chevron secondary
    /// presses report the row too. Carries gesture and position only, never
    /// menu content and never a selection move, so menus act on the row under
    /// the cursor. Unset composes no capture surface at all. Pair the point
    /// with [`enz_point_context_menu_host`](crate::context_menu_host::enz_point_context_menu_host),
    /// which anchors in the same window space. An AT-routed `ShowContextMenu`
    /// has no cursor and reports the row's own centre.
    pub fn row_on_context<F: Fn(usize, iced::Point) -> Message + 'a>(mut self, f: F) -> Self {
        self.row_on_context = Some(Box::new(f));
        self
    }

    /// One name for the tree's action set ("File actions"), not per row: the
    /// row's own name already comes from its `Role::TreeItem` node. Only
    /// meaningful with [`Self::row_on_context`] on an addressable (keyed)
    /// row; unkeyed rows advertise no node, though right-click still fires.
    pub fn row_context_label(mut self, label: impl Into<String>) -> Self {
        self.row_context_label = Some(label.into());
        self
    }

    /// Snap selection/hover instantly (`true`) or run the full ease even
    /// under a reduced-motion theme (`false`); unset follows the theme.
    /// Forwarded to each row's surface.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Positional index; `None` falls back to each item's own flag.
    /// Prefer [`Self::selected_key`]: an index names a slot, so insert or
    /// expand above it silently transfers selection to whatever drifted in.
    pub fn selected(mut self, idx: Option<usize>) -> Self {
        self.selected = idx;
        self
    }

    /// Backdrops callers can paint on: tree `accent_soft` selected wash (not
    /// `surface_selected`), unselected panel showing through the transparent
    /// item layer. Per-instance overrides compose here exactly as rendered;
    /// flat default (no fill) against an assumed `palette.surface` backdrop —
    /// a different parent supplies `.surface_background(color)` so both match
    /// by construction, like the other three widgets' inherit-do-not-guess.
    pub fn resolved_row_fills(&self, palette: &enzui_theme::Palette) -> TreeViewRowFills {
        TreeViewRowFills::resolved(
            palette,
            self.surface_background,
            self.item_background,
            self.selected_background.or_else(|| {
                self.in_sidebar
                    .then(|| crate::sidebar_row::selected_tint(palette))
            }),
        )
    }

    /// Backdrop behind row `idx`; `None` out of range. Keyed selection wins
    /// with rendering precedence; unfound keys resolve to `None` (never clamp
    /// to a neighbour), else positional, else the item flag.
    pub fn resolved_row_fill(&self, palette: &enzui_theme::Palette, idx: usize) -> Option<Color> {
        self.items.get(idx)?;
        let keys = row_keys(&self.items, &self.id_prefix);
        let selected = self.is_selected(idx, self.resolved_selected(&keys));
        Some(self.resolved_row_fills(palette).for_selected(selected))
    }

    /// Stable-key selection path: a `/`-joined ancestor-key path, resolved
    /// each frame. Wins over [`Self::selected`]; unfound keys select nothing
    /// (never clamp — clamping is silent transfer renamed) and re-resolve if
    /// the row returns. Reuses [`SequenceKey`], whose non-numeric constructor
    /// stops positional values being passed by accident.
    pub fn selected_key(mut self, key: Option<SequenceKey>) -> Self {
        self.selected_key = key;
        self
    }
    /// Accessible name; defaults to `"Tree"`.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = label.into();
        self
    }
    /// Row height in px; garbage falls back to 34.
    pub fn tree_row_height(mut self, h: f32) -> Self {
        self.tree_row_height = enzui_core::metrics::FinitePositive::new_or(h, ROW_HEIGHT).get();
        self
    }
    /// In-cell separation: pitch stays `tree_row_height` (uniform, so scroll
    /// math, hit-testing and realized-child geometry never desync — silently,
    /// past the first screenful). The gap pads *inside* the cell (`gap / 2`
    /// each side, surface paints `height - gap`); hit targets shrink with the
    /// surface, and values clamp to keep [`MIN_ROW_SURFACE`] px. Defaults to
    /// [`DEFAULT_ROW_GAP`](crate::list_row_surface::DEFAULT_ROW_GAP), so a
    /// selected row and a hovered neighbour read as two; garbage falls back
    /// to `0.0`.
    pub fn tree_row_gap(mut self, g: f32) -> Self {
        self.tree_row_gap = enzui_core::metrics::FiniteNonNeg::new_or(g, 0.0).get();
        self
    }

    /// Effective gap after clamping against row height. Clamped at use (not
    /// in the setter) so builder call order never matters; a row below the
    /// floor keeps its height and loses the gap instead of the row.
    pub(crate) fn effective_row_gap(&self) -> f32 {
        let room = (self.tree_row_height - MIN_ROW_SURFACE).max(0.0);
        self.tree_row_gap.min(room)
    }

    /// One row surface's painted height: pitch minus effective gap. Every
    /// vertical extent builds from this, so surface and spacers agree.
    pub(crate) fn row_surface_height(&self) -> f32 {
        (self.tree_row_height - self.effective_row_gap()).max(0.0)
    }

    /// Override the writing direction. By default the tree follows the
    /// locale: right-to-left indents from the right, points collapsed
    /// chevrons left and swaps the meaning of Left and Right.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Indent per nesting level in px; garbage falls back to 18.
    pub fn indent_width(mut self, w: f32) -> Self {
        self.indent_width = enzui_core::metrics::FiniteNonNeg::new_or(w, INDENT_WIDTH).get();
        self
    }
    /// Chevron size in px; garbage falls back to 16.
    pub fn disclosure_size(mut self, s: f32) -> Self {
        self.disclosure_size =
            enzui_core::metrics::FinitePositive::new_or(s, DISCLOSURE_SIZE).get();
        self
    }
    /// Row horizontal padding in px; garbage falls back to 8.
    pub fn row_padding_x(mut self, p: f32) -> Self {
        self.row_padding_x = enzui_core::metrics::FiniteNonNeg::new_or(p, ROW_PADDING_X).get();
        self
    }
    /// Label size in px; garbage falls back to 13.
    pub fn body_size(mut self, s: f32) -> Self {
        self.body_size = enzui_core::metrics::FinitePositive::new_or(s, BODY_SIZE).get();
        self
    }
    /// Panel radius in px; garbage falls back to 8.
    pub fn panel_radius(mut self, r: f32) -> Self {
        self.panel_radius = enzui_core::metrics::FiniteNonNeg::new_or(r, PANEL_RADIUS).get();
        self
    }
    /// Row surface radius in pixels; defaults to the smaller of the panel radius and 6.
    pub fn row_radius(mut self, radius: f32) -> Self {
        self.row_radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 6.0).get());
        self
    }

    /// Scroll viewport clipping radius in pixels, including its overlay scrollbar.
    ///
    /// Unset follows the panel radius when panel fill or border is painted;
    /// flat transparent trees use a rectangular clip to preserve row corners.
    pub fn viewport_radius(mut self, radius: f32) -> Self {
        self.viewport_radius =
            Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, PANEL_RADIUS).get());
        self
    }

    /// Panel border width in px; garbage falls back to 1.
    pub fn panel_border_width(mut self, w: f32) -> Self {
        self.panel_border_width =
            enzui_core::metrics::FiniteNonNeg::new_or(w, PANEL_BORDER_WIDTH).get();
        self
    }
    /// Empty-tree placeholder; empty by default.
    pub fn empty_text(mut self, s: impl Into<String>) -> Self {
        self.empty_text = s.into();
        self
    }
    /// Row-label ink override; unset resolves from the theme.
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }
    /// Muted-ink override (e.g. empty placeholder); unset resolves from theme.
    pub fn muted_color(mut self, c: Color) -> Self {
        self.muted_color = Some(c);
        self
    }
    /// Mark the tree as a sidebar list: its selection takes the sidebar's
    /// tint ([`crate::sidebar_row::selected_tint`]), like every sidebar row.
    /// Set automatically inside the shell's sidebar slot; call it for a
    /// sidebar the app composes itself.
    pub fn in_sidebar(mut self, yes: bool) -> Self {
        self.in_sidebar = yes;
        self
    }

    /// Selected-row fill override; unset resolves from the theme.
    pub fn selected_background(mut self, c: Color) -> Self {
        self.selected_background = Some(c);
        self
    }
    /// Rest fill behind every row; transparent default shows the panel.
    pub fn item_background(mut self, c: Color) -> Self {
        self.item_background = Some(c);
        self
    }
    /// Hovered-row fill; the shared hover overlay over [`Self::item_background`].
    pub fn hover_background(mut self, c: Color) -> Self {
        self.hover_background = Some(c);
        self
    }
    /// Panel surface fill override; unset resolves from the theme.
    pub fn surface_background(mut self, c: Color) -> Self {
        self.surface_background = Some(c);
        self
    }
    /// Panel border colour override; unset resolves from the theme.
    pub fn tree_border(mut self, c: Color) -> Self {
        self.tree_border = Some(c);
        self
    }
    /// Stable id for programmatic `focus(id)`.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Per-row id namespace (default [`ID_PREFIX`]); give co-screen trees
    /// their own prefix, since siblings cannot see each other collide.
    /// Distinct from [`Self::id`], which targets the tree itself.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Pin the cursor row (`aria-activedescendant`), overriding the derived
    /// one. Unlisted rows are dropped, never published dangling; precedence
    /// matches [`crate::menu::EnzMenu::active_descendant`].
    pub fn active_descendant(mut self, row: iced::advanced::widget::Id) -> Self {
        self.active_descendant = Some(row);
        self
    }

    /// Per-row routable ids in order, computable by hosts that drive focus
    /// or verify routing without duplicating the derivation.
    pub fn row_ids(&self) -> Vec<Option<iced::advanced::widget::Id>> {
        row_ids(&self.items, &self.id_prefix)
    }
    /// Row-select callback (flat index) on click or Enter/Space; unset by
    /// default.
    pub fn on_select<F: Fn(usize) -> Message + 'a>(mut self, f: F) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }
    /// Branch expand/collapse callback (flat index) on chevron click or
    /// Left/Right; the host flips `expanded` and rebuilds. Unset by default.
    pub fn on_toggle<F: Fn(usize) -> Message + 'a>(mut self, f: F) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
    /// Flattened row count.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Viewport height in px — the virtualization ON switch. Set realizes
    /// visible + overscan only (back-compat eager otherwise). `NaN`/`inf`/
    /// negative/`0.0` fall back to eager (never an invisible spacers-only
    /// tree); pair with [`Self::scroll_offset`] + [`Self::on_scroll`], or
    /// the window pins at `0.0`.
    pub fn viewport_height(mut self, height: f32) -> Self {
        self.viewport_height = if height.is_finite() && height > 0.0 {
            Some(height)
        } else {
            None
        };
        self
    }

    /// Scroll offset in px (sanitized to `0.0`): realization-window input
    /// when virtualized; first-layout seed when eager, so unmounted-then-
    /// reopened subtrees restore position (consumed once per fresh state,
    /// so per-build passing is safe). Beyond-extent offsets clamp when rows
    /// disappear. Eager retention needs no viewport;
    /// pair with [`Self::scroll_id`] to keep `Id` operations on target.
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = enzui_core::metrics::FiniteNonNeg::new_or(offset, 0.0).get();
        self
    }

    /// Caller-stable scrollable identity; unset mints per-build (enough for
    /// probes and reveal, lost across unmount). Set for unmounting subtrees
    /// so `scroll_to`/reveal keep landing; pair with offset + [`Self::on_scroll`].
    /// `scroll_id`, not `id`: one bare name routing to either part would be a
    /// coin toss.
    pub fn scroll_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.scroll_id = Some(id);
        self
    }

    /// Overscan rows per side (default [`DEFAULT_OVERSCAN`]; `0` literal).
    /// Virtualized path only.
    pub fn overscan(mut self, n: usize) -> Self {
        self.overscan = n;
        self
    }

    /// Viewport reporter: `.on_scroll(|vp| Msg::Scrolled(vp.absolute_offset().y))`
    /// stored and fed to `scroll_offset` next frame.
    pub fn on_scroll<F: Fn(Viewport) -> Message + 'a>(mut self, f: F) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Materialization probe (`#[doc(hidden)]`): the counter that makes
    /// "only visible rows built" assertable.
    #[doc(hidden)]
    pub fn row_builder_probe(mut self, probe: Rc<Cell<usize>>) -> Self {
        self.row_builder_probe = Some(probe);
        self
    }

    /// Current view-level selection index, keyed-first: unfound keys are
    /// `None` (never clamp), else the positional index verbatim (`0` valid).
    pub(crate) fn resolved_selected(&self, keys: &[String]) -> Option<usize> {
        if let Some(key) = &self.selected_key {
            return keys.iter().position(|k| k.as_str() == key.as_str());
        }
        self.selected
    }

    pub(crate) fn is_selected(&self, idx: usize, resolved: Option<usize>) -> bool {
        // Explicit host selection suppresses item flags, even if it resolves to no row.
        if self.selected.is_some() || self.selected_key.is_some() {
            return resolved == Some(idx);
        }
        self.items.get(idx).is_some_and(|i| i.selected)
    }

    /// Build the element.
    pub fn into_element(self) -> Element<'a, Message> {
        if self.items.is_empty() {
            return self.empty_element();
        }
        self.populated_element()
    }

    /// Empty-state panel, separate: no rows, cursor, or keyboard means the
    /// roving-focus wrapper would make emptiness a dead Tab stop.
    fn empty_element(self) -> Element<'a, Message> {
        let muted_pin = self.muted_color;
        let body_size = self.body_size;
        let inner: Element<'a, Message> = if self.empty_text.is_empty() {
            Space::new().width(Length::Fill).height(Length::Fill).into()
        } else {
            let mut label = enz_text(self.empty_text.clone())
                .size_px(body_size)
                .tone(crate::enz_text::TextTone::Muted)
                .align(crate::enz_text::TextAlign::Center)
                .wrap(true)
                .full_width();
            if let Some(c) = muted_pin {
                label = label.color(c);
            }
            container(label.into_element::<Message>())
                .width(Length::Fill)
                .height(Length::Fill)
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .into()
        };

        container(inner)
            .width(Length::Fill)
            .height(Length::Fill)
            // Centered text or Space cannot reach the corner arc: no full-bleed
            // slot exists, so no `rounded_clip` pass is spent for zero change.
            .clip(true)
            .style(self.panel_style())
            .into()
    }

    /// Panel chrome, flat by default (caller pins fill/border to draw).
    /// Radius needs a drawn half to show; unpinned theme leaves re-resolve
    /// live per frame, so width-only opt-in still tracks the theme.
    fn panel_style(&self) -> impl Fn(&iced::Theme) -> iced::widget::container::Style + 'static {
        let surface_pin = self.surface_background;
        let border_pin = self.tree_border;
        let radius = self.panel_radius;
        let width = self.panel_border_width;
        move |t: &iced::Theme| {
            let p = crate::theme_lookup::palette(t);
            let background = surface_pin.map(iced::Background::Color);
            // Zero-width publishes no edge: unpinned flat uses TRANSPARENT, so a
            // mis-drawn nominal 1.0 cannot render a card edge.
            let border_color = if width > 0.0 {
                border_pin.unwrap_or(p.border)
            } else {
                border_pin.unwrap_or(Color::TRANSPARENT)
            };
            iced::widget::container::Style {
                background,
                border: Border {
                    color: border_color,
                    width,
                    radius: radius.into(),
                },
                ..Default::default()
            }
        }
    }

    fn is_rtl(&self) -> bool {
        enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft
    }

    /// One row: indent, chevron slot, label — inside the shared row surface.
    #[allow(
        clippy::too_many_arguments,
        reason = "one row's position, state and callbacks"
    )]
    fn row_element(
        &self,
        idx: usize,
        item: &EnzTreeItem,
        // Unkeyed rows render identically but address nothing: no id, no
        // actions (reported honestly), though right-click still fires.
        row_id: Option<iced::advanced::widget::Id>,
        nav_view: &[TreeRow],
        // Keyed selection pre-resolved once per frame (not per row); the
        // O(N) lookup stays O(N).
        resolved_selected: Option<usize>,
        on_select: Option<&std::rc::Rc<dyn Fn(usize) -> Message + 'a>>,
        on_toggle: Option<&std::rc::Rc<dyn Fn(usize) -> Message + 'a>>,
        // Hook plus affordance id bound here, where the index is known: the
        // surface takes only a point, so the row cannot be re-decided later.
        row_on_context: Option<&std::rc::Rc<dyn Fn(usize, iced::Point) -> Message + 'a>>,
        row_context_id: Option<iced::advanced::widget::Id>,
        row_reveals: &mut Vec<(usize, crate::list_row_surface::TrailingReveal)>,
        // No row expands or nests: there is no chevron column to align, so
        // labels start at the row padding.
        flat: bool,
    ) -> Element<'a, Message> {
        // Pitch stays `tree_row_height`; everything painted uses the SURFACE
        // height (the two are equal when there is no row gap).
        let row_height = self.row_surface_height();
        let row_gap = self.effective_row_gap();
        let selected = self.is_selected(idx, resolved_selected);

        let indent = Space::new()
            .width(Length::Fixed(self.indent_width * f32::from(item.level)))
            .height(Length::Fixed(row_height));

        // Chevron slot or equal spacer, so labels align across depths.
        let disclosure: Element<'a, Message> = if item.has_children {
            let factory: Option<Box<dyn Fn() -> Message + 'a>> = on_toggle.map(|f| {
                let f = Rc::clone(f);
                Box::new(move || f(idx)) as Box<dyn Fn() -> Message + 'a>
            });
            let mut d = enz_tree_disclosure_lazy::<Message>(item.expanded)
                .direction(enzui_core::placement::resolve_direction(self.direction))
                .embedded(true)
                .button_size(self.disclosure_size)
                .row_height(row_height)
                .icon_size(self.disclosure_size * 0.6)
                // Only branches alias chevron-Expand to the row id (leaf
                // Expand stays `Unhandled`); row id keeps activation.
                .on_toggle_maybe_with(factory);
            // Alias only with an id; unkeyed rows route nothing.
            if let Some(row_id) = row_id.clone() {
                d = d.expand_alias(row_id);
            }
            if let Some(c) = self.muted_color {
                d = d.icon_color(c);
            }
            d.into()
        } else {
            Space::new()
                .width(Length::Fixed(if flat { 0.0 } else { self.disclosure_size }))
                .height(Length::Fixed(row_height))
                .into()
        };

        // Only the body swaps (default elided label otherwise): indent,
        // chevron and surface stay widget-owned navigation affordances.
        // `drawn_text` (what the default body put on screen; `None` for app
        // bodies, which are opaque) forwards as `content_text`, so an
        // `aria_label` override does not double-announce the drawn label.
        let custom_body = self.row_content.is_some();
        let (body, drawn_text): (Element<'a, Message>, Option<String>) = match &self.row_content {
            Some(build) => (build(idx, item), None),
            None => {
                // Branches read as headings over their children.
                let mut label = enz_text(item.text.clone())
                    .size_px(self.body_size)
                    .weight_rung(if item.has_children {
                        enzui_theme::WeightRung::Semibold
                    } else {
                        enzui_theme::WeightRung::Regular
                    })
                    // End-elided single line filling the width: long names truncate
                    // inside the panel, never past its border or viewport.
                    .single_line_elided()
                    .full_width();
                if let Some(c) = self.text_color {
                    label = label.color(c);
                }
                (label.into_element::<Message>(), Some(item.text.clone()))
            }
        };
        let trailing = self
            .row_trailing
            .as_ref()
            .and_then(|build| build(idx, item));
        // Trailing actions pin the body lane full-width (default bodies
        // already fill; action ownership untouched; custom rows without
        // actions unchanged).
        let body = if custom_body && trailing.is_some() {
            container(body).width(Length::Fill).into()
        } else {
            body
        };

        let mut inner_children: Vec<Element<'a, Message>> = vec![
            indent.into(),
            disclosure,
            Space::new()
                .width(Length::Fixed(if flat { 0.0 } else { LABEL_GAP }))
                .height(Length::Fixed(row_height))
                .into(),
            body,
        ];
        let mut trailing_reveal = None;
        if let Some(trailing) = trailing {
            inner_children.push(Space::new().width(Length::Fixed(LABEL_GAP)).into());
            if matches!(
                self.row_trailing_visibility,
                crate::list_row_surface::TrailingVisibility::OnHoverOrFocus
            ) {
                let reveal = std::rc::Rc::new(std::cell::Cell::new(false));
                inner_children.push(crate::list_row_surface::trailing_paint(
                    trailing,
                    self.row_trailing_visibility,
                    Rc::clone(&reveal),
                ));
                row_reveals.push((idx, Rc::clone(&reveal)));
                trailing_reveal = Some(reveal);
            } else {
                inner_children.push(trailing);
            }
        }
        let inner = row(inner_children)
            .mirrored(self.is_rtl())
            .align_y(iced::alignment::Vertical::Center);

        // Trailing inset keeps elided labels off the panel border.
        let padded = container(inner)
            .width(Length::Fill)
            .height(Length::Fixed(row_height))
            // Opaque custom bodies may exceed row height; the row boundary
            // scissors paint (hosts: prefer `single_line_elided()` with the
            // full value in the name or tooltip).
            .clip(true)
            .padding(iced::Padding {
                top: 0.0,
                right: self.row_padding_x,
                bottom: 0.0,
                left: self.row_padding_x,
            });

        // Row name plus hierarchy, without double-announce: interactive rows
        // suppress the redundant collected label (pinned with name + absence +
        // count together, since each alone passes one opposite failure mode).
        // Empty/whitespace labels leave the row unnamed (`Some("")` would
        // read as named to readers and audits; whitespace fragments already
        // dropped by `Operation::text`).
        // Sibling-relative numbering from the same rule `parent_of` uses:
        // flat `idx + 1` / `len()` misdescribed nested children.
        let (position, count) = sibling_position_of(nav_view, idx).unwrap_or((1, 1));
        let mut semantics = Semantics::new(Role::TreeItem)
            .level(usize::from(item.level) + 1)
            .set_position(SetPosition {
                index: position,
                count,
            })
            .selected(selected);
        // Click advertised only with an `on_select` carrier: unconditional
        // Click told AT even select-less rows were clickable.
        if on_select.is_some() {
            semantics = semantics.default_action();
        }
        // Name from a string (`aria_label`, else drawn text) — never the
        // opaque body, so custom rows cannot go anonymous; untrimmed, so the
        // suppression matches the default row exactly.
        if let Some(name) = item.accessible_name() {
            semantics = semantics.label(name.to_owned());
        }
        if item.has_children {
            // State without action: branches always announce open-ness; the
            // Expand/Collapse advertisement needs `on_toggle`, else the
            // chevron routes nowhere (advertised-but-dead).
            semantics = if on_toggle.is_some() {
                semantics.expanded(item.expanded)
            } else {
                semantics.expanded_state_only(item.expanded)
            };
        }

        let select_factory: Option<Box<dyn Fn() -> Message + 'a>> = on_select.map(|f| {
            let f = Rc::clone(f);
            Box::new(move || f(idx)) as Box<dyn Fn() -> Message + 'a>
        });
        // Bound to this row's index here: the surface takes only a point, so
        // selection or window offset cannot re-decide the row later.
        let context_factory: Option<Box<dyn Fn(iced::Point) -> Message + 'a>> =
            row_on_context.map(|f| {
                let f = Rc::clone(f);
                Box::new(move |at: iced::Point| f(idx, at))
                    as Box<dyn Fn(iced::Point) -> Message + 'a>
            });

        let item_bg = self.item_background;
        let hover_bg = self.hover_background;
        let selected_bg = self.selected_background;
        // In the shell's sidebar a tree selects like the sidebar's own rows.
        let in_sidebar = self.in_sidebar;
        let row_radius = self.effective_row_radius();

        let mut surface = enz_list_row_surface_lazy(padded)
            .content_padding(0.0)
            .embedded(true)
            .selected(selected)
            // Outer gutter opted out: rows own indent/padding and chevrons
            // must column-align; tree tokens stay authoritative.
            .outer_gutter(0.0)
            // Inset chrome inside the existing wrapper (no second container:
            // one retained node per session saved).
            .outer_vertical_gap(row_gap)
            .selectable(true)
            // Navigable rows are not disabled for lacking `on_select`:
            // "navigable" is not "actionable", and disabled would also drop
            // toggle-only Expand/Collapse advertisement.
            .navigable_when_inert(true)
            .semantics(semantics)
            // Forward `drawn_text` so overridden names do not second-announce;
            // `None` for opaque custom bodies (separate content, not a leak).
            .content_text_maybe(drawn_text)
            .row_height(row_height)
            .on_activate_maybe_with(select_factory)
            // This row's right-click, already bound to its logical index;
            // composed (never re-wrapped) by the row surface, so tree and list
            // rows agree on right-click meaning.
            .on_context_maybe(context_factory);
        if let Some(reveal) = trailing_reveal {
            surface = surface.parented_trailing_reveal_handle(reveal);
        }
        if row_on_context.is_some() {
            // Affordance id from the row's owner chain (labels may repeat;
            // unkeyed rows pin `None`: anonymous, pointer-only).
            surface = surface.context_id_maybe(row_context_id);
            if let Some(label) = &self.row_context_label {
                surface = surface.context_label(label.clone());
            }
        }
        // Routable id when keyed: without it the row's `Role::TreeItem` node
        // is one no AT action could address — not Expand, Collapse, activation.
        if let Some(row_id) = row_id {
            surface = surface.id(row_id);
        }
        // forward the tree's per-instance motion override onto the row
        // surface that actually owns the transition.
        if let Some(reduced) = self.reduced_motion {
            surface = surface.reduced_motion(reduced);
        }
        let surface: Element<'a, Message> = surface
            .tokens_resolved(move |theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                let mut t =
                    crate::theme_lookup::resolve_live(theme, ListRowSurfaceTokens::from_palette);
                // The tree's `accent_soft` selection (panel, not window
                // background); sampled independently so no twin accessor can drift
                // past comparing itself.
                t.selected_background = selected_bg.unwrap_or_else(|| {
                    if in_sidebar {
                        crate::sidebar_row::selected_tint(&p)
                    } else {
                        p.accent_soft
                    }
                });
                if let Some(c) = item_bg {
                    t.surface_background = c;
                }
                if let Some(c) = hover_bg {
                    // Pinned hover is a fill replacement, not a tint.
                    t.hover_overlay = c;
                    t.surface_background = Color::TRANSPARENT;
                }
                t.row_radius = row_radius;
                t
            })
            .into();
        // Gap folds into the existing outer gutter (pitch kept, no second
        // wrapper); zero preserves the pre-gap tree exactly.
        surface
    }

    fn effective_row_radius(&self) -> f32 {
        self.row_radius
            .unwrap_or_else(|| self.panel_radius.min(6.0))
    }

    fn effective_viewport_radius(&self) -> f32 {
        self.viewport_radius.unwrap_or_else(|| {
            if self.panel_border_width <= 0.0
                && self.surface_background.is_none_or(|color| color.a <= 0.0)
            {
                0.0
            } else {
                self.panel_radius
            }
        })
    }

    /// Populated tree: scrolling rows in the panel, under the roving-focus
    /// owner.
    fn populated_element(mut self) -> Element<'a, Message> {
        // Factories shared per row via `Rc`: lazy activation runs the
        // caller's closure at click time, not per row per frame.
        let on_select: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>> =
            self.on_select.take().map(std::rc::Rc::from);
        let on_toggle: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>> =
            self.on_toggle.take().map(std::rc::Rc::from);
        // Same sharing for the secondary hook: called at click time, not one
        // message per row per frame.
        let row_on_context: Option<std::rc::Rc<dyn Fn(usize, iced::Point) -> Message + 'a>> =
            self.row_on_context.take().map(std::rc::Rc::from);

        // One derived routable id per row, from the node path.
        let ids = row_ids(&self.items, &self.id_prefix);
        // Affordance ids from the same owner chain, different role, built
        // once for both — and only with a hook to publish them for.
        let ctx_ids: Vec<Option<iced::advanced::widget::Id>> = if row_on_context.is_some() {
            row_context_ids(&self.items, &self.id_prefix)
        } else {
            Vec::new()
        };
        // Path strings double as cursor reconciliation keys, derived by the
        // identical rule to stay in lockstep with `ids`.
        let keys = row_keys(&self.items, &self.id_prefix);

        // Navigation view built once (O(N)), not per row (O(N²)).
        let nav_rows: Vec<TreeRow> = self.items.iter().map(EnzTreeItem::as_nav_row).collect();

        // Windowed this frame: eager is the whole list; virtualized is
        // visible + overscan via the shared `virtual_sequence` primitive,
        // not a second copy of the arithmetic here. Only realization is
        // windowed — nav, ids, keys and selection stay full-list, so the
        // cursor, identities and a11y positions stay logical.
        let (realized, effective_scroll_offset): (std::ops::Range<usize>, f32) =
            match self.viewport_height {
                Some(vp_h) => {
                    let model = HeightModel::new(self.items.len(), self.tree_row_height);
                    // Tree shape may shrink while an offset is persisted across an unmount.
                    let max_scroll = (model.total_height() - vp_h).max(0.0);
                    let offset = self.scroll_offset.min(max_scroll);
                    let anchor = Anchor::from_scroll_top(&model, offset);
                    (
                        RealizationWindow::new(self.overscan).realize(&model, &anchor, vp_h),
                        offset,
                    )
                }
                None => (0..self.items.len(), self.scroll_offset),
            };

        // Selection resolved once (keyed wins), threaded everywhere: one O(N)
        // lookup, agreed flags.
        let resolved_selected = self.resolved_selected(&keys);

        let probe = self.row_builder_probe.clone();
        let is_virtualized = self.viewport_height.is_some();
        let mut row_reveals = Vec::new();
        let flat = !self
            .items
            .iter()
            .any(|item| item.has_children || item.level > 0);
        let rows: Vec<(usize, Element<'a, Message>)> = self.items[realized.clone()]
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                let idx = realized.start + offset;
                if let Some(p) = &probe {
                    p.set(p.get() + 1);
                }
                let el = self.row_element(
                    idx,
                    item,
                    ids[idx].clone(),
                    &nav_rows,
                    resolved_selected,
                    on_select.as_ref(),
                    on_toggle.as_ref(),
                    row_on_context.as_ref(),
                    ctx_ids.get(idx).cloned().flatten(),
                    &mut row_reveals,
                    flat,
                );
                (idx, el)
            })
            .collect();

        // Eager children are the rows; virtualized is one keyed `Column`
        // (padding = unrealized extents), so content height — scroll range,
        // thumb, document offsets — matches eager exactly.
        // Keyed by logical index: positional columns rebind slot state to new
        // rows on window shifts (press fires through the wrong callback).
        // Dropped rows drop state with their surface, correctly.
        // Eager keeps a plain `Column` (no shifts, keying is pure cost).
        // Padding, not spacer siblings: spacers would read as rows to the
        // `row_layer` screen (a 3-of-5 window makes two spacer rects).
        let scroll_children: Vec<Element<'a, Message>> = if is_virtualized {
            let model = HeightModel::new(self.items.len(), self.tree_row_height);
            let leading = model.offset_of(realized.start);
            let realized_h = realized.len() as f32 * self.tree_row_height;
            let trailing = (model.total_height() - leading - realized_h).max(0.0);
            let keyed_rows: Vec<(String, Element<'a, Message>)> = rows
                .into_iter()
                .map(|(idx, el)| (idx.to_string(), el))
                .collect();
            vec![
                container(KeyedVirtColumn::new(keyed_rows).width(Length::Fill))
                    .width(Length::Fill)
                    .padding(iced::Padding {
                        top: leading,
                        right: 0.0,
                        bottom: trailing,
                        left: 0.0,
                    })
                    .into(),
            ]
        } else {
            rows.into_iter().map(|(_key, el)| el).collect()
        };

        // Fill-height clipped viewport; `spacing(0)` (contiguous rows) with
        // the gutter off (panel padding already clears the scrollbar).
        // Tagged `Id`: live translation for hit-test/focus-ring math, plus
        // `scroll_to` reveal past the visible edge. Caller-stable id
        // preferred (same id across unmount); else one derived from the
        // row-id prefix. Never a per-build id: a scrollable resets its offset
        // whenever its id changes, so it would snap back every frame.
        let scroll_id = self
            .scroll_id
            .take()
            .unwrap_or_else(|| enzui_core::identity::derive_id(&self.id_prefix, "tree-scroll"));
        let mut body = enz_scroll_column(scroll_children)
            .id(scroll_id.clone())
            .spacing(0.0)
            .scrollbar_gutter(false)
            .width(Length::Fill)
            .height(Length::Fill);
        // Viewport movement closes the loop: without feedback the window pins
        // at 0.0 while the tree scrolls (eager path: persists across unmount).
        if let Some(f) = self.on_scroll.take() {
            body = body.on_scroll(f);
        }
        // First-layout seed restores unmounted trees to the captured offset
        // (consumed once; gestures own it after). Both paths.
        body = body.initial_offset(AbsoluteOffset {
            x: 0.0,
            y: effective_scroll_offset,
        });

        let viewport_radius = self.effective_viewport_radius();
        let masked: Element<'a, Message> =
            crate::rounded_clip::rounded_clip(body, iced::border::Radius::new(viewport_radius))
                .into();

        let panel = container(masked)
            .width(Length::Fill)
            .height(Length::Fill)
            // Straight edges stop here.
            .clip(true)
            .style(self.panel_style());

        let row_radius = self.effective_row_radius();
        let rtl = self.is_rtl();

        Element::new(TreeViewWidget {
            child: panel.into(),
            rows: nav_rows,
            realized,
            selected_flags: (0..self.items.len())
                .map(|i| self.is_selected(i, resolved_selected))
                .collect(),
            aria_label: self.aria_label,
            row_height: self.tree_row_height,
            row_radius,
            on_select,
            on_toggle,
            // Tree keeps the hook: Menu/Shift+F10 on the cursor row equals
            // right-click.
            row_on_context,
            id: self.id,
            // Stable address for row-focus restoration.
            id_prefix: self.id_prefix,
            scroll_id,
            row_ids: ids,
            row_keys: keys,
            active_descendant: self.active_descendant,
            row_reveals,
            rtl,
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzTreeView<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTreeView<'a, Message>) -> Self {
        w.into_element()
    }
}
