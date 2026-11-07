//! Sectioned shortcut references with stacked alternate keycaps and elided descriptions.
//!
//! Headers follow changes from the immediately preceding entry; empty sections
//! emit no header. Alternate badges split on `", "`, `" or "`, and the localized alternate separator. Unpinned ink
//! follows the live theme; text metrics resolve when the view is built.
//!
//! Rendering is eager by default. [`EnzShortcutList::viewport_height`] enables
//! visible-line realization with overscan, while full-list indexing and height
//! scans remain. Feed scroll callbacks back into the next view's offset.
//! [`crate::shortcuts_dialog`] provides the modal shortcut reference.

use crate::enz_text::{TextSize, TextTokens, TextTone, enz_text};
use crate::key_badge::{KeyBadgeTokens, enz_key_badge};
use crate::semantics_container::SemanticsContainer;
use crate::theme_lookup;
use enzui_core::semantics::{Role, Semantics};
use enzui_core::virtual_sequence::{
    Anchor, ItemProvider, KeyedAnchor, KeyedProvider, RealizationWindow, SequenceKey,
    VariableHeightModel,
};
use enzui_core::{ShortcutBinding, ShortcutHelpEntry, shortcut_help};
use enzui_theme::{Palette, SpacingScale, TypographyScale};
use iced::widget::scrollable::{Direction, Scrollbar, Viewport};
use iced::widget::{Id, Space, column, container, row, scrollable};
use iced::{Color, Element, Length};
use std::cell::Cell;
use std::rc::Rc;

/// One shortcut entry (section header + key chord + description).
#[derive(Debug, Clone)]
pub struct ShortcutEntry {
    /// Section header. A new section header inserts whenever this differs
    /// from the immediately previous entry's section. Empty string
    /// suppresses the section header for this entry.
    pub section: String,
    /// The key chord(s) displayed in the leading slot (e.g. `"Ctrl+N"`,
    /// `"⌘K"`). Alternate bindings joined by `", "` or `" or "` render as
    /// separate stacked keycaps.
    pub key: String,
    /// The description to the right of the badge.
    pub description: String,
    /// Optional identity for keyed scrolling; None derives identity from entry content.
    /// Use unique, stable keys when identical rows or content edits must retain identity.
    pub stable_key: Option<String>,
}

impl ShortcutEntry {
    /// Construct a new entry.
    pub fn new<S: Into<String>, K: Into<String>, D: Into<String>>(
        section: S,
        key: K,
        description: D,
    ) -> Self {
        Self {
            section: section.into(),
            key: key.into(),
            description: description.into(),
            stable_key: None,
        }
    }

    /// Use an explicit logical identity that remains stable when content or position changes.
    #[must_use]
    pub fn stable_key(mut self, key: impl Into<String>) -> Self {
        self.stable_key = Some(key.into());
        self
    }

    /// Return the explicit key, or section, chord and description joined by U+001F.
    /// Identical entries share a derived key. Embedded U+001F characters can also
    /// make distinct entries collide; use explicit unique keys for arbitrary content.
    #[must_use]
    pub fn logical_key(&self) -> String {
        if let Some(k) = &self.stable_key {
            return k.clone();
        }
        format!(
            "{}\u{1f}{}\u{1f}{}",
            self.section, self.key, self.description
        )
    }
}

/// Expose catalog metadata for shortcut-reference entries.
/// Derive badge text from the application's active bindings to keep help and dispatch aligned.
pub trait ShortcutCatalogRow {
    /// The section header this row falls under. A new header row inserts
    /// whenever this differs from the previous row's section (see
    /// [`enz_shortcut_list`]); an empty string suppresses the header.
    fn section(&self) -> &str;
    /// Return the chord text displayed in the leading badge column.
    fn badge(&self) -> String;
    /// The human description to the right of the badge.
    fn description(&self) -> &str;
    /// Return an optional identity that survives wording or chord changes; defaults to None.
    fn stable_key(&self) -> Option<String> {
        None
    }
}

/// Adapt one [`ShortcutCatalogRow`] into the [`ShortcutEntry`] the dialog
/// renders. Sets [`ShortcutEntry::stable_key`] only when the row supplies one.
#[must_use]
pub fn shortcut_entry(row: &impl ShortcutCatalogRow) -> ShortcutEntry {
    let entry = ShortcutEntry::new(row.section(), row.badge(), row.description());
    match row.stable_key() {
        Some(key) => entry.stable_key(key),
        None => entry,
    }
}

