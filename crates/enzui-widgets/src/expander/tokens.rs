//! Frame variant and colour/geometry tokens for [`super::EnzExpander`].

use super::*;

// ---------------------------------------------------------------------------
// Variant + tokens
// ---------------------------------------------------------------------------

/// Frame style: layout-stable flat (default), visible Card, or structural Plain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExpanderVariant {
    /// Framed card with a surface fill, border and corner radius.
    Card,
    /// Keeps Card's frame/divider geometry but paints their ink transparent.
    #[default]
    Flat,
    /// Flush/borderless — removes card frame structure entirely.
    Plain,
}

/// Where the expander's hover feedback lands.
///
/// The default [`Header`](HoverScope::Header) preserves header-only
/// behaviour: only the header row picks up a hover fill. [`Group`](HoverScope::Group)
/// tints the header and its expanded body as one unit — the sidebar-section
/// shape (VS Code Explorer sections, Slack channel groups, Finder tags)
/// where a workspace header and its child rows read as a single hover
/// scope. Delivered by wrapping the whole expander in an
/// [`EnzHoverGroup`](crate::hover_group::EnzHoverGroup), so the
/// group tint is resolved at draw time from `cursor.is_over(bounds)` over
/// the whole expander and needs no app state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HoverScope {
    /// Hover is header-only — the default expander behaviour. A hover on
    /// the collapsed header tints the header row; the expanded body carries
    /// no group tint of its own. This is the correct scope for a settings
    /// or advanced-options disclosure where the body is a distinct region
    /// from the header.
    #[default]
    Header,
    /// Hover is group-scoped — the header and the expanded body tint as one
    /// unit while the pointer is anywhere inside the whole expander. Wraps
    /// the expander in [`EnzHoverGroup`](crate::hover_group::EnzHoverGroup) so
    /// the tint is resolved at draw time. This is the correct scope for a
    /// sidebar section: a workspace header + its child rows that belong
    /// together. The group quad paints behind the content, so a nested
    /// [`list_row_surface`](crate::list_row_surface) still paints its own
    /// stronger per-row hover on top — both scopes compose.
    Group,
}

/// Colour + geometry tokens for an expander.
#[derive(Debug, Clone, Copy)]
pub struct ExpanderTokens {
    /// Card surface fill (default `surface_elevated`).
    pub background: Color,
    /// Card border colour (default `border`).
    pub border: Color,
    /// Header hover fill (default `overlay_hover()` — the composited
    /// `Overlay` role from `Palette`: a translucent `text`-tinted wash, not
    /// an opaque surface token).
    ///
    /// # Why an overlay, not `surface_hover`
    ///
    /// An expander is placed flush on whatever surface its parent paints — a
    /// sidebar body is `surface_alt`, a settings pane is `surface`/
    /// `surface_elevated`. The opaque `surface_hover` token is
    /// byte-identical to `surface_alt` in both shipped presets: on a sidebar
    /// it would repaint the exact colour already under the header, giving
    /// the control *zero* hover feedback. A
    /// composited overlay is the framework's answer for a flush hover on an
    /// unknown surface (the same role SidebarRow and ghost buttons read): it
    /// is a translucent wash over whatever is behind, so it is visible on
    /// `surface_alt`, `surface`, and `surface_elevated` alike, by
    /// construction.
    pub hover_background: Color,
    /// Title ink (default `text`).
    pub title_color: Color,
    /// Subtitle ink (default `text_muted`).
    ///
    /// Read per-frame by the subtitle run. Equal to what
    /// [`crate::enz_text::TextTone::Muted`] resolves to by default, so pinning
    /// it is the only way to observe it — which is the point: it exists so an
    /// expander subtitle can be themed independently of every other muted
    /// caption.
    pub subtitle_color: Color,
    /// Leading icon and chevron tint (default `text_muted`).
    pub icon_color: Color,
    /// Divider colour between the header and expanded content (default `border`).
    pub divider_color: Color,

