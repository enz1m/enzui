//! Decorative vertical toolbar separators with live row-relative insets.
//!
//! Unpinned lines use quiet text-subtle ink at 25% alpha. Their top/bottom
//! inset is the larger of live spacing-xs and the inset leaving half the row
//! height painted, rounded to whole logical pixels. A row with no remaining
//! line height paints nothing while retaining its layout allocation.
//!
//! Explicit inset wins over token inset in either call order and bypasses the
//! ratio. Tokens pin colour and thickness too; palette-derived tokens use border
//! ink and a fixed stock 4 px inset. Invalid insets fall back to 4 px, invalid
//! thickness to 1 px; zero survives. Positive thickness retains native snapping.

use crate::hairline::{rule_snap, sanitize_thickness};
use enzui_theme::{Palette, SpacingScale};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::rule::{self, FillMode};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};
fn sanitize_inset(px: f32, fallback: f32) -> f32 {
    if px.is_finite() && px >= 0.0 {
        px
    } else {
        fallback
    }
}
fn default_inset() -> f32 {
    SpacingScale::default().xs()
}

/// Unpinned quiet divider alpha: 0.25, replacing the palette text-subtle alpha.
pub const DEFAULT_DIVIDER_INK_ALPHA: f32 = 0.25;
/// Return palette text-subtle RGB with alpha 0.25 for compositing over the host surface.
pub fn default_divider_ink(palette: &Palette) -> Color {
    Color {
        a: DEFAULT_DIVIDER_INK_ALPHA,
        ..palette.text_subtle
    }
}

/// Maximum nominal unpinned line-height fraction before inset rounding: 0.5.
/// Larger theme spacing can shorten the line; pinned inset bypasses this ratio.
pub const DEFAULT_DIVIDER_HEIGHT_RATIO: f32 = 0.5;

thread_local! {
    // Rule style has no bounds; thread-local draw scope also isolates parallel renderer trees.
    static LIVE_ROW_HEIGHT: std::cell::Cell<Option<f32>> = const { std::cell::Cell::new(None) };
}
struct RowHeightGuard(Option<f32>);

impl Drop for RowHeightGuard {
    fn drop(&mut self) {
        let prev = self.0;
        // Restore the enclosing draw scope even if an inner draw unwinds.
        LIVE_ROW_HEIGHT.with(|c| c.set(prev));
    }
}

fn with_row_height<R>(h: f32, f: impl FnOnce() -> R) -> R {
    let prev = LIVE_ROW_HEIGHT.with(|c| c.replace(Some(h)));
    let _restore = RowHeightGuard(prev);
    f()
}
fn ratio_inset(row_height: f32) -> f32 {
    if !row_height.is_finite() || row_height <= 0.0 {
        return 0.0;
    }
    row_height * (1.0 - DEFAULT_DIVIDER_HEIGHT_RATIO) / 2.0
}
#[derive(Debug, Clone, Copy, PartialEq)]
enum InsetSource {
    Pinned(f32),
    Theme,
}

impl InsetSource {
    fn resolve(self, theme: &iced::Theme) -> f32 {
        match self {
            InsetSource::Pinned(v) => v,
            InsetSource::Theme => {
                let xs = sanitize_inset(crate::theme_lookup::spacing(theme).xs(), default_inset());
                match LIVE_ROW_HEIGHT.with(std::cell::Cell::get) {
                    Some(h) => xs.max(ratio_inset(h)),
                    None => xs,
                }
            }
        }
    }
}

/// Build a native vertical rule with fixed fill policy and optional pinned ink.
/// Unpinned ink follows the live quiet divider colour. Invalid thickness falls
/// back to 1 px; zero disables snapping. This helper does not apply the standalone
/// divider's collapsed-height guard, so zero-length fill may still snap to a pixel.
pub fn vertical_rule<'a, Message: 'a>(
    thickness: f32,
    fill: FillMode,
    pinned_color: Option<Color>,
) -> Element<'a, Message> {
    vertical_rule_with(thickness, move |_| fill, pinned_color)
}