/// Joins localized chords for distinct keys using the catalog’s alternate separator.
/// Later chords for the same key are omitted even when their modifiers differ.
/// Use [`shortcut_entries_with`] when every modifier variant must be displayed.
#[must_use]
pub fn shortcut_help_badge(entry: &ShortcutHelpEntry<'_>) -> String {
    let mut keys = Vec::new();
    let mut chords: Vec<String> = Vec::new();
    for shortcut in entry.shortcuts() {
        if keys.contains(&shortcut.key()) {
            continue;
        }
        keys.push(shortcut.key());
        chords.push(shortcut.localized_label());
    }
    chords.join(&enzui_core::localization::text("enzui-shortcut-or"))
}

/// Expose help metadata with [`shortcut_help_badge`] text and binding ids as stable keys.
impl ShortcutCatalogRow for ShortcutHelpEntry<'_> {
    fn section(&self) -> &str {
        ShortcutHelpEntry::section(self)
    }

    fn badge(&self) -> String {
        shortcut_help_badge(self)
    }

    fn description(&self) -> &str {
        ShortcutHelpEntry::description(self)
    }

    fn stable_key(&self) -> Option<String> {
        Some(ShortcutHelpEntry::id(self).to_owned())
    }
}

/// Convert bindings to help entries in catalog order, with one row per binding.
/// Aliases share a badge using [`shortcut_help_badge`]; binding ids supply stable keys.
#[must_use]
pub fn shortcut_catalog_entries<'a, Command: 'a, Context: 'a>(
    bindings: impl IntoIterator<Item = &'a ShortcutBinding<Command, Context>>,
) -> Vec<ShortcutEntry> {
    shortcut_entries(&shortcut_help(bindings))
}

/// Convert catalog rows in slice order, preserving their optional stable keys.
#[must_use]
pub fn shortcut_entries<T: ShortcutCatalogRow>(rows: &[T]) -> Vec<ShortcutEntry> {
    rows.iter().map(shortcut_entry).collect()
}

/// Convert a catalog row with optional replacement badge text.
/// Some, including an empty string, overrides the row badge; None uses the row
/// badge. Section, description and optional stable identity remain catalog-owned.
#[must_use]
pub fn shortcut_entry_with<T: ShortcutCatalogRow>(
    row: &T,
    resolved: Option<String>,
) -> ShortcutEntry {
    let badge = resolved.unwrap_or_else(|| row.badge());
    let entry = ShortcutEntry::new(row.section(), badge, row.description());
    match row.stable_key() {
        Some(key) => entry.stable_key(key),
        None => entry,
    }
}

/// Convert catalog rows with a badge resolver called once per row in slice order.
/// Some replaces badge text; None uses the catalog badge. Stable keys remain
/// catalog-owned, while unkeyed rows derive identity from their resulting content.
#[must_use]
pub fn shortcut_entries_with<T, F>(rows: &[T], mut resolver: F) -> Vec<ShortcutEntry>
where
    T: ShortcutCatalogRow,
    F: FnMut(&T) -> Option<String>,
{
    rows.iter()
        .map(|row| shortcut_entry_with(row, resolver(row)))
        .collect()
}

/// Expose entry logical keys for shared scroll anchoring.
/// Duplicate keys resolve to the first entry through the default linear lookup.
pub struct ShortcutEntryKeys<'e>(&'e [ShortcutEntry]);

impl ItemProvider<()> for ShortcutEntryKeys<'_> {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn item(&self, index: u64) -> Option<()> {
        (index < self.0.len() as u64).then_some(())
    }
}

impl KeyedProvider<()> for ShortcutEntryKeys<'_> {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.0
            .get(usize::try_from(index).ok()?)
            .map(|e| SequenceKey::new(e.logical_key()))
    }
}

/// Default badge-column width in logical pixels: 120.
pub const DEFAULT_KEY_WIDTH: f32 = 120.0;
/// Default gap between badge and description in logical pixels: 8.
pub const DEFAULT_ROW_SPACING: f32 = 8.0;
/// Default gap between eager lines, included in each virtual line height: 2 px.
pub const DEFAULT_SECTION_GAP: f32 = 2.0;
/// Default virtual header slot: section text size plus 6 px, giving 18 px at stock size.
/// Eager headers use their intrinsic text height; [`EnzShortcutList::section_line_height`] only affects virtualization.
pub const DEFAULT_SECTION_LINE_HEIGHT: f32 = DEFAULT_SECTION_SIZE + 6.0;

