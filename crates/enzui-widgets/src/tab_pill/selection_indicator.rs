// The indicator wraps the row so cell bounds already include scrolling and clipping.

use iced::Shadow;
use iced::animation::Animation;
use iced::time::Instant;
use std::cell::Cell;
use std::rc::Rc;

/// Theme-aware presentation of the selected tab indicator.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TabSelectionStyle {
    /// Opaque accent fill with contrasting ink; the default.
    #[default]
    FilledAccent,
    /// A tinted pill: a translucent accent wash with no outline, its label in
    /// the accent's own ink, kept readable on the wash.
    /// Reduced transparency or increased contrast restores the filled accent.
    TranslucentAccent,
    /// Regular neutral backdrop material with theme ink.
    /// Unsupported rendering, nested materials, and accessibility reductions
    /// use an opaque neutral surface.
    NeutralMaterial,
}

fn resolve_selection_tokens(
    mut tokens: TabPillTokens,
    selection: TabSelectionStyle,
    theme: &iced::Theme,
) -> TabPillTokens {
    use enzui_core::material::{FallbackReason, MaterialCapability};
    let policy = crate::theme_lookup::material_policy(theme);
    let accessibility_fallback = policy.forbids_material()
        || matches!(
            enzui_core::material::frame_capability(),
            Some(MaterialCapability::Fallback(
                FallbackReason::ReducedTransparency | FallbackReason::IncreasedContrast
            ))
        );
    if selection == TabSelectionStyle::FilledAccent
        || selection == TabSelectionStyle::TranslucentAccent && accessibility_fallback
    {
        return tokens;
    }
    let palette = crate::theme_lookup::palette(theme);
    let fill = match selection {
        TabSelectionStyle::TranslucentAccent => {
            crate::color::with_alpha(palette.accent, if palette.is_dark() { 0.26 } else { 0.16 })
        }
        TabSelectionStyle::NeutralMaterial => crate::color::with_alpha(
            crate::color::mix(
                palette.background,
                palette.surface_elevated,
                palette.surface_elevated.a,
            ),
            1.0,
        ),
        TabSelectionStyle::FilledAccent => unreachable!(),
    };
    tokens.selected_bg = fill;
    tokens.segment_selected_bg = fill;
    tokens.segment_selected_hover_bg = fill;
    tokens.segment_selected_pressed_bg = fill;
    let ink = match selection {
        // Accent ink, adjusted until it reads on the wash over the surface.
        // On glass the surface is a faint wash: judge the wash over what it
        // actually shows (dark glass under light ink), or the accent ink was
        // darkened for a white backdrop it never had.
        // The real backdrop on glass varies with the wallpaper, so the ink
        // keeps a margin over the estimate (it measured 3.9:1 on dark glass).
        TabSelectionStyle::TranslucentAccent => enzui_theme::Palette::brand_text_on_at(
            crate::color::mix(
                palette.solid_backdrop(palette.surface),
                crate::color::with_alpha(fill, 1.0),
                fill.a,
            ),
            palette.accent,
            if palette.surface.a < 0.999 { 6.0 } else { 4.5 },
        ),
        _ => palette.text,
    };
    tokens.selected_fg = ink;
    tokens.segment_selected_fg = ink;
    tokens.selected_edge = match selection {
        // A tint marks the selection on its own; an outline reads as a web
        // form control.
        TabSelectionStyle::TranslucentAccent => iced::Color::TRANSPARENT,
        _ => palette.field_border_strong,
    };
    tokens
}

pub(crate) const SELECTION_INK_REST: f32 = 1.0;

// The layer publishes before child draw, keeping shared ink local to that clipped pass.
#[derive(Debug, Default)]
pub(crate) struct SelectionInk {
    style: Cell<TabSelectionStyle>,
    layer_painted: Cell<bool>,
    pass: Cell<InkPass>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InkPass {
    // Cells paint their own backgrounds and use their selected state.
    #[default]
    Rest,
    // Clipped to the thumb: every glyph pixel sits on the moving fill, so
    // every cell paints the selected ink for its own enabled state.
    OverThumb,
    // Clipped to a region the thumb does not cover: every glyph pixel sits on
    // the resting track, so every cell paints the unselected ink.
    OverTrack,
}

impl SelectionInk {
    pub(crate) fn publish_rest(&self) {
        self.layer_painted.set(false);
        self.pass.set(InkPass::Rest);
    }

