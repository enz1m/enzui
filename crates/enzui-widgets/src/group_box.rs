//! EnzGroupBox: a titled sunken content grouping.
//!
//! Renders as a title row (optional title text + optional leading icon) above
//! a `sunken` Card containing the caller's content. The classic "Preferences
//! group" surface.
//!
//! * Title uses `EnzText { kind: "body"; weight: "semibold" }` → 13 px
//!   semibold (via [`enz_text`] `TextSize::Headline`, which is exactly
//!   13 px Semibold, so the title tracks the runtime theme colour).
//! * Optional `title-icon` / `icon-color` render an [`EnzIconSlot`](crate::icon_slot::EnzIconSlot) before the
//!   title. Icon tint defaults to the
//!   runtime theme's muted colour unless pinned.
//! * The surrounding box is a sunken Card (surface-sunken fill,
//!   transparent 1 px stroke by default; `.bordered(true)` opts in), and its
//!   outer corner is [`EnzGroupBox::radius`] (default
//!   8 px). [`EnzGroupBox::content_radius`] is a *separate* helper for an
//!   inset child placed inside the padding: its arc must be tighter than
//!   the box's by exactly the padding to stay concentric, so it is
//!   `max(0, radius - padding)`. The two are not interchangeable — feeding the
//!   concentric value to the box itself squares the panel off at the defaults
//!   (`max(0, 8 - 12) = 0`).
//!
//! # Accessibility and stable identity
//!
//! The complete title + card composition publishes one labelled
//! [`Role::Group`]. Its accessible
//! name is [`EnzGroupBox::aria_label`] when set, otherwise the visible title,
//! otherwise `"Group"` for a genuinely untitled group. The visible title is
//! decorative in the accessibility tree because the group node already speaks
//! it; publishing both would announce the same name twice.
//!
//! The group always carries an id. A caller-supplied [`EnzGroupBox::id`] wins;
//! otherwise the id is derived from the accessible name under
//! [`DEFAULT_ID_PREFIX`]. That key is stable across view rebuilds and reorder —
//! unlike a position or `Id::unique()` — so AT focus does not jump each frame.
//! Two live groups with the same accessible name share the same derived id;
//! give either one an explicit `.id(...)`. The frame-level duplicate-id guard
//! makes such a collision a test failure rather than silently misrouting AT.

use crate::card::{CardTone, enz_card};
use crate::decorative::decorative;
use crate::enz_text::{TextSize, enz_text};
use crate::icon_slot::enz_icon_slot;
use crate::modal_surface::concentric_radius;
use crate::semantics_container::SemanticsContainer;
use enzui_core::identity::derive_id;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::widget::Id;
use iced::widget::{Row, column};
use iced::{Color, Element, Length};

/// Default title text size (px). `body-size` = 13.
pub const DEFAULT_TITLE_SIZE: f32 = 13.0;
/// Default gap between title row and card (px). `spacing-sm` = 8.
pub const DEFAULT_GAP: f32 = 8.0;
/// Default inner card padding (px). `spacing-md` = 12.
pub const DEFAULT_PADDING: f32 = 12.0;
/// Default box corner radius (px). `radius-md` = 8.
pub const DEFAULT_RADIUS: f32 = 8.0;
/// Default icon slot size (px).
pub const DEFAULT_ICON_SIZE: f32 = 16.0;
/// Gap between the leading icon and the title text (px).
pub const DEFAULT_TITLE_ICON_GAP: f32 = 6.0;
/// Default namespace for the group node's derived stable id.
///
/// The accessible name is the logical key. Two co-located groups with the
/// same name must give one an explicit [`EnzGroupBox::id`] so the route-level
/// duplicate-id guard stays clean.
pub const DEFAULT_ID_PREFIX: &str = "enz-group-box";

fn sane(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        fallback
    }
}

