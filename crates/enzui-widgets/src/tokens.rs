//! Design tokens for enzUI-ICED widgets.
//!
//! Component-level tokens derived from [`enzui_theme::Palette`].
//! These map the core palette into per-widget colors/dimensions.
//!
//! # Where to import a component's tokens
//!
//! **The canonical import for a component-owned token type is the
//! component's own module**, not this shared `tokens` module: for example
//! `enzui_widgets::button::ButtonTokens` or
//! `enzui_widgets::card::CardTokens`.
//!
//! [`ButtonTokens`] and [`ButtonSizeTokens`] stay re-exported here as a
//! compatibility path. Genuinely *shared* families (e.g. [`FieldSizeTokens`],
//! the inline-control ladder no single component owns) live here by design.
//!
//! # Token anatomy
//!
//! Field names encode the anatomy after the component itself:
//!
//! ```text
//! component -> variant/part -> property -> interaction state -> exceptional unit
//! ```
//!
//! - parts — `container`/`background`, `content`/`foreground`, `text`,
//!   `icon`, `border`, `track`, `thumb`, `focus_ring`, …
//! - states — rest (implicit), `hover`, `pressed`, `selected`,
//!   `disabled`; combined-state ordering is fixed.
//! - geometry — `padding` inside an edge, `gap` between siblings, `inset`
//!   from an edge, `width`/`height`/`radius` for dimensions.
//! - tones — `error`, `danger` and `destructive` are *not*
//!   interchangeable: `error` is validation, `danger` intrinsic risk,
//!   `destructive` an action; per-widget callouts govern which publishes.
//! - units — px is implicit for `f32` dimensions; suffixes (`_ms`,
//!   `_fraction`, `_alpha`) mark non-px or dimensionless values.
//!
//! Legacy spellings (`bg`/`fg`/`radius_lg`) remain for source compatibility;
//! the generated `docs/tokens.md` index records their canonical path.
//!
//! # `.tokens(...)` — the one setter, three resolution modes
//!
//! A passed value is a fixed per-instance override; omitting the call follows
//! the runtime theme for every channel derived at render time.
//!
//! - pin — rustdoc says the value bypasses runtime-theme derivation
//!   (the pin-a-snapshot contract `ButtonTokens`/`EnzButton::tokens`
//!   originated).
//! - override — rustdoc says overrides / applies / sets / configures,
//!   without pinning specifics.
//! - runtime — installs a resolver called every draw (no live example
//!   in the crate).
//!
//! [`ButtonTokens::from_palette`] is the canonical palette-backed snapshot;
//! `light()`/`dark()` are convenience constants, not the runtime path.
//! `docs/tokens.md` / `docs/tokens.json` (from
//! `tests/contracts/token_index.rs`) record import path,
//! `from_palette`/default availability and resolution mode. Regenerate with:
//!
//! ```sh
//! ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index
//! ```

use enzui_theme::Palette;
use iced::Color;

/// Button design tokens — derived from a [`Palette`].
///
/// All visual properties of the button are driven by these tokens,
/// making it easy to swap themes without modifying widget code.
#[derive(Debug, Clone)]
pub struct ButtonTokens {
    // Primary kind
    /// Primary button background (default state).
    pub primary_bg: Color,
    /// Primary button background on hover.
    pub primary_hover_bg: Color,
    /// Primary button background when pressed.
    pub primary_pressed_bg: Color,
    /// Primary button label colour.
    pub primary_fg: Color,
    /// Primary button 1 px border colour.
    ///
    /// The Primary rest fill sits ~1.01:1 from its page in both presets — a
    /// filled-only Primary has no visible boundary — and the fill cannot
    /// move without a coordinated ladder retune (light text forces both
    /// `fill L <= 0.214` for a 3:1 boundary and `fill L >= 0.349` for 4.5:1
    /// label: mutually exclusive). The boundary is therefore a shared border
    /// role: [`Palette::control_border`], measuring >= 3:1 against every
    /// surface and against the Primary fill itself, in every state. Painted
    /// in `Active`/`Hovered`/`Pressed`; zeroed when disabled, so enabled and
    /// disabled stay visually separable.
    pub primary_border: Color,
    /// Primary button rest-state border width in px.
    ///
    /// Defaults to `1.0` — the same hairline Secondary and Outlined already
    /// paint via [`Self::border_width`]. Kept separate from `border_width`
    /// so a caller can pin one without moving the other.
    pub primary_border_width: f32,

