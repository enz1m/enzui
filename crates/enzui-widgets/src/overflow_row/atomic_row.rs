use iced::Element;
use iced::widget::text;

pub(super) struct AtomicRow;

impl AtomicRow {
    pub(super) fn build<'a, Message: 'a>(
        items: Vec<Element<'a, Message>>,
        spacing: f32,
    ) -> crate::item_strip::Row<'a, Message> {
        Self::with_indicator(items, text("…").into(), spacing)
    }

    pub(super) fn with_indicator<'a, Message: 'a>(
        items: Vec<Element<'a, Message>>,
        indicator: Element<'a, Message>,
        spacing: f32,
    ) -> crate::item_strip::Row<'a, Message> {
        crate::item_strip::Row::prefix(items, indicator, spacing.max(0.0))
    }
}
