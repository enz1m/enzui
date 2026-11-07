//! Centred empty and error states with optional artwork and action buttons.
//!
//! Page content centres inside a minimum-height region that grows with content;
//! centring within a taller parent requires a caller container. Inline content
//! has no height floor. Artwork replaces the icon; `has_image` independently
//! selects the larger Page floor. Secondary actions precede the primary action,
//! which defaults to Suggested emphasis. Status text follows the actions and wraps.
//!
//! Colours follow the live palette; geometry resolves at construction. Titles
//! publish Page or Section headings by default. Repeated action ids need distinct
//! [`EnzStatusPage::id_prefix`] values when pages share a tree.

use crate::action_emphasis::ActionEmphasis;
use crate::button::{ButtonKind, ButtonSize, enz_button};
use crate::enz_text::{TextAlign, TextTone, enz_text};
use crate::min_height::min_height;
use enzui_core::Action;
use enzui_core::semantics::{HeadingLevel, Role, Semantics};
use enzui_theme::Palette;
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length, Padding};

/// Icon tint policy; title and body tones are unaffected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusPageTone {
    /// Use muted icon ink on Page and primary text ink on Inline.
    #[default]
    Empty,
    /// Use error ink for the icon.
    Error,
}

/// Choose a height-floored Page hero or content-height Inline state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusPageVariant {
    /// Use a large icon and a minimum-height region; this is the default.
    #[default]
    Page,
    /// Use a smaller icon without a height floor.
    Inline,
}

/// Geometry overrides in logical pixels; colours always follow the live palette.
/// Only description_size uses ambient UI text scaling in the default geometry.
#[derive(Debug, Clone, Copy)]
pub struct StatusPageTokens {
    /// Page icon size; defaults to 64.
    pub icon_size_page: f32,
    /// Compact Page icon size; defaults to 40.
    pub icon_size_page_compact: f32,
    /// Inline icon size in either compact mode; defaults to 32.
    pub icon_size_inline: f32,
    /// Title size for both variants; defaults to 17.
    pub title_size_page: f32,
    /// Compact title size for both variants; defaults to 13.
    pub title_size_page_compact: f32,
    /// Body size; defaults to ambient-scaled 13 px.
    pub description_size: f32,
    /// Status-line size; defaults to 12.
    pub status_size: f32,
    /// Minimum action button width; defaults to 132.
    pub action_min_width: f32,
    /// Non-compact stack gap; defaults to 8.
    pub outer_spacing: f32,
    /// Compact stack gap; defaults to 4.
    pub outer_spacing_compact: f32,
    /// Gap between action buttons; defaults to 8.
    pub action_gap: f32,
    /// Page height floor without image intent; defaults to 180.
    pub min_height_page: f32,
    /// Compact Page height floor without image intent; defaults to 120.
    pub min_height_page_compact: f32,
    /// Page height floor with image intent; defaults to 240.
    pub min_height_has_image: f32,
    /// Compact Page height floor with image intent; defaults to 160.
    pub min_height_has_image_compact: f32,
}

impl Default for StatusPageTokens {
    fn default() -> Self {
        Self {
            icon_size_page: 64.0,
            icon_size_page_compact: 40.0,
            icon_size_inline: 32.0,
            // Titles follow the desktop text size like the description does,
            // and the compact title stays a step above its description: a
            // fixed 13 px title under a scaled 13 px description came out
            // smaller than the text it heads.
            title_size_page: enzui_theme::text_scale::ui_px(17.0),
            title_size_page_compact: enzui_theme::text_scale::ui_px(15.0),
            description_size: enzui_theme::text_scale::ui_px(13.0),
            status_size: 12.0,
            action_min_width: 132.0,
            outer_spacing: 8.0,
            outer_spacing_compact: 4.0,
            action_gap: 8.0,
            min_height_page: 180.0,
            min_height_page_compact: 120.0,
            min_height_has_image: 240.0,
            min_height_has_image_compact: 160.0,
        }
    }
}