    // Secondary kind
    /// Secondary button background (default state).
    pub secondary_bg: Color,
    /// Secondary button background on hover.
    pub secondary_hover_bg: Color,
    /// Secondary button background when pressed.
    pub secondary_pressed_bg: Color,
    /// Secondary button label colour.
    pub secondary_fg: Color,
    /// Secondary button 1px border colour: by default a translucent tint of
    /// the text colour, a shade of whatever the button sits on.
    pub secondary_border: Color,
    /// Outlined (and outline-only) button border colour. Defaults to the
    /// `border_strong` role.
    pub outlined_border: Color,

    // Ghost kind
    /// Ghost button background (transparent by default).
    pub ghost_bg: Color,
    /// Ghost button background on hover.
    pub ghost_hover_bg: Color,
    /// Ghost button background when pressed.
    pub ghost_pressed_bg: Color,
    /// Ghost button label colour (usually the accent).
    pub ghost_fg: Color,
    /// Ghost button hover label colour. Defaults to [`Self::ghost_fg`]
    /// in [`ButtonTokens::from_palette`] so existing callers are
    /// pixel-identical. Set brighter to express the "flat at rest, ink
    /// brightens on hover" idiom the design system documents on the
    /// toolbar-icon and breadcrumb hover colours.
    pub ghost_hover_fg: Color,
    /// Ghost button pressed label colour. Defaults to [`Self::ghost_fg`]
    /// for symmetry with the hover slot.
    pub ghost_pressed_fg: Color,
    /// Ghost button border colour.
    ///
    /// Transparent by default: a ghost button is identified by its accent ink
    /// and hover/pressed wash, not a permanent box. The stroke retains its
    /// width so changing the default does not move surrounding layout. Set
    /// this to a visible colour for an explicitly outlined ghost.
    pub ghost_border: Color,
    /// Ghost button rest-state border width in px.
    ///
    /// Defaults to [`Self::border_width`] (`1.0`) though the default colour
    /// is transparent; `0.0` stays available for callers wanting no stroke
    /// geometry. Drawn in the *interactive* states so hover/pressed washes
    /// sit inside rest's edge; the disabled branch still zeroes it (disabled
    /// chrome comes from `disabled_bg`/`disabled_fg`), keeping disabled
    /// distinguishable from rest with no second per-kind decision.
    pub ghost_border_width: f32,

    // Accent kind (the "suggested action" CTA)
    /// Resolved accent fill for a resting button.
    pub accent_bg: Color,
    /// Accent button background on hover.
    pub accent_hover_bg: Color,
    /// Accent button background when pressed.
    pub accent_pressed_bg: Color,
    /// Label and icon ink paired with the resting accent fill.
    pub accent_fg: Color,
    /// Label and icon ink paired with the hovered accent fill.
    pub accent_hover_fg: Color,
    /// Label and icon ink paired with the pressed accent fill.
    pub accent_pressed_fg: Color,

    // Destructive kind
    /// Destructive button background.
    pub destructive_bg: Color,
    /// Destructive button background on hover.
    pub destructive_hover_bg: Color,
    /// Destructive button background when pressed.
    pub destructive_pressed_bg: Color,
    /// Label and icon ink paired with the resolved danger fill.
    pub destructive_fg: Color,
    /// Readable danger ink for transparent destructive buttons and their outlines.
    pub quiet_destructive_fg: Color,

    // Success kind (the positive / "go" confirmation CTA)
    /// Resolved success fill for a resting button.
    pub success_bg: Color,
    /// Success button background on hover.
    pub success_hover_bg: Color,
    /// Success button background when pressed.
    pub success_pressed_bg: Color,
    /// Label and icon ink paired with the resolved success fill.
    pub success_fg: Color,

    // Disabled state
    /// Background used for any button kind when disabled.
    pub disabled_bg: Color,
    /// Label colour used for any button kind when disabled.
    pub disabled_fg: Color,

    // Focus
    /// Focus-ring outline colour.
    pub focus_ring: Color,
    /// Focus-ring outline width (px).
    pub focus_ring_width: f32,

    // Border
    /// Border width used by kinds that show one (secondary, outlined).
    pub border_width: f32,
}

