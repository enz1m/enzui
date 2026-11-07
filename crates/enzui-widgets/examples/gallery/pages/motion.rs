use crate::*;
use iced::widget::{column, row};

impl Gallery {
    pub(crate) fn motion_page(&self) -> Element<'_, Message> {
        const TRACK: f32 = 560.0;
        const DOT: f32 = 28.0;
        let now = self.now;
        let springs = self
            .springs
            .iter()
            .fold(column![].spacing(10), |list, (name, spring)| {
                // Each dot springs to where this view puts it: the app
                // sends no frame messages while it travels.
                let x = if self.springs_right { TRACK - DOT } else { 0.0 };
                let dot =
                    container(Space::new().width(DOT).height(DOT)).style(|theme: &iced::Theme| {
                        container::Style {
                            background: Some(
                                enzui_widgets::theme_lookup::palette(theme).accent.into(),
                            ),
                            border: iced::Border {
                                radius: (DOT / 2.0).into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    });
                let dot = enzui_widgets::motion::animated(dot)
                    .offset(iced::Vector::new(x, 0.0))
                    .spring(spring.spring());
                let track = container(dot)
                    .width(TRACK)
                    .clip(false)
                    .style(|theme: &iced::Theme| container::Style {
                        background: Some(
                            enzui_widgets::theme_lookup::palette(theme)
                                .text_muted
                                .scale_alpha(0.18)
                                .into(),
                        ),
                        border: iced::Border {
                            radius: (DOT / 2.0).into(),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                list.push(
                    row![
                        container(enz_text(*name).size(TextSize::Caption).into_element())
                            .width(132),
                        track
                    ]
                    .spacing(12)
                    .align_y(iced::Alignment::Center),
                )
            });
        let stage = iced::widget::canvas(FlingStage {
            card: self.fling.value(now),
        })
        .width(FLING_WIDTH)
        .height(FLING_HEIGHT);
        let panel = |label: &'static str| -> Element<'_, Message> {
            enz_card(
                container(enz_text(label).into_element())
                    .padding(12)
                    .width(150),
            )
            .into_element()
        };
        let reveal =
            |name: &'static str, effect: MotionEffect, curve: u8| -> Element<'_, Message> {
                let wrapped = motion(panel(name), self.panels_shown, effect);
                let wrapped = match curve {
                    1 => wrapped.expressive(Some(ExpressiveMotion::DefaultSpatial)),
                    2 => wrapped.spring(Some(ExpressiveMotion::FastSpatial.spring())),
                    _ => wrapped,
                };
                container(wrapped).width(170).height(64).into()
            };
        let controls = column![
            row![
                enz_checkbox(self.agree, "Checkbox", Message::Agree).into_element(),
                enz_toggle(self.notify, "Switch", Message::Notify).into_element(),
                enz_radio_group("Size", &SIZES, self.size, Message::Size).into_element(),
            ]
            .spacing(24)
            .align_y(iced::Alignment::Center),
            row![
                enzui_widgets::enz_button_group(
                    vec![
                        enzui_widgets::EnzButtonGroupItem::new("left", "Left"),
                        enzui_widgets::EnzButtonGroupItem::new("center", "Center"),
                        enzui_widgets::EnzButtonGroupItem::new("right", "Right"),
                    ],
                    Message::Align,
                )
                .selected(Some(self.align))
                .into_element(),
                enz_button("Change values", Message::NextValue)
                    .kind(ButtonKind::Secondary)
                    .into_element(),
            ]
            .spacing(16)
            .align_y(iced::Alignment::Center),
            row![
                container(enz_progress_bar(self.progress).busy(true).into_element()).width(220),
                container(enz_slider(0.0..=1.0, self.progress, Message::Progress).into_element())
                    .width(220),
                enz_gauge(self.progress).into_element(),
            ]
            .spacing(24)
            .align_y(iced::Alignment::Center),
        ]
        .spacing(16);
        let k = &self.knobs;
        let effect = k.pick("reveal.effect", 0);
        let (effect_value, effect_code) = match effect {
            1 => (MotionEffect::Fade, "MotionEffect::Fade"),
            2 => (MotionEffect::Scale, "MotionEffect::Scale"),
            3 => (
                MotionEffect::Slide(SlideDirection::Down),
                "MotionEffect::Slide(SlideDirection::Down)",
            ),
            _ => (
                MotionEffect::Slide(SlideDirection::Left),
                "MotionEffect::Slide(SlideDirection::Left)",
            ),
        };
        let curve = k.pick("reveal.curve", 2);
        let reveal_label = ["Slide", "Fade", "Scale", "Slide up"][effect];
        column![
            self.specimen(
                "Built-in widget motion",
                "Every control animates its own state changes on the theme's motion scale: hover a button, focus a field, check a box, or change the values. Reduce motion makes every change land at once, as the theme's reduced_motion does for a whole app.",
                column![
                    enz_toggle(self.reduce_motion, "Reduce motion", Message::ReduceMotion).into_element(),
                    controls,
                ]
                .spacing(16),
            )
            .code_text(
                "// Whole app: the theme's motion scale, or the desktop's reduced-motion setting.\n\
                 let _ = self.themes.set_reduced_motion_override(Some(true));\n\n\
                 // One widget: most builders take .reduced_motion(bool).\n\
                 enz_toggle(self.on, \"Wi-Fi\", Message::Wifi).reduced_motion(true)\n\n\
                 // A progress bar that jumps instead of gliding:\n\
                 enz_progress_bar(self.done).animated(false)",
            )
            .into_element(),
            self.specimen(
                "animated(..).spring(..)",
                "Each dot runs on a different spring. Press the button again mid-flight: every dot turns around with the speed it had. The app only sets the target; the widget animates without frame messages.",
                column![
                    enz_button(
                        if self.springs_right { "Send back" } else { "Send across" },
                        Message::SpringsGo
                    )
                    .into_element(),
                    springs,
                ]
                .spacing(12),
            )
            .code_text(
                "use enzui_theme::ExpressiveMotion;\n\
                 use enzui_widgets::motion::animated;\n\n\
                 animated(dot)\n    \
                     .offset(Vector::new(if self.right { 532.0 } else { 0.0 }, 0.0))\n    \
                     .spring(ExpressiveMotion::DefaultSpatial.spring())",
            )
            .into_element(),
            self.specimen(
                "SpringAnimation",
                "Drag the card and let go: it carries your speed to the nearer side. Catch it mid-flight. The app owns the spring and reads its value each frame.",
                container(stage).style(container::bordered_box),
            )
            .code_text(
                "use enzui_core::velocity::VelocityTracker;\n\
                 use enzui_widgets::motion::SpringAnimation;\n\n\
                 // On release: hand the pointer's speed to the spring.\n\
                 let [vx, vy] = self.tracker.velocity(now);\n\
                 self.card.release(from, Point::new(vx, vy), target, now);\n\n\
                 // In view: draw where the spring is now.\n\
                 let at = self.card.value(now);",
            )
            .into_element(),
            self.specimen(
                "motion(child, shown, effect)",
                "Shows and hides content with an effect: on the theme curve, an expressive curve, or a spring. Toggle twice quickly to see the spring reverse without stopping.",
                column![
                    enz_button(
                        if self.panels_shown { "Hide panels" } else { "Show panels" },
                        Message::RevealPanels
                    )
                    .kind(ButtonKind::Secondary)
                    .into_element(),
                    row![
                        column![enz_text("Theme curve").size(TextSize::Caption).into_element(), reveal(reveal_label, effect_value, 0)].spacing(6),
                        column![enz_text("Expressive").size(TextSize::Caption).into_element(), reveal(reveal_label, effect_value, 1)].spacing(6),
                        column![enz_text("Spring").size(TextSize::Caption).into_element(), reveal(reveal_label, effect_value, 2)].spacing(6),
                    ]
                    .spacing(16),
                ]
                .spacing(12),
            )
            .controls([
                k.pick_control("reveal.effect", "Effect", &["Slide", "Fade", "Scale", "Slide up"], 0),
                k.pick_control("reveal.curve", "Curve in the code", &["Theme", "Expressive", "Spring"], 2),
            ])
            .code(
                &code("enzui_widgets::motion", "motion")
                    .import("enzui_widgets::motion::{MotionEffect, SlideDirection}")
                    .import_if(curve != 0, "enzui_theme::ExpressiveMotion")
                    .arg("panel")
                    .arg("self.panels_shown")
                    .arg(effect_code)
                    .set_if(curve == 1, "expressive", "Some(ExpressiveMotion::DefaultSpatial)")
                    .set_if(curve == 2, "spring", "Some(ExpressiveMotion::FastSpatial.spring())")
                    .end(None),
            )
            .into_element(),
        ]
        .spacing(32)
        .into()
    }
}