/// Default single-chord row height in logical pixels: 30; stacked alternates may grow taller.
pub const DEFAULT_ROW_HEIGHT: f32 = 30.0;
/// Default description text size before ambient text scaling: 13 px.
pub const DEFAULT_DESCRIPTION_SIZE: f32 = 13.0;
/// Default section-header text size before ambient text scaling: 12 px.
pub const DEFAULT_SECTION_SIZE: f32 = 12.0;
/// Default badge text size before ambient text scaling: 11 px.
pub const DEFAULT_BADGE_TEXT_SIZE: f32 = 11.0;

/// Additional gap beside the virtual scrollable's reserved scrollbar: 2 px.
/// The gutter is reserved while the bar is visible so it does not obscure elided text.
pub const DEFAULT_SCROLLBAR_GUTTER: f32 = 2.0;

/// Vertical gap, in pixels, between stacked keycaps in a row that displays
/// alternate bindings.
pub const DEFAULT_KEYCAP_STACK_GAP: f32 = 4.0;
fn split_chords(key: &str) -> Vec<&str> {
    let separator = enzui_core::localization::text("enzui-shortcut-or");
    let separator = if separator.trim().is_empty() {
        " or "
    } else {
        separator.as_str()
    };
    key.split(", ")
        .flat_map(|part| part.split(" or "))
        .flat_map(|part| part.split(separator))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect()
}

fn sane(value: f32, default: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        default
    }
}

