use crate::*;
use enzui_widgets::area_chart::ChartCurve;
use iced::widget::{column, row};

impl Gallery {
    pub(crate) fn charts_page(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let wave = |phase: f64| {
            Series::from_points(
                SeriesId(0),
                (0..24).map(move |x| {
                    (
                        f64::from(x),
                        40.0 + 25.0 * (f64::from(x) / 3.0 + phase).sin(),
                    )
                }),
            )
        };
        column![
            {
                let height = k.number("line.height", 220.0);
                let crosshair = k.flag("line.crosshair", true);
                let axis = k.flag("line.axis_label", false);
                let mut chart = enz_line_chart::<Message>("Requests per minute")
                    .push(wave(0.0))
                    .width(Length::Fill)
                    .height(height)
                    .crosshair(crosshair);
                if axis {
                    chart = chart.y_axis_label("req/min");
                }
                self.specimen(
                    "enz_line_chart",
                    "Values over a continuous axis; each Series is one line.",
                    chart.into_element(),
                )
                .controls([
                    k.number_control("line.height", "Height", 140.0..=320.0, 20.0, 220.0),
                    k.flag_control("line.crosshair", "Crosshair", true),
                    k.flag_control("line.axis_label", "Y axis label", false),
                ])
                .code(
                    &code("enzui_widgets", "enz_line_chart")
                        .import("enzui_core::chart::{Series, SeriesId}")
                        .arg(quoted("Requests per minute"))
                        .set("push", "Series::from_points(SeriesId(0), points)")
                        .set("width", "Length::Fill")
                        .set("height", number(height))
                        .set_if(!crosshair, "crosshair", "false")
                        .set_if(axis, "y_axis_label", quoted("req/min")),
                )
                .into_element()
            },
            {
                let height = k.number("bar.height", 220.0);
                let bars = k.number("bar.count", 4.0) as usize;
                let data = [
                    ("Arch", 120.0),
                    ("Fedora", 80.0),
                    ("Debian", 95.0),
                    ("Ubuntu", 160.0),
                    ("NixOS", 70.0),
                    ("Alpine", 40.0),
                ];
                let chart = data
                    .iter()
                    .take(bars)
                    .fold(enz_bar_chart("Downloads by platform"), |chart, (label, value)| {
                        chart.bar(*label, *value)
                    })
                    .width(Length::Fill)
                    .height(height)
                    .selection(self.bar)
                    .on_select(Message::Bar);
                let bar_lines: String = data
                    .iter()
                    .take(bars)
                    .map(|(label, value)| format!("\n    .bar({}, {:.1})", quoted(label), value))
                    .collect();
                self.specimen(
                    "enz_bar_chart",
                    "Values by category. Click a bar to select it.",
                    chart.into_element(),
                )
                .controls([
                    k.number_control("bar.count", "Bars", 2.0..=6.0, 1.0, 4.0),
                    k.number_control("bar.height", "Height", 140.0..=320.0, 20.0, 220.0),
                ])
                .code_text(format!(
                    "use enzui_widgets::enz_bar_chart;\n\n\
                     enz_bar_chart(\"Downloads by platform\"){bar_lines}\n    \
                         .width(Length::Fill)\n    \
                         .height({})\n    \
                         .selection(self.bar)\n    \
                         .on_select(Message::Bar)\n    \
                         .into_element()",
                    number(height)
                ))
                .into_element()
            },
            {
                let smooth = k.pick("area.curve", 1) == 1;
                self.specimen(
                    "enz_area_chart",
                    "A line chart filled down to the axis, for amounts that add up over time.",
                    Element::from(
                        enzui_widgets::enz_area_chart("Memory use")
                            .push(Series::from_points(
                                SeriesId(0),
                                (0..24).map(|x| (f64::from(x), 45.0 + 15.0 * (f64::from(x) / 4.0).cos())),
                            ))
                            .curve(if smooth { ChartCurve::Monotone } else { ChartCurve::Linear })
                            .width(Length::Fill)
                            .height(200.0),
                    ),
                )
                .controls([k.pick_control("area.curve", "Curve", &["Linear", "Monotone"], 1)])
                .code(
                    &code("enzui_widgets", "enz_area_chart")
                        .import("enzui_widgets::area_chart::ChartCurve")
                        .arg(quoted("Memory use"))
                        .set("push", "Series::from_points(SeriesId(0), points)")
                        .set("curve", format!("ChartCurve::{}", if smooth { "Monotone" } else { "Linear" }))
                        .set("width", "Length::Fill")
                        .set("height", "200.0")
                        .end(None),
                )
                .into_element()
            },
            {
                let size = k.number("pie.size", 200.0);
                let donut = k.number("pie.donut", 0.0);
                let mut pie = enzui_widgets::enz_pie_chart(
                    "Disk usage",
                    [
                        enzui_widgets::pie_chart::PieSlice::new("docs", "Documents", 42.0),
                        enzui_widgets::pie_chart::PieSlice::new("media", "Media", 31.0),
                        enzui_widgets::pie_chart::PieSlice::new("apps", "Apps", 18.0),
                        enzui_widgets::pie_chart::PieSlice::new("other", "Other", 9.0),
                    ],
                )
                .size(size);
                if donut > 0.0 {
                    pie = pie.donut(donut);
                }
                self.specimen(
                    "enz_pie_chart",
                    "Shares of a whole. A donut ratio cuts out the middle.",
                    Element::from(pie),
                )
                .controls([
                    k.number_control("pie.size", "Size", 120.0..=280.0, 20.0, 200.0),
                    k.number_control("pie.donut", "Donut", 0.0..=0.8, 0.1, 0.0),
                ])
                .code(
                    &code("enzui_widgets", "enz_pie_chart")
                        .import("enzui_widgets::pie_chart::PieSlice")
                        .arg(quoted("Disk usage"))
                        .arg("[\n    PieSlice::new(\"docs\", \"Documents\", 42.0),\n    PieSlice::new(\"media\", \"Media\", 31.0),\n    PieSlice::new(\"apps\", \"Apps\", 18.0),\n]")
                        .set("size", number(size))
                        .set_if(donut > 0.0, "donut", number(donut))
                        .end(None),
                )
                .into_element()
            },
            self.specimen(
                "enz_scatter_chart",
                "Points on two axes, to show how two values relate.",
                Element::from(
                    enzui_widgets::enz_scatter_chart("Latency vs size")
                        .push(Series::from_points(
                            SeriesId(0),
                            (1..30).map(|x| (f64::from(x), f64::from(x) * 1.6 + f64::from((x * 7) % 11))),
                        ))
                        .width(Length::Fill)
                        .height(200.0),
                ),
            )
            .code(
                &code("enzui_widgets", "enz_scatter_chart")
                    .import("enzui_core::chart::{Series, SeriesId}")
                    .arg(quoted("Latency vs size"))
                    .set("push", "Series::from_points(SeriesId(0), points)")
                    .set("width", "Length::Fill")
                    .set("height", "200.0")
                    .end(None),
            )
            .into_element(),
            {
                let label = k.text("spark.label", "CPU load");
                self.specimen(
                    "enz_sparkline",
                    "A tiny trend line without axes, to sit beside a number or in a table cell.",
                    row![
                        enz_text("CPU").into_element(),
                        Element::from(
                            enzui_widgets::enz_sparkline(vec![3.0, 5.0, 4.0, 8.0, 6.0, 9.0, 7.0, 11.0, 8.0, 12.0])
                                .label(label),
                        ),
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                )
                .controls([k.text_control("spark.label", "Label (read aloud)", "CPU load")])
                .code(
                    &code("enzui_widgets", "enz_sparkline")
                        .arg("self.cpu_history.clone()")
                        .set("label", quoted(label))
                        .end(None),
                )
                .into_element()
            },
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }
}