impl ButtonTokens {
    /// Derive button tokens from a palette.
    pub fn from_palette(palette: &Palette) -> Self {
        let accent_hover_bg = distinct_or(
            palette.accent_hover,
            palette.accent,
            crate::color::darken(palette.accent, 0.08),
        );
        let accent_pressed_bg = distinct_or(
            palette.accent_pressed,
            palette.accent,
            crate::color::darken(palette.accent, 0.15),
        );
        let accent = palette.filled_color(palette.accent);
        let destructive = palette.filled_color(palette.destructive);
        let success = palette.filled_color(palette.success);
        let (accent_bg, accent_hover_bg, accent_pressed_bg) = if palette.is_dark() {
            (
                crate::color::darken(accent.color, 0.06),
                palette.filled_color(accent_hover_bg).color,
                crate::color::darken(palette.filled_color(accent_pressed_bg).color, 0.12),
            )
        } else {
            (accent.color, accent_hover_bg, accent_pressed_bg)
        };
        // On glass the accent is tinted glass: the backdrop shows through it.
        let (accent_bg, accent_hover_bg, accent_pressed_bg) = if palette.is_on_glass() {
            (
                crate::color::with_alpha(accent_bg, 0.80),
                crate::color::with_alpha(accent_hover_bg, 0.88),
                crate::color::with_alpha(accent_pressed_bg, 0.72),
            )
        } else {
            (accent_bg, accent_hover_bg, accent_pressed_bg)
        };
        // On glass a light rim edges the fill instead of a darker shade.
        // On glass a ghost control takes the ink, as glass toolbars do: an
        // accent glyph loses contrast over a warm or blue backdrop.
        let ghost_fg = if palette.is_on_glass() {
            palette.text
        } else {
            palette.accent
        };
        let primary_border = if palette.is_on_glass() || palette.is_high_contrast() {
            palette.control_border
        } else {
            crate::surface_edge::edge_color(
                palette.button_primary_bg,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            )
        };
        Self {
            primary_bg: palette.button_primary_bg,
            primary_hover_bg: palette.button_primary_hover_bg,
            primary_pressed_bg: palette.button_primary_pressed_bg,
            primary_fg: palette.text,
            // A shade of the fill itself: the raised fill identifies
            // the button, and the edge only holds its shape where the fill
            // meets a surface of nearly the same tone. The label, not a 3:1
            // outline, is what identifies a text button (WCAG 1.4.11 asks for
            // contrast on the visual information needed to identify it).
            primary_border,
            // Neutral buttons have no edge: their fill sets them apart from
            // the page. Glass and high contrast keep the line.
            primary_border_width: if palette.is_on_glass() || palette.is_high_contrast() {
                1.0
            } else {
                0.0
            },

            // secondary_bg is the plain surface (not surface_alt); hover/pressed
            // map to the *replacement* `surface_hover` role — a static swap,
            // not a composited overlay (`SurfaceRole::Hover`). Was
            // `surface_alt`: identical by default alias but semantically the
            // hover role, so a diverging theme renders the intended one.
            // A translucent tint of the text colour, so the button is a shade
            // of whatever it sits on: the page, a card, or a toolbar that is
            // itself `surface_alt`. An opaque step vanishes on a
            // surface of that same step. The edge is the same tint one step
            // stronger, again a shade of the backdrop rather than a line in
            // another colour. A theme's own `surface_hover` still drives the
            // hover when it is distinct from `surface_alt`.
            secondary_bg: crate::color::with_alpha(palette.text, 0.07),
            secondary_hover_bg: distinct_or(
                palette.surface_hover,
                palette.surface_alt,
                crate::color::with_alpha(palette.text, 0.11),
            ),
            secondary_pressed_bg: crate::color::with_alpha(palette.text, 0.15),
            secondary_fg: palette.text,
            secondary_border: if palette.is_high_contrast() {
                palette.control_border
            } else {
                Color::TRANSPARENT
            },
            // An outline is the whole of an Outlined button, so it stays a
            // visible line: `border_strong`, softer than the 3:1
            // `control_border` but still clear of a light page, where the
            // plain `border` role all but vanishes.
            outlined_border: palette.border_strong,

            ghost_bg: Color::TRANSPARENT,
            // Composited hover overlay (`SurfaceRole::Overlay`): the shared
            // `text`-at-0.07 wash. Reads the single `Palette::overlay_hover`
            // definition so this and `SidebarRow` hover cannot drift.
            ghost_hover_bg: palette.overlay_hover(),
            // Composited pressed overlay (`SurfaceRole::PressedOverlay`): the
            // shared `text`-at-0.12 wash, one alpha step stronger than hover.
            // Reads the single `Palette::overlay_pressed` definition so this
            // cannot drift from the hover wash or from a theme override.
            ghost_pressed_bg: palette.overlay_pressed(),
            ghost_fg,
            // Default hover/pressed fg to the resting ghost_fg so every
            // caller that never sets them is pixel-identical. A caller opts into
            // the "ink brightens on hover" idiom by overriding these via
            // `.tokens(..)`.
            ghost_hover_fg: ghost_fg,
            ghost_pressed_fg: ghost_fg,
            // Ghost controls are chromeless at rest by default. Keep the
            // 1px stroke geometry but make its ink transparent: callers that
            // opt into a framed ghost can set `ghost_border`, while the stock
            // toolbar/titlebar shape loses no pixel of layout.
            ghost_border: Color::TRANSPARENT,
            ghost_border_width: 1.0,

            accent_bg,
            accent_hover_bg,
            accent_pressed_bg,
            accent_fg: accent_fg(palette),
            accent_hover_fg: accent_state_fg(palette, accent_hover_bg),
            accent_pressed_fg: accent_state_fg(palette, accent_pressed_bg),

            destructive_bg: destructive.color,
            // Overridable token fields rather than literal
            // darken(bg, 0.08/0.15) inside style_for.
            destructive_hover_bg: crate::color::darken(destructive.color, 0.08),
            destructive_pressed_bg: crate::color::darken(destructive.color, 0.15),
            destructive_fg: destructive.text,
            // Borderless-danger ink lands on the app surface, not on a
            // fill, so it uses the contrast-audited danger *text* role rather
            // than the chip/border fill colour.
            quiet_destructive_fg: palette.tone_error_fg,

            success_bg: success.color,
            success_hover_bg: crate::color::darken(success.color, 0.08),
            success_pressed_bg: crate::color::darken(success.color, 0.15),
            success_fg: success.text,

            disabled_bg: palette.surface_alt,
            disabled_fg: palette.disabled,

            focus_ring: palette.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),

            border_width: 1.0,
        }
    }

    /// Dark theme tokens (convenience, derives from dark palette).
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light theme tokens (convenience, derives from light palette).
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Return `candidate` when it is visibly different from `base`, else
/// `fallback`. Guards the case where a runtime theme has no dedicated
/// hover/pressed accent slot and reports the base colour for all three, which
/// would leave a filled control visually inert under the pointer.
fn distinct_or(candidate: Color, base: Color, fallback: Color) -> Color {
    let same = (candidate.r - base.r).abs() < 1.0 / 255.0
        && (candidate.g - base.g).abs() < 1.0 / 255.0
        && (candidate.b - base.b).abs() < 1.0 / 255.0;
    if same { fallback } else { candidate }
}

