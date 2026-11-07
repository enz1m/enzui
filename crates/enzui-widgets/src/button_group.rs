//! A horizontal cluster of joined buttons sharing seams, with rounded outer
//! corners only.
//!
//! # Scope
//!
//! Horizontal only, every item a button, and `block(true)` gives equal-width
//! cells that fill the parent.
//!
//! # Segmented mode
//!
//! `.selectable(true).selected(Some(i))` makes a single-choice picker;
//! `.selected_set(..)` lights several cells (a Bold and Italic toolbar).
//! Selected cells use the audited
//! [`segment_selected_bg`](enzui_theme::Palette::segment_selected_bg) and
//! [`segment_selected_fg`](enzui_theme::Palette::segment_selected_fg)
//! pair, which clears 3:1 against unselected cells in both presets (WCAG
//! 1.4.11); `.selected_background(c)` and `.selected_foreground(c)` override
//! them, and the caller then owns that contrast. Cells publish `Role::Tab`
//! with selected and disabled state; a disabled selected cell keeps its fill,
//! dimmed.
//!
//! # Hazards
//!
//! The cells sit under a `Role::Group`, not a `TabList`, so assistive
//! technology reports no "tab 2 of 3" and arrow keys do not move between
//! them. To switch views, use [`view_switcher`](crate::view_switcher) or
//! [`inline_view_switcher`](crate::inline_view_switcher); use this for
//! options such as a body format or sort key.

use crate::button::{ButtonKind, KindInteraction, kind_is_framed, kind_visual};

/// How a selected cell of a segmented [`enz_button_group`] shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonGroupStyle {
    /// A flat neutral thumb inset in the track, with the label in text ink,
    /// as macOS and iOS segmented controls draw it: the default for a single
    /// choice.
    Thumb,
    /// A soft accent tint with accent ink, as GNOME's toggle groups draw
    /// it: the default for a set of toggles (Bold, Italic), where a raised
    /// thumb would read as a pressed button.
    Tonal,
    /// A solid accent pill with white ink: the most emphatic.
    Accent,
}

/// The style a group paints with: the caller's, else Thumb for a single
/// choice and Tonal for a set of toggles. A group outside the track, or
/// with a pinned selected background, keeps the Accent shape.
fn effective_style(
    track: bool,
    pinned_background: bool,
    chosen: Option<ButtonGroupStyle>,
    multiple: bool,
) -> ButtonGroupStyle {
    if !track || pinned_background {
        return ButtonGroupStyle::Accent;
    }
    chosen.unwrap_or(if multiple {
        ButtonGroupStyle::Tonal
    } else {
        ButtonGroupStyle::Thumb
    })
}

/// The label and icon ink of a selected cell.
fn selected_ink(
    style: ButtonGroupStyle,
    palette: &enzui_theme::Palette,
    fg: Option<Color>,
) -> Color {
    if let Some(fg) = fg {
        return fg;
    }
    match style {
        ButtonGroupStyle::Thumb => palette.text,
        ButtonGroupStyle::Tonal => tonal_ink(palette),
        ButtonGroupStyle::Accent => palette.segment_selected_fg(),
    }
}

/// Accent ink that reads as text on the tonal fill over the page.
fn tonal_ink(palette: &enzui_theme::Palette) -> Color {
    enzui_theme::Palette::with_contrast_on(palette.surface, palette.accent, 4.5)
}

/// The fill of a selected cell under `status` for the Thumb and Tonal
/// styles (Accent keeps [`resolve_selectable_bg`]).
fn selected_style_fill(
    style: ButtonGroupStyle,
    status: FocusStatus,
    palette: &enzui_theme::Palette,
) -> Color {
    let step = match status {
        FocusStatus::Hovered => 1.0,
        FocusStatus::Pressed => 2.0,
        _ => 0.0,
    };
    match style {
        // White on a light track, the raised button fill on a dark one.
        ButtonGroupStyle::Thumb => mix_over_selected(
            if palette.is_dark() {
                palette.button_primary_bg
            } else {
                palette.surface
            },
            palette.text,
            step * 0.04,
        ),
        _ => Color {
            a: if palette.is_dark() { 0.26 } else { 0.18 } + step * 0.06,
            ..palette.accent
        },
    }
}

/// How far a selected pill sits inside the track, on every side.
const TRACK_INSET: f32 = 3.0;
/// How far a seam between unselected cells stops short of the track's top
/// and bottom.
const TRACK_SEAM_INSET: f32 = 7.0;

/// `color` with its alpha multiplied by `opacity`, so a translucent colour
/// stays as translucent as it was relative to the rest.
fn dim_alpha(color: Color, opacity: f32) -> Color {
    Color {
        a: color.a * opacity,
        ..color
    }
}
use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use crate::text_field::TextFieldSize;
use crate::theme_lookup;
use crate::tokens::ButtonTokens;
use enzui_core::semantics::{Role, Semantics};
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{container, text};
use iced::{Border, Color, Element, Length};

