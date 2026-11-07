//! Settings rows in a flat section or bordered card with a labelled accessibility group.
//!
//! Flat sections have transparent fill, no border or shadow, no horizontal
//! padding, and `Xs` vertical padding. Bordered cards use 10 px corners,
//! `Md` padding on both axes and elevation 1. Both use `Sm` between the header
//! and body. Spacing defaults to the stock ladder; forward a theme's ladder
//! with [`EnzPreferencesGroup::spacing_scale`].
//!
//! Titles use bold Body text and descriptions use Caption text, following the
//! desktop text scale unless a typography ramp is forwarded. `compact()` selects
//! Body/Semibold; `title_size` and `title_weight` override individual rungs. The drawn header
//! is decorative: its title and description belong to the enclosing `Group`
//! node, without a separate heading node. Body controls remain group children.
//!
//! Header actions require a title or description to appear. Supply a unique,
//! stable [`EnzPreferencesGroup::owner`] for assistive-technology activation;
//! an explicit group ID alone does not identify the action.
//!
//! Related: [`crate::preferences_row`], [`crate::card`], [`crate::settings_dialog`].

use crate::card::{CardRhythm, CardTone, enz_card};
use crate::enz_text::TextSize;
use crate::semantics_container::SemanticsContainer;
use enzui_core::identity::{OwnerId, derive_id};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{SpaceToken, SpacingScale, TypographyScale};
use iced::advanced::widget::Id;
use iced::{Element, Length};

/// Namespace for group IDs derived from their accessible names.
/// Same-named groups need distinct explicit IDs or owners; retitling changes
/// a derived ID unless the accessible name is pinned.
pub const DEFAULT_ID_PREFIX: &str = "enz-preferences-group";
const GROUP_ROLE: &str = "group";

/// Bordered corner radius in logical pixels: 10.
/// This settings-card exception is independent of the theme's 8 px stock `lg` radius.
pub const BORDERED_RADIUS: f32 = 10.0;

/// Stock `Md` padding on both axes in logical pixels; does not read forwarded scales.
pub fn bordered_padding() -> f32 {
    SpaceToken::Md.px()
}
/// Stock `Xs` vertical padding in logical pixels; does not read forwarded scales.
pub fn flat_padding_y() -> f32 {
    SpaceToken::Xs.px()
}
/// Stock `Sm` header-to-body gap in logical pixels; does not read forwarded scales.
pub fn section_gap() -> f32 {
    SpaceToken::Sm.px()
}
/// Ambient Body font size in logical pixels, including the desktop text scale.
pub fn title_size() -> f32 {
    TypographyScale::ambient().body_size
}
/// Ambient Caption font size in logical pixels, including the desktop text scale.
pub fn description_size() -> f32 {
    TypographyScale::ambient().label_size
}

/// Builder for a settings section whose header names the enclosing accessibility group.
#[must_use]
pub struct EnzPreferencesGroup<'a, Message> {
    content: Element<'a, Message>,
    title: Option<String>,
    description: Option<String>,
    bordered: bool,
    header_action: Option<(String, Option<Message>)>,
    width: Length,
    aria_label: Option<String>,
    id: Option<Id>,
    owner: Option<OwnerId>,
    spacing_scale: Option<SpacingScale>,
    typography_scale: Option<TypographyScale>,
    title_size: TextSize,
    title_weight: enzui_theme::WeightRung,
}

/// Wrap settings content in a flat section by default.
/// Use [`EnzPreferencesGroup::bordered`] for a standalone card.
pub fn enz_preferences_group<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> EnzPreferencesGroup<'a, Message> {
    EnzPreferencesGroup {
        content: content.into(),
        title: None,
        description: None,
        bordered: false,
        header_action: None,
        width: Length::Fill,
        aria_label: None,
        id: None,
        owner: None,
        spacing_scale: None,
        typography_scale: None,
        title_size: TextSize::Body,
        title_weight: enzui_theme::WeightRung::Bold,
    }
}

