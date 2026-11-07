//! [`HitTargetScanProbe`] / [`ContrastScanProbe`] — transparent wrappers that
//! publish accessibility-check relationships into an
//! [`AccessibilityScanCollector`](enzui_core::accessibility_scan::AccessibilityScanCollector).
//!
//! Native-integration half of the `EnzHitTargetProbe` / `EnzContrastProbe` port
//! (the [`crate::accessibility_metrics`] module owns the WCAG math). Mirrors
//! [`crate::layout_spill_probe::LayoutSpillProbe`]: a wrapper that changes
//! neither layout nor visuals but, in `operate()`, hands the collector the
//! child's real post-layout bounds (hit-target size) or the resolved
//! foreground/background paint (contrast) so the collector can surface an
//! [`AccessibilityDefect`](enzui_core::accessibility_scan::AccessibilityDefect).
//!
//! ## Contrast paint is resolved late, against the live theme
//!
//! [`ContrastScanProbe`] does not take two `Color`s at view-construction
//! time. It takes a resolver closure, `Fn(&Theme) -> (Color, Color)`, and runs
//! it inside `draw()` against the same runtime `Theme` the child is being
//! painted with; `operate()` then publishes whatever the most recent draw pass
//! resolved. That is the pin-vs-token distinction applied to auditing: a
//! construction-time `Color` is frozen at the theme that happened to be active
//! when the view function ran, so it can disagree with what is on screen, and
//! an audit that reports guesses produces confident false-clean results.
//!
//! Because the pair only exists after a draw, a probe that has never been drawn
//! publishes `resolved: false`, and the collector records
//! [`AccessibilityDefectKind::UnresolvedPaint`](enzui_core::accessibility_scan::AccessibilityDefectKind::UnresolvedPaint)
//! rather than passing it.
//!
//! ### Interaction state is audited via a status-aware resolver
//!
//! The plain [`ContrastScanProbe::new`] resolver sees the runtime theme
//! only, so it audits the resting appearance: a control whose resting colours
//! clear 4.5:1 but whose hover fill does not would still report clean.
//!
//! [`ContrastScanProbe::with_status`] / [`ContrastScanProbe::all_statuses`]
//! close it. The resolver signature is `Fn(&Theme, InteractionStatus) -> (Color,
//! Color)`, and the probe drives it once per audited
//! [`InteractionStatus`] (resting, hover,
//! pressed, disabled) inside `draw()`, publishing one
//! [`AccessibilityCheck::Contrast`] node per status. The collector then produces one
//! finding per failing (widget, status) pair, each tagged with its status,
//! so a report says "fails on hover" rather than only a worst-case ratio.
//!
//! The shared-cell channel makes this status-aware style protocol possible:
//! the probe owns an
//! `Rc<Cell<InteractionStatus>>` ([`ContrastScanProbe::status_cell`]) that it writes
//! before each resolve, so a resolver closed over the same cell reads the exact
//! state being audited rather than a parallel derivation of it — the same
//! discipline `window_control_button.rs` uses to keep an icon's tint and a
//! button's fill resolving the identical `FocusStatus`.
//!
//! ### Consuming the production resolver directly
//!
//! A caller-supplied resolver is only a reproduction of the child's
//! per-status style logic. If it diverges from the widget's real style closure
//! the audit reads the reproduction, not the paint — a real widget could paint
//! a failing hover fill while auditing clean.
//!
//! Framework widgets close this by consuming the exact production style
//! resolver used for painting, rather than a parallel closure. See
//! [`EnzButton::audit_contrast`](crate::button::EnzButton::audit_contrast): the
//! button's paint path (`style_focus`) and its audit resolver both call the same
//! `style_for(kind, status, enabled, ..)` over the same per-status token source,
//! so there is a single resolver with two consumers. Changing the button's real
//! hover/pressed fill changes both the pixels and the audited pair.
//!
//! What genuinely remains is auditing arbitrary third-party paint that does
//! not route through one of the framework's style resolvers: reading back a child's actual
//! emitted quad/text colours needs a renderer-level hook, which does not exist. For
//! the framework's own widgets the shared-resolver path above is the truthful one.

use crate::accessibility_metrics::{DEFAULT_MIN_HIT_TARGET, contrast_probe_with, hit_target_probe};
use enzui_core::accessibility_scan::{
    AccessibilityCheck, AccessibilityProbeNode, InteractionStatus,
};
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};

use crate::accessibility_metrics::ContrastMode;

/// Hit-target adapter pairing [`hit_target_probe`] with the collector:
/// `Some(deficit)` when undersized, else `None`.
pub fn hit_target_deficit(width: f32, height: f32, min_size: f32, allow: bool) -> Option<f32> {
    let p = hit_target_probe(width, height, min_size, allow);
    p.undersized.then_some(p.deficit_amount)
}

/// Contrast adapter pairing [`contrast_probe_with`] with the collector:
/// `Some(ratio)` when below `min_ratio`, else `None`.
pub fn contrast_deficit(fg: Color, bg: Color, min_ratio: f32, allow: bool) -> Option<f32> {
    let p = contrast_probe_with(fg, bg, min_ratio, allow);
    p.low_contrast.then_some(p.contrast_ratio)
}

/// Elision adapter for the accessibility scan collector:
/// returns `Some(clamped_by)` when text was truncated, else `None`.
///
/// This is the trivial "length delta" predicate
/// (`full_len - displayed_len` when positive). It exists as a
/// standalone function so `update.rs` can pin it as a `const &ElisionFn`
/// alongside [`hit_target_deficit`] and [`contrast_deficit`], keeping
/// the runtime scan route free of closures that would need capture.
pub fn elision_clamp(full_len: u32, displayed_len: u32, allow: bool) -> Option<u32> {
    if allow {
        return None;
    }
    let clamped_by = full_len.saturating_sub(displayed_len);
    (clamped_by > 0).then_some(clamped_by)
}