/// One action in an [`enz_button_group`] cluster, keyed by a stable logical id.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzButtonGroupItem {
    /// Stable logical key for this item, used to derive its cell id.
    pub id: String,
    /// Text shown on the button.
    pub label: String,
    /// Optional Lucide icon name (canonical or semantic alias, resolved by
    /// `enzui-icons`). Renders before the label when set.
    pub icon: Option<String>,
    /// Whether this individual button is interactive. Defaults to `true` via
    /// [`EnzButtonGroupItem::new`].
    pub enabled: bool,
}

impl EnzButtonGroupItem {
    /// Build an enabled item with the given stable id and label.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            enabled: true,
        }
    }
    /// Set an icon for this item (Lucide canonical name or semantic alias).
    /// Empty string is treated as no icon.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        let n: String = name.into();
        self.icon = if n.is_empty() { None } else { Some(n) };
        self
    }
    /// Mark this item disabled (chainable).
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// The segment height when neither
/// [`button_height`](EnzButtonGroup::button_height) nor
/// [`size`](EnzButtonGroup::size) is set: the inline-control family's medium
/// rung, [`MD`](crate::tokens::FieldSizeTokens::MD), so a segmented control
/// beside a search field matches it and follows any retune of the field
/// ladder.
pub const DEFAULT_BUTTON_HEIGHT: f32 = crate::tokens::FieldSizeTokens::MD.height;

/// Builder for a segmented button group. See [`enz_button_group`].
#[must_use]
pub struct EnzButtonGroup<'a, Message: Clone> {
    items: Vec<EnzButtonGroupItem>,
    on_activate: Box<dyn Fn(usize) -> Message + 'a>,
    group_enabled: bool,
    kind: ButtonKind,
    button_height: f32,
    corner_radius: Option<f32>,
    block: bool,
    label_size: f32,
    padding_x: f32,
    /// Segmented mode — when true, the group treats itself as a single-
    /// choice picker and paints `selected_index` with `selected_bg`/`_fg`.
    /// Ported for `EnzInlineViewSwitcher` and other segmented use.
    /// See module doc "Deferred" section — this is the baseline segmented
    /// mode; animated pill and keyboard navigation are not provided.
    selectable: bool,
    selected_index: Option<usize>,
    /// Multi-select mode — when `Some`, *every* index in the set paints in the
    /// selected background, so a group can render two or more cells selected at
    /// once (a formatting toolbar lighting Bold and Italic together).
    /// Mutually exclusive with [`selected`](EnzButtonGroup::selected): the last
    /// of the two setters called wins (each clears the other), so a caller can
    /// never accidentally mix a single index and a set. `selectable` still gates
    /// whether any selection paints at all.
    selected_set: Option<Vec<usize>>,
    selected_bg: Option<Color>,
    selected_fg: Option<Color>,
    style: Option<ButtonGroupStyle>,
    /// Icon-only mode: cells with an icon render only the icon, and keep
    /// their `label` as the accessible name. Cells without an icon still show
    /// their label, so mixed groups are safe. Off by default. For compact
    /// toolbars (Bold, Italic) that show a glyph but announce "Bold".
    icon_only: bool,
    collapse_labels: bool,
    dividers: bool,
    /// Prefix for the stable cell ids, `{id_prefix}-{item.id}` (shared
    /// [`KeyedIds`](enzui_core::identity::KeyedIds)), so focus and
    /// semantic actions follow a segment across reorders and filtering.
    /// Defaults to `"enz-button-group"`; give groups on the same screen
    /// distinct prefixes.
    id_prefix: String,
    /// Opacity multiplied into a disabled selected cell's fill and ink;
    /// `None` follows the theme's `opacity.disabled_surface`.
    disabled_alpha: Option<f32>,
    /// Width of the outer frame and of the seams between cells.
    border_width: f32,
}

/// Construct a button group from items and an activation callback (receives the
/// activated item's index). Defaults to enabled, [`DEFAULT_BUTTON_HEIGHT`],
/// 12 px labels, shrink width, and no selection. Outer corners follow live
/// `radius.md` (6 px by default); inner corners remain square. Invalid theme
/// radii fall back to 8 px.
pub fn enz_button_group<'a, F, Message: Clone + 'a>(
    items: Vec<EnzButtonGroupItem>,
    on_activate: F,
) -> EnzButtonGroup<'a, Message>
where
    F: Fn(usize) -> Message + 'a,
{
    EnzButtonGroup {
        items,
        on_activate: Box::new(on_activate),
        group_enabled: true,
        kind: ButtonKind::Secondary,
        button_height: DEFAULT_BUTTON_HEIGHT,
        corner_radius: None,
        block: false,
        label_size: enzui_theme::text_scale::ui_px(12.0),
        padding_x: 12.0,
        selectable: false,
        selected_index: None,
        selected_set: None,
        style: None,
        selected_bg: None,
        selected_fg: None,
        icon_only: false,
        collapse_labels: false,
        dividers: true,
        id_prefix: "enz-button-group".into(),
        disabled_alpha: None,
        border_width: 1.0,
    }
}

