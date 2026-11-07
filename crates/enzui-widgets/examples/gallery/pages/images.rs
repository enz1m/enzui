use crate::*;
use enzui_widgets::image::{EnzImageFit, enz_image_handle};
use iced::widget::{column, row};

const FITS: [EnzImageFit; 4] = [
    EnzImageFit::Cover,
    EnzImageFit::Contain,
    EnzImageFit::Fill,
    EnzImageFit::None,
];
const FIT_NAMES: &[&str] = &["Cover", "Contain", "Fill", "None"];
const SVGS: [(&str, &[u8], bool); 3] = [
    ("Illustration", include_bytes!("../../app.svg"), false),
    ("Symbolic", SYMBOLIC_SVG.as_bytes(), true),
    ("Translucent", TRANSLUCENT_SVG.as_bytes(), false),
];

impl Gallery {
    pub(crate) fn images_page(&self) -> Element<'_, Message> {
        column![
            self.image_specimen(),
            self.svg_specimen(),
            self.animated_svg_specimen(),
            self.specimen(
                "enz_animated_image",
                "A GIF or APNG played on its own frame delays; it holds still with reduced motion or when paused.",
                enzui_widgets::animated_image::enz_animated_image(&self.spinner)
                    .width(48)
                    .height(48)
                    .playing(self.svg_playing)
                    .alt_text("A dot going round")
                    .into_element(),
            )
            .controls([self.knobs_playing()])
            .code(
                &code("enzui_widgets::animated_image", "enz_animated_image")
                    .arg("&self.frames")
                    .set("width", "48")
                    .set("height", "48")
                    .set_if(!self.svg_playing, "playing", "false")
                    .set("alt_text", quoted("A dot going round")),
            )
            .into_element(),
        ]
        .spacing(32)
        .into()
    }

    /// The shared play/pause switch of the animated pictures.
    fn knobs_playing(&self) -> Element<'static, Message> {
        enz_toggle(self.svg_playing, "Playing", Message::SvgPlaying).into_element()
    }

    fn image_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let fit = k.pick("image.fit", 0);
        let radius = k.number("image.radius", 12.0);
        let width = k.number("image.width", 240.0);
        self.specimen(
            "enz_image_handle",
            "A raster picture. The fit says how it fills its box when the shapes differ; the box here is 140 px tall.",
            enz_image_handle(self.photo.clone())
                .fit(FITS[fit])
                .radius(radius)
                .width(Length::Fixed(width))
                .height(Length::Fixed(140.0))
                .placeholder_background(iced::Color::from_rgba(0.5, 0.5, 0.5, 0.15))
                .alt_text("A generated landscape")
                .into_element(),
        )
        .controls([
            k.pick_control("image.fit", "Fit", FIT_NAMES, 0),
            k.number_control("image.radius", "Radius", 0.0..=40.0, 2.0, 12.0),
            k.number_control("image.width", "Width", 120.0..=480.0, 20.0, 240.0),
        ])
        .code(
            &code("enzui_widgets::image", "enz_image_handle")
                .arg("self.photo.clone()")
                .variant(fit != 0, "fit", "enzui_widgets::image", "EnzImageFit", FIT_NAMES[fit])
                .set_if(radius > 0.0, "radius", number(radius))
                .set("width", format!("Length::Fixed({})", number(width)))
                .set("height", "Length::Fixed(140.0)")
                .set("alt_text", quoted("A generated landscape")),
        )
        .into_element()
    }

    fn svg_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let which = k.pick("svg.which", 0);
        let size = k.number("svg.size", 96.0);
        let (name, bytes, symbolic_by_default) = SVGS[which];
        let symbolic = k.flag("svg.symbolic", false) || symbolic_by_default;
        self.specimen(
            "enz_svg",
            "A vector picture that stays sharp at any size. A symbolic one takes the text colour, so it follows the theme.",
            row![
                enzui_widgets::svg_image::enz_svg(iced::widget::svg::Handle::from_memory(bytes))
                    .width(size)
                    .height(size)
                    .symbolic(symbolic)
                    .label(name)
            ],
        )
        .controls([
            k.pick_control("svg.which", "Picture", &["Illustration", "Symbolic", "Translucent"], 0),
            k.number_control("svg.size", "Size", 32.0..=192.0, 8.0, 96.0),
            k.flag_control("svg.symbolic", "Symbolic", false),
        ])
        .code(
            &code("enzui_widgets::svg_image", "enz_svg")
                .arg("svg::Handle::from_memory(bytes)")
                .import("iced::widget::svg")
                .set("width", number(size))
                .set("height", number(size))
                .set_if(symbolic, "symbolic", "true")
                .set("label", quoted(name))
                .end(None),
        )
        .into_element()
    }

    fn animated_svg_specimen(&self) -> Element<'_, Message> {
        let size = self.knobs.number("asvg.size", 80.0);
        let animated = self
            .svgs
            .iter()
            .fold(row![].spacing(24), |row, (name, svg)| {
                row.push(
                    column![
                        enz_animated_svg(svg)
                            .width(size)
                            .height(size)
                            .playing(self.svg_playing)
                            .label(*name),
                        enz_text(*name).size(TextSize::Caption).into_element(),
                    ]
                    .spacing(6)
                    .align_x(iced::Alignment::Center),
                )
            });
        self.specimen(
            "enz_animated_svg",
            "SVG with SMIL and CSS animation. Each picture redraws only when it changes, and stops when hidden or paused. The last square starts when clicked.",
            animated.wrap(),
        )
        .controls([
            self.knobs_playing(),
            self.knobs.number_control("asvg.size", "Size", 48.0..=128.0, 8.0, 80.0),
        ])
        .code(
            &code("enzui_widgets::animated_svg", "enz_animated_svg")
                .arg("&self.loader")
                .set("width", number(size))
                .set("height", number(size))
                .set_if(!self.svg_playing, "playing", "false")
                .set("label", quoted("Loading"))
                .end(None),
        )
        .into_element()
    }
}
