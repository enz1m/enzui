//! EnzContentPane: full-bleed pane with padded vertical content layout.
//!
//! A pane surface (background = theme surface) that runs a vertical
//! stack of children with uniform padding on left/right/bottom and a
//! configurable top padding. Set `flush_header(true)` when the first
//! child is a full-bleed toolbar/header that should sit flush against
//! the top of the pane, avoiding an empty band above the toolbar.
//!
//! iced 0.15's `container(Column::…)` + `Padding` maps directly onto
//! the `VerticalLayout` + `padding-*` properties.

use enzui_theme::{Palette, SpaceToken, SpacingScale};
use iced::widget::{Column, container};
use iced::{Color, Element, Length, Padding, Theme};

use crate::app_shell::DEFAULT_PAGE_INSET;
use crate::theme_lookup::resolve;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Design tokens for [`enz_content_pane`].
#[derive(Debug, Clone, Copy)]
pub struct ContentPaneTokens {
    /// Pane fill — the `surface` tier, resolved from the palette rather than
    /// pinned.
    pub pane_background: Color,
}

impl ContentPaneTokens {
    /// Derive tokens from a palette (the normal path; pass the live theme's
    /// palette so the pane tracks a runtime theme switch).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            pane_background: p.surface,
        }
    }
    /// Preset built from the built-in dark palette. Prefer
    /// [`ContentPaneTokens::from_palette`] with the live palette when one is in
    /// scope.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Preset built from the built-in light palette. Prefer
    /// [`ContentPaneTokens::from_palette`] with the live palette when one is in
    /// scope.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Semantic role for the default side and bottom content padding
/// (`SpaceToken::Lg`). Resolves to 16 px on the default scale.
///
/// Page-top spacing is a shell decision in
/// [`crate::app_shell::ShellTokens::page_inset`], not a second content-pane
/// token. Existing panes retain their symmetric padding for compatibility;
/// inside an adopted shell, use [`EnzContentPane::flush_header`] so the shell's
/// page inset is the only top-edge authority.
pub const CONTENT_PADDING_TOKEN: SpaceToken = SpaceToken::Lg;

/// Semantic role for the default child spacing (`SpaceToken::Md`).
/// Resolves to 12 px on the default scale.
pub const CONTENT_SPACING_TOKEN: SpaceToken = SpaceToken::Md;

/// Default side/bottom content padding on the stock scale (16 px).
///
/// The side inset is exactly two recommended page-top insets; deriving it from
/// [`DEFAULT_PAGE_INSET`] leaves one authority for the relationship while
/// preserving the content pane's existing 16 px compatibility default.
pub const DEFAULT_CONTENT_PADDING: f32 = DEFAULT_PAGE_INSET * 2.0;

/// Default content spacing in px on the default scale (`SpaceToken::Md` = 12).
pub const DEFAULT_CONTENT_SPACING: f32 = 12.0;

