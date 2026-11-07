//! EnzGlow: coloured glow (halo) around content.
//!
//! Paints a soft, zero-offset blur behind a rounded content surface. The halo
//! fades with distance from the surface and does not enlarge its layout or hit
//! region. Tint follows the runtime accent unless overridden; spread controls
//! the blur reach, and intensity controls its opacity.

use enzui_theme::Palette;
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{
    Background, Border, Color, Element, Event, Length, Rectangle, Shadow, Size, Theme, Vector,
};

use crate::theme_lookup::resolve;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Design tokens for [`enz_glow`].
#[derive(Debug, Clone, Copy)]
pub struct GlowTokens {
    /// Surface background painted under the content.
    pub surface_elevated: Color,
    /// Default glow tint.
    pub default_glow_color: Color,
    /// Default content corner radius (`radius-lg` = 10).
    pub content_radius: f32,
}

impl GlowTokens {
    /// Derive the glow surface + tint from a palette (the normal path; pass the
    /// live theme's palette so the glow tracks a theme switch). Radius is fixed.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            // The content sits on `surface_elevated` (#3a373e dark), not the
            // base `surface` (#28262b).
            surface_elevated: p.surface_elevated,
            default_glow_color: p.accent,
            content_radius: 10.0,
        }
    }
    /// Preset built from the built-in dark palette. Prefer
    /// [`GlowTokens::from_palette`] with the live palette when one is in scope.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Preset built from the built-in light palette. Prefer
    /// [`GlowTokens::from_palette`] with the live palette when one is in scope.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default halo opacity.
pub const DEFAULT_GLOW_INTENSITY: f32 = 0.35;
/// Default blur reach in logical pixels.
pub const DEFAULT_GLOW_SPREAD: f32 = 12.0;
/// Shadow blur diameter per unit of halo spread.
pub const GLOW_SHADOW_BLUR_FACTOR: f32 = 2.0;

/// Sanitise an f32 to a finite, non-negative value (non-finite → 0).
fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

