//! Activatable list-item previews with title, metadata, excerpt and footer.
//!
//! Titles use the UI's 13px Semibold (Bold while unread) and trailing
//! strings stock 12px. The shared
//! text pair end-elides both, with an 8px gap and 160px metadata cap. A zero gap
//! is literal; a zero cap removes this ceiling while parent width and the
//! text pair's leading-em reservation still apply.
//!
//! Nonempty excerpts reserve two 16px lines by default. Requested line counts
//! clamp to 0..=8; zero removes the visual excerpt and its slot. Excerpts use
//! stock 12px text, while footer text and icons use UI-scaled 12px and 14px.
//! Footer rows require nonempty labels. Each optional row has a 4px top gap;
//! content has 7px vertical and 12px horizontal padding inside the surface's
//! 1px row gap, and grows the surface.
//!
//! [`EnzPreviewRow::leading`] puts an avatar or icon before the text, top
//! aligned; [`EnzPreviewRow::unread`] marks an unread item with an accent dot
//! just before its time (or at the end of the title row without one) and a
//! bold title, and says so to assistive technology. The dot sits before the
//! time so times stay aligned down a list of read and unread rows.
//!
//! The title names the list item. Trimmed metadata, excerpt and footer join
//! its description with `. `; the excerpt remains described even when hidden
//! by a zero line count. Child text announcements are suppressed. Ink follows
//! the live palette and selection, focus and disabling use the shared surface.
//! Ids default to title plus namespace: repeated titles in one namespace collide,
//! and renaming changes the id. Supply stable ids when titles are not unique keys.

use crate::enz_text::{TextTone, enz_text};
use crate::list_row_surface::enz_list_row_surface;
use crate::text_pair::enz_text_pair;
use enzui_core::semantics::{Role, Semantics};
use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{Space, column, container, row};
use iced::{Alignment, Element, Length};
/// Default logical title-to-metadata gap: 8px.
pub const DEFAULT_TITLE_TRAILING_GAP: f32 = 8.0;
/// Default logical metadata width ceiling: 160px.
pub const DEFAULT_TRAILING_MAX_WIDTH: f32 = 160.0;
const TITLE_SIZE: f32 = 13.0;
/// The unread dot's diameter.
const UNREAD_DOT: f32 = 8.0;
/// Gap between a leading avatar or icon and the text.
const LEADING_GAP: f32 = 10.0;
const TRAILING_SIZE: f32 = 12.0;
const EXCERPT_SIZE: f32 = 12.0;
/// Default namespace for title-derived row identity.
pub const DEFAULT_ID_PREFIX: &str = "enz-preview-row";
/// Logical excerpt line height used for text layout and slot reservation: 16px.
pub const EXCERPT_LINE_HEIGHT_PX: f32 = 16.0;
/// Largest excerpt line request retained: 8.
pub const MAX_EXCERPT_LINES: u32 = 8;
/// Clamp a line request to 0..=8; zero is preserved.
#[must_use]
pub const fn normalize_excerpt_lines(requested: u32) -> u32 {
    if requested > MAX_EXCERPT_LINES {
        MAX_EXCERPT_LINES
    } else {
        requested
    }
}
/// Activatable list item with optional metadata, bounded excerpt and footer.
#[must_use]
pub struct EnzPreviewRow<'a, Message> {
    title: String,
    trailing: String,
    excerpt: String,
    excerpt_lines: u32,
    footer_label: String,
    footer_icon: String,
    title_trailing_gap: f32,
    trailing_max_width: f32,
    selected: bool,
    enabled: bool,
    on_activate: Message,
    id: Option<iced::advanced::widget::Id>,
    id_prefix: String,
    leading: Option<Element<'a, Message>>,
    unread: bool,
}
/// Create an enabled, unselected preview with a two-line excerpt allowance.
pub fn enz_preview_row<'a, Message: Clone + 'a>(
    title: impl Into<String>,
    on_activate: Message,
) -> EnzPreviewRow<'a, Message> {
    EnzPreviewRow {
        title: title.into(),
        trailing: String::new(),
        excerpt: String::new(),
        excerpt_lines: 2,
        footer_label: String::new(),
        footer_icon: String::new(),
        title_trailing_gap: DEFAULT_TITLE_TRAILING_GAP,
        trailing_max_width: DEFAULT_TRAILING_MAX_WIDTH,
        selected: false,
        enabled: true,
        on_activate,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        leading: None,
        unread: false,
    }
}

