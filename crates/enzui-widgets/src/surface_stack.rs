//! EnzSurfaceStack: EnzSurface with a built-in vertical stack.
//!
//! Wraps
//! [`crate::surface::EnzSurface`] with a vertical column that owns the inner
//! spacing + padding via semantic space tokens (`Xs`/`Sm`/`Md`/`Lg`/`Xl`).
//!
//! Callers push children in one at a time.

use crate::surface::{
    BorderStyle, DEFAULT_RADIUS, LineSize, RadiusInput, RadiusSize, SurfaceTokens, SurfaceTone,
};
use enzui_theme::SpacingScale;
use iced::widget::{Column, Space};
use iced::{Element, Padding};

// ---------------------------------------------------------------------------
// Space tokens
// ---------------------------------------------------------------------------

/// Semantic space token. Canonically defined in the theme crate
/// ([`enzui_theme::SpaceToken`]) so every layout widget resolves the same
/// scale; re-exported here for the historical `surface_stack::SpaceToken`
/// import path.
pub use enzui_theme::SpaceToken;

/// Main-axis (vertical) alignment of children within the vertical stack — the
/// axis the `VerticalLayout.alignment` controls.
///
/// This is shared with [`crate::stack`] (EnzVStack/EnzHStack) so a single
/// vocabulary drives both. Cross-axis (horizontal) alignment is a separate
/// control: [`StackCross`] (which includes `Stretch`). Main-axis alignment is
/// only observable when the stack is taller than its content (given a
/// `Length::Fill`/fixed height); otherwise the column hugs its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StackAlign {
    /// Children packed at the start (top). `alignment: start`.
    #[default]
    Start,
    /// Children centred along the main axis. `alignment: center`.
    Center,
    /// Children pushed to the end (bottom). `alignment: end`.
    End,
    /// Children distributed to fill the main axis with equal space between
    /// them. Also the resolved default when a
    /// stack has `fill: true` and `align: start` (Original:
    /// `root.fill && align == "start" ? stretch`).
    Stretch,
}

/// Cross-axis (horizontal) alignment for a vertical stack. Broader than
/// [`StackAlign`] — includes `Stretch` (children fill the cross-axis extent,
/// `horizontal-stretch`). Shared with [`crate::stack::CrossAlign`] in
/// spirit; kept here so `surface_stack` has a self-contained cross control.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StackCross {
    /// Pack children at the cross-axis start (left) (the default).
    #[default]
    Start,
    /// Centre children on the cross axis.
    Center,
    /// Pack children at the cross-axis end (right).
    End,
    /// Children fill the cross-axis (horizontal) extent.
    Stretch,
}

impl StackCross {
    /// Map to iced's cross-axis `iced::Alignment`. `Stretch` aligns `Start`;
    /// the fill (column width = Fill) does the stretching.
    pub fn to_iced(self) -> iced::Alignment {
        match self {
            StackCross::Start | StackCross::Stretch => iced::Alignment::Start,
            StackCross::Center => iced::Alignment::Center,
            StackCross::End => iced::Alignment::End,
        }
    }

    /// Whether this stretches children across the cross axis.
    pub fn is_stretch(self) -> bool {
        matches!(self, StackCross::Stretch)
    }
}

/// Apply a main-axis (vertical) [`StackAlign`] to a built element by
/// bracketing it with `Fill` spacers.
///
/// iced's `Column` has no main-axis alignment builder, so `Center` fills both
/// ends and `End` fills the leading end. Only visible when the outer element is
/// taller than the content. `Start` returns the content unchanged.
///
/// `Stretch` means "spread the children apart with `Fill` spacers between each
/// adjacent pair", which needs the child list. This helper only ever sees one
/// already-built element and cannot take it apart, so `Stretch` returns that
/// element unchanged here; for a single child that is also what distribution
/// would produce. Main-axis distribution is therefore realized by the callers
/// that still hold the children: `EnzSurfaceStack::into_element` interleaves
/// the spacers itself, exactly like `stack.rs::pack_column`, and never routes
/// `Stretch` through this function.
pub fn apply_main_axis<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    align: StackAlign,
) -> Element<'a, Message> {
    let content = content.into();
    match align {
        // `Start` packs at the top — no spacers.
        StackAlign::Start => content,
        // `Stretch` needs Fill spacers *between* children, so it is realized
        // where the child vector still exists — see the packing loop in
        // `EnzSurfaceStack::into_element`. A single already-built element has
        // no interior gaps to fill, so this arm is the identity and the
        // surface-stack path never reaches it for `Stretch`.
        StackAlign::Stretch => content,
        StackAlign::Center => Column::new()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .push(Space::new().height(iced::Length::Fill))
            .push(content)
            .push(Space::new().height(iced::Length::Fill))
            .into(),
        StackAlign::End => Column::new()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .push(Space::new().height(iced::Length::Fill))
            .push(content)
            .into(),
    }
}

