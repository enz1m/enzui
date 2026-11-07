use super::*;
use crate::item_strip::{Key, Row};
use iced::advanced::layout;

pub(super) fn row<'a, Message: Clone + 'a>(row: TitlebarTabRow<'a, Message>) -> Row<'a, Message> {
    let policy = row.owned_policy();
    let count = row.items.len();
    let mut children = Vec::with_capacity(policy.material_count().unwrap_or(0));
    let mut keys = Vec::with_capacity(children.capacity());
    let active = row.active_index.min(count.saturating_sub(1));
    for (index, item) in row.items.iter().enumerate() {
        for close_visible in [false, true] {
            let selected = index == active;
            let mut cell = enz_document_tab(item.title.clone(), item.activate.clone())
                .selected(selected)
                .focused(selected)
                .embedded(true)
                .set_position(index + 1, count)
                .close_visible(close_visible && item.close.is_some())
                .tab_min_width(0.0)
                .tab_max_width(DEFAULT_ROW_TAB_MAX_WIDTH)
                .fill_width(true)
                .id_prefix(format!("{}-{}", row.id_prefix, item.key))
                .workspace_accent(item.workspace_accent);
            if let Some(background) = item.tab_background {
                cell = cell.tab_background(background);
            }
            if let Some(close) = item.close.clone() {
                cell = cell.on_close(close);
            }
            if !item.icon.is_empty() {
                cell = cell.icon(item.icon.clone());
            }
            cell = cell.modified(item.modified);
            let tab = super::described_titlebar_document_tab_fill(
                cell,
                item.description.as_ref(),
                item.tooltip_delay
                    .or(row.tooltip_delay)
                    .unwrap_or_else(crate::tooltip::default_tooltip_delay),
            );
            keys.push(Key::Tab(item.key.clone(), close_visible));
            children.push(tab);
        }
    }
    for hidden in 1..=count {
        keys.push(Key::List(hidden));
        children.push(row.list_trigger(hidden));
    }
    keys.push(Key::Trailing);
    children.push(match row.trailing.as_ref() {
        Some(trailing) => container(trailing())
            .width(Length::Fixed(DEFAULT_ROW_TRAILING_RESERVE))
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(Vertical::Center)
            .into(),
        None => iced::widget::Space::new().width(0).height(0).into(),
    });
    Row::keyed(layout::item_strip::Policy::Tabs(policy), keys, children)
}
