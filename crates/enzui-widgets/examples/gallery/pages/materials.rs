use crate::*;
use enzui_widgets::surface::{SurfaceTone, enz_surface};
use iced::widget::{column, row};

impl Gallery {
    pub(crate) fn materials_page(&self) -> Element<'_, Message> {
        let blob = |size: f32, color: iced::Color| -> Element<'_, Message> {
            container(Space::new().width(size).height(size))
                .style(move |_| container::Style {
                    background: Some(color.into()),
                    border: iced::Border {
                        radius: (size / 2.0).into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into()
        };
        let dropped = |top: f32, blob: Element<'static, Message>| -> Element<'static, Message> {
            column![Space::new().height(top), blob].into()
        };
        let k = &self.knobs;
        let radius = k.number("glass.radius", 16.0);
        let tint = k.number("glass.tint", 0.0);
        let level = k.number("elevation.level", 2.0) as u8;
        let angle = k.number("gradient.angle", 135.0);
        let mid = k.flag("gradient.mid", true);
        let glass = |style: enzui_theme::MaterialStyle,
                     name: &'static str,
                     note: &'static str|
         -> Element<'_, Message> {
            enzui_widgets::glass::enz_glass(
                container(
                    column![
                        enz_text(name).size(TextSize::Headline).into_element(),
                        enz_text(note).size(TextSize::Caption).into_element(),
                    ]
                    .spacing(4),
                )
                .padding(16)
                .width(Length::Fixed(156.0)),
            )
            .style(style)
            .radius(radius)
            .tint_opacity(tint)
            .into_element()
        };
        let backdrop = iced::widget::stack![
            enzui_widgets::gradient_background::enz_gradient_background(
                Space::new().width(Length::Fill).height(Length::Fill)
            )
            .color_start(iced::Color::from_rgb8(0x1E, 0x3C, 0x72))
            .color_mid(iced::Color::from_rgb8(0x6A, 0x3D, 0x9A))
            .color_end(iced::Color::from_rgb8(0xE0, 0x6C, 0x75))
            .show_mid(mid)
            .angle_degrees(angle)
            .border_radius(16.0)
            .width(Length::Fill)
            .height(Length::Fill)
            .into_element(),
            // A scattered field of colour, so each card has edges,
            // highlights and flat areas behind it.
            row![
                dropped(16.0, blob(150.0, iced::Color::from_rgb8(0xF6, 0xD3, 0x2D))),
                Space::new().width(Length::Fill),
                dropped(150.0, blob(96.0, iced::Color::from_rgb8(0xFF, 0x7A, 0x59))),
                Space::new().width(Length::Fill),
                dropped(8.0, blob(124.0, iced::Color::from_rgb8(0x2E, 0xC2, 0xB4))),
                Space::new().width(Length::Fill),
                dropped(130.0, blob(160.0, iced::Color::from_rgb8(0x34, 0xC7, 0x59))),
                Space::new().width(Length::Fill),
                dropped(36.0, blob(104.0, iced::Color::from_rgb8(0x5A, 0xC8, 0xFA))),
            ]
            .padding([0, 24])
            .height(Length::Fill),
            container(
                row![
                    glass(
                        enzui_theme::MaterialStyle::Regular,
                        "Regular",
                        "Clear, bends light at the rim"
                    ),
                    glass(
                        enzui_theme::MaterialStyle::Subtle,
                        "Subtle",
                        "A light frost"
                    ),
                    glass(
                        enzui_theme::MaterialStyle::Frosted,
                        "Frosted",
                        "Frosted, made for text"
                    ),
                    glass(
                        enzui_theme::MaterialStyle::Clear,
                        "Clear",
                        "Media glass, dims highlights"
                    ),
                ]
                .spacing(16)
                .wrap()
                .vertical_spacing(16)
            )
            .padding(24)
            .center(Length::Fill),
        ]
        .height(Length::Fixed(300.0));
        let grain_specimen = self.grain_specimen();
        column![
                    grain_specimen,
                    self.specimen(
                        "enz_glass",
                        "A material that samples what is behind it: refraction at the rim, frost and tint over a busy background. The style picks how much it hides.",
                        backdrop,
                    )
                    .controls([
                        k.number_control("glass.radius", "Radius", 0.0..=32.0, 2.0, 16.0),
                        k.number_control("glass.tint", "Tint opacity", 0.0..=0.6, 0.05, 0.0),
                    ])
                    .code(
                        &code("enzui_widgets::glass", "enz_glass")
                            .import("enzui_theme::MaterialStyle")
                            .arg("container(content).padding(16)")
                            .set("style", "MaterialStyle::Frosted")
                            .set("radius", number(radius))
                            .set_if(tint > 0.0, "tint_opacity", number(tint)),
                    )
                    .into_element(),
                    self.specimen(
                        "enz_gradient_background",
                        "A linear gradient of two or three colours behind any content; the glass above sits on one.",
                        enzui_widgets::gradient_background::enz_gradient_background(
                            Space::new().width(Length::Fill).height(Length::Fill),
                        )
                        .color_start(iced::Color::from_rgb8(0x1E, 0x3C, 0x72))
                        .color_mid(iced::Color::from_rgb8(0x6A, 0x3D, 0x9A))
                        .color_end(iced::Color::from_rgb8(0xE0, 0x6C, 0x75))
                        .show_mid(mid)
                        .angle_degrees(angle)
                        .border_radius(12.0)
                        .width(Length::Fill)
                        .height(Length::Fixed(120.0))
                        .into_element(),
                    )
                    .controls([
                        k.number_control("gradient.angle", "Angle", 0.0..=360.0, 15.0, 135.0),
                        k.flag_control("gradient.mid", "Middle colour", true),
                    ])
                    .code(
                        &code("enzui_widgets::gradient_background", "enz_gradient_background")
                            .arg("content")
                            .set("color_start", "Color::from_rgb8(0x1E, 0x3C, 0x72)")
                            .set_if(mid, "color_mid", "Color::from_rgb8(0x6A, 0x3D, 0x9A)")
                            .set("color_end", "Color::from_rgb8(0xE0, 0x6C, 0x75)")
                            .set_if(!mid, "show_mid", "false")
                            .set("angle_degrees", number(angle)),
                    )
                    .into_element(),
                    self.specimen(
                        "enz_elevation",
                        "Lifts content off the page with a shadow, from level 0 (flat) to 5.",
                        row![
                            enzui_widgets::elevation::enz_elevation(
                                // The elevation card is the surface; its content
                                // only sets the size.
                                container(enz_text(format!("Level {level}")).into_element())
                                    .padding(16)
                                    .center_x(Length::Fixed(120.0)),
                            )
                            .level(level)
                            .into_element()
                        ]
                        .padding(16),
                    )
                    .controls([k.number_control("elevation.level", "Level", 0.0..=5.0, 1.0, 2.0)])
                    .code(
                        &code("enzui_widgets::elevation", "enz_elevation")
                            .arg("container(content).padding(16)")
                            .set("level", level.to_string()),
                    )
                    .into_element(),
                ]
                .spacing(32)
                .width(Length::Fill)
                .into()
    }