fn sane_font_size(value: f32, default: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        default
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct ShortcutListStyle {
    section_color: Option<Color>,
    description_color: Option<Color>,
    text_color: Option<Color>,
}

impl ShortcutListStyle {
    fn section(&self, palette: &Palette) -> Color {
        self.section_color
            .unwrap_or_else(|| TextTokens::from_palette(palette).color_for(TextTone::Muted))
    }

    fn description(&self, palette: &Palette) -> Color {
        self.description_color
            .unwrap_or_else(|| TextTokens::from_palette(palette).color_for(TextTone::Default))
    }

    fn badge_text(&self, palette: &Palette) -> Color {
        self.text_color.unwrap_or_else(|| self.description(palette))
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedMetrics {
    description_size: f32,
    badge_text_size: f32,
    row_spacing: f32,
    section_size: f32,
    section_line_height: f32,
}

/// Build a shortcut reference with live theme ink and optional per-field colour pins.
/// Geometry and typography resolve during view construction; virtualization is opt-in.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzShortcutList {
    entries: Vec<ShortcutEntry>,
    key_width: f32,
    row_spacing: f32,
    section_gap: f32,
    row_height: f32,
    description_size: f32,
    section_size: f32,
    style: ShortcutListStyle,
    badge_tokens: Option<KeyBadgeTokens>,
    badge_radius: f32,
    badge_text_size: f32,
    row_spacing_explicit: bool,
    description_size_explicit: bool,
    badge_text_size_explicit: bool,
    section_size_explicit: bool,
    section_line_height_explicit: bool,
    typography: Option<TypographyScale>,
    spacing_scale: Option<SpacingScale>,
    section_line_height: f32,
    viewport_height: Option<f32>,
    scroll_offset: f32,
    keyed_anchor: Option<KeyedAnchor>,
    overscan: usize,
    scroll_id: Option<Id>,
    row_builder_probe: Option<Rc<Cell<usize>>>,
}

impl EnzShortcutList {
    /// Override the reserved key-column width (default 120 px). Sanitized.
    pub fn key_width(mut self, width: f32) -> Self {
        self.key_width = sane(width, DEFAULT_KEY_WIDTH);
        self
    }

    /// Pin the badge-description gap in logical pixels, overriding a forwarded spacing scale.
    /// Negative or non-finite values use 8 px; zero is preserved.
    pub fn row_spacing(mut self, spacing: f32) -> Self {
        self.row_spacing = sane(spacing, DEFAULT_ROW_SPACING);
        self.row_spacing_explicit = true;
        self
    }

    /// Override the gap between rows/sections (default 2 px). Sanitized.
    pub fn section_gap(mut self, gap: f32) -> Self {
        self.section_gap = sane(gap, DEFAULT_SECTION_GAP);
        self
    }

    /// Set the single-chord row height; defaults to 30 px.
    /// Stacked alternates use at least the space required by their keycaps. Negative
    /// or non-finite values use the default; zero is preserved for a single chord.
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height = sane(height, DEFAULT_ROW_HEIGHT);
        self
    }

    /// Pin description text size, overriding forwarded typography and ambient scaling.
    /// Nonpositive or non-finite values fall back to 13 px.
    pub fn description_size(mut self, size: f32) -> Self {
        self.description_size = sane_font_size(size, DEFAULT_DESCRIPTION_SIZE);
        self.description_size_explicit = true;
        self
    }

    /// Pin section text size, overriding forwarded typography and ambient scaling.
    /// Nonpositive or non-finite values fall back to 12 px.
    pub fn section_size(mut self, size: f32) -> Self {
        self.section_size = sane_font_size(size, DEFAULT_SECTION_SIZE);
        self.section_size_explicit = true;
        self
    }

    /// Pin header ink; unpinned header ink follows the live muted text colour.
    pub fn section_color(mut self, color: Color) -> Self {
        self.style.section_color = Some(color);
        self
    }

    /// Pin description ink and the default badge glyph ink.
    /// An explicit [`Self::text_color`] or [`Self::badge_tokens`] overrides the badge ink.
    pub fn description_color(mut self, color: Color) -> Self {
        self.style.description_color = Some(color);
        self
    }

    /// Pin badge glyph ink independently of description ink.
    /// A complete [`Self::badge_tokens`] pin takes precedence.
    pub fn text_color(mut self, color: Color) -> Self {
        self.style.text_color = Some(color);
        self
    }

    /// Pin badge fill, outline and glyph ink together, overriding [`Self::text_color`].
    pub fn badge_tokens(mut self, tokens: KeyBadgeTokens) -> Self {
        self.badge_tokens = Some(tokens);
        self
    }

    /// Override the key-badge corner radius (default: the theme's `key` radius). Sanitized.
    pub fn badge_radius(mut self, r: f32) -> Self {
        self.badge_radius = sane(r, enzui_theme::text_defaults::shape_radius(|s| s.key));
        self
    }

    /// Pin badge glyph size, overriding forwarded typography and ambient scaling.
    /// Negative or non-finite values fall back to 11 px; zero is passed to the key badge.
    pub fn badge_text_size(mut self, s: f32) -> Self {
        self.badge_text_size = sane(s, DEFAULT_BADGE_TEXT_SIZE);
        self.badge_text_size_explicit = true;
        self
    }

    /// Forward body, label and footnote sizes for descriptions, badges and headers.
    /// Explicit size setters take precedence. Without a scale, defaults use ambient
    /// text scaling. An unpinned virtual header slot uses resolved footnote size plus 6 px.
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }

    /// Forward the badge-description gap from [`SpacingScale::sm`].
    /// An explicit [`Self::row_spacing`] wins. Other layout lengths keep their own settings.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Pin the virtual header slot height; eager headers keep intrinsic text height.
    /// Unpinned slots use resolved section size plus 6 px. Negative or non-finite
    /// values use 18 px; zero is preserved.
    pub fn section_line_height(mut self, height: f32) -> Self {
        self.section_line_height = sane(height, DEFAULT_SECTION_LINE_HEIGHT);
        self.section_line_height_explicit = true;
        self
    }

    /// Enable virtualization with a measured, finite positive viewport height.
    /// Zero, negative or non-finite values select the eager fallback. The virtual
    /// path owns a scrollable; realization also needs updated offsets on rebuild.
    pub fn viewport_height(mut self, height: f32) -> Self {
        self.viewport_height = if height.is_finite() && height > 0.0 {
            Some(height)
        } else {
            None
        };
        self
    }

    /// Set the realization viewport top in logical pixels; defaults to zero.
    /// Negative or non-finite values use zero. A resolving keyed anchor takes
    /// precedence. This selects built lines; it does not issue a scroll operation.
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = sane(offset, 0.0);
        self
    }

    /// Select realization by logical entry identity and intra-row offset.
    /// The key resolves to the entry's row in the current list, even if capture was
    /// on its header. Missing keys use [`Self::scroll_offset`]; duplicates resolve
    /// to the first entry. Requires [`Self::viewport_height`]. This does not restore
    /// widget focus or issue a scroll operation.
    pub fn keyed_scroll_anchor(mut self, anchor: KeyedAnchor) -> Self {
        self.keyed_anchor = Some(anchor);
        self
    }

    /// Capture entry identity and the offset within the virtual line at the supplied top.
    /// A header captures its entry's key; reapplication targets that entry's row.
    /// Empty lists and finite offsets at or beyond the content end return None.
    /// Negative or non-finite offsets follow [`Anchor::from_scroll_top`] fallback to
    /// the first line. Capture uses virtual geometry, including the gap in each line.
    #[must_use]
    pub fn keyed_anchor_at(&self, viewport_top: f32) -> Option<KeyedAnchor> {
        // Anchor capture and realization must use the same virtual line metrics.
        let metrics = self.resolved_metrics();
        let model = self.line_model(&metrics);
        let provider = ShortcutEntryKeys(&self.entries);
        let line_anchor = Anchor::from_scroll_top(&model, viewport_top);
        let lines = build_line_index(&self.entries);
        let entry_idx = match lines.get(line_anchor.index())? {
            Line::Header(i) | Line::Row(i) => *i,
        };
        let key = provider.key(entry_idx as u64)?;
        Some(KeyedAnchor::new(key, line_anchor.offset_in_item()))
    }

    /// Realize this many extra lines on each side of the virtual viewport; defaults to 5.
    /// Zero is preserved; eager rendering ignores this setting.
    pub fn overscan(mut self, n: usize) -> Self {
        self.overscan = n;
        self
    }

    /// Name the virtual scrollable for widget scroll operations.
    /// The eager path has no internal scrollable and ignores this setting.
    pub fn scroll_id(mut self, id: Id) -> Self {
        self.scroll_id = Some(id);
        self
    }

    /// Increment the shared counter once per built header or entry row in either rendering path.
    #[doc(hidden)]
    pub fn row_builder_probe(mut self, probe: Rc<Cell<usize>>) -> Self {
        self.row_builder_probe = Some(probe);
        self
    }

    /// Resolve the header pin or the palette's muted text ink.
    pub fn resolved_section_color(&self, palette: &Palette) -> Color {
        self.style.section(palette)
    }

    /// Resolve the description pin or the palette's normal text ink.
    pub fn resolved_description_color(&self, palette: &Palette) -> Color {
        self.style.description(palette)
    }

    /// Resolve the glyph pin or description-derived default.
    /// This query excludes complete [`Self::badge_tokens`] pins.
    pub fn resolved_badge_text_color(&self, palette: &Palette) -> Color {
        self.style.badge_text(palette)
    }

    /// Resolve a complete badge pin or the alternate-surface fill and its derived outline.
    /// Unpinned glyph ink follows [`Self::resolved_badge_text_color`].
    pub fn resolved_badge_tokens(&self, palette: &Palette) -> KeyBadgeTokens {
        if let Some(bt) = self.badge_tokens {
            return bt;
        }
        KeyBadgeTokens {
            background: palette.surface_alt,
            border: crate::surface_edge::edge_color(
                palette.surface_alt,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            text_color: self.style.badge_text(palette),
        }
    }

    fn badge_for<'a, Message: 'a>(
        &self,
        key: String,
        badge_text_size: f32,
    ) -> crate::key_badge::EnzKeyBadge<'a, Message> {
        let mut badge_builder = enz_key_badge(key)
            .radius(self.badge_radius)
            .text_size(badge_text_size);
        if let Some(bt) = self.badge_tokens {
            badge_builder = badge_builder.tokens(bt);
        } else {
            let style = self.style;
            badge_builder = badge_builder.closure_resolver(std::rc::Rc::new(move |p: &Palette| {
                KeyBadgeTokens {
                    background: p.surface_alt,
                    border: crate::surface_edge::edge_color(
                        p.surface_alt,
                        crate::surface_edge::DEFAULT_EDGE_DEPTH,
                    ),
                    text_color: style.badge_text(p),
                }
            }));
        }
        badge_builder
    }

    /// Build all entries eagerly, or visible virtual lines plus overscan when enabled.
    /// Virtual builds still perform full-list indexing, height scans and allocations;
    /// keyed anchoring adds linear lookup and geometry work. Only line-element
    /// construction is bounded by the realization window. Use
    /// [`Self::into_element_on_scroll`] to receive internal scroll changes.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        self.into_element_inner::<Message>(None)
    }

    /// Build the list and report internal virtual-scroll changes through the callback.
    /// Store `viewport.absolute_offset().y` and feed it into [`Self::scroll_offset`]
    /// on rebuild to update realized lines. The eager path ignores the callback.
    pub fn into_element_on_scroll<'a, Message: 'a>(
        self,
        f: impl Fn(Viewport) -> Message + 'a,
    ) -> Element<'a, Message> {
        self.into_element_inner::<Message>(Some(Box::new(f)))
    }

    fn resolved_metrics(&self) -> ResolvedMetrics {
        let description_size = if self.description_size_explicit {
            self.description_size
        } else {
            sane_font_size(
                self.typography.map_or_else(
                    || enzui_theme::text_scale::ui_px(DEFAULT_DESCRIPTION_SIZE),
                    |t| t.body_size,
                ),
                DEFAULT_DESCRIPTION_SIZE,
            )
        };
        let badge_text_size = if self.badge_text_size_explicit {
            self.badge_text_size
        } else {
            sane(
                self.typography.map_or_else(
                    || enzui_theme::text_scale::ui_px(DEFAULT_BADGE_TEXT_SIZE),
                    |t| t.label_size,
                ),
                DEFAULT_BADGE_TEXT_SIZE,
            )
        };
        let row_spacing = if self.row_spacing_explicit {
            self.row_spacing
        } else {
            sane(
                self.spacing_scale.map_or(DEFAULT_ROW_SPACING, |s| s.sm()),
                DEFAULT_ROW_SPACING,
            )
        };
        let section_size = if self.section_size_explicit {
            self.section_size
        } else {
            sane_font_size(
                self.typography.map_or_else(
                    || enzui_theme::text_scale::ui_px(DEFAULT_SECTION_SIZE),
                    |t| t.footnote_size,
                ),
                DEFAULT_SECTION_SIZE,
            )
        };
        let section_line_height = if self.section_line_height_explicit {
            self.section_line_height
        } else {
            sane(section_size + 6.0, DEFAULT_SECTION_LINE_HEIGHT)
        };
        ResolvedMetrics {
            description_size,
            badge_text_size,
            row_spacing,
            section_size,
            section_line_height,
        }
    }

    fn line_model(&self, metrics: &ResolvedMetrics) -> VariableHeightModel {
        let lines = build_line_index(&self.entries);
        // Virtual spacing is folded into each line height; the rendered column uses no gap.
        let heights = lines.iter().map(|l| {
            let base = match l {
                Line::Header(_) => metrics.section_line_height,
                Line::Row(i) => self.row_content_height(&self.entries[*i].key),
            };
            base + self.section_gap
        });
        VariableHeightModel::new(heights)
    }
    fn row_content_height(&self, key: &str) -> f32 {
        let count = split_chords(key).len();
        if count <= 1 {
            self.row_height
        } else {
            let stacked = count as f32 * crate::key_badge::DEFAULT_HEIGHT
                + (count as f32 - 1.0) * DEFAULT_KEYCAP_STACK_GAP;
            stacked.max(self.row_height)
        }
    }

    fn resolved_scroll_offset(&self, metrics: &ResolvedMetrics) -> f32 {
        let Some(anchor) = &self.keyed_anchor else {
            return self.scroll_offset;
        };
        let provider = ShortcutEntryKeys(&self.entries);
        let Some(entry_index) = anchor.resolve(&provider) else {
            return self.scroll_offset;
        };
        let entry_index = entry_index.index();
        let lines = build_line_index(&self.entries);
        let line_index = lines.iter().position(|l| match l {
            Line::Row(i) => *i == entry_index,
            Line::Header(_) => false,
        });
        let Some(line_index) = line_index else {
            return self.scroll_offset;
        };
        let model = self.line_model(metrics);
        model.offset_of(line_index) + anchor.offset_in_item()
    }

    fn into_element_inner<'a, Message: 'a>(
        self,
        on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    ) -> Element<'a, Message> {
        let metrics = self.resolved_metrics();
        if let Some(vp_h) = self.viewport_height {
            return self.into_virtualized(vp_h, metrics, on_scroll);
        }
        let _ = on_scroll;
        let mut children: Vec<Element<'a, Message>> = Vec::new();
        let style = self.style;
        let probe = self.row_builder_probe.clone();

        for (index, entry) in self.entries.iter().enumerate() {
            let prev_section = if index == 0 {
                None
            } else {
                Some(self.entries[index - 1].section.as_str())
            };
            let new_section =
                !entry.section.is_empty() && prev_section != Some(entry.section.as_str());
            if new_section {
                if let Some(p) = &probe {
                    p.set(p.get() + 1);
                }
                children.push(self.build_header_element::<Message>(
                    entry.section.as_str(),
                    style,
                    metrics.section_size,
                ));
            }

            if let Some(p) = &probe {
                p.set(p.get() + 1);
            }
            children.push(self.build_row_element::<Message>(entry, style, metrics));
        }

        column(children)
            .spacing(self.section_gap)
            .width(Length::Fill)
            .into()
    }

    fn build_header_element<'a, Message: 'a>(
        &self,
        section: &str,
        style: ShortcutListStyle,
        section_size: f32,
    ) -> Element<'a, Message> {
        enz_text(section.to_owned())
            .size_px(section_size)
            .weight_rung(enzui_theme::WeightRung::Semibold)
            .color_resolved(move |theme| style.section(&theme_lookup::palette(theme)))
            .single_line_elided()
            .full_width()
            .into()
    }

    fn build_row_element<'a, Message: 'a>(
        &self,
        entry: &ShortcutEntry,
        style: ShortcutListStyle,
        metrics: ResolvedMetrics,
    ) -> Element<'a, Message> {
        let ResolvedMetrics {
            description_size,
            badge_text_size,
            row_spacing,
            ..
        } = metrics;
        let chords = split_chords(&entry.key);
        let key_content: Element<'a, Message> = if chords.len() > 1 {
            let badges: Vec<Element<'a, Message>> = chords
                .into_iter()
                .map(|chord| {
                    self.badge_for::<Message>(chord.to_owned(), badge_text_size)
                        .into()
                })
                .collect();
            // Alternate keycaps announce one complete chord string through the leaf wrapper.
            SemanticsContainer::leaf(
                column(badges)
                    .spacing(DEFAULT_KEYCAP_STACK_GAP)
                    .width(Length::Shrink),
                Semantics::new(Role::Label).label(entry.key.clone()),
            )
            .into()
        } else {
            self.badge_for::<Message>(entry.key.clone(), badge_text_size)
                .into()
        };
        let key_col = container(key_content)
            .width(Length::Fixed(self.key_width))
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Center);
        let description = enz_text(entry.description.clone())
            .size(TextSize::Body)
            .size_px(description_size)
            .color_resolved(move |theme| style.description(&theme_lookup::palette(theme)))
            .single_line_elided()
            .full_width();

        container(
            row![key_col, description]
                .spacing(row_spacing)
                .align_y(iced::Alignment::Center)
                .width(Length::Fill),
        )
        .height(Length::Fixed(self.row_content_height(&entry.key)))
        .align_y(iced::alignment::Vertical::Center)
        .width(Length::Fill)
        .into()
    }

    fn into_virtualized<'a, Message: 'a>(
        self,
        viewport_height: f32,
        metrics: ResolvedMetrics,
        on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    ) -> Element<'a, Message> {
        let section_size = metrics.section_size;
        let lines = build_line_index(&self.entries);
        let model = self.line_model(&metrics);
        let scroll_offset = self.resolved_scroll_offset(&metrics);
        let anchor = Anchor::from_scroll_top(&model, scroll_offset);
        let window = RealizationWindow::new(self.overscan);
        let placements: Vec<_> = window.place(&model, &anchor, viewport_height).collect();
        let leading = if let Some(first) = placements.first() {
            model.offset_of(first.index)
        } else {
            0.0
        };

        let total_h = model.total_height();
        let mut children: Vec<Element<'a, Message>> = Vec::with_capacity(placements.len() + 2);
        if leading > 0.0 {
            children.push(Space::new().width(Length::Fill).height(leading).into());
        }

        let style = self.style;
        let probe = self.row_builder_probe.clone();
        let scroll_id = self.scroll_id.clone();

        for p in &placements {
            let line_idx = p.index;
            if line_idx >= lines.len() {
                continue;
            }
            let entry_idx = match lines[line_idx] {
                Line::Header(i) | Line::Row(i) => i,
            };
            if entry_idx >= self.entries.len() {
                continue;
            }
            let entry = &self.entries[entry_idx];
            if let Some(pr) = &probe {
                pr.set(pr.get() + 1);
            }
            let el: Element<'a, Message> = match lines[line_idx] {
                Line::Header(_) => self.build_header_element::<Message>(
                    entry.section.as_str(),
                    style,
                    section_size,
                ),
                Line::Row(_) => self.build_row_element::<Message>(entry, style, metrics),
            };
            children.push(
                container(el)
                    .height(Length::Fixed(p.height))
                    .width(Length::Fill)
                    .into(),
            );
        }
        let used: f32 = leading + placements.iter().map(|p| p.height).sum::<f32>();
        let trailing = (total_h - used).max(0.0);
        if trailing > 0.0 {
            children.push(Space::new().width(Length::Fill).height(trailing).into());
        }
        let col = column(children).width(Length::Fill);
        // Reserve scrollbar space so trailing elision stays outside the bar while it is visible.
        let bar = Scrollbar::default()
            .scroller_min_length(enzui_theme::text_defaults::scrollbar().min_thumb_length)
            .spacing(DEFAULT_SCROLLBAR_GUTTER);
        let mut scroll = scrollable(col)
            .direction(Direction::Vertical(bar))
            .width(Length::Fill)
            .height(Length::Fill);
        if let Some(id) = scroll_id {
            scroll = scroll.id(id);
        }
        if let Some(f) = on_scroll {
            scroll = scroll
                .on_scroll(move |scroll: iced::widget::scrollable::Scroll| f(scroll.viewport));
        }
        scroll.into()
    }
}

