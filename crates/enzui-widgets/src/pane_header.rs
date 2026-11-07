//! Left-aligned pane title with optional leading, inline subtitle and trailing slots.
//!
//! The title defaults to the ambient pane-title size and Semibold weight rung
//! captured when the builder is constructed (18 px and Semibold stock).
//! The row fills available width, uses a stock 8 px gap and centres items vertically.
//!
//! The title elides to one line. The inline subtitle is a muted caption; the
//! trailing slot accepts arbitrary content and a Fill spacer pushes it to the edge.
//! Surviving logical slots keep their widget state across optional-slot changes
//! when their widget types remain compatible.
//!
//! The title publishes a heading annotation at [`HeadingLevel::Section`] by
//! default. [`EnzPaneHeader::heading`] chooses another rung, and
//! [`EnzPaneHeader::announce`] can suppress the title's accessibility node.
//! The subtitle remains a plain label. Heading annotation exports as an AccessKit
//! heading while retaining the framework's Label role in collected semantics.
//! Related headers: [`crate::section_header`] and [`crate::page_header`].

use crate::enz_text::{TextSize, TextTone, enz_text};
use enzui_core::semantics::HeadingLevel;
use iced::font::Weight;
use iced::widget::Space;
use iced::{Alignment, Element, Length};

const SLOT_LEADING: &str = "pane-header:leading";
const SLOT_TITLE: &str = "pane-header:title";
const SLOT_SUBTITLE: &str = "pane-header:subtitle";
const SLOT_SPACER: &str = "pane-header:spacer";
const SLOT_TRAILING: &str = "pane-header:trailing";

/// Read the ambient pane-title size in logical pixels (18 px stock).
/// The builder captures this value at construction; rebuild it after scale changes.
pub fn default_title_size() -> f32 {
    enzui_theme::TypographyScale::ambient().title_size
}

/// Stock pane-title size in logical pixels. The ambient [`default_title_size`] may differ.
pub const DEFAULT_TITLE_SIZE: f32 = 18.0;

/// Default title outline rung: [`HeadingLevel::Section`] (level two).
pub const DEFAULT_HEADING_LEVEL: HeadingLevel = HeadingLevel::Section;

fn sane(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        fallback
    }
}

/// Builder for a pane-title header. Construct with [`enz_pane_header`].
#[must_use]
pub struct EnzPaneHeader<'a, Message> {
    title: String,
    title_size: f32,
    weight: Weight,
    heading: HeadingLevel,
    announce: bool,
    subtitle: Option<String>,
    leading: Option<Element<'a, Message>>,
    trailing: Option<Element<'a, Message>>,
    spacing: f32,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
}

/// Construct a pane header with the given title.
pub fn enz_pane_header<'a, Message>(title: impl Into<String>) -> EnzPaneHeader<'a, Message> {
    EnzPaneHeader {
        title: title.into(),
        title_size: default_title_size(),
        weight: enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Semibold),
        heading: DEFAULT_HEADING_LEVEL,
        announce: true,
        subtitle: None,
        leading: None,
        trailing: None,
        spacing: enzui_theme::SpacingScale::default().sm(),
        direction: None,
    }
}

impl<'a, Message: 'a> EnzPaneHeader<'a, Message> {
    /// Override the title size (px). Non-finite / non-positive values are
    /// ignored, keeping the current size.
    pub fn title_size(mut self, size: f32) -> Self {
        self.title_size = sane(size, self.title_size);
        self
    }

    /// Override the title weight. The constructor captures the ambient Semibold rung
    /// (stock [`Weight::Semibold`]).
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = weight;
        self
    }

    /// Choose the title's outline rung; defaults to [`HeadingLevel::Section`].
    /// The collected Label carries a heading annotation that exports as an AccessKit heading.
    pub fn heading(mut self, level: HeadingLevel) -> Self {
        self.heading = level;
        self
    }

    /// Publish the title's heading-label semantics; defaults to true.
    /// False suppresses the title's entire accessibility node, including its label,
    /// while retaining rendered text. Subtitle and slot semantics are unaffected.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }

    /// Set an inline subtitle/count caption after the title (e.g.
    /// `"490 processes"`, `"12 apps"`). Rendered muted, and is not a
    /// heading.
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Place arbitrary leading content before the title (a sidebar toggle, an
    /// icon). Any `Element`, not a typed action.
    pub fn leading(mut self, element: Element<'a, Message>) -> Self {
        self.leading = Some(element);
        self
    }

    /// Place arbitrary content at the trailing edge, after the Fill spacer.
    pub fn trailing(mut self, element: Element<'a, Message>) -> Self {
        self.trailing = Some(element);
        self
    }

    /// Override the gap (px) between the header's items. Default `spacing-sm`
    /// (8). Non-finite / non-positive values are ignored.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = sane(spacing, self.spacing);
        self
    }

    /// Override the writing direction. By default the header follows the
    /// locale; right-to-left puts the leading slot and title on the right and
    /// the trailing slot on the left. Focus and reading order stay logical.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Read the captured or overridden title size in logical pixels.
    pub fn resolved_title_size(&self) -> f32 {
        self.title_size
    }

    /// Read the selected outline rung; announcement can still be disabled.
    pub fn heading_level(&self) -> HeadingLevel {
        self.heading
    }

    pub(crate) fn build_title(&self) -> crate::enz_text::EnzText {
        let mut t = enz_text(self.title.clone())
            .size_px(self.title_size)
            .weight(self.weight)
            .tone(TextTone::Default)
            .single_line_elided()
            .announce(self.announce);
        if self.announce {
            t = t.heading(self.heading);
        }
        t
    }

    fn build_subtitle(&self) -> Option<crate::enz_text::EnzText> {
        self.subtitle.as_ref().map(|s| {
            enz_text(s.clone())
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
        })
    }

    fn into_keyed_row(self) -> crate::keyed_row::KeyedRow<'a, Message> {
        let spacing = self.spacing;
        let title = self.build_title();
        let subtitle = self.build_subtitle();
        let leading = self.leading;
        let trailing = self.trailing;
        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;

        let mut items: Vec<(String, Element<'a, Message>)> = Vec::with_capacity(5);
        if let Some(lead) = leading {
            items.push((SLOT_LEADING.to_owned(), lead));
        }
        items.push((SLOT_TITLE.to_owned(), title.into_element()));
        if let Some(sub) = subtitle {
            items.push((SLOT_SUBTITLE.to_owned(), sub.into_element()));
        }
        items.push((
            SLOT_SPACER.to_owned(),
            Space::new().width(Length::Fill).into(),
        ));
        if let Some(trail) = trailing {
            items.push((SLOT_TRAILING.to_owned(), trail));
        }

        // Logical slot keys keep compatible child state attached across optional-slot changes.
        crate::keyed_row::keyed_row(items)
            .mirrored(rtl)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .spacing(spacing)
    }

    /// Convert into an iced `Element`.
    pub fn into_element(self) -> Element<'a, Message> {
        self.into_keyed_row().into()
    }
}

impl<'a, Message: 'a> From<EnzPaneHeader<'a, Message>> for Element<'a, Message> {
    fn from(h: EnzPaneHeader<'a, Message>) -> Self {
        h.into_element()
    }
}