/// Build a native vertical rule whose fill policy resolves from the draw-time theme.
/// Thickness and ink follow vertical_rule. The fill callback receives no layout
/// bounds.
pub fn vertical_rule_with<'a, Message: 'a>(
    thickness: f32,
    fill: impl Fn(&iced::Theme) -> FillMode + 'a,
    pinned_color: Option<Color>,
) -> Element<'a, Message> {
    let thickness = sanitize_thickness(thickness, 1.0);
    rule::vertical(thickness)
        .style(move |theme: &iced::Theme| {
            let color = match pinned_color {
                Some(c) => c,
                None => default_divider_ink(&crate::theme_lookup::palette(theme)),
            };
            rule_style(color, thickness, fill(theme))
        })
        .into()
}
fn rule_style(color: Color, thickness: f32, fill: FillMode) -> rule::Style {
    rule::Style {
        color,
        radius: 0.0.into(),
        fill_mode: fill,
        snap: rule_snap(thickness),
    }
}
fn visible_line_height(row_height: f32, inset: f32) -> f32 {
    (row_height - 2.0 * inset).max(0.0)
}
fn should_draw(row_height: f32, inset: f32) -> bool {
    visible_line_height(row_height, inset) > 0.0
}
struct CollapseGuard<'a, Message> {
    inner: Element<'a, Message>,
    inset: InsetSource,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CollapseGuard<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.inner.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.inner.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.inner.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.inner.as_widget_mut().layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.inner
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.inner
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        let row_h = layout.bounds().height;
        // Guard and rule style must resolve against the same live row height.
        with_row_height(row_h, || {
            // Native snapping would resurrect collapsed padded fill outside the assigned row.
            if !should_draw(row_h, self.inset.resolve(theme).round()) {
                return;
            }
            self.inner
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
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
        self.inner
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.inner
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
    fn is_void(&self) -> bool {
        self.inner.as_widget().is_void()
    }
}

/// Pinned separator ink and logical-pixel geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToolbarDividerTokens {
    /// Pinned ink, including alpha; palette-derived tokens use border colour.
    pub rule: Color,
    /// Top/bottom inset; palette-derived tokens use 4 px.
    /// Invalid values fall back to 4 px; native padded fill uses rounded, u16-saturated values.
    pub inset: f32,
    /// Allocated line width; defaults to 1 px.
    /// Negative or non-finite input falls back to 1; zero disables snapping and line paint.
    pub thickness: f32,
}

impl ToolbarDividerTokens {
    /// Use palette border ink, a fixed stock 4 px inset and 1 px thickness.
    /// Pinning this token set bypasses live spacing and the row-height ratio.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            rule: p.border,
            inset: default_inset(),
            thickness: 1.0,
        }
    }
}

/// Build an unpinned, fixed-width separator that fills its assigned height.
pub fn enz_toolbar_divider<'a, Message>() -> EnzToolbarDivider<'a, Message>
where
    Message: 'a,
{
    EnzToolbarDivider::new()
}

/// Builder for a decorative separator without label or interaction semantics.
#[must_use]
pub struct EnzToolbarDivider<'a, Message> {
    tokens: Option<ToolbarDividerTokens>,
    inset_override: Option<f32>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

impl<'a, Message> Default for EnzToolbarDivider<'a, Message>
where
    Message: 'a,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, Message> EnzToolbarDivider<'a, Message>
where
    Message: 'a,
{
    /// Build an unpinned 1 px divider with live quiet ink and row-relative inset.
    pub fn new() -> Self {
        Self {
            tokens: None,
            inset_override: None,
            _phantom: std::marker::PhantomData,
        }
    }
    /// Pin top/bottom inset, overriding token inset in either call order.
    /// Zero is preserved; negative or non-finite input falls back to 4 px.
    /// Pinned inset bypasses live spacing and the height ratio.
    pub fn inset(mut self, inset: f32) -> Self {
        self.inset_override = Some(sanitize_inset(inset, default_inset()));
        self
    }
    /// Pin ink, inset and thickness; an explicit inset still wins.
    /// Geometry is validated when building the element; ink is passed through.
    pub fn tokens(mut self, tokens: ToolbarDividerTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
}

impl<'a, Message> EnzToolbarDivider<'a, Message>
where
    Message: 'a,
{
    /// Build a fixed-width, fill-height separator with a draw-time collapse guard.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzToolbarDivider<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzToolbarDivider<'a, Message>) -> Self {
        let pinned = w.tokens;
        let thickness = sanitize_thickness(pinned.map_or(1.0, |t| t.thickness), 1.0);
        let inset = match (w.inset_override, pinned) {
            (Some(i), _) => InsetSource::Pinned(sanitize_inset(i, default_inset())),
            (None, Some(t)) => InsetSource::Pinned(sanitize_inset(t.inset, default_inset())),
            (None, None) => InsetSource::Theme,
        };
        let rule = vertical_rule_with(
            thickness,
            move |theme: &iced::Theme| FillMode::Padded(inset.resolve(theme).round() as u16),
            pinned.map(|t| t.rule),
        );
        Element::new(CollapseGuard { inner: rule, inset })
    }
}