    pub(crate) fn publish(&self, pass: InkPass) {
        self.layer_painted.set(true);
        self.pass.set(pass);
    }

    // Cell fills must not cover backgrounds already painted by the indicator layer.
    pub(crate) fn fill_alpha(&self) -> f32 {
        if self.layer_painted.get() {
            0.0
        } else {
            SELECTION_INK_REST
        }
    }

    pub(crate) fn pass(&self) -> InkPass {
        self.pass.get()
    }
}

pub(crate) type SelectionInkHandle = Rc<SelectionInk>;

#[inline]
pub(crate) fn selection_ink_handle() -> SelectionInkHandle {
    Rc::new(SelectionInk::default())
}

#[derive(Debug, Clone)]
pub(crate) struct CellInk {
    handle: SelectionInkHandle,
}

impl CellInk {
    pub(crate) fn new(handle: SelectionInkHandle) -> Self {
        Self { handle }
    }

    pub(crate) fn fill_alpha(&self) -> f32 {
        self.handle.fill_alpha()
    }

    pub(crate) fn pass(&self) -> InkPass {
        self.handle.pass()
    }
}

// The alpha multiplier a cell applies to its fill, treating an absent handle
// (a lone pill) as "paint normally".
#[inline]
pub(crate) fn selection_ink_alpha(ink: Option<&CellInk>) -> f32 {
    ink.map_or(SELECTION_INK_REST, CellInk::fill_alpha)
        .clamp(0.0, 1.0)
}

// The same multiplier read straight off the strip's handle, for the fills
// that belong to no single cell (the hairline seams).
#[inline]
pub(crate) fn strip_ink_alpha(ink: &SelectionInkHandle) -> f32 {
    ink.fill_alpha().clamp(0.0, 1.0)
}

#[derive(Clone)]
pub(crate) struct PillInkResolver {
    ink: Option<CellInk>,
    selected: bool,
    enabled: bool,
    segmented: bool,
    pinned: Option<TabPillTokens>,
    geo: Option<TabPillGeometry>,
}

impl PillInkResolver {
    pub(crate) fn color(&self, theme: &iced::Theme) -> Color {
        let t = resolve_selection_tokens(
            resolve_tokens_with(self.pinned, self.geo, theme),
            self.ink
                .as_ref()
                .map_or_else(TabSelectionStyle::default, |ink| ink.handle.style.get()),
            theme,
        );
        let pass = self.ink.as_ref().map_or(InkPass::Rest, CellInk::pass);
        pass_fg_role(pass, self.enabled, self.selected, self.segmented, &t).color(&t)
    }
}

// The animation state the [`IndicatorLayer`] keeps in its widget-tree slot.
#[derive(Debug)]
pub(crate) struct IndicatorState {
    // `false` = settled at source, `true` = heading to / settled at target.
    anim: Animation<bool>,
    // The selected index the last `diff` observed. `None` before the first.
    prev_selected: Option<usize>,
    // The index the current transition started from, so the thumb's fill and
    // corner radii can blend between the source and destination cells rather
    // than popping to the destination's shape on the first frame.
    from_index: Option<usize>,
    // Row-local source rectangle the thumb animates from.
    source_bounds: Option<Rectangle>,
    // Row-local rectangle the previous frame's selected cell occupied.
    // Written from `draw` (which sees `&self`), hence the `Cell`.
    last_settled_bounds: Cell<Option<Rectangle>>,
    // Armed by `diff`, consumed by the next `RedrawRequested` — the same
    // deferred-start rule `row_motion.rs::pending` documents.
    pending: bool,
    // The frame timestamp iced last handed us. `None` pre-first-frame.
    frame_at: Cell<Option<Instant>>,
    // Raw duration / reduced flag / easing latch, shared with every other
    // animated widget in the crate.
    motion: crate::motion::MotionLatch,
}

impl IndicatorState {
    pub(crate) fn new() -> Self {
        let raw = enzui_theme::MotionScale::default().normal_ms;
        let easing = enzui_theme::MotionEasing::default();
        Self {
            anim: crate::motion::build_anim(false, 0.0, easing),
            prev_selected: None,
            from_index: None,
            source_bounds: None,
            last_settled_bounds: Cell::new(None),
            pending: false,
            frame_at: Cell::new(None),
            motion: crate::motion::MotionLatch::new(raw, 0.0, easing),
        }
    }

