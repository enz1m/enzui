fn bubble_fill(role: ChatRole, pinned: Option<ChatBubbleTokens>, p: &Palette) -> Color {
    match pinned {
        Some(t) => t.role_colors(role).0,
        None => match role {
            ChatRole::User => p.filled_color(p.accent).color,
            ChatRole::Assistant => p.surface_alt,
            ChatRole::System => p.surface,
        },
    }
}

fn bubble_body_ink(role: ChatRole, pinned: Option<ChatBubbleTokens>, p: &Palette) -> Color {
    match pinned {
        Some(t) => t.role_colors(role).1,
        None => match role {
            ChatRole::User => p.filled_color(p.accent).text,
            ChatRole::Assistant => p.text,
            ChatRole::System => p.text_muted_on(p.surface),
        },
    }
}

/// Fixed colors and logical-pixel geometry for a pinned chat bubble.
///
/// [`EnzChatBubble::tokens`] normalizes geometry and preserves colors verbatim.
/// Text sizes apply to plain body, author, and timestamp; rich and streaming
/// bodies retain their own size rules.
#[derive(Debug, Clone, Copy)]
pub struct ChatBubbleTokens {
    /// User bubble fill resolved from the accent control pair.
    pub user_background: Color,
    /// User ink paired with the resolved accent fill.
    pub user_foreground: Color,
    /// Assistant bubble fill (default `surface_alt`).
    pub assistant_background: Color,
    /// Assistant bubble text ink (default `text`).
    pub assistant_foreground: Color,
    /// System bubble fill (default `surface`).
    pub system_background: Color,
    /// System ink resolved from muted text against the system surface.
    pub system_foreground: Color,
    /// Author caption ink (default `text_muted`).
    pub author_color: Color,
    /// Footnote / timestamp ink (default `text_muted`) — the metadata lane
    /// below the bubble.
    pub footnote_color: Color,
    /// Drop-shadow colour for assistant/system bubbles (default none: flat
    /// bubbles; set a colour to lift them).
    pub shadow_color: Color,
    /// Corner radius; default 10 logical pixels.
    pub bubble_radius: f32,
    /// Horizontal body padding; default 14 logical pixels.
    pub bubble_padding_x: f32,
    /// Vertical body padding; default 10 logical pixels.
    pub bubble_padding_y: f32,
    /// Column width cap; default 540 logical pixels.
    pub bubble_max_width: f32,
    /// Requested minimum body height; default 40 logical pixels.
    pub bubble_min_height: f32,
    /// Pinned author caption size; default 11 logical pixels.
    pub author_size: f32,
    /// Pinned plain-body size; default 13 logical pixels.
    pub body_size: f32,
    /// Author-to-body gap; default 4 logical pixels.
    pub author_gap: f32,
    /// Pinned timestamp size; default 10 logical pixels.
    pub footnote_size: f32,
    /// Body-to-footnote gap; default 4 logical pixels.
    pub footnote_gap: f32,
    /// Assistant/system shadow blur; default 6 logical pixels.
    pub shadow_blur: f32,
    /// Assistant/system shadow vertical offset; default 1 logical pixel.
    pub shadow_offset_y: f32,
}

impl ChatBubbleTokens {
    /// Snapshot colors from `p` with default geometry; later palette edits do not propagate.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            user_background: p.filled_color(p.accent).color,
            user_foreground: p.filled_color(p.accent).text,
            assistant_background: p.surface_alt,
            assistant_foreground: p.text,
            system_background: p.surface,
            system_foreground: p.text_muted_on(p.surface),
            author_color: p.text_muted,
            footnote_color: p.text_muted,
            // Flat, as message bubbles are on every platform: the fill
            // already parts the bubble from the page.
            shadow_color: Color::TRANSPARENT,
            bubble_radius: 10.0,
            bubble_padding_x: 14.0,
            bubble_padding_y: 10.0,
            bubble_max_width: 540.0,
            bubble_min_height: 40.0,
            author_size: 11.0,
            body_size: 13.0,
            author_gap: 4.0,
            footnote_size: 10.0,
            footnote_gap: 4.0,
            shadow_blur: 6.0,
            shadow_offset_y: 1.0,
        }
    }

    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    fn role_colors(&self, role: ChatRole) -> (Color, Color) {
        match role {
            ChatRole::User => (self.user_background, self.user_foreground),
            ChatRole::Assistant => (self.assistant_background, self.assistant_foreground),
            ChatRole::System => (self.system_background, self.system_foreground),
        }
    }

    fn sanitized(self) -> Self {
        let d = Self::dark();
        let pos = |v: f32, fb: f32| if v.is_finite() && v > 0.0 { v } else { fb };
        let nonneg = |v: f32, fb: f32| if v.is_finite() && v >= 0.0 { v } else { fb };
        Self {
            bubble_radius: nonneg(self.bubble_radius, d.bubble_radius),
            bubble_padding_x: nonneg(self.bubble_padding_x, d.bubble_padding_x),
            bubble_padding_y: nonneg(self.bubble_padding_y, d.bubble_padding_y),
            bubble_max_width: pos(self.bubble_max_width, d.bubble_max_width),
            bubble_min_height: nonneg(self.bubble_min_height, d.bubble_min_height),
            author_size: pos(self.author_size, d.author_size),
            body_size: pos(self.body_size, d.body_size),
            author_gap: nonneg(self.author_gap, d.author_gap),
            footnote_size: pos(self.footnote_size, d.footnote_size),
            footnote_gap: nonneg(self.footnote_gap, d.footnote_gap),
            shadow_blur: nonneg(self.shadow_blur, d.shadow_blur),
            shadow_offset_y: nonneg(self.shadow_offset_y, d.shadow_offset_y),
            ..self
        }
    }
}
