/// Chat message role. Controls fill, foreground and horizontal alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChatRole {
    /// Human message; right-aligned with the resolved accent fill and ink pair.
    User,
    /// Author is the assistant / bot. Bubble fills `surface_alt`, left-aligns.
    #[default]
    Assistant,
    /// Author is the system. Bubble fills `surface`, left-aligns.
    System,
}

impl ChatRole {
    fn is_user(self) -> bool {
        matches!(self, ChatRole::User)
    }

    fn description(self) -> String {
        match self {
            ChatRole::User => enzui_core::localization::text("enzui-chat-bubble-your-message"),
            ChatRole::Assistant => enzui_core::localization::text("enzui-chat-bubble-assistant-message"),
            ChatRole::System => enzui_core::localization::text("enzui-chat-bubble-system-message"),
        }
    }
}

enum Body<'a, Message> {
    Text(Arc<str>),
    Blocks(Vec<EnzBlock>),
    Streaming { text: Arc<str>, caret_visible: bool },
    Custom(iced::Element<'a, Message>),
}

enum FootnoteSlot<'a, Message> {
    Text(Arc<str>),
    Custom(iced::Element<'a, Message>),
}