impl<'a, Message: 'a> EnzPreferencesGroup<'a, Message> {
    /// Set the visible title and default accessible group name; an empty string clears it.
    /// Whitespace-only titles are retained.
    pub fn title(mut self, t: &str) -> Self {
        self.title = if t.is_empty() {
            None
        } else {
            Some(t.to_owned())
        };
        self
    }

    /// Use Body/Semibold headings for compact settings; body content is unchanged.
    /// Later title size and weight setters override these choices.
    pub fn compact(mut self) -> Self {
        self.title_size = TextSize::Body;
        self.title_weight = enzui_theme::WeightRung::Semibold;
        self
    }

    /// Select the header title's semantic size rung; defaults to Body.
    /// A forwarded typography scale supplies the size for this rung.
    pub fn title_size(mut self, size: TextSize) -> Self {
        self.title_size = size;
        self
    }

    /// Select the header title's semantic weight; defaults to Bold.
    /// A forwarded typography scale supplies the weight for this rung.
    pub fn title_weight(mut self, weight: enzui_theme::WeightRung) -> Self {
        self.title_weight = weight;
        self
    }

    /// Set the visible and accessible group description; an empty string clears it.
    /// Whitespace-only descriptions are retained.
    pub fn description(mut self, d: &str) -> Self {
        self.description = if d.is_empty() {
            None
        } else {
            Some(d.to_owned())
        };
        self
    }

    /// Choose bordered chrome; defaults to a flat section.
    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Choose flat chrome; `.flat(true)` is equivalent to `.bordered(false)`.
    pub fn flat(mut self, flat: bool) -> Self {
        self.bordered = !flat;
        self
    }

    /// Set a trailing header action; an empty label removes it when built.
    /// The action appears only with a title or description.
    ///
    /// # Accessibility
    ///
    /// [`Self::owner`] gives the action a routable identity. Without an owner,
    /// the button is published without Click or Focus actions, even if the group
    /// has an explicit ID. Pointer activation does not require an owner.
    pub fn header_action(mut self, label: &str, on_press: Message) -> Self {
        self.header_action = Some((label.to_owned(), Some(on_press)));
        self
    }

    /// Set a trailing header action, disabled when its message is `None`.
    /// An empty label clears it; header visibility and owner requirements match
    /// [`Self::header_action`].
    pub fn header_action_maybe(mut self, label: &str, on_press: Option<Message>) -> Self {
        if label.is_empty() {
            self.header_action = None;
        } else {
            self.header_action = Some((label.to_owned(), on_press));
        }
        self
    }

    /// Set the section width; defaults to [`Length::Fill`].
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Override the accessible group name without changing the drawn title.
    /// Otherwise the title is used, falling back to `Settings group` when absent.
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// Pin the group node ID, taking precedence over an owner or name-derived ID.
    /// This identifies only the group; use [`Self::owner`] to identify its header action.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set a stable identity namespace unique among groups mounted in one window.
    /// The group uses `owner.id("group")` unless [`Self::id`] overrides it; the
    /// header action uses [`card_header_action_id`](crate::card::card_header_action_id).
    /// Keep the owner stable across rebuilds and retitling for routable actions.
    pub fn owner(mut self, owner: OwnerId) -> Self {
        self.owner = Some(owner);
        self
    }

    /// Resolve `Md`/`Xs` padding and `Sm` section and header-action gaps from this ladder.
    /// Without a forwarded ladder, spacing uses [`SpacingScale::default`].
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Resolve the selected title size and weight and the Caption description from this ramp.
    /// The title defaults to Body/Bold. Without a ramp, headers use application
    /// weight defaults and desktop text scaling; body content is unchanged.
    pub fn typography_scale(mut self, typo: TypographyScale) -> Self {
        self.typography_scale = Some(typo);
        self
    }

    /// Returns the aria-label override, title, or localized fallback group name.
    pub fn group_label(&self) -> String {
        self.aria_label
            .as_deref()
            .or(self.title.as_deref())
            .map_or_else(
                || enzui_core::localization::text("enzui-preferences-group-settings-group"),
                str::to_owned,
            )
    }

    /// Return the explicit ID, owner's `group` component, or name-derived ID, in that order.
    pub fn group_id(&self) -> Id {
        self.id
            .clone()
            .or_else(|| self.owner.as_ref().map(|owner| owner.id(GROUP_ROLE)))
            .unwrap_or_else(|| {
                derive_id(
                    DEFAULT_ID_PREFIX,
                    self.aria_label
                        .as_deref()
                        .or(self.title.as_deref())
                        .unwrap_or("Settings group"),
                )
            })
    }

    /// Build the card and its enclosing labelled accessibility group.
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: Clone,
    {
        let bordered = self.bordered;
        let group_label = self.group_label();
        let group_id = self.group_id();
        let owner = self.owner.clone();
        let group_description = self.description.clone();

        let sp_default = SpacingScale::default();
        let sp = self.spacing_scale.as_ref().unwrap_or(&sp_default);
        let rhythm = CardRhythm {
            radius: Some(if bordered { BORDERED_RADIUS } else { 0.0 }),
            padding_x: Some(if bordered {
                SpaceToken::Md.resolve(sp)
            } else {
                0.0
            }),
            padding_y: Some(if bordered {
                SpaceToken::Md.resolve(sp)
            } else {
                SpaceToken::Xs.resolve(sp)
            }),
            gap: Some(SpaceToken::Sm.resolve(sp)),
            title_size: Some(self.title_size.resolve_size(self.typography_scale.as_ref())),
            description_size: Some(TextSize::Caption.resolve_size(self.typography_scale.as_ref())),
            header_action_gap: Some(SpaceToken::Sm.resolve(sp)),
            // A flat group's rows inset their labels by the row padding (for
            // the hover fill); the title takes the same edge so heading and
            // labels line up. A bordered card pads both alike already.
            header_inset_x: (!bordered)
                .then(|| crate::preferences_row::PreferencesRowTokens::default().padding_x),
        };

        let tone = if bordered {
            CardTone::Default
        } else {
            CardTone::Flat
        };

        let mut card = enz_card(self.content)
            .tone(tone)
            .rhythm(rhythm)
            .title_weight(self.title_weight)
            .width_length(self.width)
            .transparent(!bordered)
            // Publish header text on the group once, while retaining body and action semantics.
            .header_decorative(true);
        if bordered {
            card = card.elevation_override(1, None);
        }
        if let Some(typography) = self.typography_scale {
            card = card.typography(typography);
        }

        if let Some(title) = self.title {
            card = card.title(&title);
        }
        if let Some(desc) = self.description {
            card = card.description(&desc);
        }
        if let Some(owner) = owner {
            card = card.owner(owner);
        }
        if let Some((label, on_press)) = self.header_action {
            card = card.header_action_maybe(&label, on_press);
        }
        let mut group_semantics = Semantics::new(Role::Group).label(group_label);
        if let Some(desc) = group_description {
            group_semantics = group_semantics.description(desc);
        }
        SemanticsContainer::new(card, group_semantics)
            .id(group_id)
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzPreferencesGroup<'a, Message>> for Element<'a, Message> {
    fn from(g: EnzPreferencesGroup<'a, Message>) -> Self {
        g.into_element()
    }
}