/// Sanitise an f32 dimension to finite, non-negative (non-finite → 0).
fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_content_pane(children)`.
pub fn enz_content_pane<'a, Message>(
    children: Vec<Element<'a, Message>>,
) -> EnzContentPane<'a, Message>
where
    Message: 'a,
{
    // Layout-time properties (`Container::padding`, `Column::spacing`) are
    // consumed in `layout()` where iced supplies no `&Theme`. The only way to
    // receive a custom density is explicit forwarding via
    // [`EnzContentPane::spacing_scale`]. Without it, the stock
    // ladder applies.
    let scale = SpacingScale::default();
    EnzContentPane {
        children,
        content_padding: CONTENT_PADDING_TOKEN.resolve(&scale),
        content_spacing: CONTENT_SPACING_TOKEN.resolve(&scale),
        flush_header: false,
        tokens: None,
        width: Length::Fill,
        height: Length::Fill,
        spacing_scale: None,
    }
}

/// Builder returned by [`enz_content_pane`].
#[must_use]
pub struct EnzContentPane<'a, Message> {
    children: Vec<Element<'a, Message>>,
    content_padding: f32,
    content_spacing: f32,
    flush_header: bool,
    tokens: Option<ContentPaneTokens>,
    width: Length,
    height: Length,
    /// Explicit spacing-scale override for layout-time consumers.
    /// When `Some`, the semantic tokens ([`CONTENT_PADDING_TOKEN`] /
    /// [`CONTENT_SPACING_TOKEN`]) are re-resolved against this ladder in
    /// `into_element()`, overriding the stock defaults baked at construction.
    /// An explicit `content_padding(..)` / `content_spacing(..)` value takes
    /// precedence (it is a raw px, not a token lookup).
    spacing_scale: Option<SpacingScale>,
}

impl<'a, Message> EnzContentPane<'a, Message>
where
    Message: 'a,
{
    /// Set the uniform padding on left/right/bottom (top too when
    /// `flush_header` is false). Default is `spacing-lg` = 16 px.
    /// Non-finite/negative → 0 (escape hatch).
    pub fn content_padding(mut self, p: f32) -> Self {
        self.content_padding = finite_nonneg(p);
        self
    }

    /// Set the inter-child vertical spacing. Default is `spacing-md` = 12 px.
    /// Non-finite/negative → 0.
    pub fn content_spacing(mut self, s: f32) -> Self {
        self.content_spacing = finite_nonneg(s);
        self
    }

    /// When `true`, zero the top content padding so the first child
    /// (a toolbar/header row) sits flush against the top of the pane.
    /// Horizontal + bottom padding are unchanged.
    pub fn flush_header(mut self, flush: bool) -> Self {
        self.flush_header = flush;
        self
    }

    /// Override the tokens (pane background).
    pub fn tokens(mut self, tokens: ContentPaneTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Outer height (default `Length::Fill`).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Forward a custom [`SpacingScale`] so layout-time consumers
    /// (`Container::padding`, `Column::spacing`) resolve against it instead
    /// of the stock ladder.
    ///
    /// `Widget::layout` has no `&Theme` parameter — the spacing scale must be
    /// threaded in explicitly at build time. Without this setter, the pane
    /// uses the compile-time default density. Pass
    /// `theme.spacing` from the app's `EnzTheme` to make the pane follow a
    /// custom density.
    ///
    /// An explicit [`Self::content_padding`] / [`Self::content_spacing`]
    /// value takes precedence — it is a raw px override and does not
    /// go through a token lookup.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
}

impl<'a, Message> EnzContentPane<'a, Message>
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

impl<'a, Message> From<EnzContentPane<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzContentPane<'a, Message>) -> Self {
        let captured = w.tokens;

        // If a custom spacing scale was forwarded and no explicit
        // px override was set, re-resolve the semantic tokens against the
        // custom ladder. This is the layout-time forwarding seam: the px
        // values that feed `Container::padding` and `Column::spacing` now
        // move with the custom density.
        let default_scale = SpacingScale::default();
        let (content_padding, content_spacing) = match w.spacing_scale {
            Some(ref scale) => {
                // Only re-resolve when the caller did not pin the value via
                // the raw-px setter. The px setters store their value; the
                // constructor stores the stock-ladder value, so "equals the
                // stock resolution" is the sentinel for "was not overridden".
                let stock_pad = CONTENT_PADDING_TOKEN.resolve(&default_scale);
                let stock_gap = CONTENT_SPACING_TOKEN.resolve(&default_scale);
                let pad = if (w.content_padding - stock_pad).abs() < f32::EPSILON {
                    CONTENT_PADDING_TOKEN.resolve(scale)
                } else {
                    w.content_padding
                };
                let gap = if (w.content_spacing - stock_gap).abs() < f32::EPSILON {
                    CONTENT_SPACING_TOKEN.resolve(scale)
                } else {
                    w.content_spacing
                };
                (pad, gap)
            }
            None => (w.content_padding, w.content_spacing),
        };

        let padding = Padding {
            top: if w.flush_header { 0.0 } else { content_padding },
            right: content_padding,
            bottom: content_padding,
            left: content_padding,
        };
        let column = Column::with_children(w.children)
            .spacing(content_spacing)
            .padding(padding)
            .width(Length::Fill)
            .height(Length::Fill);
        container(column)
            .width(w.width)
            .height(w.height)
            .style(move |theme: &Theme| {
                let tokens =
                    captured.unwrap_or_else(|| resolve(theme, ContentPaneTokens::from_palette));
                container::Style {
                    background: Some(tokens.pane_background.into()),
                    ..Default::default()
                }
            })
            .into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
