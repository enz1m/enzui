use crate::*;
use enzui_theme::SpaceToken;
use enzui_widgets::carousel::Indicator;
use iced::widget::column;

const GAPS: [SpaceToken; 4] = [
    SpaceToken::Xs,
    SpaceToken::Sm,
    SpaceToken::Md,
    SpaceToken::Lg,
];
const GAP_NAMES: &[&str] = &["Xs", "Sm", "Md", "Lg"];
const INDICATORS: [Indicator; 3] = [Indicator::Lines, Indicator::Dots, Indicator::None];
const PLACEMENTS: [enzui_widgets::carousel::ArrowPlacement; 3] = [
    enzui_widgets::carousel::ArrowPlacement::Inside,
    enzui_widgets::carousel::ArrowPlacement::Edge,
    enzui_widgets::carousel::ArrowPlacement::Outside,
];
const PLACEMENT_NAMES: &[&str] = &["Inside", "Edge", "Outside"];
const INDICATOR_NAMES: &[&str] = &["Lines", "Dots", "None"];

impl Gallery {
    pub(crate) fn layouts_page(&self) -> Element<'_, Message> {
        let tile = |title: &'static str, body: &'static str| -> Element<'_, Message> {
            enz_card(enz_text(body).into_element())
                .title(title)
                .into_element()
        };
        let cells: Vec<enzui_widgets::responsive_grid::CellFn<'_, Message>> = vec![
            Box::new(move || tile("Inbox", "12 unread")),
            Box::new(move || tile("Calendar", "3 events today")),
            Box::new(move || tile("Files", "48 GB free")),
            Box::new(move || tile("Music", "Now playing")),
            Box::new(move || tile("Photos", "1,204 items")),
            Box::new(move || tile("Notes", "Last edited 2 h ago")),
        ];
        let slide = |id: &'static str, title: &'static str, a: iced::Color, b: iced::Color| {
            enzui_widgets::carousel::CarouselPage::new(id, move || {
                iced::widget::stack![
                    enzui_widgets::gradient_background::enz_gradient_background(
                        Space::new().width(Length::Fill).height(Length::Fill)
                    )
                    .color_start(a)
                    .color_end(b)
                    .show_mid(false)
                    .angle_degrees(135.0)
                    .border_radius(12.0)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into_element(),
                    container(
                        enz_text(title)
                            .size(TextSize::Title2)
                            .color(iced::Color::WHITE)
                            .into_element()
                    )
                    .padding(24)
                    .align_bottom(Length::Fill),
                ]
                .into()
            })
        };
        let k = &self.knobs;
        let grid_gap = k.pick("grid.gap", 1);
        let flow_gap = k.pick("flow.gap", 2);
        let indicator = k.pick("carousel.indicator", 0);
        let arrows = k.flag("carousel.arrows", true);
        let placement = k.pick("carousel.placement", 0);
        let wrap = k.flag("carousel.wrap", false);
        let bordered = k.flag("carousel.bordered", false);
        let height = k.number("carousel.height", 220.0);
        column![
                    self.specimen(
                        "enz_responsive_grid",
                        "Cells in columns that follow the width: three here, one on a phone. Resize the window to see it.",
                        enzui_widgets::responsive_grid::enz_responsive_grid(cells)
                            .gap(GAPS[grid_gap])
                            .into_element(),
                    )
                    .controls([k.pick_control("grid.gap", "Gap", GAP_NAMES, 1)])
                    .code(
                        &code("enzui_widgets::responsive_grid", "enz_responsive_grid")
                            .arg("cells")
                            .variant(grid_gap != 1, "gap", "enzui_theme", "SpaceToken", GAP_NAMES[grid_gap]),
                    )
                    .into_element(),
                    self.specimen(
                        "enz_flow",
                        "Items in a row that wraps onto new lines, like words in a paragraph.",
                        [
                            "Rust",
                            "Linux",
                            "Wayland",
                            "GPU",
                            "Accessibility",
                            "Design",
                            "Motion",
                            "Typography"
                        ]
                        .into_iter()
                        .fold(
                            enzui_widgets::flow::enz_flow()
                                .gap(GAPS[flow_gap])
                                .cross_gap(GAPS[flow_gap]),
                            |flow, tag| {
                                flow.push(
                                    enzui_widgets::button::enz_filter_chip(
                                        tag,
                                        tag == "Design",
                                        Message::Clicked(tag),
                                    )
                                    .into_element(),
                                )
                            }
                        )
                        .into_element(),
                    )
                    .controls([k.pick_control("flow.gap", "Gap", GAP_NAMES, 2)])
                    .code_text(format!(
                        "use enzui_theme::SpaceToken;\nuse enzui_widgets::flow::enz_flow;\n\n\
                         tags.iter().fold(\n    \
                             enz_flow()\n        .gap(SpaceToken::{0})\n        .cross_gap(SpaceToken::{0}),\n    \
                             |flow, tag| flow.push(enz_filter_chip(tag, false, Message::Filter(tag)).into_element()),\n\
                         )\n\
                         .into_element()",
                        GAP_NAMES[flow_gap]
                    ))
                    .into_element(),
                    self.specimen(
                        "enz_carousel",
                        "Pages one at a time, with indicators, arrow keys and swipe. Each page is built only when shown.",
                        enzui_widgets::carousel::enz_carousel(
                            vec![
                                slide(
                                    "dawn",
                                    "Dawn",
                                    iced::Color::from_rgb8(0xF6, 0x8B, 0x5A),
                                    iced::Color::from_rgb8(0x91, 0x41, 0xAC)
                                ),
                                slide(
                                    "noon",
                                    "Noon",
                                    iced::Color::from_rgb8(0x35, 0x84, 0xE4),
                                    iced::Color::from_rgb8(0x2E, 0xC2, 0x7E)
                                ),
                                slide(
                                    "dusk",
                                    "Dusk",
                                    iced::Color::from_rgb8(0x1C, 0x2B, 0x5E),
                                    iced::Color::from_rgb8(0xC0, 0x1C, 0x28)
                                ),
                            ],
                            Some(self.slide.clone()),
                            Message::Slide,
                        )
                        .indicator(INDICATORS[indicator])
                        .show_navigation(arrows)
                        .arrow_placement(PLACEMENTS[placement])
                        .wrap(if wrap {
                            enzui_core::paging::Wrap::Wrap
                        } else {
                            enzui_core::paging::Wrap::Clamp
                        })
                        .bordered(bordered)
                        .height(Length::Fixed(height))
                        .into_element(),
                    )
                    .controls([
                        k.pick_control("carousel.indicator", "Indicator", INDICATOR_NAMES, 0),
                        k.number_control("carousel.height", "Height", 160.0..=320.0, 20.0, 220.0),
                        k.flag_control("carousel.arrows", "Arrows", true),
                        k.pick_control("carousel.placement", "Arrows at", PLACEMENT_NAMES, 0),
                        k.flag_control("carousel.wrap", "Wrap around", false),
                        k.flag_control("carousel.bordered", "Bordered", false),
                    ])
                    .code(
                        &code("enzui_widgets", "enz_carousel")
                            .import("enzui_widgets::carousel::CarouselPage")
                            .arg("vec![\n    CarouselPage::new(\"dawn\", || dawn_slide()),\n    CarouselPage::new(\"noon\", || noon_slide()),\n]")
                            .arg("Some(self.slide.clone())")
                            .arg("Message::Slide")
                            .variant(
                                indicator != 0,
                                "indicator",
                                "enzui_widgets::carousel",
                                "Indicator",
                                INDICATOR_NAMES[indicator],
                            )
                            .set_if(arrows, "show_navigation", "true")
                            .variant(
                                arrows && placement != 0,
                                "arrow_placement",
                                "enzui_widgets::carousel",
                                "ArrowPlacement",
                                PLACEMENT_NAMES[placement],
                            )
                            .variant(wrap, "wrap", "enzui_core::paging", "Wrap", "Wrap")
                            .set_if(bordered, "bordered", "true")
                            .set("height", format!("Length::Fixed({})", number(height))),
                    )
                    .into_element(),
                ]
                .spacing(32)
                .width(Length::Fill)
                .into()
    }
}