/// A transparent wrapper that publishes a hit-target [`AccessibilityProbeNode`].
#[must_use]
pub struct HitTargetScanProbe<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    min_size: f32,
    allow_small: bool,
    hidden: bool,
    id: Option<iced::advanced::widget::Id>,
}

impl<'a, Message, Theme, Renderer> HitTargetScanProbe<'a, Message, Theme, Renderer> {
    /// Wrap `child` in a hit-target scan probe (default min 24 px).
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
            min_size: DEFAULT_MIN_HIT_TARGET,
            allow_small: false,
            hidden: false,
            id: None,
        }
    }

    /// Override the comfortable minimum size (px).
    ///
    /// Normalized to finite non-negative: `0` is expressible intent
    /// meaning "no hit-target minimum", and is honoured literally —
    /// [`hit_target_probe`] already computes `(0 - measured).max(0.0) == 0`
    /// for every size, so a zero threshold audits everything clean.
    /// Substituting the 24 px default for a caller's `0` would quietly
    /// re-arm an audit the caller disarmed, the same reversal as reading a
    /// `0` cap as "unbounded". A `0` threshold is a
    /// blunter instrument than [`allow_small`](Self::allow_small), which is
    /// per-widget; prefer that.
    ///
    /// `NaN` / `±inf` / negative name no threshold at all and still fall back
    /// to 24.
    pub fn min_size(mut self, px: f32) -> Self {
        self.min_size = enzui_core::metrics::FiniteNonNeg::new_or(px, 24.0).get();
        self
    }

    /// Mark an intentionally-small affordance (never flagged).
    pub fn allow_small(mut self, allow: bool) -> Self {
        self.allow_small = allow;
        self
    }

    /// Declare that the wrapped control is not presented — collapsed away,
    /// behind an inactive tab, or otherwise off screen — so its geometry carries
    /// no accessibility meaning and is skipped.
    ///
    /// This is the only way to exempt a zero-sized control. The measurement
    /// itself no longer implies it: a `0x0` rendered size is a collapsed
    /// target, which is the worst possible hit target and is reported with a
    /// maximal deficit.
    pub fn hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }

    /// Attach a stable finding id.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }
}

/// Convenience constructor.
pub fn hit_target_scan_probe<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> HitTargetScanProbe<'a, Message, Theme, Renderer> {
    HitTargetScanProbe::new(child)
}

/// Convenience constructor for [`ContrastScanProbe`] — see its docs for why the
/// pair is a resolver rather than two `Color`s.
pub fn contrast_scan_probe<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    mode: ContrastMode,
    resolve: impl Fn(&Theme) -> (Color, Color) + 'a,
) -> ContrastScanProbe<'a, Message, Theme, Renderer> {
    ContrastScanProbe::new(child, mode, resolve)
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for HitTargetScanProbe<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    // Shares the child's tree node, so its frame interest is the child's.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut node = AccessibilityProbeNode {
            bounds: layout.bounds(),
            kind: AccessibilityCheck::HitTarget {
                min_size: self.min_size,
                allow: self.allow_small,
                hidden: self.hidden,
            },
        };
        operation.custom(self.id.as_ref(), layout.bounds(), &mut node);
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<HitTargetScanProbe<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: HitTargetScanProbe<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

/// A transparent wrapper that publishes a text-elision [`AccessibilityProbeNode`] for
/// its child, so the crate's a11y audit can observe an over-long label
/// being truncated. It is [`elision_clamp`]'s live consumer; without it the
/// audit would report `clean` even when a label was visibly sliced.
///
/// # What it observes
///
/// At [`Widget::layout`] time the probe shapes `full_label` against the same
/// text-shaping code that iced paints with
/// (`iced::advanced::graphics::text::Paragraph`) under infinite bounds —
/// with `Ellipsis::None`, so ellipsize never activates, and
/// `Wrapping::None`, so `min_bounds().width` returns the natural single-line
/// width the label would occupy if nothing constrained it. It stashes that
/// number alongside the child's assigned width (`layout.bounds().width`) in a
/// [`Cell`](std::cell::Cell), and `operate()` publishes both.
///
/// A caller-supplied `space_reserved` is subtracted from the child's assigned
/// width before comparison, so a probe wrapping a whole button (label,
/// icon, padding and gap) can still audit only the label's share of the box.
/// For a probe wrapping only the label element, pass `0.0`.
///
/// # Length units
///
/// The `AccessibilityCheck::Elision` node's `full_len` / `displayed_len` are
/// scalar units the caller chooses; the collector treats them as opaque and
/// only checks `displayed_len < full_len`. This probe uses character counts:
/// `full_len = label.chars().count()`, and `displayed_len` is proportionally
/// scaled to the assigned/natural width ratio (rounded down, clamped to at
/// most `full_len`). That gives a meaningful `clamped_by` (≈ how many
/// characters were hidden by ellipsize) without pretending to know the exact
/// glyph the ellipsis cut.
///
/// # Why not just wrap the label text widget
///
/// A probe wrapping the label alone would be simpler, but the label's
/// assigned width is decided by the button's own layout algorithm and it
/// is what the audit needs to compare against. Wrapping the label directly
/// with `space_reserved = 0.0` is exactly the intended shape for that
/// arrangement; the more general `space_reserved` argument exists for wrapping
/// composite content (icon + label) whose natural width the audit still wants
/// to reduce to the label's share.
#[must_use]
pub struct ElisionScanProbe<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    full_label: String,
    text_size: f32,
    font: iced::Font,
    shaping: crate::enz_text::TextShaping,
    allow: bool,
    id: Option<iced::advanced::widget::Id>,
    /// Portion of the child's assigned width that is not label — the
    /// padding, icon column and gap for a probe wrapping the whole button.
    /// `0.0` when the child is the label element itself.
    space_reserved: f32,
    /// The observation cache written in `layout()` and read in `operate()`.
    /// Interior-mutable because `Widget::operate` takes `&mut self` but
    /// [`Widget::layout`] runs first and is the only place a renderer is
    /// available.
    observed: std::cell::Cell<Option<Observed>>,
    // Measured once: label, size, font and shaping are fixed per instance.
    natural_width: std::cell::Cell<Option<f32>>,
}