    /// Card corner radius in px (default 12).
    pub card_radius: f32,
    /// Header row height in px (default 44).
    pub header_height: f32,
    /// Inset around the expanded content in px (default 16).
    pub content_inset: f32,
    /// Vertical gap between expanded content children in px (default 8).
    pub content_gap: f32,
    /// Leading icon size in px (default 16).
    pub icon_size: f32,
    /// Chevron glyph size in px (default 12).
    pub chevron_size: f32,
    /// Title text size in px (default 13).
    pub title_size: f32,
    /// Subtitle text size in px (default 12).
    pub subtitle_size: f32,
}

impl ExpanderTokens {
    /// Derive from an enzUI palette. Sizes follow defaults:
    /// - `header-height` = `list-row-height + 8` = 44 px.
    /// - `content-inset` = `spacing-lg` = 16 px.
    /// - `content-gap` = `spacing-sm` = 8 px.
    /// - `icon-size` = `icon-sm` = 16 px.
    /// - `chevron-size` = `icon-xs` = 12 px.
    /// - `card-radius` = 12 px.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_elevated,
            // A shade of the card's own fill.
            border: crate::surface_edge::edge_color(
                p.surface_elevated,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            // The header is painted flush on its parent surface, so a
            // hover must be a composited `Overlay` (a translucent `text`
            // wash over whatever is behind), not the opaque `surface_hover`
            // token — which is byte-identical to `surface_alt` in both
            // presets and therefore invisible on a `surface_alt` sidebar. The
            // header button paints this as `Background::Color`, so its alpha
            // composites over the real backdrop (card fill or sidebar).
            hover_background: p.overlay_hover(),
            title_color: p.text,
            subtitle_color: p.text_muted,
            icon_color: p.text_muted,
            divider_color: p.border,
            card_radius: 12.0,
            header_height: 44.0,
            content_inset: 16.0,
            content_gap: 8.0,
            icon_size: 16.0,
            chevron_size: 12.0,
            // Callout (13px) semibold title, Footnote (12px)
            // subtitle.
            title_size: enzui_theme::text_scale::ui_px(13.0),
            subtitle_size: enzui_theme::text_scale::ui_px(12.0),
        }
    }

    /// Normalize the raw `f32` geometry so no non-finite (NaN / ±inf) or
    /// negative value from an external caller can reach renderer coordinates.
    ///
    /// Every `f32` field is a px extent that feeds layout: `card_radius` a
    /// `Border` radius, `header_height` a fixed height, `content_inset` /
    /// `content_gap` a `Padding` / spacing, `icon_size` / `chevron_size` glyph
    /// sizes, and `title_size` / `subtitle_size` text sizes. The public
    /// [`EnzExpander::tokens`] setter runs every override through this single
    /// normalization point.
    ///
    /// Every field is [`FiniteNonNeg`](enzui_core::metrics::FiniteNonNeg):
    /// px extents that may legitimately be zero, so zero is
    /// preserved, not floored. Fallbacks are the [`from_palette`](Self::from_palette)
    /// values. Colours pass through.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::from_palette(&Palette::dark());
        Self {
            card_radius: FiniteNonNeg::new_or(self.card_radius, d.card_radius).get(),
            header_height: FiniteNonNeg::new_or(self.header_height, d.header_height).get(),
            content_inset: FiniteNonNeg::new_or(self.content_inset, d.content_inset).get(),
            content_gap: FiniteNonNeg::new_or(self.content_gap, d.content_gap).get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, d.icon_size).get(),
            chevron_size: FiniteNonNeg::new_or(self.chevron_size, d.chevron_size).get(),
            title_size: FiniteNonNeg::new_or(self.title_size, d.title_size).get(),
            subtitle_size: FiniteNonNeg::new_or(self.subtitle_size, d.subtitle_size).get(),
            ..*self
        }
    }

    /// Dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Light preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}