/// Builder for an enzUI-styled group box.
///
/// Use [`enz_group_box`] to construct one, then optionally chain
/// `.title_icon(_)`, `.title_size(_)`, `.gap(_)`, `.padding(_)`, `.radius(_)`,
/// `.title_color(_)`, `.icon_color(_)`, `.aria_label(_)`, or `.id(_)`.
#[must_use]
pub struct EnzGroupBox<'a, Message> {
    title: String,
    content: Element<'a, Message>,
    /// Optional leading icon name (Lucide/EnzIcon).
    title_icon: Option<String>,
    /// Optional explicit icon tint (else runtime theme muted).
    icon_color: Option<Color>,
    /// Optional explicit title colour (else runtime theme default text).
    title_color: Option<Color>,
    /// Title text size (px). Default [`DEFAULT_TITLE_SIZE`].
    title_size: f32,
    /// Vertical gap between title and card (px). Default [`DEFAULT_GAP`].
    gap: f32,
    /// Inner padding on the surrounding card (px). Default [`DEFAULT_PADDING`].
    padding: f32,
    /// Box corner radius (px). Default [`DEFAULT_RADIUS`].
    radius: f32,
    /// Card tone for the content surface.
    tone: CardTone,
    /// Whether the Card tone's border ink is visible. Width is retained when
    /// false so the default cannot move content.
    bordered: bool,
    /// Icon slot size (px). Default [`DEFAULT_ICON_SIZE`].
    icon_size: f32,
    /// Narrow per-field pin for the box background. `None` = the sunken
    /// Card's tone-resolved fill (`surface_alt`) tracked from the runtime
    /// theme. Same shape as `status_bar.rs`'s `StatusBarStyle.background`;
    /// this is *not* a token snapshot, so unpinned fields keep their runtime
    /// resolution.
    box_background: Option<Color>,
    /// Narrow per-field pin for the box border. `None` = runtime
    /// `border`. Same shape as `StatusBarStyle.border_color`.
    box_border: Option<Color>,
    /// Accessible-name override for the semantic `Role::Group`. `None` uses
    /// the visible title, or `"Group"` when untitled.
    aria_label: Option<String>,
    /// Stable group-node id override. `None` derives from the accessible name.
    id: Option<Id>,
}

/// Creates a new enzUI-styled group box.
///
/// `title` may be empty — in which case the title row is skipped and the
/// widget renders as a sunken card only. The semantic group then uses the
/// generic name `"Group"`; prefer [`EnzGroupBox::aria_label`] for a useful
/// untitled-group name. This skips the visible title row while preserving
/// a named grouping region for assistive technology.
pub fn enz_group_box<'a, Message: 'a>(
    title: impl Into<String>,
    content: impl Into<Element<'a, Message>>,
) -> EnzGroupBox<'a, Message> {
    EnzGroupBox {
        title: title.into(),
        content: content.into(),
        title_icon: None,
        icon_color: None,
        title_color: None,
        title_size: DEFAULT_TITLE_SIZE,
        gap: DEFAULT_GAP,
        padding: DEFAULT_PADDING,
        radius: DEFAULT_RADIUS,
        tone: CardTone::Sunken,
        bordered: false,
        icon_size: DEFAULT_ICON_SIZE,
        box_background: None,
        box_border: None,
        aria_label: None,
        id: None,
    }
}