impl StatusPageTokens {
    /// Return default geometry, ignoring the palette.
    pub fn from_palette(_p: &Palette) -> Self {
        Self::default()
    }
    /// Return default geometry, including ambient-scaled description size.
    pub fn dark() -> Self {
        Self::default()
    }
    /// Return default geometry, including ambient-scaled description size.
    pub fn light() -> Self {
        Self::default()
    }
    /// Replace negative or non-finite geometry with each field's default.
    /// Zero is preserved by normalization; downstream text and button sizing may
    /// apply their own minimums or fallbacks.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::default();
        let f = |v: f32, fb: f32| FiniteNonNeg::new_or(v, fb).get();
        Self {
            icon_size_page: f(self.icon_size_page, d.icon_size_page),
            icon_size_page_compact: f(self.icon_size_page_compact, d.icon_size_page_compact),
            icon_size_inline: f(self.icon_size_inline, d.icon_size_inline),
            title_size_page: f(self.title_size_page, d.title_size_page),
            title_size_page_compact: f(self.title_size_page_compact, d.title_size_page_compact),
            description_size: f(self.description_size, d.description_size),
            status_size: f(self.status_size, d.status_size),
            action_min_width: f(self.action_min_width, d.action_min_width),
            outer_spacing: f(self.outer_spacing, d.outer_spacing),
            outer_spacing_compact: f(self.outer_spacing_compact, d.outer_spacing_compact),
            action_gap: f(self.action_gap, d.action_gap),
            min_height_page: f(self.min_height_page, d.min_height_page),
            min_height_page_compact: f(self.min_height_page_compact, d.min_height_page_compact),
            min_height_has_image: f(self.min_height_has_image, d.min_height_has_image),
            min_height_has_image_compact: f(
                self.min_height_has_image_compact,
                d.min_height_has_image_compact,
            ),
        }
    }
}

/// Builder for a centred hero with optional body text, artwork and actions.
#[must_use]
pub struct EnzStatusPage<'a, Message> {
    title: String,
    description: String,
    status_text: String,
    icon: String,
    tone: StatusPageTone,
    variant: StatusPageVariant,
    compact: bool,
    primary: Option<Action<Message>>,
    secondary: Option<Action<Message>>,
    primary_emphasis: ActionEmphasis,
    artwork: Option<Element<'a, Message>>,
    has_image: bool,
    tokens: Option<StatusPageTokens>,
    announce_heading: bool,
    id_prefix: Option<String>,
}

/// Build a Page state with an announced title, no icon or actions, and default geometry.
pub fn enz_status_page<'a, Message: Clone + 'a>(
    title: impl Into<String>,
) -> EnzStatusPage<'a, Message> {
    EnzStatusPage {
        title: title.into(),
        description: String::new(),
        status_text: String::new(),
        icon: String::new(),
        tone: StatusPageTone::default(),
        variant: StatusPageVariant::default(),
        compact: false,
        primary: None,
        secondary: None,
        primary_emphasis: ActionEmphasis::Suggested,
        artwork: None,
        has_image: false,
        tokens: None,
        announce_heading: true,
        id_prefix: None,
    }
}