/// Row (horizontal main-axis) analog of [`apply_main_axis`], for
/// [`crate::stack::EnzHStack`]. `Center` fills both ends, `End` fills the
/// leading end, using horizontal `Fill` spacers.
pub fn apply_main_axis_row<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    align: StackAlign,
) -> Element<'a, Message> {
    let content = content.into();
    match align {
        // See [`apply_main_axis`]: `Start`/`Stretch` realized inside the stack.
        StackAlign::Start | StackAlign::Stretch => content,
        StackAlign::Center => iced::widget::Row::new()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .push(Space::new().width(iced::Length::Fill))
            .push(content)
            .push(Space::new().width(iced::Length::Fill))
            .into(),
        StackAlign::End => iced::widget::Row::new()
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .push(Space::new().width(iced::Length::Fill))
            .push(content)
            .into(),
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// A themed vertical-stack surface.
#[must_use]
pub struct EnzSurfaceStack<'a, Message> {
    tone: SurfaceTone,
    border_style: BorderStyle,
    radius: RadiusInput,
    /// `None` = user has not explicitly set border width; `Some(v)` = user set
    /// it to `v`. Distinguishing these is what makes `.border_width(0.0)` +
    /// `.border_style(Thin)` honour the explicit zero rather than auto-bumping
    /// to 1 (call-order dependence).
    border_width_explicit: Option<f32>,
    tokens: Option<SurfaceTokens>,
    gap: SpaceToken,
    inset_left: SpaceToken,
    inset_right: SpaceToken,
    inset_top: SpaceToken,
    inset_bottom: SpaceToken,
    align: StackAlign,
    cross: StackCross,
    width: iced::Length,
    height: iced::Length,
    children: Vec<Element<'a, Message>>,
    /// Spacing scale for resolving `SpaceToken`s. Defaults to
    /// `SpacingScale::default()` (the stock ladder); override via
    /// [`Self::spacing_scale`] to use the active theme's custom density.
    spacing_scale: SpacingScale,
    /// Elevation level forwarded to the underlying `EnzSurface` (0 = none,
    /// max 5).
    elevation: u8,
    /// Clip toggle forwarded to `EnzSurface` (default `true`)..
    clip: bool,
    /// Direct fill colour override forwarded to `EnzSurface`..
    fill_override: Option<iced::Color>,
    /// Direct border colour override forwarded to `EnzSurface`..
    border_color_override: Option<iced::Color>,
}

/// Normalize a caller-supplied corner radius at the public boundary.
///
/// `EnzSurface` already sanitizes downstream via its private `sane_dim`, but
/// this crate's contract is to normalize once at the public boundary so the
/// stack's own stored state is never non-finite either (the contract's
/// "surface_stack duplicates the policy differently" finding). Falls back to
/// [`DEFAULT_RADIUS`], matching `EnzSurface`'s policy exactly.
fn sane_radius(v: f32) -> f32 {
    enzui_core::metrics::FiniteNonNeg::new_or(v, DEFAULT_RADIUS).get()
}