// Passive setters do not touch the `Message` type at all, so they must not be
// gated on `Message: Clone`. A single `impl<Message: Clone>` block would forbid
// a non-Clone message type from *ever* constructing a group box. The `Clone`
// bound is only genuinely required by `into_element`/`From`, which invoke
// `EnzCard::into_element` — that composes an optional Clone-bounded header
// action. See
// `group_box_non_clone_message` in the tests file for the pin.
impl<'a, Message: 'a> EnzGroupBox<'a, Message> {
    /// Set a leading title icon. Rendered as an
    /// [`EnzIconSlot`](crate::icon_slot::EnzIconSlot) before the title text.
    pub fn title_icon(mut self, name: impl Into<String>) -> Self {
        self.title_icon = Some(name.into());
        self
    }

    /// Pin the icon tint. Defaults to the runtime theme's
    /// muted colour when unset.
    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }

    /// Pin the title text colour. Defaults to the runtime theme's default text
    /// colour when unset.
    pub fn title_color(mut self, color: Color) -> Self {
        self.title_color = Some(color);
        self
    }

    /// Set the title text size. Non-finite/negative ignored.
    pub fn title_size(mut self, size: f32) -> Self {
        self.title_size = sane(size, self.title_size);
        self
    }

    /// Set the vertical gap between title and card. Non-finite/negative ignored.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = sane(gap, self.gap);
        self
    }

    /// Set the inner card padding. Non-finite/negative ignored.
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = sane(padding, self.padding);
        self
    }

    /// Set the box corner radius. Non-finite/negative ignored. The content's
    /// concentric radius is derived from this and the padding.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = sane(radius, self.radius);
        self
    }

    /// Set the content surface tone. Defaults to [`CardTone::Sunken`].
    pub fn tone(mut self, tone: CardTone) -> Self {
        self.tone = tone;
        self
    }

    /// Show the tone's border. Off by default.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Set the leading icon slot size. Non-finite/negative ignored.
    pub fn icon_size(mut self, size: f32) -> Self {
        self.icon_size = sane(size, self.icon_size);
        self
    }

    /// Pin the box background for this instance only, overriding the sunken
    /// Card's runtime-resolved
    /// `surface_alt`. Narrow: border and title ink still track the live theme
    /// unless separately pinned. Modelled on [`crate::status_bar`]'s
    /// `StatusBarStyle`.
    pub fn box_background(mut self, color: Color) -> Self {
        self.box_background = Some(color);
        self
    }

    /// Pin the box border for this instance only, overriding the runtime
    /// `border`. Fill and title ink still track the
    /// live theme unless separately pinned.
    pub fn box_border(mut self, color: Color) -> Self {
        self.box_border = Some(color);
        self
    }

    /// Override the semantic group's accessible name.
    ///
    /// Unset or whitespace-only values fall back to the visible title; when
    /// that is empty too, the group is named `"Group"`. Set this for an
    /// untitled/icon-only group so its purpose is meaningful to AT rather than
    /// merely generic.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Override the semantic group's stable widget id.
    ///
    /// By default it is [`derive_id`]`(DEFAULT_ID_PREFIX, accessible_name)`,
    /// which is stable across rebuild and reorder because the name is a
    /// logical key, never a position. Use this override for two live groups
    /// that intentionally share one accessible name.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// The accessible name used by the published `Role::Group`.
    pub fn group_label(&self) -> String {
        self.aria_label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
            .or_else(|| (!self.title.trim().is_empty()).then_some(self.title.as_str()))
            .map_or_else(
                || enzui_core::localization::text("enzui-group-box-group"),
                str::to_owned,
            )
    }

    /// The stable id used by the published `Role::Group`.
    pub fn group_id(&self) -> Id {
        self.id.clone().unwrap_or_else(|| {
            derive_id(
                DEFAULT_ID_PREFIX,
                self.aria_label
                    .as_deref()
                    .filter(|label| !label.trim().is_empty())
                    .or_else(|| (!self.title.trim().is_empty()).then_some(self.title.as_str()))
                    .unwrap_or("Group"),
            )
        })
    }

    /// The box background this group resolves to under `palette`, following
    /// the pin-over-runtime precedence. Returns the exact colour the Card will
    /// paint under a theme carrying that palette.
    pub fn resolved_box_background(&self, palette: &enzui_theme::Palette) -> Color {
        self.box_background.unwrap_or(palette.surface_alt)
    }

    /// The box border this group resolves to under `palette` (pin, else a
    /// shade of the box's own background).
    pub fn resolved_box_border(&self, palette: &enzui_theme::Palette) -> Color {
        self.box_border.unwrap_or_else(|| {
            crate::surface_edge::edge_color(
                self.resolved_box_background(palette),
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            )
        })
    }

    /// The concentric radius an *inset child* of this box should use:
    /// `max(0, box-radius - inner-padding)` (shared
    /// [`concentric_radius`] helper).
    ///
    /// This is not the box's own outer radius — [`Self::outer_radius`] is.
    /// A concentric radius describes the corner an element nested *inside* the
    /// padding must adopt so its arc stays parallel to the enclosing box's
    /// arc. Handing this value to the surrounding surface squares the panel
    /// off (at the defaults `max(0, 8 - 12)` = 0).
    ///
    /// Kept public so a caller composing their own inset content can use the
    /// same rule as the group box itself.
    pub fn content_radius(&self) -> f32 {
        concentric_radius(self.radius, self.padding)
    }

    /// The corner radius actually handed to the surrounding [`enz_card`] — the
    /// outer panel radius.
    ///
    /// Exposed so a test can assert the value that reaches the Card rather
    /// than re-deriving it: a correct `content_radius()` wired to the wrong
    /// destination passes every arithmetic-only test.
    pub fn outer_radius(&self) -> f32 {
        self.radius
    }
}

