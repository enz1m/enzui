//! Per-role family bindings and parsing of desktop font descriptions.
//!
//! [`FontCatalog`] stores UI, document and monospace families. Resolving a
//! role preserves caller-supplied weight, style and stretch. Catalog changes
//! affect later resolutions; they do not rebuild widgets or load font files.

use iced::font::{Family, Font, Stretch, Style, Weight};

/// The three semantic font roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FontRole {
    /// UI chrome font.
    Ui,
    /// Document / reading-prose font.
    Document,
    /// Monospace / code font.
    Monospace,
}

impl FontRole {
    /// Every role in stable iteration order.
    pub const ALL: [FontRole; 3] = [FontRole::Ui, FontRole::Document, FontRole::Monospace];

    /// Default generic family: sans-serif for UI and documents, monospace for code.
    /// This selects a renderer family category without checking host font files.
    pub const fn generic_family(self) -> Family {
        match self {
            FontRole::Ui => Family::SansSerif,
            FontRole::Document => Family::SansSerif,
            FontRole::Monospace => Family::Monospace,
        }
    }
}

/// An unvalidated font description as handed over by a platform.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RawFontDescription(String);

impl RawFontDescription {
    /// Wraps a platform string without validation; [`validate`](Self::validate)
    /// or [`parse`](Self::parse) performs validation later.
    pub fn new(raw: impl Into<String>) -> Self {
        RawFontDescription(raw.into())
    }

    /// The original string, including whitespace and unparsed attributes.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the parsed family, or `None` when no non-empty comma-separated
    /// family remains, including empty, whitespace-only, and comma-only input.
    pub fn validate(&self) -> Option<FontFamily> {
        self.parse()
            .map(|parsed| match parsed.family.to_ascii_lowercase().as_str() {
                "sans" | "sans-serif" | "sans serif" | "system-ui" => {
                    FontFamily::Generic(GenericFamily::SansSerif)
                }
                "serif" => FontFamily::Generic(GenericFamily::Serif),
                "monospace" | "monospaced" => FontFamily::Generic(GenericFamily::Monospace),
                _ => FontFamily::Named(parsed.family),
            })
    }

    /// Parse a Pango-style family list with supported trailing style and size tokens.
    ///
    /// A trailing numeric token is removed as a size even when its value is
    /// rejected; accepted sizes are positive finite points, or pixels with an
    /// exact `px` or `PX` suffix. A lone number or style word stays a family.
    /// Supported style words are matched after lowercasing and removing hyphens;
    /// unknown words stop style peeling. The first nonempty comma-separated
    /// family wins. Returns `None` when no family remains; host availability is
    /// not checked. Trailing variation/feature metadata is excluded from the
    /// family; an integer `wght` axis selects the nearest supported weight.
    /// Other axes and OpenType features are not applied by this parser.
    pub fn parse(&self) -> Option<ParsedFontDescription> {
        parse_pango(&self.0)
    }
}

/// A parsed Pango font description.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedFontDescription {
    /// The family (validated non-empty, trimmed).
    pub family: String,
    /// The size, if the description carried one and it was finite +
    /// strictly positive. A rejected size (`0`, negative, `NaN`, `±inf`)
    /// yields `None` so the consumer falls back to its role default.
    pub size: Option<FontSize>,
    /// Weight from a supported style name or a normalized numeric token
    /// in `100..=1000`. `None` preserves the caller default.
    pub weight: Option<Weight>,
    /// Parsed italic or oblique style; `None` preserves the caller default.
    /// Normal/Roman does not record an explicit reset, so an earlier trailing
    /// italic or oblique token can still supply this field.
    pub style: Option<Style>,
    /// Stretch, if the description carried one (`Condensed`, `Expanded`, …).
    pub stretch: Option<Stretch>,
}

/// A Pango font size, preserving its original unit.
///
/// Pango sizes are points by default and pixels only when followed by `px`.
/// Converting points to pixels needs a DPI value this pure module cannot know,
/// so the unit is kept and the consumer converts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FontSize {
    /// Points (Pango's default; `"Sans 12"`).
    Points(f32),
    /// Pixels (Pango's `px` suffix; `"Sans 12px"`).
    Pixels(f32),
}

impl FontSize {
    /// The bare numeric value, in whatever unit the description carried.
    pub fn value(self) -> f32 {
        match self {
            FontSize::Points(v) | FontSize::Pixels(v) => v,
        }
    }
}