impl<'a, Message: 'a> EnzSurfaceStack<'a, Message> {
    /// Set the surface fill tone. Defaults to the [`SurfaceTone`] default.
    pub fn tone(mut self, tone: SurfaceTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the border style. Selecting any style other than
    /// [`BorderStyle::None`] auto-bumps the border width to 1 px **only if the
    /// caller has not explicitly set a width** via [`Self::border_width`] or
    /// [`Self::line_size`]. An explicit zero is honoured literally — it means
    /// "no visible border stroke" and is not overridden regardless of call
    /// order.
    pub fn border_style(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        // Only auto-bump when the user has not explicitly set a width.
        // `border_width_explicit == None` means "no explicit call".
        if !matches!(style, BorderStyle::None) && self.border_width_explicit.is_none() {
            self.border_width_explicit = Some(1.0);
        }
        self
    }

    /// Uniform corner radius in px applied to all four corners. Non-finite or
    /// negative input falls back to the default radius.
    pub fn radius(mut self, radius: f32) -> Self {
        let r = sane_radius(radius);
        self.radius = RadiusInput::Corners([r; 4]);
        self
    }

    /// Set the uniform corner radius from a semantic [`RadiusSize`].
    /// Forwards to [`crate::surface::EnzSurface::radius_size`].
    pub fn radius_size(mut self, size: RadiusSize) -> Self {
        self.radius = RadiusInput::Size(size);
        self
    }

    /// Per-corner radii in px (same corner order as
    /// [`crate::surface::EnzSurface::radius_corners`]).
    /// Each corner is independently clamped; non-finite or negative values fall
    /// back to the default radius.
    pub fn radius_corners(mut self, corners: [f32; 4]) -> Self {
        self.radius = RadiusInput::Corners([
            sane_radius(corners[0]),
            sane_radius(corners[1]),
            sane_radius(corners[2]),
            sane_radius(corners[3]),
        ]);
        self
    }

    /// Border width in px. Non-finite or negative input falls back to **its own
    /// previous value** (the field's fallback is itself, not a different field;
    ///  ). An explicit zero is expressible intent ("no border") and
    /// is honoured literally.
    pub fn border_width(mut self, width: f32) -> Self {
        let sanitized = enzui_core::metrics::FiniteNonNeg::new_or(
            width,
            self.border_width_explicit.unwrap_or(0.0),
        )
        .get();
        self.border_width_explicit = Some(sanitized);
        self
    }

    /// Set the border width from a semantic [`LineSize`].
    /// Forwards to [`crate::surface::EnzSurface::line_size`].
    pub fn line_size(mut self, size: LineSize) -> Self {
        self.border_width_explicit = Some(size.px());
        self
    }

    /// Override the surface colour tokens. Unset by default (resolved from the
    /// runtime theme).
    pub fn tokens(mut self, tokens: SurfaceTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Override the spacing scale used to resolve `SpaceToken`s (gap + insets).
    ///
    /// Defaults to [`SpacingScale::default`] (the stock ladder). Pass the
    /// active theme's scale — retrieved via
    /// [`crate::theme_lookup::spacing`] — so a theme with a custom density
    /// flows through.
    pub fn spacing_scale(mut self, scale: SpacingScale) -> Self {
        self.spacing_scale = scale;
        self
    }

    /// Vertical gap between children (default `Md` = 12 px).
    pub fn gap(mut self, gap: SpaceToken) -> Self {
        self.gap = gap;
        self
    }

    /// Symmetric inset (padding) on all four sides.
    pub fn inset(mut self, inset: SpaceToken) -> Self {
        self.inset_left = inset;
        self.inset_right = inset;
        self.inset_top = inset;
        self.inset_bottom = inset;
        self
    }

    /// Independent horizontal / vertical inset.
    pub fn inset_xy(mut self, x: SpaceToken, y: SpaceToken) -> Self {
        self.inset_left = x;
        self.inset_right = x;
        self.inset_top = y;
        self.inset_bottom = y;
        self
    }

    /// Per-side inset.
    pub fn inset_sides(
        mut self,
        left: SpaceToken,
        right: SpaceToken,
        top: SpaceToken,
        bottom: SpaceToken,
    ) -> Self {
        self.inset_left = left;
        self.inset_right = right;
        self.inset_top = top;
        self.inset_bottom = bottom;
        self
    }

    /// Main-axis (vertical) alignment of children (default `Start`). Mirrors
    /// `VerticalLayout.alignment`. Only observable when
    /// the stack is taller than its content (via [`Self::height`]/[`Self::fill`]).
    ///
    /// `Stretch` distributes children along the main axis by interleaving
    /// `Fill` spacers between each adjacent pair — the same mechanic
    /// `EnzVStack` uses. It is not a pass-through.
    pub fn align(mut self, align: StackAlign) -> Self {
        self.align = align;
        self
    }

    /// Cross-axis (horizontal) alignment of children (default `Start`).
    /// `Stretch` fills the cross axis so `Fill`-width children stretch
    /// to the outer width.
    ///
    /// `Center` and `End` give the inner column a `Fill` width so there is
    /// space within which to align — Shrink width would collapse to the
    /// content and make the alignment invisible.
    pub fn cross(mut self, cross: StackCross) -> Self {
        self.cross = cross;
        self
    }

    /// Set the stack width (default `iced::Length::Shrink` — hug content).
    /// Use `iced::Length::Fill` to participate as a fill-width sibling
    /// in a parent row.
    pub fn width(mut self, width: impl Into<iced::Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the stack height (default `iced::Length::Shrink`).
    pub fn height(mut self, height: impl Into<iced::Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Fill both axes.
    pub fn fill(mut self) -> Self {
        self.width = iced::Length::Fill;
        self.height = iced::Length::Fill;
        self
    }

    /// Fill the cross axis (width) only.
    pub fn fill_width(mut self) -> Self {
        self.width = iced::Length::Fill;
        self
    }

    /// Elevation level 0-5 (drop shadow), forwarded to the underlying
    /// `EnzSurface`. Level 0 (default) = no shadow.
    pub fn elevation(mut self, level: u8) -> Self {
        self.elevation = level.min(5);
        self
    }

    /// Whether to clip content to the surface outline. Defaults to `true`.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Override the resolved fill colour directly, bypassing tone lookup.
    /// Forwarded to [`crate::surface::EnzSurface::fill`].
    pub fn fill_color(mut self, color: iced::Color) -> Self {
        self.fill_override = Some(color);
        self
    }

    /// Override the resolved border colour directly, bypassing border-style
    /// lookup. Forwarded to [`crate::surface::EnzSurface::border_color`].
    pub fn border_color(mut self, color: iced::Color) -> Self {
        self.border_color_override = Some(color);
        self
    }

    /// Push a child element.
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Convert into an iced [`Element`].
    pub fn into_element(self) -> Element<'a, Message> {
        let scale = &self.spacing_scale;
        let padding = Padding {
            top: self.inset_top.resolve(scale),
            right: self.inset_right.resolve(scale),
            bottom: self.inset_bottom.resolve(scale),
            left: self.inset_left.resolve(scale),
        };
        let gap = self.gap.resolve(scale);

        // Cross-axis (horizontal). `Stretch` and `Center`/`End` all need the
        // column to be Fill width: Stretch so Fill-width children stretch;
        // Center/End so the `align_x` has space within which to align rather
        // than collapsing to the content width.
        let col_width = match self.cross {
            StackCross::Start => iced::Length::Shrink,
            StackCross::Center | StackCross::End | StackCross::Stretch => iced::Length::Fill,
        };

        // Main-axis (vertical) alignment: instead of building a Column and
        // wrapping it with `apply_main_axis` (which treats Stretch as a
        // pass-through), pack children directly with Fill
        // spacers, exactly as `stack.rs::pack_column` does.
        let children_vec = self.children;
        let items: Vec<Element<'a, Message>> = match self.align {
            StackAlign::Start => children_vec,
            StackAlign::Center => {
                let mut v = Vec::with_capacity(children_vec.len() + 2);
                v.push(Space::new().height(iced::Length::Fill).into());
                v.extend(children_vec);
                v.push(Space::new().height(iced::Length::Fill).into());
                v
            }
            StackAlign::End => {
                let mut v = Vec::with_capacity(children_vec.len() + 1);
                v.push(Space::new().height(iced::Length::Fill).into());
                v.extend(children_vec);
                v
            }
            StackAlign::Stretch => {
                let last = children_vec.len().saturating_sub(1);
                let mut v = Vec::with_capacity(if children_vec.len() > 1 {
                    2 * children_vec.len() - 1
                } else {
                    children_vec.len()
                });
                for (i, c) in children_vec.into_iter().enumerate() {
                    v.push(c);
                    if i != last {
                        v.push(Space::new().height(iced::Length::Fill).into());
                    }
                }
                v
            }
        };

        let col: Column<'a, Message> = Column::with_children(items)
            .spacing(gap)
            .align_x(self.cross.to_iced())
            .width(col_width)
            .height(iced::Length::Fill);

        let border_width = self.border_width_explicit.unwrap_or(0.0);
        let mut surface = crate::surface::enz_surface(col)
            .tone(self.tone)
            .border_style(self.border_style)
            .border_width(border_width)
            .padding(padding)
            .width(self.width)
            .height(self.height)
            .elevation(self.elevation)
            .clip(self.clip);
        surface = match self.radius {
            RadiusInput::Corners(corners) => surface.radius_corners(corners),
            RadiusInput::Size(size) => surface.radius_size(size),
        };
        if let Some(t) = self.tokens {
            surface = surface.tokens(t);
        }
        if let Some(c) = self.fill_override {
            surface = surface.fill(c);
        }
        if let Some(c) = self.border_color_override {
            surface = surface.border_color(c);
        }
        surface.into_element()
    }
}

impl<'a, Message: 'a> From<EnzSurfaceStack<'a, Message>> for Element<'a, Message> {
    fn from(s: EnzSurfaceStack<'a, Message>) -> Self {
        s.into_element()
    }
}

/// Construct an empty surface stack (default `SurfaceTone::Surface`, `Md` gap, no inset).
pub fn enz_surface_stack<'a, Message: 'a>() -> EnzSurfaceStack<'a, Message> {
    EnzSurfaceStack {
        tone: SurfaceTone::default(),
        border_style: BorderStyle::default(),
        radius: RadiusInput::Size(RadiusSize::Md),
        border_width_explicit: None,
        tokens: None,
        gap: SpaceToken::default(),
        inset_left: SpaceToken::None,
        inset_right: SpaceToken::None,
        inset_top: SpaceToken::None,
        inset_bottom: SpaceToken::None,
        align: StackAlign::default(),
        cross: StackCross::default(),
        width: iced::Length::Shrink,
        height: iced::Length::Shrink,
        children: Vec::new(),
        spacing_scale: SpacingScale::default(),
        elevation: 0,
        clip: true,
        fill_override: None,
        border_color_override: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