/// Pick the ink for anything drawn on top of a filled surface: labels,
/// checkmarks, avatar initials, icons on a thumb.
///
/// The one helper for foreground-on-filled-control. It follows
/// [`Palette::ink_on`]: white on fills that read best with white (an accent
/// blue, a destructive red), a near-black in the fill's own hue on fills too
/// light for it, never black on blue. On a neutral surface where the
/// palette's own `text` already reads at 4.5:1 with the same polarity, that
/// text colour is used, so body text keeps the theme's ink.
///
/// Text on a fill the caller can choose should come from
/// [`Palette::filled_color`], which deepens the fill until white reaches
/// 4.5:1; [`ink_for_with_floor`] verifies a ratio.
pub fn ink_for(fill: Color, palette: &Palette) -> Color {
    let ink = Palette::ink_on(fill);
    let light = |c: Color| {
        crate::accessibility_metrics::contrast_ratio(c, Color::BLACK)
            > crate::accessibility_metrics::contrast_ratio(c, Color::WHITE)
    };
    if light(palette.text) == light(ink)
        && crate::accessibility_metrics::contrast_ratio(palette.text, fill) >= 4.5
    {
        palette.text
    } else {
        ink
    }
}

/// Why [`ink_for_with_floor`] could not verify an ink.
///
/// The report carries the best candidate so the caller can make an explicit
/// product decision (paint it anyway, change the fill, or omit the label)
/// without making failure indistinguishable from success.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum InkContrastFailure {
    /// One or more fill channels are non-finite or outside iced's `0..=1`
    /// colour range, so no contrast measurement is meaningful.
    InvalidFill {
        /// A deterministic palette fallback; no contrast claim is made for it.
        best: Color,
        /// The malformed fill supplied by the caller.
        fill: Color,
    },
    /// `fill` is translucent, so its painted colour is unknowable without the
    /// surface behind it. Composite it first, then call the resolver again.
    UnresolvedBackdrop {
        /// The same best-effort candidate [`ink_for`] would return.
        best: Color,
        /// The unresolved translucent fill.
        fill: Color,
    },
    /// The best candidate did not reach the requested floor.
    BelowFloor {
        /// The same best-effort candidate [`ink_for`] would return.
        best: Color,
        /// Its measured contrast ratio against `fill`.
        achieved_ratio: f32,
        /// The ratio required by the requested floor.
        required_ratio: f32,
    },
}