impl<'a, Message: 'a> From<EnzShortcutList> for Element<'a, Message> {
    fn from(list: EnzShortcutList) -> Self {
        list.into_element()
    }
}

/// Construct a new shortcut list from a vector of [`ShortcutEntry`].
pub fn enz_shortcut_list(entries: Vec<ShortcutEntry>) -> EnzShortcutList {
    EnzShortcutList {
        entries,
        key_width: DEFAULT_KEY_WIDTH,
        row_spacing: DEFAULT_ROW_SPACING,
        section_gap: DEFAULT_SECTION_GAP,
        row_height: DEFAULT_ROW_HEIGHT,
        description_size: DEFAULT_DESCRIPTION_SIZE,
        section_size: DEFAULT_SECTION_SIZE,
        style: ShortcutListStyle::default(),
        badge_tokens: None,
        badge_radius: enzui_theme::text_defaults::shape_radius(|s| s.key),
        badge_text_size: DEFAULT_BADGE_TEXT_SIZE,
        row_spacing_explicit: false,
        description_size_explicit: false,
        badge_text_size_explicit: false,
        section_size_explicit: false,
        section_line_height_explicit: false,
        typography: None,
        spacing_scale: None,
        section_line_height: DEFAULT_SECTION_LINE_HEIGHT,
        viewport_height: None,
        scroll_offset: 0.0,
        keyed_anchor: None,
        overscan: 5,
        scroll_id: None,
        row_builder_probe: None,
    }
}

/// A header or entry row in the flattened render sequence; indices address entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// A section header for the entry at this `entries` index. The header's
    /// label text is `entries[i].section`.
    Header(usize),
    /// The row for the entry at this `entries` index.
    Row(usize),
}

/// Flatten entries into header and row lines in linear time.
/// Headers precede nonempty sections that differ from the immediately previous entry.
pub fn build_line_index(entries: &[ShortcutEntry]) -> Vec<Line> {
    let mut lines = Vec::with_capacity(entries.len() + 4);
    for (i, e) in entries.iter().enumerate() {
        let prev_section = if i == 0 {
            None
        } else {
            Some(entries[i - 1].section.as_str())
        };
        let new_section = !e.section.is_empty() && prev_section != Some(e.section.as_str());
        if new_section {
            lines.push(Line::Header(i));
        }
        lines.push(Line::Row(i));
    }
    lines
}

/// Select nonempty sections that differ from the immediately previous entry.
pub fn section_header_flags(entries: &[ShortcutEntry]) -> Vec<bool> {
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            if e.section.is_empty() {
                false
            } else if i == 0 {
                true
            } else {
                entries[i - 1].section != e.section
            }
        })
        .collect()
}