    // Latch this frame's `RedrawRequested` timestamp. Called before anything
    // else reads progress on that frame, so a reversal latches against the
    // *current* frame rather than the previous one.
    #[inline]
    pub(crate) fn note_frame(&self, at: Instant) {
        self.frame_at.set(Some(at));
    }

    // Effective transition duration in ms after reduced motion is applied.
    // `0.0` until a theme has been observed (conservative, matches
    // `PanelMotion`).
    pub(crate) fn effective_ms(&self) -> f32 {
        self.motion.effective_ms(None)
    }

    // Interpolated, eased progress in `0.0..=1.0`.
    pub(crate) fn progress(&self) -> f32 {
        match self.frame_at.get() {
            Some(now) => self.anim.interpolate(0.0, 1.0, now).clamp(0.0, 1.0),
            None => f32::from(u8::from(self.anim.value())),
        }
    }

    // Whether the animation clock still asks for another frame.
    pub(crate) fn is_animating(&self) -> bool {
        self.frame_at
            .get()
            .is_some_and(|now| self.anim.is_animating(now))
    }

    // Whether the indicator needs a frame: armed by `diff` but not started
    // (`pending`), or still animating. With neither, an idle switcher leaves
    // the frame route.
    pub(crate) fn needs_frames(&self) -> bool {
        self.pending || self.is_animating()
    }

    // Arm a selection edge for the next painted frame.
    pub(crate) fn note_selection(&mut self, selected: Option<usize>) {
        if self.prev_selected != selected {
            self.from_index = self.prev_selected;
            self.prev_selected = selected;
            self.pending = true;
        }
    }

    // The index the in-flight transition started from.
    #[inline]
    pub(crate) fn origin_index(&self) -> Option<usize> {
        self.from_index
    }

    // Commit an armed edge at `now`, animating from `source`.
    pub(crate) fn commit_pending(&mut self, now: Instant, source: Option<Rectangle>) -> bool {
        if !self.pending {
            return false;
        }
        self.pending = false;
        self.source_bounds = source;
        let ms = self.effective_ms();
        // Rebuild from the source endpoint rather than reusing the previous
        // trajectory, so a rapid mid-flight reversal starts from the thumb's
        // actual on-screen position.
        self.anim = self.motion.build_animation(false, ms);
        self.anim.go_mut(true, now);
        true
    }

    // Record where the completed frame painted the selected cell.
    pub(crate) fn note_settled_bounds(&self, bounds: Rectangle) {
        self.last_settled_bounds.set(Some(bounds));
    }

    #[inline]
    pub(crate) fn last_settled_bounds(&self) -> Option<Rectangle> {
        self.last_settled_bounds.get()
    }

    // Observe the theme's motion policy. `draw` is the only lifecycle method
    // that carries a `&Theme`.
    pub(crate) fn observe_theme(&self, theme: &iced::Theme) {
        let motion = crate::theme_lookup::motion(theme);
        self.motion.observe(
            motion.normal_ms,
            motion.reduced_motion,
            motion.easing_or(None),
        );
    }

    // Track a policy change (reduced-motion toggle, theme swap) without
    // restarting an in-flight transition.
    pub(crate) fn sync_duration(&mut self, ms: f32) {
        self.motion.sync_animation(&mut self.anim, ms);
    }

    #[inline]
    pub(crate) fn source_bounds(&self) -> Option<Rectangle> {
        self.source_bounds
    }
}

// Linearly blend two rectangles by an already-eased `t`.
#[must_use]
pub(crate) fn lerp_rect(a: Rectangle, b: Rectangle, t: f32) -> Rectangle {
    let t = t.clamp(0.0, 1.0);
    Rectangle {
        x: a.x + (b.x - a.x) * t,
        y: a.y + (b.y - a.y) * t,
        width: a.width + (b.width - a.width) * t,
        height: a.height + (b.height - a.height) * t,
    }
}

// Blend corner radii to avoid a shape jump when entering a square interior segment.
#[must_use]
pub(crate) fn lerp_radius(
    a: iced::border::Radius,
    b: iced::border::Radius,
    t: f32,
) -> iced::border::Radius {
    let t = t.clamp(0.0, 1.0);
    let l = |x: f32, y: f32| x + (y - x) * t;
    iced::border::Radius {
        top_left: l(a.top_left, b.top_left),
        top_right: l(a.top_right, b.top_right),
        bottom_right: l(a.bottom_right, b.bottom_right),
        bottom_left: l(a.bottom_left, b.bottom_left),
    }
}

pub(crate) fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let l = |x: f32, y: f32| x + (y - x) * t;
    Color {
        r: l(a.r, b.r),
        g: l(a.g, b.g),
        b: l(a.b, b.b),
        a: l(a.a, b.a),
    }
}