impl InkContrastFailure {
    /// Return the best-effort candidate for a caller that deliberately chooses
    /// to paint despite the failed verification.
    pub const fn best(self) -> Color {
        match self {
            Self::InvalidFill { best, .. }
            | Self::UnresolvedBackdrop { best, .. }
            | Self::BelowFloor { best, .. } => best,
        }
    }
}

/// Resolve ink for an opaque `fill` and verify a semantic contrast `floor`.
///
/// Unlike [`ink_for`], success is a claim: the colour measured at or above
/// [`ContrastFloor::minimum_ratio`](enzui_theme::ContrastFloor::minimum_ratio).
/// Failure returns [`InkContrastFailure`], never a silent best candidate.
/// The fill is never mutated — darkening artwork is a product decision, not
/// rendering policy. A translucent `fill` is rejected (its painted colour
/// needs the missing backdrop): composite via [`crate::composite_over`]
/// first. For finite opaque sRGB and today's floors, black/white guarantee a
/// 4.5:1 clearer; the failure channel keeps that invariant observable.
///
/// # Errors
///
/// An [`InkContrastFailure`] for an invalid fill, an unresolved backdrop, or no ink reaching the
/// floor.
pub fn ink_for_with_floor(
    fill: Color,
    palette: &Palette,
    floor: enzui_theme::ContrastFloor,
) -> Result<Color, InkContrastFailure> {
    let required_ratio = floor.minimum_ratio();
    if ![fill.r, fill.g, fill.b, fill.a]
        .into_iter()
        .all(|channel| channel.is_finite() && (0.0..=1.0).contains(&channel))
    {
        return Err(InkContrastFailure::InvalidFill {
            best: palette.text,
            fill,
        });
    }

    let best = ink_for(fill, palette);
    if fill.a < 1.0 {
        return Err(InkContrastFailure::UnresolvedBackdrop { best, fill });
    }

    let achieved_ratio = crate::accessibility_metrics::contrast_ratio(best, fill);
    if achieved_ratio.is_finite() && achieved_ratio >= required_ratio {
        Ok(best)
    } else if let Some(verified) = [palette.text, Color::WHITE, Color::BLACK]
        .into_iter()
        .find(|ink| crate::accessibility_metrics::contrast_ratio(*ink, fill) >= required_ratio)
    {
        // The preferred polarity cannot reach a strict floor on this fill
        // (white on a mid blue at 4.5:1); a verified ink wins over style.
        Ok(verified)
    } else {
        Err(InkContrastFailure::BelowFloor {
            best,
            achieved_ratio,
            required_ratio,
        })
    }
}

/// Label and icon ink for the palette's resolved accent control fill.
///
/// Pair with [`Palette::filled_color`], rather than the raw link/indicator accent.
pub fn accent_fg(palette: &Palette) -> Color {
    palette.filled_color(palette.accent).text
}

/// The label on a hovered or pressed accent fill: the resting label while it
/// still reads at 4.5:1 there, so the label does not change colour under the
/// pointer, else the best ink for that fill.
fn accent_state_fg(palette: &Palette, fill: Color) -> Color {
    let resting = accent_fg(palette);
    if palette.is_dark() || crate::accessibility_metrics::contrast_ratio(resting, fill) >= 4.5 {
        resting
    } else {
        ink_for(fill, palette)
    }
}