impl<'a, Message: Clone + 'a> EnzButtonGroup<'a, Message> {
    /// Enable or disable the whole group at once. Defaults to `true`. Per-item
    /// `enabled` flags still apply within an enabled group.
    pub fn group_enabled(mut self, enabled: bool) -> Self {
        self.group_enabled = enabled;
        self
    }
    /// Shared kind for the whole cluster. Defaults to
    /// [`ButtonKind::Secondary`]. The kind controls background fills,
    /// foreground ink, border treatment, and destructive action disposition
    /// for every cell in the group.
    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }
    /// Button height in px. Non-finite or non-positive input falls back to the
    /// shared [`DEFAULT_BUTTON_HEIGHT`].
    pub fn button_height(mut self, h: f32) -> Self {
        self.button_height =
            enzui_core::metrics::FinitePositive::new_or(h, DEFAULT_BUTTON_HEIGHT).get();
        self
    }

    /// Put the segmented control on an inline-control size tier. This is
    /// the setter for a toolbar row shared with fields:
    /// `.size(TextFieldSize::Sm)` makes the control exactly as tall as an
    /// `enz_search_field(..).size(TextFieldSize::Sm)` beside it, because both
    /// resolve through [`tokens`](TextFieldSize::tokens). Equivalent to
    /// `button_height(size.tokens().height)`; the label size and horizontal
    /// padding are left alone.
    pub fn size(mut self, size: TextFieldSize) -> Self {
        self.button_height = size.tokens().height;
        self
    }
    /// Pin outer corners in logical pixels, overriding the live theme.
    /// Non-finite or negative input pins 8 px.
    pub fn corner_radius(mut self, r: f32) -> Self {
        self.corner_radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(r, 8.0).get());
        self
    }
    /// Opacity multiplied into a disabled selected cell's fill and ink, so it
    /// still reads as the choice but not as available. Unset, it follows the
    /// theme's `opacity.disabled_surface` (default 0.55, the same role as
    /// [`TabPillTokens::segment_disabled_alpha`](crate::tab_pill::TabPillTokens::segment_disabled_alpha)).
    /// Finite values clamp to `0..=1`; others leave it following the theme.
    /// Unselected disabled cells use the button tokens' disabled colours.
    pub fn disabled_alpha(mut self, alpha: f32) -> Self {
        self.disabled_alpha = alpha.is_finite().then(|| alpha.clamp(0.0, 1.0));
        self
    }
    /// Width in logical pixels of the outer frame and of the seams between
    /// cells (default 1). It applies to the framed kinds (Secondary and
    /// Outlined); zero hides both. Invalid values use the default.
    pub fn border_width(mut self, width: f32) -> Self {
        self.border_width = enzui_core::metrics::FiniteNonNeg::new_or(width, 1.0).get();
        self
    }
    /// Stretch the group to fill its container's width, sharing it equally
    /// across cells. Defaults to `false` (cells hug their content).
    pub fn block(mut self, b: bool) -> Self {
        self.block = b;
        self
    }
    /// Button-label text size in px. Non-finite or non-positive input falls
    /// back to 12.
    pub fn label_size(mut self, s: f32) -> Self {
        self.label_size = enzui_core::metrics::FinitePositive::new_or(s, 12.0).get();
        self
    }
    /// Horizontal cell padding in px. Normalized to finite non-negative;
    /// non-finite input falls back to `0`.
    pub fn padding_x(mut self, p: f32) -> Self {
        self.padding_x = enzui_core::metrics::FiniteNonNeg::new_or(p, 0.0).get();
        self
    }
    /// Turn on segmented (single-choice) mode: the cell at `selected_index`
    /// paints the selected fill and every cell publishes its selected state.
    /// See the module docs for when to use a view switcher instead.
    pub fn selectable(mut self, s: bool) -> Self {
        self.selectable = s;
        self
    }
    /// Show a cell's label only while it is selected: the others show their
    /// icon alone, and a label slides open as its cell is chosen and closed
    /// as it is left. Cells without an icon keep their label. Off by
    /// default.
    pub fn collapse_inactive_labels(mut self, collapse: bool) -> Self {
        self.collapse_labels = collapse;
        self
    }

    /// Draw the short dividers between cells (default `true`).
    pub fn dividers(mut self, dividers: bool) -> Self {
        self.dividers = dividers;
        self
    }

    /// Render cells that have an icon as icon only, keeping their labels
    /// as accessible names. Cells without an icon still show their label. For
    /// compact clusters with self-evident glyphs (toolbar Bold, Italic,
    /// Code). Off by default.
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }
    /// Namespace for the stable, item-key-derived cell ids. Set a
    /// distinct prefix when several groups share a screen.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }
    /// Which item is currently selected. Naming one turns on segmented mode
    /// ([`selectable`](Self::selectable)): a group told what is selected
    /// shows it. `None` leaves the mode as it was.
    ///
    /// Mutually exclusive with [`selected_set`](Self::selected_set): calling
    /// this clears any previously supplied multi-select set, so the group is
    /// unambiguously single-select.
    pub fn selected(mut self, idx: Option<usize>) -> Self {
        self.selectable |= idx.is_some();
        self.selected_index = idx;
        self.selected_set = None;
        self
    }
    /// Multi-select mode: paint every cell in `indices` as selected, for
    /// groups like Bold and Italic that can be on together. Turns on
    /// [`selectable`](Self::selectable), as [`selected`](Self::selected)
    /// does. Clears any earlier [`selected`](Self::selected). Out-of-range
    /// indices match no cell, and duplicates count once.
    pub fn selected_set(mut self, indices: impl Into<Vec<usize>>) -> Self {
        self.selectable = true;
        self.selected_set = Some(indices.into());
        self.selected_index = None;
        self
    }
    /// How a selected cell of the default Secondary group shows; see
    /// [`ButtonGroupStyle`]. Unset, a single choice takes
    /// [`Thumb`](ButtonGroupStyle::Thumb) and a set of toggles
    /// [`Tonal`](ButtonGroupStyle::Tonal).
    pub fn style(mut self, style: ButtonGroupStyle) -> Self {
        self.style = Some(style);
        self
    }

    /// Override the selected-cell background; the selected cells then take
    /// the [`Accent`](ButtonGroupStyle::Accent) shape in this colour.
    pub fn selected_background(mut self, c: Color) -> Self {
        self.selected_bg = Some(c);
        self
    }
    /// Override the selected-cell foreground (default: theme primary text).
    pub fn selected_foreground(mut self, c: Color) -> Self {
        self.selected_fg = Some(c);
        self
    }
    /// The number of items in the group.
    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // Secondary `EnzButton`s draw their own 1 px `border_strong`, which
        // would frame every cell where cells meet. The group instead draws
        // one outer border plus 1 px dividers between adjacent cells. Every
        // colour resolves from the runtime theme inside the style closure
        // (`theme_lookup`, `ButtonTokens::from_palette`), so the group follows
        // a light theme; caller `selected_bg`/`selected_fg` overrides win.
        let selected_bg_override = self.selected_bg;
        let selected_fg_override = self.selected_fg;
        let n = self.items.len();
        let radius = self.corner_radius;
        let height = self.button_height;
        let label_size = self.label_size;
        let padding_x = self.padding_x;
        let group_enabled = self.group_enabled;
        let kind = self.kind;
        let block = self.block;
        let pinned_disabled_alpha = self.disabled_alpha;
        let disabled_alpha = move |theme: &iced::Theme| {
            pinned_disabled_alpha.unwrap_or_else(|| theme_lookup::opacity(theme).disabled_surface())
        };
        let border_width = self.border_width;
        // Only the bordered kinds (Secondary, Outlined) and segmented mode
        // draw the shared frame and dividers; the fill and frame come from the
        // same `kind_visual`.
        let selectable = self.selectable;
        let icon_only = self.icon_only;
        let collapse_labels = self.collapse_labels;
        let dividers = self.dividers;
        let framed = selectable || kind_is_framed(kind);
        // A macOS-style segmented control: a neutral track, and each selected cell
        // a flat pill inset in it.
        let track = framed && kind == ButtonKind::Secondary;
        let style = effective_style(
            track,
            selected_bg_override.is_some(),
            self.style,
            self.selected_set.is_some(),
        );
        let selected_index = self.selected_index;
        // multi-select set. `selected` and `selected_set` are mutually
        // exclusive at the builder (each clears the other), so at most one of
        // these is populated; a cell is selected if it matches either.
        let selected_set = self.selected_set;
        // cell ids come from the shared deriver rather than a local
        // `format!`. Unique item keys keep their direct derivation; a *repeated*
        // item key is disambiguated by occurrence so two cells never silently
        // share one Id and leave one unreachable.
        let cell_ids: Vec<iced::advanced::widget::Id> = {
            let mut keyed = enzui_core::identity::KeyedIds::new(self.id_prefix);
            self.items.iter().map(|i| keyed.id_for(&i.id)).collect()
        };
        // hoist the activation factory into an Rc so each cell's lazy
        // `on_press_with` closure holds its own handle. The factory then runs
        // once per real activation (not at view-build time), and disabled cells
        // construct no message.
        let on_activate: std::rc::Rc<dyn Fn(usize) -> Message + 'a> =
            std::rc::Rc::from(self.on_activate);

        // Cells sit in a `KeyedRow` keyed like their ids, so retained focus,
        // press and hover state follow an item when items are inserted,
        // removed or reordered.
        let mut row_keys = enzui_core::identity::KeyedIds::new("cell");
        let cell_keys: Vec<String> = self.items.iter().map(|i| row_keys.key_for(&i.id)).collect();
        let mut keyed_children: Vec<(String, Element<'a, Message>)> = Vec::new();
        // One definition of "this cell paints selected", used by
        // both the cell fill and the seam that precedes it. A cell paints
        // selected if it is the single selected index or a member of the
        // multi-select set; the two are mutually exclusive at the builder, so
        // at most one branch is ever non-empty.
        let is_selected_at = |i: usize| -> bool {
            selectable
                && (selected_index == Some(i)
                    || selected_set.as_ref().is_some_and(|set| set.contains(&i)))
        };
        for (index, item) in self.items.into_iter().enumerate() {
            let cell_key = cell_keys[index].clone();
            let is_first = index == 0;
            let is_last = index + 1 == n;
            let enabled = group_enabled && item.enabled;

            // Insert a 1 px vertical divider before every non-first cell so
            // adjacent buttons show a visible seam. Dividers only belong to
            // framed kinds; a borderless-kind group
            // (Primary/Ghost/Destructive/Accent) shows no internal seams.
            if !is_first && framed && dividers {
                // The seam between two selected cells is a deeper shade of the
                // selected fill; elsewhere it is the usual hairline.
                let between_selected = is_selected_at(index - 1) && is_selected_at(index);
                if track {
                    // A short, faint seam between every two cells, the
                    // selected pill's included, so the rhythm of the strip
                    // stays even whichever cell is chosen.
                    let seam = container(
                        iced::widget::Space::new()
                            .width(Length::Fixed(border_width))
                            .height(Length::Fixed((height - 2.0 * TRACK_SEAM_INSET).max(0.0))),
                    )
                    .style(move |theme: &iced::Theme| {
                        iced::widget::container::Style {
                            // A tint of the ink rather than the `border` role,
                            // which all but vanishes on a light track.
                            background: Some(
                                crate::color::with_alpha(theme_lookup::palette(theme).text, 0.16)
                                    .into(),
                            ),
                            ..Default::default()
                        }
                    });
                    let sep = container(seam)
                        .width(Length::Fixed(border_width))
                        .height(Length::Fixed(height))
                        .center_y(Length::Fixed(height));
                    keyed_children.push((format!("sep:{cell_key}"), sep.into()));
                } else {
                    let sep = container(
                        iced::widget::Space::new()
                            .width(Length::Fixed(border_width))
                            .height(Length::Fixed(height)),
                    )
                    .width(Length::Fixed(border_width))
                    .height(Length::Fixed(height))
                    .style(move |theme: &iced::Theme| {
                        iced::widget::container::Style {
                            // The seam between two segments of one control, like
                            // `tab_pill`'s seam, is a divider line: the soft `border`
                            // role, not a 3:1 outline.
                            background: Some(if between_selected {
                                theme_lookup::palette(theme).segment_selected_seam().into()
                            } else {
                                theme_lookup::palette(theme).border.into()
                            }),
                            ..Default::default()
                        }
                    });
                    // The divider is a child of the strip too: an unkeyed gap in a
                    // keyed row still shifts every sibling after it. Keyed by the
                    // cell it precedes, so it is exactly as stable as that cell.
                    keyed_children.push((format!("sep:{cell_key}"), sep.into()));
                }
            }

            let cell_id = cell_ids[index].clone();
            let aria_label = item.label.clone();
            let item_icon = item.icon.clone();
            let label_el = group_label(&item.label, label_size);
            let on_press: Option<Box<dyn Fn() -> Message + 'a>> = if enabled {
                let on_activate = std::rc::Rc::clone(&on_activate);
                Some(Box::new(move || (on_activate)(index)))
            } else {
                None
            };
            let is_selected = is_selected_at(index);
            // In segmented (single-choice) mode each cell is a `Tab` in the
            // group's strip. Action-cluster mode keeps `Button`. The group is
            // deliberately not a `TabList` and does not claim roving focus or
            // arrow-key navigation.
            let cell_role = if selectable { Role::Tab } else { Role::Button };
            let mut cell_semantics = Semantics::new(cell_role)
                .label(aria_label)
                .destructive_action(matches!(
                    kind,
                    ButtonKind::Destructive
                        | ButtonKind::DestructiveOutlined
                        | ButtonKind::QuietDestructive
                ))
                .disabled(!enabled)
                // Each cell is positioned within the cluster, 1-based
                // ("3 of 5"), so it reads as a segmented control rather than
                // unrelated buttons; dividers are not cells and do not count.
                // The default action (AccessKit `Click`) is not set here:
                // `FocusButton::operate` adds it only when `on_press` is set,
                // so disabled cells do not advertise it
                // (`activatable_widgets_advertise_click.rs`).
                .set_position(enzui_core::semantics::SetPosition {
                    index: index + 1,
                    count: n,
                });
            // Only expose selected-state in segmented mode (a `tab` is
            // selectable; an action `button` is not).
            if selectable {
                cell_semantics = cell_semantics.selected(is_selected);
            }

            // Build cell content: optional icon + label.
            // The icon follows the same pipeline as `enz_button`: it is
            // tinted with the per-kind foreground resolved from the runtime
            // theme, so it re-tints on light/dark switch in lock-step with
            // the label. We use `tint_resolved` with a closure that reads
            // the per-kind fg from ButtonTokens.
            let cell_content: Element<'a, Message> = if let Some(ref icon_name) = item_icon {
                let fg_role = crate::icon::ButtonFgRole::from(kind);
                // icon size matches label size
                let icon_el = crate::icon::enz_icon(icon_name).size(label_size);
                // a SELECTED cell's icon takes the selected ink, not the
                // per-kind one. The label already did (`text_color` below); the
                // icon did not, so raising the selected fill to the mid-tone
                // `segment_selected_bg` would have left `text` glyphs at 2.15:1
                // on it — the lesson (fixing the fill moves the defect onto
                // the ink) applied to the channel that is easy to forget.
                let icon_el = if is_selected {
                    icon_el.tint_resolved(move |theme: &iced::Theme| {
                        let fg = selected_ink(
                            style,
                            &theme_lookup::palette(theme),
                            selected_fg_override,
                        );
                        if enabled {
                            fg
                        } else {
                            dim_alpha(fg, disabled_alpha(theme))
                        }
                    })
                } else {
                    icon_el.tint_button_fg(fg_role, enabled)
                };
                if icon_only {
                    // icon-only cell — the label is dropped from the
                    // visible content but survives as `aria_label` on the
                    // cell's semantics above, so the a11y tree still names it.
                    icon_el.into()
                } else {
                    if collapse_labels {
                        // The label and its gap slide open as the cell is
                        // chosen and closed as it is left.
                        iced::widget::row![
                            icon_el,
                            crate::reveal_width::reveal_width(
                                iced::widget::row![iced::widget::Space::new().width(4), label_el]
                                    .align_y(iced::Alignment::Center),
                                is_selected,
                            )
                        ]
                        .align_y(iced::Alignment::Center)
                        .into()
                    } else {
                        iced::widget::row![icon_el, label_el]
                            .spacing(4)
                            .align_y(iced::Alignment::Center)
                            .into()
                    }
                }
            } else {
                label_el.into()
            };

            // The cell's status, written by its style and read by the pill
            // painted inside it, which draws before the content.
            let status_cell = std::rc::Rc::new(std::cell::Cell::new(FocusStatus::Active));
            // How much of the selected pill shows: it fades in on the chosen
            // cell as it fades out on the one left.
            let fade: crate::row_motion::RowMotionProgress =
                std::rc::Rc::new(std::cell::Cell::new(if is_selected { 1.0 } else { 0.0 }));
            let cell_body: Element<'a, Message> = if track {
                let status_cell = std::rc::Rc::clone(&status_cell);
                let fade = std::rc::Rc::clone(&fade);
                let pill = container(cell_content)
                    .center_y(Length::Fill)
                    .center_x(Length::Fill)
                    .padding([0, enzui_core::metrics::to_u16_saturating(padding_x)])
                    .style(move |theme: &iced::Theme| {
                        let palette = theme_lookup::palette(theme);
                        let status = status_cell.get();
                        let shown = fade.get().clamp(0.0, 1.0);
                        // A cell fading its pill out shows it at rest.
                        let pill_status = if is_selected {
                            status
                        } else {
                            FocusStatus::Active
                        };
                        let background = if shown > 0.0 {
                            let fill = if style == ButtonGroupStyle::Accent {
                                resolve_selectable_bg(
                                    pill_status,
                                    true,
                                    enabled,
                                    selected_bg_override
                                        .unwrap_or_else(|| palette.segment_selected_bg()),
                                    selected_bg_override.is_some(),
                                    kind,
                                    &palette,
                                    &ButtonTokens::from_palette(&palette),
                                    disabled_alpha(theme),
                                )
                            } else {
                                let fill = selected_style_fill(style, pill_status, &palette);
                                if enabled {
                                    fill
                                } else {
                                    dim_alpha(fill, disabled_alpha(theme))
                                }
                            };
                            Some(dim_alpha(fill, shown))
                        } else if !enabled {
                            None
                        } else {
                            match status {
                                FocusStatus::Hovered => Some(palette.overlay_hover()),
                                FocusStatus::Pressed => Some(palette.overlay_pressed()),
                                _ => None,
                            }
                        };
                        let edged = shown > 0.0
                            && style == ButtonGroupStyle::Thumb
                            && enabled
                            && !palette.is_on_glass()
                            && !palette.is_high_contrast();
                        iced::widget::container::Style {
                            background: background.map(Into::into),
                            border: Border {
                                radius: (resolve_radius(theme, radius) - TRACK_INSET)
                                    .max(0.0)
                                    .into(),
                                // A light thumb takes a hairline where it
                                // meets the light track.
                                color: Color {
                                    a: 0.07 * shown,
                                    ..Color::BLACK
                                },
                                width: if edged && !palette.is_dark() {
                                    0.5
                                } else {
                                    0.0
                                },
                            },
                            ..Default::default()
                        }
                    });
                container(pill)
                    .padding(TRACK_INSET)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            } else {
                container(cell_content)
                    .center_y(Length::Fill)
                    .center_x(Length::Fill)
                    .padding([0, enzui_core::metrics::to_u16_saturating(padding_x)])
                    .into()
            };
            let mut btn = focus_button_maybe_with(cell_body, on_press)
                .height(Length::Fixed(height))
                .padding(0)
                .id(cell_id)
                .semantics(cell_semantics)
                // use contrast-aware outset ring so the focus
                // indicator is visible even when the cell fill matches
                // `tokens.focus_ring`.
                .outset_focus_ring(true)
                .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                    status_cell.set(status);
                    let palette = theme_lookup::palette(theme);
                    let bt = ButtonTokens::from_palette(&palette);
                    // The audited segment selection pair keeps the selected cell 3:1 from the
                    // unselected ones (WCAG SC 1.4.11); see `Palette::segment_selected_bg`.
                    let selected_bg =
                        selected_bg_override.unwrap_or_else(|| palette.segment_selected_bg());
                    let selected_fg = selected_ink(style, &palette, selected_fg_override);
                    // Selected cells derive their hover and press steps from the
                    // selected fill (or its override), so a selected cell still
                    // responds to the pointer. Focus keeps the resting fill.
                    let bg_col = resolve_selectable_bg(
                        status,
                        is_selected,
                        enabled,
                        selected_bg,
                        selected_bg_override.is_some(),
                        kind,
                        &palette,
                        &bt,
                        disabled_alpha(theme),
                    );
                    let radius = resolve_radius(theme, radius);
                    let border = Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: iced::border::Radius {
                            top_left: if is_first { radius } else { 0.0 },
                            bottom_left: if is_first { radius } else { 0.0 },
                            top_right: if is_last { radius } else { 0.0 },
                            bottom_right: if is_last { radius } else { 0.0 },
                        },
                    };
                    ButtonStyle {
                        // In the track the pill inside paints the cell.
                        background: (!track).then(|| bg_col.into()),
                        text_color: if is_selected && !enabled {
                            // Keep the selected foreground, dimmed like the
                            // whole selected cell.
                            dim_alpha(selected_fg, disabled_alpha(theme))
                        } else if is_selected {
                            selected_fg
                        } else if !enabled {
                            bt.disabled_fg
                        } else {
                            kind_fg(&bt, kind, status)
                        },
                        border,
                        ..ButtonStyle::default()
                    }
                });
            if block {
                btn = btn.width(Length::Fill);
            }
            let btn: Element<'a, Message> = if track {
                crate::row_motion::RowMotion::new(btn.into(), is_selected, None, fade).into()
            } else {
                btn.into()
            };
            keyed_children.push((cell_key, btn));
        }
        let row = crate::keyed_row::keyed_row(keyed_children).spacing(0.0);

        // A `Container` paints before its child, so putting the frame on the
        // row's own container lets opaque cells erase it. Instead, make the
        // unpadded cell strip the Stack's base (and therefore its intrinsic
        // geometry), then paint an inert frame layer over it. This preserves
        // both the historic shrink width and the requested total height while
        // keeping all four straight edges visible.
        let mut strip =
            container(row)
                .height(Length::Fixed(height))
                .style(move |theme: &iced::Theme| iced::widget::container::Style {
                    background: track.then(|| {
                        ButtonTokens::from_palette(&theme_lookup::palette(theme))
                            .secondary_bg
                            .into()
                    }),
                    border: Border {
                        radius: resolve_radius(theme, radius).into(),
                        ..Border::default()
                    },
                    ..Default::default()
                });
        if block {
            strip = strip.width(Length::Fill);
        }
        let frame = container(
            iced::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |theme: &iced::Theme| iced::widget::container::Style {
            background: None,
            border: if framed && !track {
                Border {
                    // The frame is the kind's own edge: a shade of the
                    // Secondary fill, or Outlined's outline.
                    color: {
                        let bt = ButtonTokens::from_palette(&theme_lookup::palette(theme));
                        let frame_kind = if kind_is_framed(kind) {
                            kind
                        } else {
                            ButtonKind::Secondary
                        };
                        kind_visual(frame_kind, KindInteraction::Active, &bt).border_color
                    },
                    width: border_width,
                    radius: resolve_radius(theme, radius).into(),
                }
            } else {
                Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: resolve_radius(theme, radius).into(),
                }
            },
            ..Default::default()
        });
        let mut c = iced::widget::stack![strip, frame].height(Length::Fixed(height));
        if block {
            c = c.width(Length::Fill);
        }

        // A nameless group parents the cells, so assistive technology hears
        // one control with positions rather than unrelated buttons, and it
        // forwards the group's disabled state.
        let group_semantics = Semantics::new(Role::Group).disabled(!group_enabled);
        crate::semantics_container::SemanticsContainer::new(c, group_semantics).into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzButtonGroup<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzButtonGroup<'a, Message>) -> Self {
        w.into_element()
    }
}