// Over a moving thumb, disabled cells use the more legible of their two dimmed inks.
#[must_use]
fn pass_fg_role(
    pass: InkPass,
    enabled: bool,
    selected: bool,
    segmented: bool,
    tokens: &TabPillTokens,
) -> PillFg {
    let selected_ink = match pass {
        InkPass::Rest => selected,
        InkPass::OverThumb => true,
        InkPass::OverTrack => false,
    };
    if enabled || pass != InkPass::OverThumb {
        return pill_fg_role(enabled, selected_ink, segmented);
    }
    let fill = if segmented {
        tokens.segment_selected_bg
    } else {
        tokens.selected_bg
    };
    let legibility = |role: PillFg| {
        crate::accessibility_metrics::contrast_ratio_over(
            role.color(tokens),
            fill,
            tokens.segment_bg,
        )
    };
    let dimmed_selected = pill_fg_role(false, true, segmented);
    let inert = pill_fg_role(false, false, segmented);
    if legibility(dimmed_selected) >= legibility(inert) {
        dimmed_selected
    } else {
        inert
    }
}

// Disjoint regions preserve glyph antialiasing while each pass uses its backdrop ink.
#[must_use]
pub(crate) fn split_regions(
    outer: Rectangle,
    thumb: Rectangle,
) -> (Option<Rectangle>, Vec<Rectangle>) {
    if outer.width <= 0.0 || outer.height <= 0.0 {
        return (None, Vec::new());
    }
    let covered = outer
        .intersection(&thumb)
        .filter(|i| i.width > 0.0 && i.height > 0.0);
    let Some(i) = covered else {
        return (None, vec![outer]);
    };

    let mut rest = Vec::with_capacity(4);
    let mut push = |x: f32, y: f32, width: f32, height: f32| {
        if width > 0.0 && height > 0.0 {
            rest.push(Rectangle {
                x,
                y,
                width,
                height,
            });
        }
    };
    // Above and below span the full width; left and right only the covered
    // band's height. Four rectangles that tile `outer` minus `i`.
    push(outer.x, outer.y, outer.width, i.y - outer.y);
    let (i_bottom, o_bottom) = (i.y + i.height, outer.y + outer.height);
    push(outer.x, i_bottom, outer.width, o_bottom - i_bottom);
    push(outer.x, i.y, i.x - outer.x, i.height);
    let (i_right, o_right) = (i.x + i.width, outer.x + outer.width);
    push(i_right, i.y, o_right - i_right, i.height);

    (Some(i), rest)
}

// Paint the moving selection thumb.
pub(crate) fn paint_selection_thumb<R>(
    renderer: &mut R,
    bounds: Rectangle,
    fill: Color,
    radius: iced::border::Radius,
    edge: Color,
) where
    R: iced::advanced::Renderer,
{
    <R as iced::advanced::Renderer>::fill_quad(
        renderer,
        iced::advanced::renderer::Quad {
            bounds,
            border: Border {
                color: edge,
                width: if edge.a > 0.0 { 1.0 } else { 0.0 },
                radius,
            },
            shadow: Shadow::default(),
            // Subpixel placement avoids quantising selection motion to whole pixels.
            snap: false,
        },
        Background::Color(fill),
    );
}

// ---------------------------------------------------------------------------
// The layer widget
// ---------------------------------------------------------------------------

// Everything the layer needs to resolve a cell's resting fill colour at draw
// time, without re-deriving it from cell inspection.
pub(crate) struct IndicatorCells {
    // Row-layout children per cell (segmented rows interleave seams).
    pub(crate) cell_stride: usize,
    // Which fill token the preset paints.
    pub(crate) cell_fill: TabCellFill,
    pub(crate) joined_hover: bool,
    // Per-cell enabled flag — a disabled destination page must be reached by
    // a thumb painted in the *disabled* fill, not the accent one.
    pub(crate) enabled_flags: Vec<bool>,
    // Per-cell corner radii, index-aligned (`EnzTabList::cell_radii`), built
    // from `static_radius`; corners of that radius paint at the live
    // `pill_radius`, which follows the theme's `shape.button` step.
    pub(crate) radii: Vec<iced::border::Radius>,
    pub(crate) static_radius: f32,
}