/// Resolve the glyph ink for prose sitting inside a persisted annotation
/// highlight (a "yellow marker" run) over a known `paper` surface.
///
/// A persisted highlight paints a caller-owned, often translucent fill: the
/// reader experiences ink on the *composite*, not on `paper`. The shipped
/// defect: Reader Dark paper (`#222222`) + `yellow` at 0.55 alpha composites
/// near `#9C8B47`, where light prose (`#E0E0E0`) measures 2.47:1 — while
/// the same ink on raw paper reads 12+:1 to a token-only audit that never
/// composites. Naming and resolving that defect is this role's job.
///
/// Exactly three states: `prose_ink` clearing the floor returns
/// byte-identical (legible author intent is not relit); failing ink
/// returns the higher-contrast of pure black/white (one always clears a
/// text-body floor on an opaque composite); `highlight_fill.a == 0.0` returns
/// `prose_ink`, so unconditional callers pay nothing on unmarked runs. No
/// palette candidates: a persisted highlight is user annotation on a
/// reading-theme paper, not chrome — mixing chrome inks in would drag
/// legible ink toward the chrome hue for no gain.
///
/// The single seam before setting a span's `.color(..)`. Callers whose paper
/// is caller-owned must forward the paper they know; the helper cannot invent
/// a backdrop.
pub fn annotation_glyph_ink(
    prose_ink: Color,
    highlight_fill: Color,
    paper: Color,
    floor: enzui_theme::ContrastFloor,
) -> Color {
    // No highlight at all → nothing to resolve against.
    if !highlight_fill.a.is_finite() || highlight_fill.a <= 0.0 {
        return prose_ink;
    }
    let composite = crate::accessibility_metrics::composite_over(highlight_fill, paper);
    let target = floor.minimum_ratio();
    let prose_contrast = crate::accessibility_metrics::contrast_ratio(prose_ink, composite);
    if prose_contrast.is_finite() && prose_contrast >= target {
        return prose_ink;
    }
    // Prose ink failed the floor on the composite. Fall back to the pole
    // (black/white) that maximises contrast against the composite; one of the
    // two always clears the floor for an opaque composite and today's floors.
    let white_ratio = crate::accessibility_metrics::contrast_ratio(Color::WHITE, composite);
    let black_ratio = crate::accessibility_metrics::contrast_ratio(Color::BLACK, composite);
    if white_ratio >= black_ratio {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

/// The ink a file-type glyph paints in, for `tone` over `backdrop`.
///
/// One colour per [`FileTypeTone`](enzui_icons::file_type::FileTypeTone), so told-apart families never share
/// pixels: shared roles carry the convention (PDF red, prose blue, ebooks
/// yellow, spreadsheets green) and an otherwise-occupied family separates
/// *by hue* — what survives the contrast raise. The result clears the
/// non-text floor (3:1, WCAG SC 1.4.11 graphical objects), not the text
/// floor, so the family hue is not washed out. Pass the opaque fill the
/// glyph really paints on (a selected row's, when selected). The raise holds
/// hue/chroma fixed, so a near-luminance caller fill may top out below 3:1;
/// the best reachable ink returns rather than a different hue.
pub fn file_type_ink(
    tone: enzui_icons::file_type::FileTypeTone,
    palette: &Palette,
    backdrop: Color,
) -> Color {
    use enzui_icons::file_type::FileTypeTone as Tone;

    // `mix` separates a family from the role it shares by moving it toward a
    // neighbouring role's hue. Lightness separation is not available here: the
    // floor raise below normalises lightness along a fixed hue, so two families
    // told apart only by shade would converge on whichever backdrop needed the
    // raise.
    let mix = |base: Color, toward: Color, factor: f32| {
        crate::accessibility_metrics::composite_over(
            Color {
                a: factor,
                ..toward
            },
            base,
        )
    };
    let raw = match tone {
        Tone::Neutral => palette.text_muted,
        Tone::Document => palette.info,
        Tone::Data => mix(palette.info, palette.tone_success_fg, 0.5),
        Tone::Pdf => palette.tone_error_fg,
        Tone::Presentation => mix(palette.tone_error_fg, palette.tone_warning_fg, 0.35),
        Tone::Video => mix(palette.tone_error_fg, palette.accent, 0.5),
        Tone::Archive => mix(palette.tone_warning_fg, palette.text_muted, 0.4),
        Tone::Package => mix(palette.tone_warning_fg, palette.tone_error_fg, 0.35),
        Tone::Audio => mix(palette.tone_warning_fg, palette.tone_success_fg, 0.5),
        Tone::Ebook => palette.tone_warning_fg,
        Tone::Spreadsheet => palette.tone_success_fg,
        Tone::Transfer => mix(palette.tone_success_fg, palette.accent, 0.45),
        Tone::Image => palette.accent,
        Tone::Code => mix(palette.accent, palette.info, 0.6),
        Tone::Font => mix(palette.tone_error_fg, palette.accent, 0.28),
    };
    enzui_theme::raise_to_floor(raw, backdrop, enzui_theme::ContrastFloor::NonText).color()
}

/// Size tokens for each button size variant.
#[derive(Debug, Clone, Copy)]
pub struct ButtonSizeTokens {
    /// Total height.
    pub height: f32,
    /// Horizontal padding.
    pub padding_x: f32,
    /// Text size.
    pub text_size: f32,
    /// Icon size.
    pub icon_size: f32,
    /// Border radius.
    pub radius: f32,
    /// Gap between icon and label.
    pub gap: f32,
}

impl ButtonSizeTokens {
    // Icon/label optical balance: a Lucide glyph fills ~0.860 of its box while
    // text occupies ~0.71em. Solving `icon = text * 0.71 / 0.860`
    // for the 1.0 cap-height target and rounding to the slightly larger whole
    // pixel (`icon_size = text_size - 1` throughout) lands 1.10-1.13x flat —
    // inside the ~1.0-1.15 UI band, whole pixels only (fractional blurs when
    // snapped). Pinned in `button.rs`; re-run it before editing an `icon_size`.
    //
    // `MD` is 28px tall: 32px would carry 2.4x the cap height in dead space and
    // read as a tall pill, while 28px lands ~2.0x with GTK/macOS/Material. Its
    // `radius` of 9 holds the corner proportion and keeps the ladder monotonic,
    // and its `gap` of 6 keeps the glyph grouped with the label. Round icon
    // controls carry their own `28.0`, which agrees with `MD.height`.

    /// These sizes at the effective desktop text scale: the label, the icon
    /// and the height that holds them grow together; padding, radius and gap
    /// keep their rhythm. Unchanged at 100%.
    #[must_use]
    pub fn at_text_scale(self) -> Self {
        let ui = enzui_theme::text_scale::ui_px;
        Self {
            height: ui(self.height),
            text_size: ui(self.text_size),
            icon_size: ui(self.icon_size),
            ..self
        }
    }

    /// Extra-small: 22px height.
    pub const XS: Self = Self {
        height: 22.0,
        padding_x: 8.0,
        text_size: 11.0,
        icon_size: 10.0, // 1.10x optical ratio
        radius: 8.0,
        gap: 4.0,
    };

    /// Small: 26px height.
    pub const SM: Self = Self {
        height: 26.0,
        padding_x: 10.0,
        text_size: 12.0,
        icon_size: 11.0, // 1.11x optical ratio
        radius: 9.0,
        gap: 6.0,
    };

    /// Medium (default): 28px height.
    pub const MD: Self = Self {
        height: 28.0, // 2.0x dead-space cap
        padding_x: 12.0,
        text_size: 13.0,
        icon_size: 12.0, // 1.12x optical ratio
        radius: 9.0,     // keeps the 10/32 corner proportion at 28px
        gap: 6.0,        // keeps the glyph grouped with the label
    };

    /// Large: 40px height.
    pub const LG: Self = Self {
        height: 40.0,
        padding_x: 16.0,
        text_size: 14.0,
        icon_size: 13.0, // 1.13x optical ratio
        radius: 12.0,
        gap: 10.0,
    };

    /// Extra-large: 52px height.
    pub const XL: Self = Self {
        height: 52.0,
        padding_x: 22.0,
        text_size: 15.0,
        icon_size: 14.0, // 1.13x optical ratio
        radius: 14.0,
        gap: 12.0,
    };
}

/// Fixed geometry presets shared by inline text-entry controls.
///
/// [`TextFieldSize::tokens`](crate::text_field::TextFieldSize::tokens) returns
/// these presets, not a live theme snapshot. Text and secure fields resolve
/// unpinned corner radii separately at paint time; their height, padding, and
/// text size still use this shared ladder.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FieldSizeTokens {
    /// Total slot height in px.
    pub height: f32,
    /// Horizontal content padding in px.
    pub padding_x: f32,
    /// Text size in px.
    pub text_size: f32,
    /// Corner radius in px.
    pub radius: f32,
}

/// The resting fill shared by every text-entry field in the family:
/// `EnzTextField`, `EnzSecureField`, `EnzComboBox`, `EnzTextArea`,
/// `EnzDateField` and `EnzAutocompleteField`.
///
/// The field body is [`Palette::field`]: white on a grey light page, edged
/// by a soft hairline ([`Palette::field_border`]), the way macOS and web
/// inputs draw it; in dark mode a lifted fill told apart from
/// the pane by its body. In light mode an enabled field never wears the grey
/// well ([`Palette::surface_alt`]) a disabled field sits in. Pinned in
/// `tests/fields/field_body.rs`. A `const fn` so one retune moves every
/// member at once, and so `DateFieldTokens::from_palette` can call it.
/// Equivalent to the default treatment; `Outlined` selects the
/// page-coloured fill via [`field_fill_for`].
pub const fn field_fill(palette: &Palette) -> Color {
    field_fill_for(palette, FieldVariant::Filled)
}

/// Resting-fill treatment for a text-entry field.
///
/// The family defaults to [`FieldVariant::Filled`]; [`FieldVariant::Outlined`]
/// is the opt-in for dense/inline cases (a table cell, an inline editor) that
/// genuinely want the field to disappear into the surface behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldVariant {
    /// A field with a body: fill [`Palette::field`], edged by
    /// [`Palette::field_border`]. The default, because with 88 call sites and zero restyles,
    /// whatever the default renders is what the framework looks like.
    #[default]
    Filled,
    /// No fill: the surface behind shows through, and a clearly visible
    /// outline is the *sole* affordance separating the field from the page.
    Outlined,
    /// No body, no outline and no horizontal inset: the text edits in place
    /// where a label stood (renaming a row), marked only by the caret and the
    /// selection. An error still draws its edge.
    Inline,
}

