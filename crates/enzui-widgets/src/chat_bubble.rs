//! Role-aligned chat bubbles with plain, rich-block, streaming, or custom bodies.
//!
//! User bubbles align right; assistant and system bubbles align left. The
//! whole author, bubble, and footnote column has a default 540 logical pixel
//! width cap. Only the padded body has the requested 40 pixel minimum height,
//! subject to parent limits. Author and metadata captions share the role edge.
//!
//! Unpinned body ink and fill follow the runtime palette; geometry and shadow
//! use dark-token defaults. User fill and ink use the palette's filled-color
//! pair. Pinned tokens preserve caller colors without contrast adjustment.
//!
//! A group label contains `author: body`, even when the author caption is
//! hidden; its description contains the role and optional timestamp. Built-in
//! body and caption nodes are suppressed to avoid duplicate labels. Custom
//! bodies and footnotes retain their own semantics. [`EnzChatBubble::message_id`]
//! supplies a stable group ID; callers must keep keys unique within the tree.
//!
//! Use [`crate::chat_list`] or [`crate::chat_transcript`] for transcript scrolling.

use enzui_theme::{Palette, TypographyScale};
use iced::widget::{column, container};
use iced::{Alignment, Border, Color, Length, Padding, Shadow, Vector};
use std::sync::Arc;

use crate::block_list::{EnzBlock, EnzBlockFlow, enz_block_list};
use crate::decorative::decorative;
use crate::enz_text::{TextSize, TextTone, enz_text};
use crate::max_width::max_width;
use crate::min_height::min_height;
use crate::semantics_container::SemanticsContainer;
use crate::streaming_text::enz_streaming_text;
use enzui_core::identity::derive_id;
use enzui_core::semantics::{Role, Semantics};

include!("chat_bubble/model.rs");
include!("chat_bubble/tokens.rs");

/// Chat-bubble builder.
#[must_use]
pub struct EnzChatBubble<'a, Message> {
    body: Body<'a, Message>,
    body_text: Arc<str>,
    role: ChatRole,
    author: Arc<str>,
    show_author: bool,
    max_width: Option<f32>,
    tokens: Option<ChatBubbleTokens>,
    message_id: Option<String>,
    typography: Option<TypographyScale>,
    footnote: Option<FootnoteSlot<'a, Message>>,
}