// A dependency on enzui-core would invert the theme dependency graph.
fn finite_positive(v: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then_some(v)
}

fn peel_trailing_size(rest: &str) -> Option<(Option<FontSize>, &str)> {
    let trimmed = rest.trim_end();
    let (head, last) = match trimmed.rsplit_once(char::is_whitespace) {
        Some((head, last)) => (head, last.trim_start()),
        // The caller preserves a lone size token as a family name.
        None => ("", trimmed),
    };
    let (num_str, is_px) = if let Some(prefix) = last.strip_suffix("px") {
        (prefix, true)
    } else if let Some(prefix) = last.strip_suffix("PX") {
        (prefix, true)
    } else {
        (last, false)
    };
    let parsed: f32 = num_str.parse().ok()?;
    let accepted = finite_positive(parsed).map(|v| {
        if is_px {
            FontSize::Pixels(v)
        } else {
            FontSize::Points(v)
        }
    });
    Some((accepted, head.trim_end()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum StyleToken {
    Weight(Weight),
    Style(Style),
    Stretch(Stretch),
    DiscardedVariantOrGravity,
}

fn recognise_style_token(tok: &str) -> Option<StyleToken> {
    let mut norm = String::with_capacity(tok.len());
    for c in tok.chars() {
        if c == '-' {
            continue;
        }
        for lc in c.to_lowercase() {
            norm.push(lc);
        }
    }
    match norm.as_str() {
        "normal" | "roman" => Some(StyleToken::Style(Style::Normal)),
        "italic" => Some(StyleToken::Style(Style::Italic)),
        "oblique" => Some(StyleToken::Style(Style::Oblique)),

        "thin" => Some(StyleToken::Weight(Weight::Thin)),
        "ultralight" | "extralight" => Some(StyleToken::Weight(Weight::ExtraLight)),
        "light" => Some(StyleToken::Weight(Weight::Light)),
        // Iced has no semilight or book weight; both map to Light.
        "semilight" | "book" => Some(StyleToken::Weight(Weight::Light)),
        "regular" => Some(StyleToken::Weight(Weight::Normal)),
        "medium" => Some(StyleToken::Weight(Weight::Medium)),
        "semibold" => Some(StyleToken::Weight(Weight::Semibold)),
        "bold" => Some(StyleToken::Weight(Weight::Bold)),
        "ultrabold" | "extrabold" => Some(StyleToken::Weight(Weight::ExtraBold)),
        "heavy" | "black" => Some(StyleToken::Weight(Weight::Black)),

        "ultracondensed" => Some(StyleToken::Stretch(Stretch::UltraCondensed)),
        "extracondensed" => Some(StyleToken::Stretch(Stretch::ExtraCondensed)),
        "condensed" => Some(StyleToken::Stretch(Stretch::Condensed)),
        "semicondensed" => Some(StyleToken::Stretch(Stretch::SemiCondensed)),
        "semiexpanded" => Some(StyleToken::Stretch(Stretch::SemiExpanded)),
        "expanded" => Some(StyleToken::Stretch(Stretch::Expanded)),
        "extraexpanded" => Some(StyleToken::Stretch(Stretch::ExtraExpanded)),
        "ultraexpanded" => Some(StyleToken::Stretch(Stretch::UltraExpanded)),

        "smallcaps" => Some(StyleToken::DiscardedVariantOrGravity),

        "south" | "east" | "north" | "west" | "auto" => {
            // Bare direction words remain available as family names.
            None
        }

        _ => {
            if let Ok(n) = norm.parse::<u16>()
                && (100..=1000).contains(&n)
            {
                return Some(StyleToken::Weight(numeric_weight_to_iced(n)));
            }
            None
        }
    }
}

fn numeric_weight_to_iced(n: u16) -> Weight {
    match n {
        0..=149 => Weight::Thin,
        150..=249 => Weight::ExtraLight,
        250..=349 => Weight::Light,
        350..=449 => Weight::Normal,
        450..=549 => Weight::Medium,
        550..=649 => Weight::Semibold,
        650..=749 => Weight::Bold,
        750..=849 => Weight::ExtraBold,
        _ => Weight::Black,
    }
}

fn peel_style_options(head: &str) -> (Option<Weight>, Option<Style>, Option<Stretch>, &str) {
    let mut weight = None;
    let mut style = None;
    let mut stretch = None;
    let mut rest = head.trim_end();

    loop {
        let trimmed = rest.trim_end();
        let (candidate_head, last_tok) = match trimmed.rsplit_once(char::is_whitespace) {
            Some((head, last)) => (head, last.trim_start()),
            None => ("", trimmed),
        };
        if last_tok.is_empty() {
            break;
        }
        match recognise_style_token(last_tok) {
            Some(StyleToken::Weight(w)) => {
                if candidate_head.trim().is_empty() {
                    break;
                }
                // The rightmost recognized weight wins.
                if weight.is_none() {
                    weight = Some(w);
                }
                rest = candidate_head.trim_end();
            }
            Some(StyleToken::Style(s)) => {
                if candidate_head.trim().is_empty() {
                    break;
                }
                if style.is_none() {
                    style = if s == Style::Normal { None } else { Some(s) };
                }
                rest = candidate_head.trim_end();
            }
            Some(StyleToken::Stretch(st)) => {
                if candidate_head.trim().is_empty() {
                    break;
                }
                if stretch.is_none() {
                    stretch = Some(st);
                }
                rest = candidate_head.trim_end();
            }
            Some(StyleToken::DiscardedVariantOrGravity) => {
                if candidate_head.trim().is_empty() {
                    break;
                }
                rest = candidate_head.trim_end();
            }
            None => break,
        }
    }

    (weight, style, stretch, rest)
}

fn parse_pango(raw: &str) -> Option<ParsedFontDescription> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Pango appends variation axes and OpenType features after the size. They
    // are metadata, never part of the family name (GNOME writes @wght by default).
    let suffix = trimmed.char_indices().find_map(|(index, ch)| {
        ((ch == '@' || ch == '#')
            && index > 0
            && trimmed
                .get(..index)
                .is_some_and(|head| head.ends_with(char::is_whitespace)))
        .then_some(index)
    });
    let (head, metadata) = suffix.map_or((trimmed, ""), |index| trimmed.split_at(index));
    let trimmed = head.trim_end();
    let axis_weight = metadata
        .split_whitespace()
        .find_map(|token| token.strip_prefix('@'))
        .and_then(|axes| {
            axes.split(',').find_map(|axis| {
                axis.strip_prefix("wght=")
                    .and_then(|value| value.parse::<u16>().ok())
                    .filter(|value| (100..=1000).contains(value))
                    .map(numeric_weight_to_iced)
            })
        });
    let (size, after_size) = match peel_trailing_size(trimmed) {
        Some((s, r)) => (s, r),
        None => (None, trimmed),
    };

    let (weight, style, stretch, family_text) = peel_style_options(after_size);
    let weight = axis_weight.or(weight);
    let family_text = family_text.trim();

    let (family_source, size) = if family_text.is_empty() {
        // A lone numeric token must remain a usable family name.
        (trimmed, None)
    } else {
        (family_text, size)
    };

    let family = family_source
        .split(',')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_owned)?;

    Some(ParsedFontDescription {
        family,
        size,
        weight,
        style,
        stretch,
    })
}

/// A generic category or owned family name for font resolution.
/// `Named` values can be constructed without parsing or validation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FontFamily {
    /// One of iced's generic families (sans-serif, serif, monospace, …).
    Generic(GenericFamily),
    /// An owned family name; construction does not check syntax or installation.
    Named(String),
}