/// The border of a field in `variant`, resting or hovered.
///
/// A `Filled` field reads the palette's field roles, which the built-in
/// palettes make a shade of the field body ([`Palette::field_border`]). An
/// `Outlined` field has only its outline, over whatever surface it sits on,
/// where that shade would vanish, so it keeps a visible line:
/// `control_border_strong` at rest, `text_muted` on hover. Focus and error colours are the caller's.
pub const fn field_edge(palette: &Palette, variant: FieldVariant, hovered: bool) -> Color {
    match (variant, hovered) {
        (FieldVariant::Filled, false) => palette.field_border,
        (FieldVariant::Filled, true) => palette.field_border_strong,
        // An outline is the whole field: a clearly visible edge at rest,
        // stronger under the pointer.
        (FieldVariant::Outlined, false) => palette.control_border_strong,
        (FieldVariant::Outlined, true) => palette.text_muted,
        (FieldVariant::Inline, _) => Color::TRANSPARENT,
    }
}

/// Resting fill for a given [`FieldVariant`].
///
/// `Filled` → [`Palette::field`] (a body); `Outlined` → transparent, so the
/// surface it sits on shows through and the edge alone draws the field.
/// Border roles are selected separately by [`field_edge`].
pub const fn field_fill_for(palette: &Palette, variant: FieldVariant) -> Color {
    match variant {
        FieldVariant::Filled => palette.field,
        FieldVariant::Outlined | FieldVariant::Inline => Color::TRANSPARENT,
    }
}

impl FieldSizeTokens {
    /// Small: 26px height.
    pub const SM: Self = Self {
        height: 26.0,
        padding_x: 8.0,
        text_size: 12.0,
        radius: 5.0,
    };

    /// Medium (default): 32px height. `text-field-height: 32px`
    /// / `text-field-radius: 6px`.
    pub const MD: Self = Self {
        height: 32.0,
        padding_x: 10.0,
        text_size: 13.0,
        radius: 6.0,
    };

    /// Large: 40px height.
    pub const LG: Self = Self {
        height: 40.0,
        padding_x: 14.0,
        text_size: 15.0,
        radius: 8.0,
    };

    /// The iced default line height is `1.3 * text_size`; this returns the
    /// vertical padding that vertically centers that line inside `height`,
    /// floored at 0 so the painted chrome and hit region fill the whole slot.
    pub fn padding_y(&self) -> f32 {
        let line = 1.3 * self.text_size;
        ((self.height - line) / 2.0).max(0.0)
    }
}