    fn grain_specimen(&self) -> Element<'_, Message> {
        use enzui_theme::{Grain, GrainMapping, GrainPlacement, GrainTiling, GrainTone};
        const PRESETS: &[&str] = &["Default", "Subtle", "Paper", "Film", "Halftone SVG"];
        const TONES: &[&str] = &["Both", "Dark", "Light"];
        const PLACEMENTS: &[&str] = &["Behind", "Over"];
        let k = &self.knobs;
        let preset = k.pick("grain.preset", 0);
        let base = match preset {
            1 => Grain::subtle(),
            2 => Grain::paper(),
            3 => Grain::film(),
            4 => Grain::svg(HALFTONE_SVG)
                .mapping(GrainMapping::Artwork)
                .tiling(GrainTiling::Mirror),
            _ => Grain::default(),
        };
        let intensity = k.number("grain.intensity", base.intensity);
        let density = k.number("grain.density", base.density);
        let scale = k.number("grain.scale", base.scale);
        let opacity = k.number("grain.opacity", 1.0);
        let tone = k.pick("grain.tone", 0);
        let placement = k.pick(
            "grain.placement",
            usize::from(base.placement == GrainPlacement::Over),
        );
        let tinted = k.flag("grain.tint", false);
        let accent = enzui_theme::Palette::dark().accent;
        let mut grain = base
            .intensity(intensity)
            .density(density)
            .scale(scale)
            .opacity(opacity)
            .tone([GrainTone::Both, GrainTone::Dark, GrainTone::Light][tone.min(2)])
            .placement([GrainPlacement::Behind, GrainPlacement::Over][placement.min(1)]);
        if tinted {
            grain = grain.tint(accent);
        }
        let panel = |title: &'static str, note: &'static str, tone: SurfaceTone| {
            enz_surface(
                column![
                    enz_text(title).size(TextSize::Headline).into_element(),
                    enz_text(note).size(TextSize::Footnote).into_element(),
                ]
                .spacing(4),
            )
            .tone(tone)
            .padding(20)
            .radius(16.0)
            .grain(grain)
            .width(Length::Fill)
            .height(Length::Fixed(200.0))
            .into_element()
        };
        let on_accent = enzui_theme::Palette::dark().on_accent();
        let card = enz_surface(
            column![
                enz_text("Accent card")
                    .size(TextSize::Headline)
                    .color(on_accent)
                    .into_element(),
                enz_text("Grain reads on any fill")
                    .size(TextSize::Footnote)
                    .color(on_accent)
                    .into_element(),
            ]
            .spacing(4),
        )
        .fill_resolved(|theme| enzui_widgets::theme_lookup::palette(theme).accent)
        .padding(20)
        .radius(16.0)
        .grain(grain)
        .width(Length::Fill)
        .height(Length::Fixed(200.0))
        .into_element();
        let picture = enzui_widgets::grain::enz_grain(
            enzui_widgets::gradient_background::enz_gradient_background(
                container(
                    enz_text("Over a picture")
                        .size(TextSize::Headline)
                        .color(iced::Color::WHITE)
                        .into_element(),
                )
                .padding(20),
            )
            .color_start(iced::Color::from_rgb8(0x1E, 0x3C, 0x72))
            .color_mid(iced::Color::from_rgb8(0x6A, 0x3D, 0x9A))
            .color_end(iced::Color::from_rgb8(0xE0, 0x6C, 0x75))
            .angle_degrees(135.0)
            .border_radius(16.0)
            .width(Length::Fill)
            .height(Length::Fixed(200.0))
            .into_element(),
        )
        .grain(grain)
        .radius(16.0)
        .into_element();
        let preview = row![
            panel("Page", "enz_surface(..).grain(..)", SurfaceTone::Background),
            panel(
                "Sidebar tone",
                "PaneStyle::grain for shell panes",
                SurfaceTone::SurfaceAlt
            ),
            card,
            picture,
        ]
        .spacing(16);
        let preset_call = match preset {
            1 => "Grain::subtle()",
            2 => "Grain::paper()",
            3 => "Grain::film()",
            4 => "Grain::svg(HALFTONE_SVG)",
            _ => "Grain::default()",
        };
        let mut steps: Vec<String> = vec![preset_call.to_owned()];
        if preset == 4 {
            steps.push(".mapping(GrainMapping::Artwork)".to_owned());
            steps.push(".tiling(GrainTiling::Mirror)".to_owned());
        }
        if (intensity - base.intensity).abs() > 1e-3 {
            steps.push(format!(".intensity({})", number(intensity)));
        }
        if (density - base.density).abs() > 1e-3 {
            steps.push(format!(".density({})", number(density)));
        }
        if (scale - base.scale).abs() > 1e-3 {
            steps.push(format!(".scale({})", number(scale)));
        }
        if opacity < 1.0 {
            steps.push(format!(".opacity({})", number(opacity)));
        }
        if tone > 0 {
            steps.push(format!(".tone(GrainTone::{})", TONES[tone.min(2)]));
        }
        if usize::from(base.placement == GrainPlacement::Over) != placement {
            steps.push(format!(
                ".placement(GrainPlacement::{})",
                PLACEMENTS[placement.min(1)]
            ));
        }
        if tinted {
            steps.push(".tint(palette.accent)".to_owned());
        }
        let grain_code = steps.join("\n        ");
        self.specimen(
            "grain",
            "A paper-like speckle on any surface: enz_surface(..).grain(..), a shell pane's PaneStyle::grain, or enz_grain around anything. The built-in grain is a seamless noise tile rasterised once per screen scale; any SVG can be a grain too. GrainSpec::Theme follows the theme's own grain.",
            preview,
        )
        .controls([
            k.pick_control("grain.preset", "Grain", PRESETS, 0),
            k.number_control("grain.intensity", "Intensity", 0.0..=0.6, 0.02, base.intensity),
            k.number_control("grain.density", "Density", 0.0..=1.0, 0.05, base.density),
            k.number_control("grain.scale", "Scale", 0.5..=4.0, 0.25, base.scale),
            k.number_control("grain.opacity", "Opacity", 0.0..=1.0, 0.05, 1.0),
            k.pick_control("grain.tone", "Tone", TONES, 0),
            k.pick_control(
                "grain.placement",
                "Placement",
                PLACEMENTS,
                usize::from(base.placement == GrainPlacement::Over),
            ),
            k.flag_control("grain.tint", "Accent tint", false),
        ])
        .code(
            &code("enzui_widgets::surface", "enz_surface")
                .import("enzui_theme::Grain")
                .import_if(preset == 4, "enzui_theme::{GrainMapping, GrainTiling}")
                .import_if(tone > 0, "enzui_theme::GrainTone")
                .import_if(
                    usize::from(base.placement == GrainPlacement::Over) != placement,
                    "enzui_theme::GrainPlacement",
                )
                .arg("content")
                .set("padding", "20")
                .set("grain", grain_code),
        )
        .into_element()
    }
}

/// A halftone dot screen, one 12-unit cell: hand-made speckle art for a
/// custom SVG grain.
const HALFTONE_SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 12 12'>\
<circle cx='3' cy='3' r='1.3' fill='#000' fill-opacity='0.55'/>\
<circle cx='9' cy='9' r='1.3' fill='#000' fill-opacity='0.55'/>\
<circle cx='9' cy='3' r='0.7' fill='#fff' fill-opacity='0.6'/>\
<circle cx='3' cy='9' r='0.7' fill='#fff' fill-opacity='0.6'/>\
</svg>";