/// `radius` with every corner of `from` turned into `to`: radii built at
/// view time from the static pill radius, painted at the theme's.
pub(crate) fn retarget_radius(
    radius: iced::border::Radius,
    from: f32,
    to: f32,
) -> iced::border::Radius {
    let map = |corner: f32| {
        if (corner - from).abs() < 0.01 {
            to
        } else {
            corner
        }
    };
    iced::border::Radius {
        top_left: map(radius.top_left),
        top_right: map(radius.top_right),
        bottom_right: map(radius.bottom_right),
        bottom_left: map(radius.bottom_left),
    }
}

// Pass-through wrapper around the switcher row that paints the moving
// selection indicator underneath it.
pub(crate) struct IndicatorLayer<'a, Message> {
    child: Element<'a, Message>,
    selected: usize,
    count: usize,
    cells: IndicatorCells,
    ink: SelectionInkHandle,
    pinned: Option<TabPillTokens>,
    geometry: Option<TabPillGeometry>,
}

impl<'a, Message> IndicatorLayer<'a, Message> {
    pub(crate) fn new(
        child: Element<'a, Message>,
        selected: usize,
        count: usize,
        cells: IndicatorCells,
        ink: SelectionInkHandle,
        pinned: Option<TabPillTokens>,
        geometry: Option<TabPillGeometry>,
    ) -> Self {
        Self {
            child,
            selected,
            count,
            cells,
            ink,
            pinned,
            geometry,
        }
    }

    fn paint_joined_hover(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        tokens: &TabPillTokens,
    ) {
        if (!self.cells.joined_hover && self.ink.style.get() != TabSelectionStyle::NeutralMaterial)
            || !cursor.position().is_some_and(|p| viewport.contains(p))
        {
            return;
        }
        let origin = layout.bounds().position();
        let cells = &tree.children[0].children;
        for i in 0..self.count {
            if i == self.selected || !self.cells.enabled_flags.get(i).copied().unwrap_or(false) {
                continue;
            }
            let Some(mut bounds) = self.cell_bounds(layout, cells, i) else {
                continue;
            };
            bounds.x += origin.x;
            bounds.y += origin.y;
            if !cursor.is_over(bounds) {
                continue;
            }
            if !self.cells.joined_hover {
                paint_selection_thumb(
                    renderer,
                    bounds,
                    tokens.hover_overlay,
                    self.cell_radius(i, tokens),
                    Color::TRANSPARENT,
                );
                break;
            }
            if let Some(selected) = self.cell_bounds(layout, cells, self.selected) {
                let radius = self.cell_radius(self.selected, tokens);
                let cap = (selected.width / 2.0).min(selected.height / 2.0);
                if i == self.selected.saturating_add(1) {
                    let overlap = radius.top_right.max(radius.bottom_right).min(cap);
                    bounds.x -= overlap;
                    bounds.width += overlap;
                } else if i.saturating_add(1) == self.selected {
                    bounds.width += radius.top_left.max(radius.bottom_left).min(cap);
                }
            }
            let radius = iced::border::Radius {
                top_left: if i == 0 { tokens.pill_radius } else { 0.0 },
                bottom_left: if i == 0 { tokens.pill_radius } else { 0.0 },
                top_right: if i + 1 == self.count {
                    tokens.pill_radius
                } else {
                    0.0
                },
                bottom_right: if i + 1 == self.count {
                    tokens.pill_radius
                } else {
                    0.0
                },
            };
            // The selected pill draws above this wash, exposing only its corner cutouts.
            paint_selection_thumb(
                renderer,
                bounds,
                tokens.hover_overlay,
                radius,
                Color::TRANSPARENT,
            );
            break;
        }
    }