impl<'a, Message: Clone + 'a> EnzStatusPage<'a, Message> {
    /// Set optional wrapping body text below the title; empty text removes it.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }
    /// Set optional muted wrapping text below the actions; empty text removes it.
    pub fn status_text(mut self, s: impl Into<String>) -> Self {
        self.status_text = s.into();
        self
    }
    /// Set the hero icon name; artwork takes precedence, and empty text removes the icon.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }
    /// Choose the icon tint; defaults to Empty.
    pub fn tone(mut self, t: StatusPageTone) -> Self {
        self.tone = t;
        self
    }
    /// Choose Page or Inline layout; defaults to Page.
    pub fn variant(mut self, v: StatusPageVariant) -> Self {
        self.variant = v;
        self
    }
    /// Use smaller title, Page icon, gaps and Page floor; defaults to false.
    /// Inline icon size and description size are unaffected.
    pub fn compact(mut self, c: bool) -> Self {
        self.compact = c;
        self
    }
    /// Set the primary action after the secondary action in traversal order.
    pub fn primary_action(mut self, action: Action<Message>) -> Self {
        self.primary = Some(action);
        self
    }
    /// Set the secondary action before the primary action in traversal order.
    pub fn secondary_action(mut self, action: Action<Message>) -> Self {
        self.secondary = Some(action);
        self
    }
    /// Choose primary button emphasis; defaults to Suggested (Accent).
    /// Neutral uses Secondary treatment, as the secondary button always does.
    pub fn primary_action_emphasis(mut self, emphasis: ActionEmphasis) -> Self {
        self.primary_emphasis = emphasis;
        self
    }
    /// Namespace action widget ids with the shared action-id derivation.
    /// Without a prefix, canonical action ids are forwarded unchanged. Use distinct
    /// prefixes for pages containing the same command; repeated ids within one page
    /// also require distinct action ids.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }
    /// Replace the icon with caller-sized artwork above the title.
    /// This does not change has_image or select a larger height floor.
    pub fn artwork(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.artwork = Some(element.into());
        self
    }
    /// Select the larger Page height floor, independently of artwork.
    /// Defaults to false; ignored by Inline. No illustration is constructed here.
    pub fn has_image(mut self, has: bool) -> Self {
        self.has_image = has;
        self
    }
    /// Pin normalized geometry while retaining live palette colours.
    /// Negative and non-finite fields fall back to defaults; zero survives normalization.
    pub fn tokens(mut self, t: StatusPageTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }
    /// Publish the title as a heading; defaults to true.
    /// False removes the heading annotation while retaining the rendered text and
    /// its ordinary label semantics.
    pub fn announce_heading(mut self, announce: bool) -> Self {
        self.announce_heading = announce;
        self
    }
    /// Return Page for the Page variant or Section for Inline, unless heading announcement is disabled.
    pub fn heading_level(&self) -> Option<HeadingLevel> {
        if !self.announce_heading {
            return None;
        }
        Some(match self.variant {
            StatusPageVariant::Page => HeadingLevel::Page,
            StatusPageVariant::Inline => HeadingLevel::Section,
        })
    }
    /// Return the title's heading-label description, or None when heading announcement is disabled.
    /// The description uses Role::Label with the heading-level annotation.
    pub fn heading_semantics(&self) -> Option<Semantics> {
        let level = self.heading_level()?;
        Some(
            Semantics::new(Role::Label)
                .label(self.title.clone())
                .heading_label(level),
        )
    }
    /// Build the hero with 16 px outer padding and wrapping title, description and status text.
    pub fn into_element(self) -> Element<'a, Message> {
        let t = self.tokens.unwrap_or_default();
        let inline = matches!(self.variant, StatusPageVariant::Inline);
        let title_heading = self.heading_level();
        let compact = self.compact;
        let tone = self.tone;
        let has_image = self.has_image;
        let hero_slot: Option<Element<'a, Message>> = if let Some(art) = self.artwork {
            Some(
                container(art)
                    .align_x(Alignment::Center)
                    .width(Length::Fill)
                    .into(),
            )
        } else if self.icon.is_empty() {
            None
        } else {
            let icon_size = if inline {
                t.icon_size_inline
            } else if compact {
                t.icon_size_page_compact
            } else {
                t.icon_size_page
            };
            let icon = crate::icon::enz_icon(&self.icon)
                .size(icon_size)
                .tint_resolved(move |theme: &iced::Theme| {
                    let p = crate::theme_lookup::palette(theme);
                    match (inline, tone) {
                        (_, StatusPageTone::Error) => p.error,
                        (true, StatusPageTone::Empty) => p.text,
                        (false, StatusPageTone::Empty) => p.text_muted,
                    }
                });

            let slot: Element<'a, Message> = icon.into();
            Some(
                container(slot)
                    .align_x(Alignment::Center)
                    .width(Length::Fill)
                    .into(),
            )
        };
        let title_size = if compact {
            t.title_size_page_compact
        } else {
            t.title_size_page
        };
        let title_weight = if inline {
            enzui_theme::WeightRung::Semibold
        } else {
            enzui_theme::WeightRung::ExtraBold
        };
        let mut title_widget = enz_text(self.title)
            .size_px(title_size)
            .weight_rung(title_weight)
            .tone(TextTone::Default)
            .align(TextAlign::Center)
            .wrap(true)
            .full_width();
        if let Some(level) = title_heading {
            title_widget = title_widget.heading(level);
        }
        let description_widget: Option<Element<'a, Message>> = if self.description.is_empty() {
            None
        } else {
            Some(
                enz_text(self.description)
                    .size_px(t.description_size)
                    .tone(TextTone::Muted)
                    .align(TextAlign::Center)
                    .wrap(true)
                    .full_width()
                    .into(),
            )
        };
        let action_min_w = t.action_min_width;
        let id_prefix = self.id_prefix;
        let primary_kind = self.primary_emphasis.button_kind();
        let mut action_row: Vec<Element<'a, Message>> = Vec::new();
        if let Some(s) = self.secondary {
            action_row.push(action_button(
                s,
                ButtonKind::Secondary,
                action_min_w,
                id_prefix.as_deref(),
            ));
        }
        if let Some(p) = self.primary {
            action_row.push(action_button(
                p,
                primary_kind,
                action_min_w,
                id_prefix.as_deref(),
            ));
        }
        let mut children: Vec<Element<'a, Message>> = Vec::new();
        if let Some(h) = hero_slot {
            children.push(h);
        }
        children.push(
            container(title_widget)
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .into(),
        );
        if let Some(d) = description_widget {
            children.push(
                container(d)
                    .align_x(Alignment::Center)
                    .width(Length::Fill)
                    .into(),
            );
        }
        if !action_row.is_empty() {
            let r = row(action_row)
                .spacing(t.action_gap)
                .align_y(Alignment::Center);
            children.push(
                container(r)
                    .align_x(Alignment::Center)
                    .width(Length::Fill)
                    .into(),
            );
        }
        if !self.status_text.is_empty() {
            children.push(
                container(
                    enz_text(self.status_text)
                        .size_px(t.status_size)
                        .tone(TextTone::Muted)
                        .align(TextAlign::Center)
                        .wrap(true)
                        .full_width(),
                )
                .align_x(Alignment::Center)
                .width(Length::Fill)
                .into(),
            );
        }

        let inner = column(children).spacing(if compact {
            t.outer_spacing_compact
        } else {
            t.outer_spacing
        });

        if inline {
            container(inner)
                .width(Length::Fill)
                .padding(Padding::new(16.0))
                .align_x(Alignment::Center)
                .into()
        } else {
            let padded = container(inner)
                .width(Length::Fill)
                .padding(Padding::new(16.0))
                .align_x(Alignment::Center);

            let floor = match (compact, has_image) {
                (true, true) => t.min_height_has_image_compact,
                (true, false) => t.min_height_page_compact,
                (false, true) => t.min_height_has_image,
                (false, false) => t.min_height_page,
            };
            // Centre inside the floor so short content is not pinned to its top.
            min_height(padded, floor).align_y(Alignment::Center).into()
        }
    }
}
fn action_button<'a, Message: Clone + 'a>(
    action: Action<Message>,
    kind: ButtonKind,
    min_w: f32,
    id_prefix: Option<&str>,
) -> Element<'a, Message> {
    let label = action.label().to_owned();
    // Preserve unknown/custom icon identity instead of flattening it to the fallback name.
    let icon = action.icon().map(|i| i.requested_name().to_owned());
    let enabled = action.is_enabled();
    let id = crate::action_id::derive_action_id(id_prefix, action.id());
    let message = action.into_message();

    let mut b = enz_button(label, message)
        .kind(kind)
        .size(ButtonSize::Md)
        .min_width(min_w)
        .id(id)
        .enabled(enabled);
    if let Some(name) = icon {
        b = b.icon(name);
    }
    b.into_element()
}

impl<'a, Message: Clone + 'a> From<EnzStatusPage<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzStatusPage<'a, Message>) -> Self {
        w.into_element()
    }
}