// The two numbers the probe records in `layout()` and reads in
// `operate()`.
//
// * `assigned_width` — the child's assigned width from `layout.bounds()`.
// * `natural_width` — the full label's natural single-line width, shaped
//   under infinite bounds with no ellipsize.
#[derive(Debug, Clone, Copy)]
struct Observed {
    assigned_width: f32,
    natural_width: f32,
}

impl<'a, Message, Theme, Renderer> ElisionScanProbe<'a, Message, Theme, Renderer> {
    /// Wrap `child` in an elision scan probe.
    ///
    /// `full_label` is the complete, un-elided text the child paints; the
    /// probe compares its natural width against the child's assigned width.
    /// `text_size` and `font` must match what the child renders with, or
    /// the natural-width shape disagrees with the paint and the audit
    /// speaks about a different label.
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        full_label: impl Into<String>,
        text_size: f32,
        font: iced::Font,
    ) -> Self {
        let full_label = full_label.into();
        // This is the single construction seam for this type (there are no
        // struct-literal constructions of `ElisionScanProbe` in the crate), so
        // noting here sees every probe that is really built. No-op unless a
        // test switched recording on.
        ledger_note(&full_label);
        Self {
            child: child.into(),
            full_label,
            text_size,
            font,
            shaping: crate::enz_text::TextShaping::Advanced,
            allow: false,
            id: None,
            space_reserved: 0.0,
            observed: std::cell::Cell::new(None),
            natural_width: std::cell::Cell::new(None),
        }
    }

    /// Match the shaping policy used by the probed label.
    ///
    /// Defaults to Advanced; forward the label's policy when it differs.
    pub fn shaping(mut self, shaping: impl Into<crate::enz_text::TextShaping>) -> Self {
        self.shaping = shaping.into();
        self
    }

    /// Reserve `px` of the child's assigned width for non-label content
    /// (icon + gap + padding), so only the label's share is compared against
    /// the natural width. Pass `0.0` when wrapping the label element itself.
    pub fn space_reserved(mut self, px: f32) -> Self {
        // Negative / NaN reservations are meaningless — clamp to zero.
        self.space_reserved = if px.is_finite() && px > 0.0 { px } else { 0.0 };
        self
    }

    /// Mark this control's elision as intentional (never flagged). E.g. a
    /// deliberately-capped preview line.
    pub fn allow(mut self, allow: bool) -> Self {
        self.allow = allow;
        self
    }

    /// Attach a stable finding id.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    // The natural (un-elided, single-line) width of `full_label` at
    // `text_size` in `font`, shaped against the process-wide font
    // system.
    //
    // Same shape as [`crate::chart_geometry::label_width`] — the chart
    // module uses this exact call for the same reason: `min_bounds()`
    // returns the widest laid-out run's width, and with `bounds =
    // INFINITY` + `Wrapping::None` + `Ellipsis::None` there is exactly
    // one run and it is the natural width.
    fn natural_width(&self) -> f32 {
        if let Some(width) = self.natural_width.get() {
            return width;
        }
        let width = natural_widths::get_or_measure(
            &self.full_label,
            self.text_size,
            self.font,
            self.shaping,
            || self.measure_natural_width(),
        );
        self.natural_width.set(Some(width));
        width
    }

    fn measure_natural_width(&self) -> f32 {
        use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
        use iced::advanced::text::Paragraph;
        GraphicsParagraph::with_text(iced::advanced::text::Text {
            content: self.full_label.as_str(),
            bounds: Size::new(f32::INFINITY, self.text_size.max(1.0)),
            size: self.text_size.into(),
            line_height: iced::widget::text::LineHeight::default(),
            font: self.font,
            align_x: iced::advanced::text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: self.shaping.into(),
            wrapping: iced::widget::text::Wrapping::None,
            ellipsis: iced::widget::text::Ellipsis::None,
            hint_factor: None,
        })
        .min_bounds()
        .width
    }

    // Compute the `(full_len, displayed_len)` character counts the probe
    // publishes into an [`AccessibilityCheck::Elision`] node from an
    // [`Observed`].
    //
    // Split out as an associated function so unit tests can hit it
    // without a live renderer; production drives it from `layout()`.
    fn resolve_lengths(&self, obs: Observed) -> (u32, u32) {
        let full_len = self.full_label.chars().count() as u32;
        if full_len == 0 {
            return (0, 0);
        }
        let available = (obs.assigned_width - self.space_reserved).max(0.0);
        let overflow = obs.natural_width - available;
        // Sub-pixel calibration. `natural_width` is re-shaped by the
        // probe under infinite bounds + `Ellipsis::None`; `assigned_width`
        // is the child's laid-out `min_bounds`, shaped under finite bounds
        // (`limits.max()`) + the child's own `Ellipsis::End`/right-align
        // (see `core::widget::text::layout`). For a label that paints
        // completely but slightly exceeds the cell it was allotted (the
        // paint overflows into the cell's padding/clip slack and stays
        // fully visible), their difference is a sub-pixel layout
        // artifact. Measured on real app geometry: `52%`
        // (natural 23.530, allotted 22.997 at 13 px) overflows by 0.533
        // px == 0.041*size yet renders whole; a synthetic 300 px pair
        // row puts the same class of overflow at 0.010..0.041*size
        // depending on fractional distribution. Flooring every positive
        // overflow to `clamped_by >= 1` would print that sub-pixel slack
        // as a finding on the `52%` tile.
        //
        // The probe does not ask the paint path directly (route 1):
        // the child is an opaque `Element` whose painted `Paragraph`
        // lives in that element's own tree state, unreachable from the
        // probe, and re-shaping the label with `Ellipsis::End` at the
        // assigned bounds truncates at 0.01 px of overflow (measured) —
        // strictly more sensitive than today and no fix. So the probe
        // keeps the width prediction and gates it with a measured
        // epsilon.
        //
        // Epsilon = 10% of the font size. That sits ~2.4x above the worst
        // measured false-positive slack (0.041*size on the real `52%` tile) and
        // ~2.2x below the narrowest single-character advance (apostrophe
        // 0.219*size, measured) — near the geometric centre of the two bounds,
        // so it is robust to per-font/per-layout variation on both sides. A
        // genuine one-character clamp (overflow >= one glyph advance >=
        // 0.219*size) is above it and still reports; the near-fit floor below
        // still fires for any overflow the epsilon admits.
        let epsilon = 0.10 * self.text_size.max(0.0);
        if obs.natural_width <= 0.0 || overflow <= epsilon {
            // Fits, or the overflow is within the calibrated measurement band —
            // no clamp.
            return (full_len, full_len);
        }
        // Overflow. Proportionally scale by width ratio, so `clamped_by` is a
        // meaningful "about this many characters were hidden". Floor + clamp
        // guarantees `displayed_len < full_len` when overflow is real, so the
        // collector produces a finding.
        let ratio = (available / obs.natural_width).clamp(0.0, 1.0);
        let displayed = ((full_len as f32) * ratio).floor() as u32;
        // The overflow floor: once past the epsilon the overflow is
        // real, and the probe deliberately never reports it clean.
        // `floor()` above already yields `displayed < full_len` whenever
        // `ratio < 1` (which holds here, since `overflow > epsilon > 0`
        // forces `available < natural_width`); this `.min` is the
        // belt-and-suspenders backstop for the degenerate `available /
        // natural_width == 1.0f32` fp edge. The two are mutually
        // redundant — either alone guarantees `displayed < full_len` —
        // so removing them together (not just this line) is what
        // reddens the one-character boundary gate; see the sabotage
        // notes.
        let displayed = displayed.min(full_len.saturating_sub(1));
        (full_len, displayed)
    }
}