fn resolve_radius(theme: &iced::Theme, pin: Option<f32>) -> f32 {
    pin.unwrap_or_else(|| {
        let radius = theme_lookup::shape_radius(theme, |shape| shape.button_group);
        enzui_core::metrics::FiniteNonNeg::new_or(radius, 9999.0).get()
    })
}

/// A cell label on one line: a long label is clipped by the fixed-height cell
/// rather than wrapping onto a second line outside it. Every cell's label
/// has the same weight, selected or not, so selecting moves nothing.
fn group_label<'a>(label: &str, size: f32) -> iced::widget::Text<'a, iced::Theme> {
    text(label.to_owned())
        .size(size)
        .font(enzui_theme::text_defaults::font(
            enzui_theme::FontRole::Ui,
            enzui_theme::WeightRung::Medium,
        ))
        .wrapping(iced::widget::text::Wrapping::None)
}

/// `base` with the text colour washed over it at `alpha` (0.07 hover, 0.12
/// pressed), composited to an opaque colour, so an overridden selected fill
/// still gets hover and press steps.
fn mix_over_selected(base: Color, text: Color, alpha: f32) -> Color {
    let a = alpha.clamp(0.0, 1.0);
    let g = 1.0 - a;
    Color::from_rgb(
        base.r * g + text.r * a,
        base.g * g + text.g * a,
        base.b * g + text.b * a,
    )
}