impl<'a, Message: 'a> EnzChatBubble<'a, Message> {
    /// Set the optional author caption above the bubble.
    pub fn author(mut self, s: impl Into<Arc<str>>) -> Self {
        self.author = s.into();
        self
    }

    /// Show / hide the author caption (default `true`).
    pub fn show_author(mut self, on: bool) -> Self {
        self.show_author = on;
        self
    }

    /// Set a muted metadata caption below the bubble, on its role-aligned edge.
    ///
    /// The text joins the group description and publishes no separate label node.
    /// An empty string clears the slot; whitespace is retained. The last timestamp
    /// or [`Self::footnote`] call wins.
    pub fn timestamp(mut self, s: impl Into<Arc<str>>) -> Self {
        let s: Arc<str> = s.into();
        self.footnote = if s.is_empty() {
            None
        } else {
            Some(FootnoteSlot::Text(s))
        };
        self
    }

    /// Set custom metadata below the bubble, on its role-aligned edge.
    ///
    /// Preserves child semantics, including interactive controls. Wrap decorative
    /// content in [`decorative`] if needed. Replaces any previous timestamp or footnote.
    pub fn footnote(mut self, content: impl Into<iced::Element<'a, Message>>) -> Self {
        self.footnote = Some(FootnoteSlot::Custom(content.into()));
        self
    }

    /// Override the column width cap in logical pixels; defaults to 540.
    ///
    /// Zero is literal; negative or non-finite values fall back to the token cap.
    /// The cap also determines the fixed rich-block body width after padding.
    pub fn max_width(mut self, w: f32) -> Self {
        self.max_width = Some(w);
        self
    }

    /// Pin colors and geometry; invalid geometry falls back to dark-token defaults.
    ///
    /// Width cap and text sizes must be finite and positive. Other geometry accepts
    /// finite nonnegative values. Colors are neither validated nor contrast-adjusted.
    /// `body_size` applies only to plain text; streaming and rich blocks keep their
    /// respective size rules.
    pub fn tokens(mut self, t: ChatBubbleTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }

    /// Publish a stable group ID derived from `enz-chat-bubble` and `id`.
    ///
    /// Empty strings leave the group anonymous. Callers must use unique keys and
    /// retain them across transcript changes; this setter does not key the widget
    /// tree or independently preserve assistive-technology focus.
    pub fn message_id(mut self, id: impl Into<String>) -> Self {
        let id = id.into();
        self.message_id = if id.is_empty() { None } else { Some(id) };
        self
    }

    /// Forward a typography scale to plain and streaming text, author, and timestamp.
    ///
    /// Pinned plain-body and caption pixel sizes take precedence. Rich blocks and
    /// custom elements keep their own typography; the scale is not forwarded there.
    pub fn typography(mut self, scale: TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// Render a rich [`EnzBlock`] body  through the shared prose
    /// engine at the `Chat` rhythm. Replaces the plain-text body.
    ///
    /// Requires `Message: 'static` because [`enz_block_list`] owns its block
    /// data with a `'static` element lifetime.
    pub fn blocks(mut self, blocks: Vec<EnzBlock>) -> Self
    where
        Message: 'static,
    {
        self.body_text = Arc::from(blocks_plaintext(&blocks));
        self.body = Body::Blocks(blocks);
        self
    }

    /// Replace the body with streaming text and a caller-controlled caret.
    ///
    /// This builder owns no blink timer. Typography is forwarded, but pinned
    /// `body_size` is not; built-in child semantics are suppressed in favor of the
    /// group label.
    pub fn streaming(mut self, text: impl Into<Arc<str>>, caret_visible: bool) -> Self {
        let text = text.into();
        self.body_text = Arc::clone(&text);
        self.body = Body::Streaming {
            text,
            caret_visible,
        };
        self
    }

    /// Replace the body with a caller-owned element and group-label text.
    ///
    /// Preserves child semantics; wrap decorative children in [`decorative`] to
    /// avoid a duplicate label.
    pub fn custom_body(
        mut self,
        child: impl Into<iced::Element<'a, Message>>,
        label: impl Into<Arc<str>>,
    ) -> Self {
        self.body_text = label.into();
        self.body = Body::Custom(child.into());
        self
    }

    fn aria_label(&self) -> String {
        if self.author.is_empty() {
            self.body_text.to_string()
        } else {
            format!("{}: {}", self.author, self.body_text)
        }
    }

    fn aria_description(&self) -> String {
        match &self.footnote {
            Some(FootnoteSlot::Text(stamp)) => {
                enzui_core::localized!("enzui-chat-bubble-role-time", "role" => self.role.description(), "timestamp" => stamp)
            }
            _ => self.role.description(),
        }
    }

    /// Resolve the body background color for `palette`, with pinned colors first.
    ///
    /// The body container uses this rule at draw time. Alpha and renderer
    /// compositing can make screenshot pixels differ from the returned color.
    pub fn resolved_bubble_fill(&self, palette: &Palette) -> Color {
        bubble_fill(self.role, self.tokens, palette)
    }

    /// Resolve built-in body ink for `palette`, with pinned colors first.
    ///
    /// User ink is paired with the resolved accent fill; assistant uses primary
    /// text; system resolves muted ink against its surface. Custom children own
    /// their colors. This does not measure rendered glyph contrast.
    pub fn resolved_body_ink(&self, palette: &Palette) -> Color {
        bubble_body_ink(self.role, self.tokens, palette)
    }

    /// Convert into an iced `iced::Element`.
    ///
    /// Requires `Message: 'static` because the rich-block body path composes
    /// [`enz_block_list`], whose element lifetime is `'static`.
    pub fn into_element(self) -> iced::Element<'a, Message>
    where
        Message: 'static,
    {
        let t = self.tokens.unwrap_or_else(ChatBubbleTokens::dark);
        let pinned = self.tokens;
        let typography = self.typography;
        let role = self.role;
        let is_user = role.is_user();
        let aria_label = self.aria_label();
        let description = self.aria_description();
        let message_id = self.message_id.clone();

        let user_ink =
            move |pal: &Palette| -> Color { bubble_body_ink(ChatRole::User, pinned, pal) };

        let system_ink =
            move |pal: &Palette| -> Color { bubble_body_ink(ChatRole::System, pinned, pal) };

        let effective_max_width = match self.max_width {
            Some(w) if w.is_finite() && w >= 0.0 => w,
            _ => t.bubble_max_width,
        };

        let body_padding = Padding::from([t.bubble_padding_y, t.bubble_padding_x]);
        let readable_body_w = (effective_max_width - 2.0 * t.bubble_padding_x).max(0.0);
        let body_el: iced::Element<'a, Message> = match self.body {
            Body::Text(s) => {
                let mut txt = enz_text(s.to_string()).size(TextSize::Body).wrap(true);
                if let Some(p) = pinned {
                    txt = txt.size_px(p.body_size);
                }
                if let Some(scale) = typography {
                    txt = txt.typography(scale);
                }
                txt = match role {
                    ChatRole::User => {
                        txt.color_resolved(move |t| user_ink(&crate::theme_lookup::palette(t)))
                    }
                    ChatRole::Assistant => match pinned {
                        Some(p) => txt.color(p.assistant_foreground),
                        None => txt.tone(TextTone::Default),
                    },
                    ChatRole::System => {
                        txt.color_resolved(move |t| system_ink(&crate::theme_lookup::palette(t)))
                    }
                };
                // The group already carries the body text; suppress duplicate child semantics.
                decorative(txt).into()
            }
            Body::Blocks(blocks) => {
                let mut list = enz_block_list(blocks).flow(EnzBlockFlow::Chat);
                list = match role {
                    ChatRole::User => list
                        .text_color_resolved(move |t| user_ink(&crate::theme_lookup::palette(t))),
                    ChatRole::Assistant => match pinned {
                        Some(p) => list.text_color(p.assistant_foreground),
                        None => list.text_tone(TextTone::Default),
                    },
                    ChatRole::System => list
                        .text_color_resolved(move |t| system_ink(&crate::theme_lookup::palette(t))),
                };
                // Rich blocks need a fixed cell width and retain their own typography.
                let capped = container(list).width(Length::Fixed(readable_body_w));
                decorative(capped).into()
            }
            Body::Streaming {
                text,
                caret_visible,
            } => {
                let mut st = enz_streaming_text(text.to_string())
                    .caret_visible(caret_visible)
                    .announce(false);
                if let Some(scale) = typography {
                    st = st.typography(scale);
                }
                st = match role {
                    ChatRole::User => st.text_color_resolved(move |pal| user_ink(pal)),
                    ChatRole::Assistant => match pinned {
                        Some(p) => st.text_color(p.assistant_foreground),
                        None => st,
                    },
                    ChatRole::System => st.text_color_resolved(move |pal| system_ink(pal)),
                };
                decorative(st).into()
            }
            Body::Custom(child) => {
                // Custom content may contain interactive controls whose semantics must survive.
                child
            }
        };

        let radius = t.bubble_radius;
        let min_h = t.bubble_min_height;
        let shadow = if is_user {
            Shadow::default()
        } else {
            Shadow {
                color: t.shadow_color,
                offset: Vector::new(0.0, t.shadow_offset_y),
                blur_radius: t.shadow_blur,
            }
        };

        let bubble = min_height(
            container(body_el)
                .padding(body_padding)
                .style(move |theme: &iced::Theme| {
                    let bg = bubble_fill(role, pinned, &crate::theme_lookup::palette(theme));
                    container::Style {
                        background: Some(bg.into()),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: radius.into(),
                        },
                        shadow,
                        ..Default::default()
                    }
                }),
            min_h,
        );

        let col_align = if is_user {
            Alignment::End
        } else {
            Alignment::Start
        };
        let inner: iced::Element<'a, Message> = if self.show_author && !self.author.is_empty() {
            let mut author = enz_text(self.author.to_string())
                .size(TextSize::Caption)
                .weight_rung(enzui_theme::WeightRung::Semibold)
                .tone(TextTone::Muted)
                .align(if is_user {
                    crate::enz_text::TextAlign::Right
                } else {
                    crate::enz_text::TextAlign::Left
                });
            if let Some(p) = pinned {
                author = author.size_px(p.author_size);
            }
            if let Some(scale) = typography {
                author = author.typography(scale);
            }
            if let Some(p) = pinned {
                author = author.color(p.author_color);
            }
            let author_el: iced::Element<'a, Message> = author.announce(false).into();
            column![author_el, bubble]
                .spacing(t.author_gap)
                .align_x(col_align)
                .into()
        } else {
            bubble.into()
        };

        let inner: iced::Element<'a, Message> = match self.footnote {
            None => inner,
            Some(slot) => {
                let footnote_el: iced::Element<'a, Message> = match slot {
                    FootnoteSlot::Text(s) => {
                        let mut stamp = enz_text(s.to_string())
                            .size(TextSize::Caption2)
                            .tone(TextTone::Muted)
                            .align(if is_user {
                                crate::enz_text::TextAlign::Right
                            } else {
                                crate::enz_text::TextAlign::Left
                            });
                        if let Some(p) = pinned {
                            stamp = stamp.size_px(p.footnote_size);
                        }
                        if let Some(scale) = typography {
                            stamp = stamp.typography(scale);
                        }
                        if let Some(p) = pinned {
                            stamp = stamp.color(p.footnote_color);
                        }
                        stamp.announce(false).into()
                    }
                    FootnoteSlot::Custom(el) => el,
                };
                column![inner, footnote_el]
                    .spacing(t.footnote_gap)
                    .align_x(col_align)
                    .into()
            }
        };

        let capped = max_width(inner, effective_max_width);
        let mut grouped = SemanticsContainer::new(
            capped,
            Semantics::new(Role::Group)
                .label(aria_label)
                .description(description),
        );
        if let Some(mid) = message_id {
            grouped = grouped.id(derive_id("enz-chat-bubble", &mid));
        }

        container(grouped)
            .width(Length::Fill)
            .align_x(if is_user {
                iced::alignment::Horizontal::Right
            } else {
                iced::alignment::Horizontal::Left
            })
            .into()
    }
}

impl<'a, Message: 'static> From<EnzChatBubble<'a, Message>> for iced::Element<'a, Message> {
    fn from(b: EnzChatBubble<'a, Message>) -> Self {
        b.into_element()
    }
}

/// Construct a chat bubble with the given plain-text body and role.
pub fn enz_chat_bubble<'a, Message: 'a>(
    body: impl Into<Arc<str>>,
    role: ChatRole,
) -> EnzChatBubble<'a, Message> {
    let body: Arc<str> = body.into();
    EnzChatBubble {
        body_text: Arc::clone(&body),
        body: Body::Text(body),
        role,
        author: Arc::from(""),
        show_author: true,
        max_width: None,
        tokens: None,
        message_id: None,
        typography: None,
        footnote: None,
    }
}

fn blocks_plaintext(blocks: &[EnzBlock]) -> String {
    blocks
        .iter()
        .map(|b| b.text.as_str())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