/// Convenience constructor.
pub fn elision_scan_probe<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    full_label: impl Into<String>,
    text_size: f32,
    font: iced::Font,
) -> ElisionScanProbe<'a, Message, Theme, Renderer> {
    ElisionScanProbe::new(child, full_label, text_size, font)
}

// Construction ledger — a runtime record of which labels were wrapped
// in an elision probe.
//
// ## Why this exists, and why it is not a source grep
//
// The auto-enable mechanism (`enz_text::single_line_elided` → `audit_elision`)
// covers every widget that routes its label through `enz_text`. That is an
// argument, not a test: if a widget is ever refactored to render its label
// through something else — a raw `iced::widget::text`, a bespoke shaper, a
// hand-rolled `Ellipsis::End` — it quietly drops out of the audit and every
// existing test stays green, because those tests only assert the behaviour
// of widgets they name. That is precisely the island failure this runtime
// construction ledger prevents.
//
// A source grep for `single_line_elided()` cannot close this: name-based
// sweeps match comments, wrappers and unrelated identifiers far more often
// than real constructions. So this ledger observes the
// runtime construction seam instead: `ElisionScanProbe::new` is the single
// point every probe is built through (verified: there are zero struct-literal
// constructions of this type anywhere in the crate), so a counter here sees
// every probe that is really built and nothing that only resembles one in
// source.
//
// The distinction that makes this a real detector: a grep answers "does
// this string appear in the file", which a comment satisfies; the ledger
// answers "did a probe get constructed when this widget was built", which
// only working code satisfies. Renames, comments and moved code cannot fool
// it.
//
// Cost: one `Cell` increment and one `String` clone per probe construction,
// and only when recording has been explicitly switched on. Recording is off
// by default and is only enabled by [`ElisionLedger::record`] inside a test,
// so production view-building pays a single `bool` load per probe.
//
// `thread_local` (not a global) so the ledger is correct at any
// `--test-threads` value — a process-global counter here would need
// `--test-threads=1` and would be quietly wrong without it.

thread_local! {
    // `(enabled, labels-seen-since-reset)`.
    static ELISION_LEDGER: std::cell::RefCell<(bool, Vec<String>)> =
        const { std::cell::RefCell::new((false, Vec::new())) };
}