/// The background of one selectable cell for its status and flags, in order:
/// disabled-selected, selected-hover, selected-pressed, selected-rest,
/// disabled-unselected, then the kind's own resolver.
#[allow(clippy::too_many_arguments, reason = "the style inputs of one segment")]
fn resolve_selectable_bg(
    status: FocusStatus,
    is_selected: bool,
    enabled: bool,
    selected_bg: Color,
    override_present: bool,
    kind: ButtonKind,
    palette: &enzui_theme::palette::Palette,
    bt: &crate::tokens::ButtonTokens,
    disabled_alpha: f32,
) -> Color {
    if is_selected && !enabled {
        return dim_alpha(selected_bg, disabled_alpha);
    }
    if is_selected {
        return match status {
            FocusStatus::Hovered => {
                if override_present {
                    mix_over_selected(selected_bg, palette.text, 0.07)
                } else {
                    palette.segment_selected_hover_bg()
                }
            }
            FocusStatus::Pressed => {
                if override_present {
                    mix_over_selected(selected_bg, palette.text, 0.12)
                } else {
                    palette.segment_selected_pressed_bg()
                }
            }
            // Focused keeps the resting fill — the focus ring is the cue,
            // not a bg step. Active is the resting state.
            FocusStatus::Active | FocusStatus::Focused | FocusStatus::Disabled => selected_bg,
        };
    }
    if !enabled {
        return bt.disabled_bg;
    }
    if kind == ButtonKind::Secondary {
        return segment_unselected_bg(status, palette);
    }
    kind_bg(bt, kind, status)
}