/// Sanitise an alpha to finite `0..=1` (non-finite → 0).
fn finite_unit(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_glow(child)`.
pub fn enz_glow<'a, Message>(child: impl Into<Element<'a, Message>>) -> EnzGlow<'a, Message>
where
    Message: 'a,
{
    EnzGlow {
        child: child.into(),
        glow_color: None,
        glow_intensity: DEFAULT_GLOW_INTENSITY,
        glow_spread: DEFAULT_GLOW_SPREAD,
        content_radius: None,
        tokens: None,
        width: Length::Shrink,
        height: Length::Shrink,
    }
}

/// Builder returned by [`enz_glow`].
#[must_use]
pub struct EnzGlow<'a, Message> {
    child: Element<'a, Message>,
    glow_color: Option<Color>,
    glow_intensity: f32,
    glow_spread: f32,
    content_radius: Option<f32>,
    tokens: Option<GlowTokens>,
    width: Length,
    height: Length,
}

impl<'a, Message> EnzGlow<'a, Message>
where
    Message: 'a,
{
    /// Override the glow tint (default = theme accent).
    pub fn glow_color(mut self, color: Color) -> Self {
        self.glow_color = Some(color);
        self
    }

    /// Set the glow intensity (alpha of the halo). Clamped to `0..=1`;
    /// non-finite normalises to 0. Default `0.35`.
    pub fn glow_intensity(mut self, i: f32) -> Self {
        self.glow_intensity = finite_unit(i);
        self
    }

    /// Set the halo spread in pixels. Non-finite/negative → 0. Default `12.0`.
    pub fn glow_spread(mut self, s: f32) -> Self {
        self.glow_spread = finite_nonneg(s);
        self
    }

    /// Override the inner content corner radius. Non-finite/negative → 0.
    /// Default follows tokens (`radius-lg`, `10.0`).
    pub fn content_radius(mut self, r: f32) -> Self {
        self.content_radius = Some(finite_nonneg(r));
        self
    }

    /// Override the tokens (surface background, default glow color,
    /// default content radius).
    pub fn tokens(mut self, tokens: GlowTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Outer width (default `Length::Shrink`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Outer height (default `Length::Shrink`).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// The content clip radius used for radius-aware hit exclusion.
    ///
    /// `update`/`mouse_interaction` have no `theme`, so this resolves the
    /// setter value with the theme-invariant default token (`from_palette`
    /// pins `content_radius = 10.0` for every palette, so this equals the drawn
    /// radius unless a caller passes a custom `GlowTokens`; the setter value
    /// wins whenever it is set, which is the common case).
    fn hit_radius(&self) -> f32 {
        finite_nonneg(
            self.content_radius
                .unwrap_or_else(|| GlowTokens::dark().content_radius),
        )
    }

    /// The cursor the child should see: an available cursor sitting in a clipped
    /// corner (invisible, and therefore not interactive)
    /// is presented as [`mouse::Cursor::Unavailable`]. The glow content clips to
    /// the laid-out bounds with a *uniform* `content_radius` and no outset.
    fn masked_cursor(&self, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Cursor {
        let radius = iced::border::Radius::from(self.hit_radius());
        match cursor.position() {
            Some(p) if !enzui_core::rounded_geometry::point_in_rounded_rect(p, bounds, radius) => {
                mouse::Cursor::Unavailable
            }
            _ => cursor,
        }
    }
}

// ---------------------------------------------------------------------------
// Widget impl (delegating; draws the two glow layers around the child)
// ---------------------------------------------------------------------------

impl<'a, Message> Widget<Message, Theme, iced::Renderer> for EnzGlow<'a, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }
    fn state(&self) -> tree::State {
        tree::State::None
    }
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }
    fn is_void(&self) -> bool {
        // Defaulted trait method: delegate so a `Void` child stays void and
        // Row/Column/Stack still filter it out.
        self.child.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // The decorative halo must not change the child's allocation.
        let limits = limits.width(self.width).height(self.height);
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, &limits);
        iced::advanced::layout::pass_through(tree, 0);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
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
        // Radius-aware hit exclusion: a cursor in a clipped
        // corner is invisible, so present it to the child as unavailable.
        let cursor = self.masked_cursor(layout.bounds(), cursor);
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

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
        // Frame delivery preserves the same rounded hit exclusion as ordinary
        // events; a frame-aware descendant must not see a cursor through a
        // clipped corner.
        let cursor = self.masked_cursor(layout.bounds(), cursor);
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
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
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();

        // Resolve tokens / colors (runtime theme unless overridden).
        let tokens = self
            .tokens
            .unwrap_or_else(|| resolve(theme, GlowTokens::from_palette));
        let spread = self.glow_spread;
        // `content_radius` the *setter* is sanitized on the way in, but
        // `GlowTokens.content_radius` is a plain public field, so the token
        // path is sanitized here instead. It matters because
        // this radius reaches the renderer's clip *mask*, and the
        // two backends disagree about NaN (measured with a NaN token: wgpu's
        // SDF rejects every pixel and masks the child away entirely, 0/2968,
        // while tiny_skia's `r.min(..).max(0.0)` keeps NaN, `rounded_rectangle`
        // degenerates and nothing is masked, 2968/2968) — capacities are
        // scale-dependent (SCALE=3, r=24; see
        // `tests/rounded_clip_widget_sweep.rs`'s row 10).
        let radius = finite_nonneg(self.content_radius.unwrap_or(tokens.content_radius));

        let base = self.glow_color.unwrap_or(tokens.default_glow_color);
        let shadow_color = Color {
            a: base.a * self.glow_intensity,
            ..base
        };
        if spread > 0.0 && shadow_color.a > 0.0 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: Border {
                        radius: radius.into(),
                        ..Border::default()
                    },
                    shadow: Shadow {
                        color: shadow_color,
                        offset: Vector::ZERO,
                        blur_radius: spread * GLOW_SHADOW_BLUR_FACTOR,
                    },
                    snap: false,
                },
                Background::Color(Color::TRANSPARENT),
            );
        }

        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: radius.into(),
                },
                shadow: Shadow::default(),
                snap: false,
            },
            Background::Color(tokens.surface_elevated),
        );

        // Child, clipped to the content rect and to its corner arc.
        //
        // The rounded shape is the full laid-out `bounds`, never
        // `bounds ∩ viewport` (the same rule as `image_viewer.rs`).
        // The intersection is a *culling*
        // rectangle, not the mask shape: rounding it invents an arc wherever
        // the viewport truncates the frame — a partially-scrolled glow would
        // grow a corner mid-content on the straight edge the scroll cut, and
        // square off the real corners the intersection has already lost. An
        // unscrolled glow has `bounds == bounds ∩ viewport`, which is why such
        // a defect stays invisible until something scrolls the widget.
        //
        // This is `RoundedClip::draw`'s split (in the `rounded_clip` module): the
        // rounded layer carries the arc at the frame's real corners, and the
        // child is handed the viewport-narrowed rectangle as *its* viewport so
        // its straight edges are cut at the viewport rather than rounded. The
        // renderer's rounded clip is a single mask that reaches quads, meshes
        // and images on both enzICED backends, so no second
        // rectangular layer is needed here (glow's child is delegated, not a
        // directly-drawn image as in `image_viewer`).
        //
        // Fully scrolled off (`intersection == None`) there is nothing to draw.
        let Some(child_viewport) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer_rounded(bounds, radius.into(), |renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                &child_viewport,
            );
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
        // Same corner exclusion as `update`: no cursor shape in a clipped corner.
        let cursor = self.masked_cursor(layout.bounds(), cursor);
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
    ) -> Vec<overlay::Element<'b, Message, Theme, iced::Renderer>> {
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

impl<'a, Message> EnzGlow<'a, Message>
where
    Message: 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzGlow<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzGlow<'a, Message>) -> Self {
        Element::new(w)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