impl<'a, Message: Clone + 'a> EnzPreviewRow<'a, Message> {
    /// Set trailing metadata; exactly empty text omits its cell and gap.
    pub fn trailing(mut self, t: impl Into<String>) -> Self {
        self.trailing = t.into();
        self
    }
    /// Set excerpt text; exactly empty text omits its slot.
    pub fn excerpt(mut self, e: impl Into<String>) -> Self {
        self.excerpt = e.into();
        self
    }
    /// Request excerpt lines, default 2, clamped to 0..=8 at composition.
    /// Zero removes the visual excerpt and slot but leaves the accessible description.
    pub fn excerpt_lines(mut self, l: u32) -> Self {
        self.excerpt_lines = l;
        self
    }
    /// Set footer text and optional catalog icon; empty text omits the entire footer.
    pub fn footer(mut self, label: impl Into<String>, icon: impl Into<String>) -> Self {
        self.footer_label = label.into();
        self.footer_icon = icon.into();
        self
    }
    /// Set the logical title-to-metadata gap, default 8; zero is literal.
    /// Negative or non-finite input uses stock 8px.
    pub fn title_trailing_gap(mut self, px: f32) -> Self {
        self.title_trailing_gap = if px.is_finite() && px >= 0.0 {
            px
        } else {
            DEFAULT_TITLE_TRAILING_GAP
        };
        self
    }
    /// Set the logical metadata width ceiling, default 160px.
    /// Zero removes this ceiling; parent limits and the text pair reservation still apply.
    /// Negative or non-finite input uses stock 160px.
    pub fn trailing_max_width(mut self, px: f32) -> Self {
        self.trailing_max_width = if px.is_finite() && px > 0.0 {
            px
        } else if px == 0.0 {
            // The text pair rejects infinite caps, so use its largest accepted finite ceiling.
            f32::MAX
        } else {
            DEFAULT_TRAILING_MAX_WIDTH
        };
        self
    }
    /// Put an avatar or icon before the text, aligned with the title.
    pub fn leading(mut self, leading: impl Into<Element<'a, Message>>) -> Self {
        self.leading = Some(leading.into());
        self
    }
    /// Mark the item unread: an accent dot before its time, a bold
    /// title, and "Unread" in its accessible description.
    pub fn unread(mut self, unread: bool) -> Self {
        self.unread = unread;
        self
    }
    /// Set the shared surface selection treatment and selected semantics.
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    /// Enable activation by default; false disables activation and dims inherited ink.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Pin a stable focus and accessibility action address, overriding title-derived identity.
    /// Use this for repeated titles and identity that must survive renaming.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }
    /// Namespace title-derived ids; repeated titles within one namespace still collide.
    /// An explicit id is unaffected.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }
    /// Compose growing row content with live ink and list-item semantics.
    pub fn into_element(self) -> Element<'a, Message> {
        let title = self.title.clone();
        // Suppressed child labels are represented by the row name and secondary description.
        let unread_word = self
            .unread
            .then(|| enzui_core::localization::text("enzui-preview-row-unread"));
        let description = Self::build_description(
            unread_word.as_deref().unwrap_or(""),
            &self.trailing,
            &self.excerpt,
            &self.footer_label,
        );
        let title_row: Element<'a, Message> = Self::title_row_element(
            self.title,
            self.trailing,
            self.title_trailing_gap,
            self.trailing_max_width,
            self.unread,
        );
        let excerpt_lines_n = normalize_excerpt_lines(self.excerpt_lines);
        let excerpt: Option<Element<'a, Message>> =
            if excerpt_lines_n == 0 || self.excerpt.is_empty() {
                None
            } else {
                // One normalized count drives both text height and the reserved slot.
                let reserved_h = excerpt_lines_n as f32 * EXCERPT_LINE_HEIGHT_PX;
                Some(
                    container(
                        enz_text(self.excerpt)
                            .size_px(EXCERPT_SIZE)
                            .tone(TextTone::Muted)
                            .announce(false)
                            .wrap(true)
                            .lines(excerpt_lines_n as u16)
                            .line_height(LineHeight::Absolute(EXCERPT_LINE_HEIGHT_PX.into())),
                    )
                    .width(Length::Fill)
                    .height(Length::Fixed(reserved_h))
                    .into(),
                )
            };
        let footer: Option<Element<'a, Message>> = if !self.footer_label.is_empty() {
            let mut fr = row![].align_y(Alignment::Center).spacing(6);
            if !self.footer_icon.is_empty() {
                fr = fr.push(
                    crate::icon::enz_icon(&self.footer_icon)
                        .size(enzui_theme::text_scale::ui_px(14.0))
                        .tint_theme_tone(crate::icon::IconTone::Muted),
                );
            }
            fr = fr.push(
                enz_text(self.footer_label)
                    .size_px(enzui_theme::text_scale::ui_px(12.0))
                    .tone(TextTone::Muted)
                    .announce(false),
            );
            Some(fr.into())
        } else {
            None
        };

        let mut body = column![title_row].spacing(0);
        if let Some(ex) = excerpt {
            body = body.push(Space::new().height(Length::Fixed(4.0)));
            body = body.push(ex);
        }
        if let Some(f) = footer {
            body = body.push(Space::new().height(Length::Fixed(4.0)));
            body = body.push(f);
        }
        let mut content = row![].align_y(Alignment::Start);
        if let Some(leading) = self.leading {
            content =
                content.push(container(leading).padding(iced::Padding::ZERO.right(LEADING_GAP)));
        }
        content = content.push(body);
        let inner: Element<'a, Message> = container(content)
            // One pixel each way less than its 8: the surface's row gap
            // takes the same back outside the fill.
            .padding([7.0, 12.0])
            .into();
        let mut item = Semantics::new(Role::ListItem).default_action().label(title);
        if let Some(desc) = description {
            item = item.description(desc);
        }
        if self.selected {
            item = item.selected(true);
        }
        let mut surface = enz_list_row_surface::<Message>(inner, self.on_activate)
            .content_padding(0.0)
            .selected(self.selected)
            .semantics(item)
            // Content owns its insets and must grow beyond the surface default row height.
            .content_height(true)
            .outer_gutter(0.0)
            .id_prefix(self.id_prefix);
        if let Some(id) = self.id {
            surface = surface.id(id);
        }
        surface = surface.enabled(self.enabled);
        surface.into()
    }
    fn build_description(
        unread: &str,
        trailing: &str,
        excerpt: &str,
        footer: &str,
    ) -> Option<String> {
        let parts: Vec<&str> = [unread, trailing, excerpt, footer]
            .into_iter()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(". "))
        }
    }
    fn title_row_element(
        title: String,
        trailing: String,
        gap: f32,
        trailing_max_width: f32,
        unread: bool,
    ) -> Element<'a, Message> {
        let weight = if unread {
            Weight::Bold
        } else {
            Weight::Semibold
        };
        if !unread {
            return enz_text_pair(title, trailing)
                .leading_size(enzui_theme::text_scale::ui_px(TITLE_SIZE))
                .leading_weight(weight)
                .trailing_size(TRAILING_SIZE)
                .spacing(gap)
                .trailing_max_width(trailing_max_width)
                .announce(false)
                .into();
        }
        // Unread: the title, then the dot, then the time.
        let dot = container(Space::new().width(UNREAD_DOT).height(UNREAD_DOT)).style(
            |theme: &iced::Theme| iced::widget::container::Style {
                background: Some(crate::theme_lookup::palette(theme).accent.into()),
                border: iced::Border::default().rounded(UNREAD_DOT / 2.0),
                ..Default::default()
            },
        );
        let mut line = row![
            container(
                enz_text_pair(title, String::new())
                    .leading_size(enzui_theme::text_scale::ui_px(TITLE_SIZE))
                    .leading_weight(weight)
                    .announce(false)
            )
            .width(Length::Fill),
            dot,
        ]
        .spacing(gap)
        .align_y(Alignment::Center);
        if !trailing.is_empty() {
            line = line.push(crate::max_width::max_width(
                enz_text(trailing)
                    .size_px(TRAILING_SIZE)
                    .tone(TextTone::Muted)
                    .announce(false)
                    .single_line_elided(),
                trailing_max_width,
            ));
        }
        line.into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzPreviewRow<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzPreviewRow<'a, Message>) -> Self {
        w.into_element()
    }
}