/// Generic renderer family categories, stored without interning a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GenericFamily {
    /// Serif.
    Serif,
    /// Sans-serif.
    SansSerif,
    /// Cursive.
    Cursive,
    /// Fantasy.
    Fantasy,
    /// Monospace.
    Monospace,
}

impl GenericFamily {
    /// The corresponding generic renderer family, without name interning.
    pub const fn to_iced(self) -> Family {
        match self {
            GenericFamily::Serif => Family::Serif,
            GenericFamily::SansSerif => Family::SansSerif,
            GenericFamily::Cursive => Family::Cursive,
            GenericFamily::Fantasy => Family::Fantasy,
            GenericFamily::Monospace => Family::Monospace,
        }
    }
}

impl FontFamily {
    /// Convenience for a generic family.
    pub const fn generic(g: GenericFamily) -> Self {
        FontFamily::Generic(g)
    }

    /// Convert to an `iced::font::Family`.
    ///
    /// Named families use iced's process-wide cache. Each distinct name is
    /// retained permanently; repeated names reuse the cached string. Resolving
    /// arbitrary candidates therefore retains names even when never painted.
    ///
    /// # Panics
    ///
    /// Panics for a named family if iced's name-cache lock is poisoned.
    pub fn to_iced(&self) -> Family {
        match self {
            FontFamily::Generic(g) => g.to_iced(),
            FontFamily::Named(name) => Family::name(name),
        }
    }
}