    fn paint_indicator(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        viewport: &Rectangle,
        quad: renderer::Quad,
        fill: Color,
        enabled: bool,
    ) -> Option<enzui_core::material::MaterialScope> {
        use enzui_core::material::{
            self, MaterialCapability, MaterialDecision, MaterialGeometry, MaterialRect,
            MaterialRegionRequest,
        };
        use iced::advanced::Renderer as _;
        if self.ink.style.get() == TabSelectionStyle::NeutralMaterial && enabled {
            let policy = crate::theme_lookup::material_policy(theme);
            let requested = policy.settle(enzui_theme::MaterialStyle::Regular.tokens());
            let support = match renderer.material_support() {
                iced::advanced::material::Support::Optical { max_blur_sigma } => {
                    MaterialCapability::Optical { max_blur_sigma }
                }
                _ => MaterialCapability::Fallback(material::FallbackReason::RendererUnsupported),
            };
            let capability =
                material::under_policy(material::frame_capability().unwrap_or(support), policy);
            let bounds = quad.bounds;
            let radius = quad.border.radius;
            let radii = [
                radius.top_left,
                radius.top_right,
                radius.bottom_right,
                radius.bottom_left,
            ];
            let rect = |b: Rectangle| MaterialRect::new(b.x, b.y, b.width, b.height);
            if let MaterialDecision::Optical(tokens) = material::decide_with(capability, &requested)
                && let Ok(scope) = material::record_with(
                    capability,
                    MaterialRegionRequest {
                        geometry: MaterialGeometry::new(rect(bounds), radii),
                        clip: rect(*viewport),
                        tokens: requested,
                        tint: [fill.r, fill.g, fill.b],
                    },
                )
            {
                renderer.start_layer_material(bounds, tokens.to_material(radii, fill));
                renderer.fill_quad(quad, Background::Color(Color::TRANSPARENT));
                renderer.end_layer();
                return Some(scope);
            }
        }
        renderer.fill_quad(quad, Background::Color(fill));
        None
    }

    fn cell_bounds(&self, layout: Layout, cells: &[Tree], index: usize) -> Option<Rectangle> {
        let cell = layout.children(cells).nth(index * self.cells.cell_stride)?;
        let b = cell.bounds();
        let origin = layout.bounds().position();
        Some(Rectangle {
            x: b.x - origin.x,
            y: b.y - origin.y,
            width: b.width,
            height: b.height,
        })
    }

    // Row-local bounds of the hairline seam that precedes cell `index`, in a
    // segmented strip. `None` in pill mode (no seams) and for cell 0.
    fn seam_bounds(&self, layout: Layout, cells: &[Tree], index: usize) -> Option<Rectangle> {
        if self.cells.cell_stride < 2 || index == 0 {
            return None;
        }
        let seam = layout
            .children(cells)
            .nth(index * self.cells.cell_stride - 1)?;
        let b = seam.bounds();
        let origin = layout.bounds().position();
        Some(Rectangle {
            x: b.x - origin.x,
            y: b.y - origin.y,
            width: b.width,
            height: b.height,
        })
    }

    // The fill a cell shows once the moving thumb has left it — its resting
    // unselected paint. While a transition runs the cells paint nothing,
    // so this is what the layer draws under the thumb; without it the source
    // cell would vacate to a hole in the joined block instead of to the track.
    //
    // `None` means "paint nothing": an enabled pill-mode cell has no resting
    // fill of its own (its background is `None` unless selected).
    fn cell_track_color(&self, index: usize, tokens: &TabPillTokens) -> Option<Color> {
        let enabled = self.cells.enabled_flags.get(index).copied().unwrap_or(true);
        match (self.cells.cell_fill, enabled) {
            (TabCellFill::Segmented, true) => Some(tokens.segment_bg),
            (TabCellFill::Segmented | TabCellFill::Pill, false) => Some(tokens.disabled_bg),
            (TabCellFill::Pill, true) => None,
        }
    }

    // The resting fill a cell paints for itself, which is therefore what the
    // thumb must be painted in when it is over that cell.
    fn cell_fill_color(&self, index: usize, tokens: &TabPillTokens) -> Color {
        let enabled = self.cells.enabled_flags.get(index).copied().unwrap_or(true);
        match (self.cells.cell_fill, enabled) {
            (TabCellFill::Segmented, true) => tokens.segment_selected_bg,
            (TabCellFill::Segmented, false) => crate::color::with_opacity(
                tokens.segment_selected_bg,
                tokens.segment_disabled_opacity(),
            ),
            (TabCellFill::Pill, true) => tokens.selected_bg,
            (TabCellFill::Pill, false) => tokens.disabled_bg,
        }
    }

    fn cell_radius(&self, index: usize, tokens: &TabPillTokens) -> iced::border::Radius {
        self.cells.radii.get(index).map_or_else(
            || tokens.pill_radius.into(),
            |radius| retarget_radius(*radius, self.cells.static_radius, tokens.pill_radius),
        )
    }