/// The fill of an unselected Secondary cell in a selectable (segmented)
/// group. The standalone Secondary button is a translucent tint, but
/// a segmented cell has to hold the 3:1 state cue against the selected cell
/// beside it on any backdrop, so it stays an opaque neutral: `surface`, and
/// `surface_hover` under the pointer.
fn segment_unselected_bg(status: FocusStatus, palette: &enzui_theme::palette::Palette) -> Color {
    match status {
        FocusStatus::Hovered | FocusStatus::Pressed => palette.surface_hover,
        FocusStatus::Active | FocusStatus::Focused | FocusStatus::Disabled => palette.surface,
    }
}

/// Collapse the group's [`FocusStatus`] onto the shared three-state
/// [`KindInteraction`] axis. `Focused` and `Disabled` never reach the per-kind
/// resolver — the group resolves the focus ring and disabled fill itself,
/// before calling [`kind_bg`]/[`kind_fg`] — so both map to the resting
/// `Active` interaction.
fn interaction_of(status: FocusStatus) -> KindInteraction {
    match status {
        FocusStatus::Hovered => KindInteraction::Hovered,
        FocusStatus::Pressed => KindInteraction::Pressed,
        FocusStatus::Active | FocusStatus::Focused | FocusStatus::Disabled => {
            KindInteraction::Active
        }
    }
}

/// Return the background colour for the given kind + focus status.
///
/// Delegates to the shared [`kind_visual`] resolver in `button.rs` so the group
/// and the standalone button paint identical per-kind chrome — in particular an
/// Outlined group is transparent at rest, not an opaque secondary fill.
fn kind_bg(bt: &ButtonTokens, kind: ButtonKind, status: FocusStatus) -> Color {
    kind_visual(kind, interaction_of(status), bt).bg
}

/// Return the foreground (label ink) colour for the given kind and state.
///
/// Delegates to the shared [`kind_visual`] resolver. `status` matters
/// only for `Accent`, whose ink is measured per state.
fn kind_fg(bt: &ButtonTokens, kind: ButtonKind, status: FocusStatus) -> Color {
    kind_visual(kind, interaction_of(status), bt).fg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