// Record that an [`ElisionScanProbe`] was constructed for `label`.
//
// Called unconditionally from [`ElisionScanProbe::new`]; does nothing
// unless recording is active, so the production cost is one `bool` load.
fn ledger_note(label: &str) {
    ELISION_LEDGER.with(|l| {
        // `try_borrow_mut` rather than `borrow_mut`: a probe constructed
        // during a ledger read would otherwise panic. Dropping the note
        // is the right failure mode — the ledger is a diagnostic, and it
        // must never be able to crash a view build.
        if let Ok(mut cell) = l.try_borrow_mut()
            && cell.0
        {
            cell.1.push(label.to_owned());
        }
    });
}

/// Scoped recorder for elision-probe construction.
///
/// Wrap a view build in [`ElisionLedger::record`] and it returns every label
/// that was wrapped in an [`ElisionScanProbe`] during that build. A widget
/// whose label stops routing through the audited path disappears from the
/// returned list, which is the failure this detector exists to catch.
///
/// Unlike the walk-level sweep tests, this does not need the label to
/// overflow: it observes that the audit machinery was installed, which
/// is the property that quietly regresses. A label that fits is still
/// audited — it simply reports no finding.
///
/// # Example
///
/// ```
/// use enzui_widgets::accessibility_scan_widgets::ElisionLedger;
/// use enzui_widgets::enz_text;
///
/// #[derive(Debug, Clone)]
/// enum Msg {}
///
/// let labels = ElisionLedger::record(|| {
///     let _: iced::Element<'_, Msg> =
///         enz_text("Some label").single_line_elided().into_element();
/// });
/// assert_eq!(labels, vec!["Some label".to_string()]);
/// ```
pub struct ElisionLedger;