impl From<GenericFamily> for FontFamily {
    fn from(g: GenericFamily) -> Self {
        FontFamily::Generic(g)
    }
}

/// The current family for each role plus an effective-change counter.
///
/// Re-applying the current family is a no-op. A changed family increments the
/// `u64` counter, subject to ordinary integer-overflow behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontCatalog {
    ui: FontFamily,
    document: FontFamily,
    monospace: FontFamily,
    generation: u64,
}

impl Default for FontCatalog {
    fn default() -> Self {
        FontCatalog {
            ui: FontRole::Ui.generic_font_family(),
            document: FontRole::Document.generic_font_family(),
            monospace: FontRole::Monospace.generic_font_family(),
            generation: 0,
        }
    }
}

impl FontRole {
    /// The catalog's default family category for this role.
    /// This is the owned-family counterpart of [`generic_family`](Self::generic_family).
    pub const fn generic_font_family(self) -> FontFamily {
        match self {
            FontRole::Ui => FontFamily::Generic(GenericFamily::SansSerif),
            FontRole::Document => FontFamily::Generic(GenericFamily::SansSerif),
            FontRole::Monospace => FontFamily::Generic(GenericFamily::Monospace),
        }
    }
}

impl FontCatalog {
    /// The family currently bound to `role`.
    pub fn family(&self, role: FontRole) -> &FontFamily {
        match role {
            FontRole::Ui => &self.ui,
            FontRole::Document => &self.document,
            FontRole::Monospace => &self.monospace,
        }
    }

    /// Returns the change counter, which advances when an effective family
    /// changes and is unchanged by no-op assignments.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Resolve this role's family with the supplied weight, style and stretch.
    /// Named families use [`FontFamily::to_iced`] and its cache contract.
    pub fn resolve_font(
        &self,
        role: FontRole,
        weight: Weight,
        style: Style,
        stretch: Stretch,
    ) -> Font {
        Font {
            family: self.family(role).to_iced(),
            weight,
            style,
            stretch,
            ..Font::DEFAULT
        }
    }

    /// Set `role`'s family, bumping the generation iff the family actually
    /// changes.
    ///
    /// Returns `true` after changing the family and incrementing the generation,
    /// or `false` when the role already has that family.
    ///
    /// # Panics
    ///
    /// Panics on generation overflow when integer overflow checks are enabled;
    /// otherwise the counter wraps.
    pub fn set_family(&mut self, role: FontRole, family: FontFamily) -> bool {
        let slot = match role {
            FontRole::Ui => &mut self.ui,
            FontRole::Document => &mut self.document,
            FontRole::Monospace => &mut self.monospace,
        };
        if *slot == family {
            false
        } else {
            *slot = family;
            self.generation += 1;
            true
        }
    }
}

/// Resolves font roles against an owned [`FontCatalog`] snapshot.
///
/// Role selection changes only the family; weight, style, and stretch pass
/// through unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontResolver {
    catalog: FontCatalog,
}

impl FontResolver {
    /// A resolver over the given catalog.
    pub fn new(catalog: FontCatalog) -> Self {
        FontResolver { catalog }
    }

    /// The catalog this resolver reads from.
    pub fn catalog(&self) -> &FontCatalog {
        &self.catalog
    }

    /// The snapshot catalog's change counter.
    pub fn generation(&self) -> u64 {
        self.catalog.generation()
    }

    /// Resolve a role with caller-supplied weight, style and stretch.
    /// Named families use [`FontFamily::to_iced`] and its cache contract.
    pub fn resolve(&self, role: FontRole, weight: Weight, style: Style, stretch: Stretch) -> Font {
        Font {
            family: self.catalog.family(role).to_iced(),
            weight,
            style,
            stretch,
            ..Font::DEFAULT
        }
    }
}
