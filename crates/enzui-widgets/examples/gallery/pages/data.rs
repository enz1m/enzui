use crate::*;
use iced::widget::column;

impl Gallery {
    pub(crate) fn data_page(&self) -> Element<'_, Message> {
        let selected = self.table_selected;
        let sort = self.table_sort.clone();
        let table = enzui_widgets::table::enz_table(
            &self.files,
            enzui_core::virtual_sequence::HeightModel::new(FILE_ROWS as usize, FILE_ROW_HEIGHT),
            enzui_core::virtual_sequence::VariableHeightModel::new(self.table_widths),
            |_row, column, value: &String| {
                // Sizes are numbers: right-aligned so their digits line up.
                enzui_widgets::table_text_cell::enz_table_text_cell(value.clone())
                    .align(if column == SIZE_COLUMN {
                        enzui_widgets::table_text_cell::EnzTableTextAlign::Right
                    } else {
                        enzui_widgets::table_text_cell::EnzTableTextAlign::Left
                    })
                    .into_element()
            },
        )
        .header(move |index, key| {
            let active = sort.as_ref().filter(|(column, _)| column == key);
            enzui_widgets::table_header_cell::enz_table_header_cell(FILE_COLUMNS[index].1)
                .align(if index == SIZE_COLUMN {
                    enzui_widgets::table_header_cell::EnzTableHeaderCellAlign::Right
                } else {
                    enzui_widgets::table_header_cell::EnzTableHeaderCellAlign::Left
                })
                .sortable(true)
                .sort_active(active.is_some())
                .sort_ascending(active.is_some_and(|(_, direction)| {
                    *direction == enzui_core::table::SortDirection::Ascending
                }))
                .into_element()
        })
        .width(Length::Fill)
        .height(Length::Fixed(320.0))
        .scroll_y(self.table_offset)
        .on_scroll(|_dx, dy| Message::TableScroll(dy))
        .viewport_state(&self.table_viewport)
        .on_column_sort(Message::TableSort)
        .on_column_resize(Message::TableResize)
        .sort_state(self.table_sort.clone())
        .row_selection(move |row| Some(row) == selected)
        .on_row_activate(|row| (format!("Row {}", row + 1), Message::TableSelect(row)))
        .into_element();
        let k = &self.knobs;
        let indent = k.number("tree.indent", 18.0);
        column![
                    self.specimen(
                        "enz_table",
                        "A virtualised table: only the visible rows are built, so 500 rows cost as much as 10. Click a header to sort, drag its edge to resize, click a row to select it.",
                        column![
                            table,
                            enz_text(
                                match self.table_selected.and_then(|row| self.files.row(row)) {
                                    Some((name, _, owner)) => format!("Selected {name} ({owner})"),
                                    None => String::from("Nothing selected"),
                                }
                            )
                            .into_element(),
                        ]
                        .spacing(12),
                    )
                    .code_text(TABLE_CODE)
                    .into_element(),
                    self.specimen(
                        "enz_notification_inbox",
                        "A list of notifications with read state, grouping and dismissal. The core Inbox holds them; apply() updates it.",
                        container(enzui_widgets::notification_inbox::enz_notification_inbox(
                            &self.inbox,
                            Message::Inbox
                        ))
                        .width(Length::Fill.max(420.0)),
                    )
                    .code_text(
                        "use enzui_widgets::notification_inbox::{apply, enz_notification_inbox};\n\n\
                         // In the state: inbox: enzui_core::notification_inbox::Inbox\n\
                         // In update:    Message::Inbox(event) => apply(&mut self.inbox, event),\n\
                         enz_notification_inbox(&self.inbox, Message::Inbox)",
                    )
                    .into_element(),
                    {
                        let selected = self.tree_selected;
                        self.specimen(
                            "enz_tree_view",
                            "Nested rows that open and close. The app keeps which branches are open and passes the visible rows, each with its depth.",
                            enzui_widgets::tree_view::enz_tree_view(
                                tree_rows(self.tree_open)
                                    .into_iter()
                                    .enumerate()
                                    .map(|(row, (label, depth, branch))| {
                                        let item = match branch {
                                            Some(b) => enzui_widgets::tree_view::EnzTreeItem::branch(
                                                label,
                                                depth,
                                                self.tree_open[b],
                                            ),
                                            None => enzui_widgets::tree_view::EnzTreeItem::leaf(
                                                label, depth,
                                            ),
                                        };
                                        item.selected(selected == Some(row))
                                    })
                                    .collect(),
                            )
                            .indent_width(indent)
                            .on_toggle(Message::TreeToggle)
                            .on_select(Message::TreeSelect)
                            .into_element(),
                        )
                        .controls([k.number_control("tree.indent", "Indent", 6.0..=42.0, 4.0, 18.0)])
                        .code(
                            &code("enzui_widgets", "enz_tree_view")
                                .import("enzui_widgets::EnzTreeItem")
                                .arg("vec![\n    EnzTreeItem::branch(\"Projects\", 0, self.open),\n    EnzTreeItem::leaf(\"enzUI\", 1).selected(true),\n]")
                                .set_if(indent != 18.0, "indent_width", number(indent))
                                .set("on_toggle", "Message::ToggleRow")
                                .set("on_select", "Message::SelectRow"),
                        )
                        .into_element()
                    },
                ]
                .spacing(32)
                .width(Length::Fill)
                .into()
    }
}

const TABLE_CODE: &str = "use enzui_core::virtual_sequence::{HeightModel, VariableHeightModel};
use enzui_widgets::table::enz_table;
use enzui_widgets::table_text_cell::enz_table_text_cell;

// `self.files` implements TableProvider: a row count and a cell per column.
enz_table(
    &self.files,
    HeightModel::new(row_count, 32.0),
    VariableHeightModel::new(self.column_widths),
    |_row, _column, value: &String| enz_table_text_cell(value.clone()).into_element(),
)
.header(|index, key| header_cell(index, key))
.height(Length::Fixed(320.0))
.scroll_y(self.offset)
.on_scroll(|_dx, dy| Message::Scroll(dy))
.on_column_sort(Message::Sort)
.on_column_resize(Message::Resize)
.row_selection(move |row| Some(row) == selected)
.into_element()";