impl ElisionLedger {
    /// Run `build` with probe-construction recording enabled and return every
    /// label that was wrapped in an [`ElisionScanProbe`], in construction
    /// order.
    ///
    /// Restores the previous recording state on the way out, so nesting is
    /// safe and a panic inside `build` cannot leave recording stuck on.
    pub fn record<R>(build: impl FnOnce() -> R) -> Vec<String> {
        // Save-and-restore rather than blind reset: `record` calls may nest,
        // and an inner call must not switch recording off for its caller.
        let previous = ELISION_LEDGER.with(|l| {
            let mut cell = l.borrow_mut();
            std::mem::replace(&mut *cell, (true, Vec::new()))
        });

        // Catch panics so recording state is always restored; a poisoned
        // ledger would make every later test in the binary lie.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(build));

        let seen = ELISION_LEDGER.with(|l| {
            let mut cell = l.borrow_mut();
            let (_, seen) = std::mem::replace(&mut *cell, previous);
            seen
        });

        match outcome {
            Ok(_) => seen,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ElisionScanProbe<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    // Shares the child's tree node, so its frame interest is the child's.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let node = self.child.as_widget_mut().measure(tree, renderer, limits);
        let natural_width = self.natural_width();
        self.observed.set(Some(Observed {
            assigned_width: node.size().width,
            natural_width,
        }));
        node.apply(tree);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        // The observation was taken during `layout()`. If layout has not run
        // yet (a purely operate-driven test that skipped it), fall back to the
        // child's post-layout bounds width for `assigned_width` and remeasure
        // the natural width. This keeps the audit meaningful in the tests that
        // build a tree and jump straight to `operate`.
        let obs = self.observed.get().unwrap_or_else(|| Observed {
            assigned_width: layout.bounds().width,
            natural_width: self.natural_width(),
        });
        let (full_len, displayed_len) = self.resolve_lengths(obs);
        let mut node = AccessibilityProbeNode {
            bounds: layout.bounds(),
            kind: enzui_core::accessibility_scan::AccessibilityCheck::Elision {
                full_len,
                displayed_len,
                allow: self.allow,
            },
        };
        operation.custom(self.id.as_ref(), layout.bounds(), &mut node);
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<ElisionScanProbe<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: ElisionScanProbe<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

/// Resolves the audited foreground/background pair from a runtime theme and
/// the interaction status being audited.
///
/// The whole point of taking a closure rather than two `Color`s is that a
/// colour written into a view function is frozen at whatever theme was active
/// when that function ran, while real paint is resolved later inside style
/// closures against both the theme and the widget status. A status-unaware
/// resolver only ever observes the resting appearance, so a control whose hover
/// fill fails contrast audits clean.
///
/// The probe drives this once per audited [`InteractionStatus`], in `draw()`, so give
/// it the identical per-status token logic the child's own style closure
/// uses. The trustworthy way to do that is the shared-cell pattern: close the
/// resolver over the same `Rc<Cell<..>>` the child writes, so both resolve the
/// same state — see [`ContrastScanProbe::status_cell`].
pub type ContrastResolver<'a, Theme> =
    Box<dyn Fn(&Theme, InteractionStatus) -> (Color, Color) + 'a>;

/// The interaction statuses a status-aware [`ContrastScanProbe`] audits by
/// default: resting, hover, pressed and disabled. Focus is omitted because
/// focus paint in this crate reuses the resting fill and adds only a ring, so it
/// is not a distinct contrast pair; a caller who needs it can pass an explicit
/// status set.
pub const DEFAULT_AUDITED_STATUSES: [InteractionStatus; 4] = [
    InteractionStatus::Resting,
    InteractionStatus::Hovered,
    InteractionStatus::Pressed,
    InteractionStatus::Disabled,
];

/// The pair the most recent `draw()` resolved, kept in the widget's
/// `tree::State` rather than on the
/// widget struct.
///
/// This distinction is load-bearing, and a headless
/// `build -> draw -> operate` test cannot see it: iced rebuilds the whole widget
/// tree from `view()` on every frame, so a `Cell` on the struct is a rebuilt
/// `None` by the time the runtime delivers an `Operation`. The scan route in
/// the probe app reported `UnresolvedPaint` for every contrast probe until this
/// moved into the tree, which persists across rebuilds by design.
/// `draw` takes `&self` alongside `&Tree`, so the cache has to be
/// interior-mutable: the persistence comes from living in the tree, the
/// writability from the [`Cell`](std::cell::Cell).
#[derive(Debug, Default)]
pub struct ContrastProbeState {
    /// The per-status `(foreground, background)` pairs from the last draw, in the
    /// probe's audited-status order, or empty if this probe has never been
    /// drawn. Each entry publishes one
    /// [`AccessibilityCheck::Contrast`](enzui_core::accessibility_scan::AccessibilityCheck::Contrast)
    /// node; an empty vector publishes `resolved: false`, which the collector
    /// reports as
    /// [`AccessibilityDefectKind::UnresolvedPaint`](enzui_core::accessibility_scan::AccessibilityDefectKind::UnresolvedPaint)
    /// rather than as clean.
    resolved: std::cell::RefCell<Vec<(InteractionStatus, Color, Color)>>,
}

impl ContrastProbeState {
    /// The resting `(foreground, background)` pair resolved by the most recent
    /// draw, if any. This compatibility accessor exposes the resting entry
    /// from the single-pair API.
    pub fn resolved(&self) -> Option<(Color, Color)> {
        self.resolved
            .borrow()
            .iter()
            .find(|(s, _, _)| *s == InteractionStatus::Resting)
            .map(|(_, fg, bg)| (*fg, *bg))
    }

    /// The per-status pairs the most recent draw resolved (empty if never
    /// drawn).
    pub fn resolved_by_status(&self) -> Vec<(InteractionStatus, Color, Color)> {
        self.resolved.borrow().clone()
    }
}

/// A transparent wrapper that publishes a contrast [`AccessibilityProbeNode`] whose
/// foreground/background pair is resolved from the live theme at draw time.
#[must_use]
pub struct ContrastScanProbe<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    resolve: ContrastResolver<'a, Theme>,
    /// The interaction statuses to audit, in publish order. The probe
    /// drives `resolve` once per entry in `draw()`.
    statuses: Vec<InteractionStatus>,
    /// Shared status channel: the probe writes the status it is about to
    /// resolve here before each `resolve` call, so a resolver closed over the
    /// same cell reads the identical state — the trustworthy way to reproduce a
    /// child's per-status paint without re-deriving it. Handed out by
    /// [`ContrastScanProbe::status_cell`].
    status_cell: std::rc::Rc<std::cell::Cell<InteractionStatus>>,
    min_ratio: f32,
    allow_low: bool,
    /// Statuses whose low contrast is intentionally exempt, applied
    /// per status. Distinct from the blanket
    /// [`allow_low`](Self::allow_low): a probe may legitimately exempt
    /// its disabled paint (theme disabled ink is allowed to be the
    /// lowest-contrast state) while still failing on an enabled status.
    /// A single probe-wide exemption cannot express that — it would let
    /// an exempt disabled pair suppress a real enabled failure.
    allow_statuses: Vec<InteractionStatus>,
    id: Option<iced::advanced::widget::Id>,
}

impl<'a, Message, Theme, Renderer> ContrastScanProbe<'a, Message, Theme, Renderer> {
    /// Wrap `child`, auditing the resting `(foreground, background)` pair
    /// that `resolve` derives from the runtime theme, against `mode`'s WCAG
    /// threshold.
    ///
    /// `resolve` runs inside `draw()`, with the same `&Theme` the child is being
    /// painted with, so switching light/dark re-resolves the audited pair
    /// instead of republishing a construction-time guess. Give it the same
    /// token lookup the child's own style closure uses — e.g.
    ///
    /// This resting-only form audits a single state. To also audit hover /
    /// pressed / disabled paint, use
    /// [`ContrastScanProbe::with_status`].
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        mode: ContrastMode,
        resolve: impl Fn(&Theme) -> (Color, Color) + 'a,
    ) -> Self {
        // Wrap the status-unaware closure as a resolver that ignores the status,
        // and audit only the resting state so behaviour is unchanged.
        Self::with_status(
            child,
            mode,
            vec![InteractionStatus::Resting],
            move |theme, _| resolve(theme),
        )
    }

    /// Wrap `child`, auditing `statuses` (each a distinct
    /// [`InteractionStatus`]) against `mode`'s WCAG threshold. `resolve` is driven
    /// once per status in `draw()` and receives the status being resolved, so a
    /// control whose hover fill fails contrast is caught.
    ///
    /// One [`AccessibilityDefect`](enzui_core::accessibility_scan::AccessibilityDefect) is produced
    /// per failing (widget, status) pair, each carrying its
    /// [`InteractionStatus`], so a report names the state that failed rather
    /// than only a worst-case ratio.
    ///
    /// For a resolver that mirrors a child's real per-status style closure
    /// without re-deriving it, close it over the same shared cell the child
    /// writes; the probe publishes its cell via
    /// [`status_cell`](ContrastScanProbe::status_cell) and sets it before each
    /// `resolve` call.
    pub fn with_status(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        mode: ContrastMode,
        statuses: Vec<InteractionStatus>,
        resolve: impl Fn(&Theme, InteractionStatus) -> (Color, Color) + 'a,
    ) -> Self {
        let min_ratio = match mode {
            ContrastMode::Small => crate::accessibility_metrics::DEFAULT_MIN_CONTRAST_RATIO,
            ContrastMode::Large => crate::accessibility_metrics::DEFAULT_MIN_CONTRAST_RATIO_LARGE,
        };
        let statuses = if statuses.is_empty() {
            vec![InteractionStatus::Resting]
        } else {
            statuses
        };
        Self {
            child: child.into(),
            resolve: Box::new(resolve),
            statuses,
            status_cell: std::rc::Rc::new(std::cell::Cell::new(InteractionStatus::Resting)),
            min_ratio,
            allow_low: false,
            allow_statuses: Vec::new(),
            id: None,
        }
    }

    /// Audit the default status set (resting / hover / pressed / disabled — see
    /// [`DEFAULT_AUDITED_STATUSES`]) with a status-aware resolver.
    pub fn all_statuses(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        mode: ContrastMode,
        resolve: impl Fn(&Theme, InteractionStatus) -> (Color, Color) + 'a,
    ) -> Self {
        Self::with_status(child, mode, DEFAULT_AUDITED_STATUSES.to_vec(), resolve)
    }

    /// Audit `statuses` using an externally-owned shared status cell.
    /// The probe writes the status it is about to resolve into `cell` before
    /// each `resolve` call, so a resolver closed over that same `cell` reads
    /// the identical state — the trustworthy way to have the probe's resolver
    /// and a child widget's style closure agree on the interaction state
    /// without re-deriving it. This is the exact shape
    /// [`window_control_button.rs`](crate::window_control_button) uses.
    pub fn with_shared_cell(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        mode: ContrastMode,
        statuses: Vec<InteractionStatus>,
        cell: std::rc::Rc<std::cell::Cell<InteractionStatus>>,
        resolve: impl Fn(&Theme, InteractionStatus) -> (Color, Color) + 'a,
    ) -> Self {
        let mut probe = Self::with_status(child, mode, statuses, resolve);
        probe.status_cell = cell;
        probe
    }

    /// The shared status cell. Close a resolver over a clone of this so it
    /// reads the exact interaction state the probe is resolving, rather than a
    /// parallel derivation of it. The probe sets it before every `resolve` call.
    pub fn status_cell(&self) -> std::rc::Rc<std::cell::Cell<InteractionStatus>> {
        std::rc::Rc::clone(&self.status_cell)
    }

    /// The resting pair the last `draw()` resolved into `tree`, for tests and
    /// reporters. `None` means this probe has not been drawn (see
    /// [`ContrastProbeState`]).
    pub fn last_resolved(tree: &Tree) -> Option<(Color, Color)> {
        tree.state.downcast_ref::<ContrastProbeState>().resolved()
    }

    /// The per-status pairs the last `draw()` resolved into `tree` (empty if
    /// never drawn), for tests and reporters.
    pub fn last_resolved_by_status(tree: &Tree) -> Vec<(InteractionStatus, Color, Color)> {
        tree.state
            .downcast_ref::<ContrastProbeState>()
            .resolved_by_status()
    }

    /// Override the minimum ratio.
    ///
    /// Normalized to finite non-negative: `0` is expressible intent
    /// meaning "no contrast minimum", and is honoured literally — a WCAG
    /// contrast ratio is always `>= 1.0`, so `ratio < 0.0` is never true and a
    /// zero threshold audits everything clean. Substituting the 4.5 default
    /// for a caller's `0` would re-arm an audit the caller disarmed. A `0`
    /// threshold is blunter than [`allow_status`](Self::allow_status); prefer
    /// that when only one interaction state is exempt.
    ///
    /// `NaN` / `±inf` / negative name no threshold at all and still fall back
    /// to 4.5.
    pub fn min_ratio(mut self, ratio: f32) -> Self {
        self.min_ratio = enzui_core::metrics::FiniteNonNeg::new_or(ratio, 4.5).get();
        self
    }

    /// Mark intentional low contrast on every audited status (never
    /// flagged). This is the blanket exemption; prefer
    /// [`allow_status`](Self::allow_status) so an exempt disabled pair cannot
    /// suppress an enabled failure.
    pub fn allow_low(mut self, allow: bool) -> Self {
        self.allow_low = allow;
        self
    }

    /// Exempt low contrast on one interaction status only.
    ///
    /// The canonical use is `allow_status(InteractionStatus::Disabled)`: disabled ink
    /// is legitimately the lowest-contrast state in a theme, so a control's
    /// disabled pair is allowed to fall short — but its enabled statuses
    /// (resting / hover / pressed) are still held to the threshold. A blanket
    /// [`allow_low`](Self::allow_low) cannot draw that line; it would let the
    /// exempt disabled paint silence a real enabled failure.
    pub fn allow_status(mut self, status: InteractionStatus) -> Self {
        if !self.allow_statuses.contains(&status) {
            self.allow_statuses.push(status);
        }
        self
    }

    // Whether the given status is exempt from contrast reporting: the
    // blanket [`allow_low`](Self::allow_low) or a per-status exemption
    // from [`allow_status`](Self::allow_status).
    fn status_allowed(&self, status: InteractionStatus) -> bool {
        self.allow_low || self.allow_statuses.contains(&status)
    }

    /// Attach a stable finding id.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ContrastScanProbe<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    // The probe owns its own tree slot (rather than transparently forwarding to
    // the child's, as `HitTargetScanProbe` does) because it has real state to
    // persist: the theme-resolved pair from the last draw. The child gets a
    // nested slot via `diff_children`.
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<ContrastProbeState>()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(ContrastProbeState::default())
    }
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
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
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        // Publish what the live theme resolved during the last draw pass, not a
        // construction-time guess — one node per audited status. Never
        // drawn => one `resolved: false` node (per configured status), which the
        // collector reports as `UnresolvedPaint`. The pairs are read from
        // the tree, which survives the per-frame `view()` rebuild that would
        // reset any cache held on the widget struct.
        let resolved = tree
            .state
            .downcast_ref::<ContrastProbeState>()
            .resolved_by_status();
        let bounds = layout.bounds();
        if resolved.is_empty() {
            // Not drawn: publish one unresolved node per configured status so a
            // never-drawn probe is reported, not quietly passed.
            for status in &self.statuses {
                let mut node = AccessibilityProbeNode {
                    bounds,
                    kind: AccessibilityCheck::Contrast {
                        foreground: Color::BLACK,
                        background: Color::BLACK,
                        min_ratio: self.min_ratio,
                        allow: self.status_allowed(*status),
                        resolved: false,
                        status: *status,
                    },
                };
                operation.custom(self.id.as_ref(), bounds, &mut node);
            }
        } else {
            for (status, foreground, background) in resolved {
                let mut node = AccessibilityProbeNode {
                    bounds,
                    kind: AccessibilityCheck::Contrast {
                        foreground,
                        background,
                        min_ratio: self.min_ratio,
                        allow: self.status_allowed(status),
                        resolved: true,
                        status,
                    },
                };
                operation.custom(self.id.as_ref(), bounds, &mut node);
            }
        }
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
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
        self.child.as_widget_mut().update(
            &mut tree.children[0],
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
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Resolve the audited pair against the runtime theme, in the same pass
        // that paints the child, so a light/dark switch changes what is audited,
        // once per audited status. Before each resolve, publish the
        // status into the shared cell so a resolver closed over it reads
        // the same state, then call the resolver. `operate()` reads these back.
        let pairs: Vec<(InteractionStatus, Color, Color)> = self
            .statuses
            .iter()
            .map(|&status| {
                self.status_cell.set(status);
                let (fg, bg) = (self.resolve)(theme, status);
                (status, fg, bg)
            })
            .collect();
        *tree
            .state
            .downcast_ref::<ContrastProbeState>()
            .resolved
            .borrow_mut() = pairs;
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
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
        renderer: &Renderer,
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
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

impl<'a, Message, Theme, Renderer> From<ContrastScanProbe<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: ContrastScanProbe<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

/// A transparent wrapper that publishes an
/// [`AccessibilityCheck::MissingName`] node.
///
/// Designed for icon-only controls whose accessible name was never set
/// explicitly by the caller and quietly fell back to the icon’s glyph
/// name (e.g. `"plus"`, `"chevron-left"`). The glyph name is artwork
/// metadata, not a user-facing action description, and the fallback
/// produces a string plausible enough to pass every automated check —
/// the defect is detectable only by a human reading the announced text.
///
/// The probe itself does no layout or paint; it delegates everything to
/// its child and publishes a single `AccessibilityProbeNode` in `operate()` so
/// that an [`AccessibilityScanCollector`](enzui_core::accessibility_scan::AccessibilityScanCollector)
/// walk can report the finding.
#[must_use]
pub struct MissingNameScanProbe<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    allow: bool,
}

impl<'a, Message, Theme, Renderer> MissingNameScanProbe<'a, Message, Theme, Renderer> {
    /// Wrap `child` in a missing-name scan probe.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
            allow: false,
        }
    }

    /// Mark the glyph-name fallback as intentional (never flagged).
    pub fn allow(mut self, allow: bool) -> Self {
        self.allow = allow;
        self
    }
}