// The `Message: Clone` bound is genuine on `into_element`: it invokes
// `EnzCard::into_element`, whose signature requires `Message: Clone` for the
// optional header-action button. It is *not* required by the passive setters
// above.
impl<'a, Message: Clone + 'a> EnzGroupBox<'a, Message> {
    /// Converts this builder into an iced `Element`.
    pub fn into_element(self) -> Element<'a, Message> {
        // Resolve semantic identity before moving any visual fields. The
        // accessible name is a stable logical key (title/aria label), never a
        // position or per-build unique counter, so the derived id survives
        // rebuild and reorder. An explicit id wins for duplicate names.
        let group_label = self.group_label();
        let group_id = self.group_id();

        // The default keeps the sunken tone for grouping but suppresses its
        // border ink; callers can opt into a frame explicitly.
        // The Card's `radius` setter controls the outer border radius, so
        // it receives `self.radius` — the box's own corner. Passing
        // `content_radius()` here is a category error: that value is for an
        // *inset child*, and at the defaults `max(0, 8 - 12)` clamps to 0,
        // squaring off the whole panel.
        let outer_radius = self.outer_radius();
        let mut card = enz_card(self.content)
            .tone(self.tone)
            .padding(self.padding)
            .radius(outer_radius);
        if !self.bordered {
            card = card.border_color(Color::TRANSPARENT);
        }
        // Forward the narrow pins through Card's own per-field pins.
        // Each `None` here keeps Card's runtime resolver for that field, so
        // this is not a token snapshot — the fields we do not pin still
        // re-tint on a dark/light/custom theme switch.
        if let Some(c) = self.box_background {
            card = card.surface_color(c);
        }
        if let Some(c) = self.box_border {
            card = card.border_color(c);
        } else {
            // A shade of the box's painted fill, pinned or runtime.
            card = card.edge(crate::surface_edge::DEFAULT_EDGE_DEPTH);
        }
        let card: Element<'a, Message> = card.into();

        // Title/icon gating. Treat an *empty-string* icon name as
        // "no icon" (an empty slot would otherwise reserve `icon_size` px of
        // width for a glyph that never paints), and gate the whole title row
        // on there being *something* to paint. The three cases are:
        //
        //   * no title, no icon        → bare card (no empty row taking
        //                                 vertical space).
        //   * icon, no title           → icon-only row. Legal and deliberate:
        //                                 useful for status/chevron prefixes
        //                                 above a group's body.
        //   * title, icon or not      → titled row, icon rendered if present.
        let icon_name: Option<&str> = self.title_icon.as_deref().filter(|s| !s.is_empty());
        let visual: Element<'a, Message> = if self.title.is_empty() && icon_name.is_none() {
            card
        } else {
            // Build a title element only when there is text to render — an
            // empty title with a real icon must not leave a phantom `enz_text`
            // node consuming a row slot.
            let title_el: Option<Element<'a, Message>> = if self.title.is_empty() {
                None
            } else {
                // Stay on EnzText for every path. Default is
                // `Headline` (13 px Semibold, runtime-themed colour). A non-default
                // size uses `.size_px()`; a pinned colour uses `.color()` (which
                // wins over tone/theme) — both are expressible on EnzText, so
                // there is no need to drop to raw `text()` and lose EnzText's line
                // height / overflow semantics.
                let default_size = (self.title_size - DEFAULT_TITLE_SIZE).abs() < f32::EPSILON;
                let mut t = if default_size {
                    enz_text(self.title).size(TextSize::Headline)
                } else {
                    enz_text(self.title)
                        .size(TextSize::Headline)
                        .size_px(self.title_size)
                };
                if let Some(color) = self.title_color {
                    t = t.color(color);
                }
                Some(t.into())
            };

            let title_row: Element<'a, Message> = match (icon_name, title_el) {
                (Some(icon), Some(title)) => {
                    let mut slot = enz_icon_slot(icon.to_owned()).icon_size(self.icon_size);
                    if let Some(color) = self.icon_color {
                        slot = slot.icon_color(color);
                    }
                    Row::with_children(vec![slot.into(), title])
                        .spacing(DEFAULT_TITLE_ICON_GAP)
                        .align_y(iced::Alignment::Center)
                        .width(Length::Shrink)
                        .into()
                }
                (Some(icon), None) => {
                    // Icon-only row: no text, no gap — just the icon slot.
                    let mut slot = enz_icon_slot(icon.to_owned()).icon_size(self.icon_size);
                    if let Some(color) = self.icon_color {
                        slot = slot.icon_color(color);
                    }
                    slot.into()
                }
                (None, Some(title)) => title,
                (None, None) => unreachable!("guarded by the visual branch above"),
            };

            // The labelled Group below already speaks the visible title. Keep
            // the drawn title row decorative so `Operation::text` does not
            // add a duplicate `Role::Label` child. Body content remains
            // outside this suppression and is still collected under the group.
            column![decorative(title_row), card]
                .spacing(self.gap)
                .into()
        };

        SemanticsContainer::new(visual, Semantics::new(Role::Group).label(group_label))
            .id(group_id)
            .into()
    }
}

// NOTE on the `Clone` bound: EnzGroupBox itself emits no messages, but it
// composes `EnzCard`, whose `into_element` requires `Message: Clone` for its
// optional header-action button. The bound is therefore *inherited* from
// Card, not a GroupBox requirement — it cannot be dropped here without
// restructuring Card's header-action feature.
impl<'a, Message: Clone + 'a> From<EnzGroupBox<'a, Message>> for Element<'a, Message> {
    fn from(g: EnzGroupBox<'a, Message>) -> Self {
        g.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