    // Advance the indicator's own clock for a presented frame.
    //
    // The *only* thing this widget does at redraw time. Called from both
    // [`Widget::update_frame`] (the frame route) and [`Widget::update`]
    // (the fallback path an ancestor that still uses the default
    // `update_frame` produces). Idempotent within one frame: `note_frame`
    // re-latches the same instant, `commit_pending` clears its own flag, and
    // a repeated `request_redraw` coalesces.
    //
    // Returns whether the event was a frame this widget consumed, which is
    // what the routing tests assert against.
    fn note_frame_event(
        &self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        shell: &mut Shell<'_, Message>,
    ) -> bool {
        let Event::Window(iced::window::Event::RedrawRequested(at)) = event else {
            return false;
        };
        // Latch the frame instant first: the reversal source must be the
        // thumb's position on *this* frame, not the previous one, or a
        // mid-flight reversal visibly backtracks by one frame's travel.
        {
            let state = tree.state.downcast_ref::<IndicatorState>();
            state.note_frame(*at);
        }
        let source = {
            let state = tree.state.downcast_ref::<IndicatorState>();
            self.live_bounds(state, layout, &tree.children[0].children)
        };
        let state = tree.state.downcast_mut::<IndicatorState>();
        let effective_ms = state.effective_ms();
        state.sync_duration(effective_ms);
        let _ = state.commit_pending(*at, source);
        if state.is_animating() {
            shell.request_redraw();
        }
        true
    }