/// Convenience constructor for [`MissingNameScanProbe`].
pub fn missing_name_scan_probe<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> MissingNameScanProbe<'a, Message, Theme, Renderer> {
    MissingNameScanProbe::new(child)
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for MissingNameScanProbe<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    // Shares the child's tree node, so its frame interest is the child's.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut node = AccessibilityProbeNode {
            bounds: layout.bounds(),
            kind: AccessibilityCheck::MissingName { allow: self.allow },
        };
        operation.custom(None, layout.bounds(), &mut node);
        // Delegate to the child so any nested probes keep working.
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<MissingNameScanProbe<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: MissingNameScanProbe<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

/// Natural widths measured by every elision probe, kept across views.
///
/// A probe is rebuilt with every view, so a width cached on it alone was
/// shaped again on every app message: on a page of labels that was a large
/// share of each rebuild. Entries are dropped when the font system changes,
/// since a newly loaded face can change any width.
mod natural_widths {
    use crate::text::TextShaping;
    use iced::advanced::graphics::text::{Version, font_system};
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    /// More distinct labels than this starts the cache over.
    const CAPACITY: usize = 4096;

    type Key = (String, u32, iced::Font, TextShaping);

    struct Cache {
        version: Option<Version>,
        widths: HashMap<Key, f32>,
    }

    pub(super) fn get_or_measure(
        label: &str,
        size: f32,
        font: iced::Font,
        shaping: TextShaping,
        measure: impl FnOnce() -> f32,
    ) -> f32 {
        static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| {
            Mutex::new(Cache {
                version: None,
                widths: HashMap::new(),
            })
        });
        let version = font_system().read().ok().map(|system| system.version());
        let key = (label.to_owned(), size.to_bits(), font, shaping);
        if let Ok(mut cache) = cache.lock() {
            if cache.version != version {
                cache.version = version;
                cache.widths.clear();
            }
            if let Some(width) = cache.widths.get(&key) {
                return *width;
            }
        }
        // Measured outside the lock: shaping takes the font system's own lock.
        let width = measure();
        if let Ok(mut cache) = cache.lock()
            && cache.version == version
        {
            if cache.widths.len() >= CAPACITY {
                cache.widths.clear();
            }
            let _ = cache.widths.insert(key, width);
        }
        width
    }
}