    // The thumb's current row-local rectangle, used as the source seed for a
    // pending edge so a rapid reversal starts from where the thumb is.
    fn live_bounds(
        &self,
        state: &IndicatorState,
        layout: Layout,
        cells: &[Tree],
    ) -> Option<Rectangle> {
        let dest = state.last_settled_bounds();
        let src = state.source_bounds();
        if let (Some(src), Some(dest)) = (src, dest) {
            return Some(lerp_rect(src, dest, state.progress()));
        }
        dest.or(src)
            .or_else(|| self.cell_bounds(layout, cells, self.selected))
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for IndicatorLayer<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<IndicatorState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(IndicatorState::new())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Edge-detect the selection. `diff` runs during
        // `UserInterface::build`, outside the frame loop and with no frame
        // instant, so the transition only *starts* on the next
        // `RedrawRequested`.
        let sel = (self.selected < self.count).then_some(self.selected);
        tree.state
            .downcast_mut::<IndicatorState>()
            .note_selection(sel);
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    // On the frame route only while the indicator or something below it
    // needs a frame. A transition armed in `diff` joins in the same build, and
    // the branch leaves on the frame it settles. Child slots not yet diffed
    // count as interested.
    fn frame_interest(&self, tree: &Tree) -> bool {
        let own = tree.state.downcast_ref::<IndicatorState>().needs_frames();
        own || tree.children.first().is_none_or(Tree::has_frame_interest)
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
        // A `RedrawRequested` reaches `update` only when some ANCESTOR still
        // uses the default `update_frame` (which calls `update`). Handling it
        // here as well keeps such a strip animating; the work is idempotent
        // per frame, so a widget whose ancestors do participate pays nothing
        // and double-delivery cannot double-commit (`commit_pending` clears
        // its own flag, and `note_frame` re-latches the same instant).
        self.note_frame_event(tree, event, layout, shell);

        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        // The child may have STARTED an animation in response to this very
        // event (a click that begins a badge pulse inside a cell). Publish its
        // new interest, then reconcile the route index, or the frame it just
        // asked for would be delivered to nobody.
        tree.children[0].refresh_frame_interest(self.child.as_widget());
        tree.refresh_frame_children();
    }

    // Handles only the indicator's own redraw state, then forwards to the
    // children on the route and refreshes their interest, rather than
    // falling back to `update` across the whole row.
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.note_frame_event(tree, event, layout, shell);

        if tree.children[0].has_frame_interest() {
            self.child.as_widget_mut().update_frame(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            tree.children[0].refresh_frame_interest(self.child.as_widget());
            tree.refresh_frame_children();
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
        let state = tree.state.downcast_ref::<IndicatorState>();
        state.observe_theme(theme);
        let tokens = resolve_selection_tokens(
            resolve_tokens_with(self.pinned, self.geometry, theme),
            self.ink.style.get(),
            theme,
        );

        let animating = state.is_animating();
        let dest = self.cell_bounds(layout, &tree.children[0].children, self.selected);

        let draw_child = |renderer: &mut iced::Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        };

        let material_at_rest = self.ink.style.get() == TabSelectionStyle::NeutralMaterial;
        let from_bounds = if animating {
            state.source_bounds()
        } else {
            dest
        };
        let Some((dest, src)) = (animating || material_at_rest)
            .then(|| dest.zip(from_bounds))
            .flatten()
        else {
            self.ink.publish_rest();
            self.paint_joined_hover(tree, renderer, layout, cursor, viewport, &tokens);
            draw_child(renderer);
            if let Some(dest) = dest {
                state.note_settled_bounds(dest);
            }
            return;
        };

        let progress = if animating { state.progress() } else { 1.0 };
        let local = lerp_rect(src, dest, progress);

        let from = if animating {
            state.origin_index().unwrap_or(self.selected)
        } else {
            self.selected
        };
        let thumb_fill = lerp_color(
            self.cell_fill_color(from, &tokens),
            self.cell_fill_color(self.selected, &tokens),
            progress,
        );

        let origin = layout.bounds().position();
        let to_screen = |r: Rectangle| Rectangle {
            x: r.x + origin.x,
            y: r.y + origin.y,
            width: r.width,
            height: r.height,
        };

        // Track fills stay below the thumb and retain each cell's disabled paint and radii.
        for i in 0..self.count {
            let (Some(bounds), Some(fill)) = (
                self.cell_bounds(layout, &tree.children[0].children, i),
                self.cell_track_color(i, &tokens),
            ) else {
                continue;
            };
            paint_selection_thumb(
                renderer,
                to_screen(bounds),
                fill,
                self.cell_radius(i, &tokens),
                Color::TRANSPARENT,
            );
            if let Some(seam) = self.seam_bounds(layout, &tree.children[0].children, i) {
                // Overlap fractional edges so seam antialiasing cannot expose the backdrop.
                paint_selection_thumb(
                    renderer,
                    to_screen(Rectangle {
                        x: seam.x - 0.5,
                        width: seam.width + 1.0,
                        ..seam
                    }),
                    tokens.segment_bg,
                    iced::border::Radius::default(),
                    Color::TRANSPARENT,
                );
                if i != from && i != from + 1 && i != self.selected && i != self.selected + 1 {
                    let inset = Rectangle {
                        y: seam.y + seam.height / 4.0,
                        height: seam.height / 2.0,
                        ..seam
                    };
                    paint_selection_thumb(
                        renderer,
                        to_screen(inset),
                        tokens.seam,
                        iced::border::Radius::default(),
                        Color::TRANSPARENT,
                    );
                }
            }
        }

        let radius = lerp_radius(
            self.cell_radius(from, &tokens),
            self.cell_radius(self.selected, &tokens),
            progress,
        );
        let thumb = to_screen(local);
        if !animating {
            self.paint_joined_hover(tree, renderer, layout, cursor, viewport, &tokens);
        }
        let scope = self.paint_indicator(
            renderer,
            theme,
            viewport,
            renderer::Quad {
                bounds: thumb,
                border: Border {
                    color: if matches!(self.cells.cell_fill, TabCellFill::Segmented) {
                        Color::TRANSPARENT
                    } else {
                        tokens.selected_edge
                    },
                    width: if matches!(self.cells.cell_fill, TabCellFill::Pill)
                        && tokens.selected_edge.a > 0.0
                    {
                        1.0
                    } else {
                        0.0
                    },
                    radius,
                },
                shadow: Shadow::default(),
                snap: false,
            },
            thumb_fill,
            self.cells.enabled_flags.get(from).copied().unwrap_or(true)
                && self
                    .cells
                    .enabled_flags
                    .get(self.selected)
                    .copied()
                    .unwrap_or(true),
        );

        // Include the viewport so overhanging rings and badges are painted exactly once.
        use iced::advanced::Renderer as _;
        let outer = layout.bounds().union(viewport);
        let (covered, uncovered) = split_regions(outer, thumb);
        for region in uncovered {
            renderer.with_layer(region, |renderer| {
                self.ink.publish(InkPass::OverTrack);
                draw_child(renderer);
            });
        }
        if let Some(covered) = covered {
            renderer.with_layer(covered, |renderer| {
                self.ink.publish(InkPass::OverThumb);
                draw_child(renderer);
            });
        }
        // Overlays drawn after the row must see resting ink rather than the last clipped pass.
        self.ink.publish_rest();
        // Foreground remains material-owned so it cannot enter a later backdrop sample.
        drop(scope);

        state.note_settled_bounds(dest);
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
