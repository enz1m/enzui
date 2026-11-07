//! Headless color-presence measurements with a rendered positive control.
//!
//! [`ColorProbe`] renders the subject twice to check repeatability and renders
//! a control under the same [`EnzTheme`]. The control must reach the matching-pixel
//! floor before either presence or absence is reported. Use the same subtree
//! with its tone changed so the control exercises the subject's color path.
//!
//! Presence means at least the configured floor of matching pixels; absence
//! means fewer, rather than necessarily zero. Untouched subjects are inconclusive
//! unless [`ColorProbe::allow_nothing_painted`] is enabled. [`sample`] and [`verdict`]
//! support caller-owned frames with explicit [`palette_provenance`].

use enzui_theme::{EnzTheme, Palette};
use iced::advanced::layout::Measure as _;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, layout, renderer};
use iced::{Color, Element, Length, Rectangle, Size};

/// Default RGB L1 matching tolerance, 40 on a sum-of-channel-differences scale of 0..=765.
pub const DEFAULT_TOLERANCE: u16 = 40;

/// Default maximum-channel distance from the clear color required to count a painted pixel.
pub const DEFAULT_INK_DELTA: u8 = 12;

/// What one region of one frame contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Pixels examined within the clipped region.
    pub examined: usize,
    /// Pixels that differ from the clear colour by at least the ink delta.
    pub painted: usize,
    /// Pixels within tolerance of the asserted colour.
    pub matching: usize,
    /// Most frequent painted RGB value, if any.
    pub dominant: Option<[u8; 3]>,
    /// Most frequent matching RGB value, if any.
    pub dominant_matching: Option<[u8; 3]>,
}

impl Sample {
    /// A human-readable summary, naming the ink it measured.
    pub fn describe(&self) -> String {
        let name = |c: Option<[u8; 3]>| match c {
            Some([r, g, b]) => format!("({r}, {g}, {b})"),
            None => "none".to_owned(),
        };
        format!(
            "{} matching px (dominant match {}) / {} painted px (dominant ink {}) / \
             {} examined",
            self.matching,
            name(self.dominant_matching),
            self.painted,
            name(self.dominant),
            self.examined
        )
    }
}

/// Measures a rectangle of an RGBA8 frame with the supplied pixel width.
///
/// The region is clipped to complete frame rows, with fractional edges rounded outward.
/// RGB matching uses an inclusive L1 tolerance and ignores alpha. Painted pixels differ
/// from `clear` by at least [`DEFAULT_INK_DELTA`] on any channel; matching pixels can
/// include clear pixels. A zero width examines no pixels.
pub fn sample(
    frame: &[u8],
    width: u32,
    region: Rectangle,
    colour: Color,
    tolerance: u16,
    clear: Color,
) -> Sample {
    let target = rgb8(colour);
    let clear8 = rgb8(clear);
    let height = if width == 0 {
        0
    } else {
        (frame.len() / 4 / width as usize) as u32
    };
    let x0 = region.x.max(0.0) as u32;
    let y0 = region.y.max(0.0) as u32;
    let x1 = ((region.x + region.width).max(0.0).ceil() as u32).min(width);
    let y1 = ((region.y + region.height).max(0.0).ceil() as u32).min(height);

    let mut out = Sample {
        examined: 0,
        painted: 0,
        matching: 0,
        dominant: None,
        dominant_matching: None,
    };
    let mut histogram: Vec<([u8; 3], usize)> = Vec::new();
    let mut matches: Vec<([u8; 3], usize)> = Vec::new();
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * width + x) * 4) as usize;
            if i + 2 >= frame.len() {
                continue;
            }
            let px = [frame[i], frame[i + 1], frame[i + 2]];
            out.examined += 1;
            let d = u16::from(px[0].abs_diff(target[0]))
                + u16::from(px[1].abs_diff(target[1]))
                + u16::from(px[2].abs_diff(target[2]));
            if d <= tolerance {
                out.matching += 1;
                match matches.iter_mut().find(|(c, _)| *c == px) {
                    Some((_, n)) => *n += 1,
                    None => matches.push((px, 1)),
                }
            }
            let from_clear = px[0]
                .abs_diff(clear8[0])
                .max(px[1].abs_diff(clear8[1]))
                .max(px[2].abs_diff(clear8[2]));
            if from_clear >= DEFAULT_INK_DELTA {
                out.painted += 1;
                match histogram.iter_mut().find(|(c, _)| *c == px) {
                    Some((_, n)) => *n += 1,
                    None => histogram.push((px, 1)),
                }
            }
        }
    }
    out.dominant = histogram.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c);
    out.dominant_matching = matches.iter().max_by_key(|(_, n)| *n).map(|(c, _)| *c);
    out
}

/// Identifies whether a runtime theme carries the asserted enzUI palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteProvenance {
    /// The carried palette matches the asserted Debug representation.
    Own,
    /// The theme transports an enzUI palette, but a different one — a light
    /// theme measured against dark tokens, or another brand.
    OtherEnzPalette,
    /// The theme carries no enzUI palette payload.
    StockDerived,
}

impl PaletteProvenance {
    /// `true` only for [`PaletteProvenance::Own`].
    pub fn is_own(&self) -> bool {
        matches!(self, PaletteProvenance::Own)
    }
}

/// Compares the palette carried by `theme` with `asserted` using their complete Debug representations.
/// Themes without an enzUI payload return [`PaletteProvenance::StockDerived`].
pub fn palette_provenance(theme: &iced::Theme, asserted: &Palette) -> PaletteProvenance {
    let Some(carried) = EnzTheme::from_iced(theme) else {
        return PaletteProvenance::StockDerived;
    };
    if format!("{:?}", carried.palette) == format!("{asserted:?}") {
        PaletteProvenance::Own
    } else {
        PaletteProvenance::OtherEnzPalette
    }
}

/// A condition that prevents the probe from reporting a measured result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blind {
    /// The subject has no pixels differing sufficiently from the clear color.
    NothingPainted {
        /// Pixels examined (0 means the region itself was empty).
        examined: usize,
    },
    /// The control has fewer matching pixels than the configured floor.
    ControlBlind {
        /// What the control actually painted.
        control: Sample,
        /// Matching pixels the control had to reach.
        floor: usize,
    },
    /// The frame was painted under a theme that does not carry the palette
    /// being asserted.
    ForeignPalette(PaletteProvenance),
    /// Repeated subject renders differ.
    Nondeterministic {
        /// Pixels that differ between two identical renders.
        differing: usize,
    },
}

/// A color-presence result at the configured matching-pixel floor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColorPresence {
    /// The subject reaches the matching-pixel floor.
    Present {
        /// The subject's measurement.
        subject: Sample,
    },
    /// The subject is below the matching-pixel floor, with a control that reaches it.
    Absent {
        /// The subject's measurement.
        subject: Sample,
        /// The control's measurement.
        control: Sample,
    },
    /// The question cannot be answered by this probe. Never a pass.
    Inconclusive(Blind),
}

impl ColorPresence {
    /// `true` only for a measured [`ColorPresence::Absent`].
    pub fn is_absent(&self) -> bool {
        matches!(self, ColorPresence::Absent { .. })
    }

    /// `true` only for a measured [`ColorPresence::Present`].
    pub fn is_present(&self) -> bool {
        matches!(self, ColorPresence::Present { .. })
    }

    /// A report that names the measured ink, for a test's failure message.
    pub fn report(&self, what: &str) -> String {
        match self {
            ColorPresence::Present { subject } => {
                format!("{what}: the colour IS present — {}", subject.describe())
            }
            ColorPresence::Absent { subject, control } => format!(
                "{what}: the colour is absent — {} (control: {})",
                subject.describe(),
                control.describe()
            ),
            ColorPresence::Inconclusive(Blind::NothingPainted { examined }) => format!(
                "INCONCLUSIVE — {what}: {examined} px examined and NOTHING painted. \
                 A pixel probe cannot tell \"nothing is drawn here\" from \"the \
                 renderer cannot draw it\"; this assertion cannot fail. Give the \
                 probe a control that paints, or opt out with \
                 `allow_nothing_painted(reason)`."
            ),
            ColorPresence::Inconclusive(Blind::ControlBlind { control, floor }) => format!(
                "INCONCLUSIVE — {what}: the control painted only {} matching px \
                 (floor {floor}) — {}. The subject's absence of the colour is \
                 unconditionally satisfied, so this assertion cannot fail. \
                 Check that the control resolves the colour through the SAME \
                 path as the subject (a theme-resolved tone, not a literal \
                 raster) and that the frame's theme carries your palette.",
                control.matching,
                control.describe()
            ),
            ColorPresence::Inconclusive(Blind::ForeignPalette(p)) => format!(
                "INCONCLUSIVE — {what}: the frame's theme has palette provenance {p:?}, \
                 so it does not carry the palette being asserted. Theme-resolved \
                 colors may differ. Draw with the asserted `EnzTheme::iced_theme()`."
            ),
            ColorPresence::Inconclusive(Blind::Nondeterministic { differing }) => format!(
                "INCONCLUSIVE — {what}: two identical renders differed in \
                 {differing} px, so no colour count from this frame means \
                 anything."
            ),
        }
    }

    /// Requires an absent verdict and includes measured ink in the failure report.
    ///
    /// # Panics
    ///
    /// Panics for present or inconclusive verdicts.
    #[track_caller]
    pub fn assert_absent(&self, what: &str) {
        assert!(self.is_absent(), "{}", self.report(what));
    }

    /// Requires a present verdict and includes measured ink in the failure report.
    ///
    /// # Panics
    ///
    /// Panics for absent or inconclusive verdicts.
    #[track_caller]
    pub fn assert_present(&self, what: &str) {
        assert!(self.is_present(), "{}", self.report(what));
    }
}

/// Classifies samples using a shared matching-pixel floor for control and subject.
///
/// Foreign provenance, insufficient control matches, and an unpainted subject are
/// inconclusive, in that order. Otherwise the subject is present at or above
/// `control_floor`, absent below it. Zero is accepted and removes the control threshold.
pub fn verdict(
    subject: Sample,
    control: Sample,
    provenance: &PaletteProvenance,
    control_floor: usize,
) -> ColorPresence {
    if !provenance.is_own() {
        return ColorPresence::Inconclusive(Blind::ForeignPalette(provenance.clone()));
    }
    if control.matching < control_floor {
        return ColorPresence::Inconclusive(Blind::ControlBlind {
            control,
            floor: control_floor,
        });
    }
    if subject.painted == 0 {
        return ColorPresence::Inconclusive(Blind::NothingPainted {
            examined: subject.examined,
        });
    }
    if subject.matching >= control_floor {
        ColorPresence::Present { subject }
    } else {
        ColorPresence::Absent { subject, control }
    }
}

/// Renders repeatable subject and control samples under an explicit enzUI theme.
#[derive(Debug, Clone)]
#[must_use]
pub struct ColorProbe {
    size: Size<u32>,
    theme: EnzTheme,
    region: Option<Rectangle>,
    tolerance: u16,
    control_floor: usize,
    allow_empty: Option<&'static str>,
}

impl ColorProbe {
    /// Creates a 96×120 pixel canvas with the theme background, whole-canvas sampling,
    /// default tolerance, and a matching-pixel floor of 20.
    pub fn new(theme: EnzTheme) -> Self {
        Self {
            size: Size::new(96, 120),
            theme,
            region: None,
            tolerance: DEFAULT_TOLERANCE,
            control_floor: 20,
            allow_empty: None,
        }
    }

    /// Canvas size in physical pixels.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Size::new(width, height);
        self
    }

    /// Samples this rectangle in canvas pixels instead of the whole canvas.
    /// The screenshot uses scale 1.0, so logical and physical coordinates coincide.
    pub fn region(mut self, region: Rectangle) -> Self {
        self.region = Some(region);
        self
    }

    /// L1 colour tolerance (default [`DEFAULT_TOLERANCE`]).
    pub fn tolerance(mut self, tolerance: u16) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Sets the shared matching-pixel threshold for control validity and subject presence, defaulting to 20.
    /// Zero is accepted and removes the control threshold.
    pub fn control_floor(mut self, floor: usize) -> Self {
        self.control_floor = floor;
        self
    }

    /// Treats an unpainted subject as absent after the control and provenance checks pass.
    /// The reason is accepted for call-site documentation but is not included in reports.
    pub fn allow_nothing_painted(mut self, reason: &'static str) -> Self {
        self.allow_empty = Some(reason);
        self
    }

    /// Renders the subject twice and the control once under the probe theme.
    ///
    /// A differing subject repeat returns [`Blind::Nondeterministic`] without rendering
    /// the control. Otherwise [`verdict`] classifies samples in the configured region,
    /// with the empty-subject override applied afterward. Factories must produce stable
    /// content; the control should exercise the same color path as the subject.
    ///
    /// # Panics
    ///
    /// Panics if headless renderer initialization fails.
    pub fn run<'a, Message: 'a>(
        &self,
        colour: Color,
        mut subject: impl FnMut() -> Element<'a, Message>,
        mut control: impl FnMut() -> Element<'a, Message>,
    ) -> ColorPresence {
        let theme = self.theme.iced_theme();
        let clear = self.theme.palette.background;
        let frame = self.frame(subject(), &theme, clear);
        let again = self.frame(subject(), &theme, clear);
        let differing = frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(again.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count();
        if differing > 0 {
            return ColorPresence::Inconclusive(Blind::Nondeterministic { differing });
        }
        let control_frame = self.frame(control(), &theme, clear);

        let region = self.region.unwrap_or_else(|| {
            Rectangle::with_size(Size::new(self.size.width as f32, self.size.height as f32))
        });
        let subject_sample = sample(
            &frame,
            self.size.width,
            region,
            colour,
            self.tolerance,
            clear,
        );
        let control_sample = sample(
            &control_frame,
            self.size.width,
            region,
            colour,
            self.tolerance,
            clear,
        );

        let provenance = palette_provenance(&theme, &self.theme.palette);
        let v = verdict(
            subject_sample,
            control_sample,
            &provenance,
            self.control_floor,
        );
        match (v, self.allow_empty) {
            (ColorPresence::Inconclusive(Blind::NothingPainted { .. }), Some(_reason)) => {
                ColorPresence::Absent {
                    subject: subject_sample,
                    control: control_sample,
                }
            }
            (other, _) => other,
        }
    }

    fn frame<'a, Message: 'a>(
        &self,
        content: Element<'a, Message>,
        theme: &iced::Theme,
        clear: Color,
    ) -> Vec<u8> {
        // A fresh renderer prevents accumulated layers from contaminating another frame.
        let mut renderer = headless_renderer();
        let renderer = &mut renderer;
        let mut element: Element<'a, Message> = iced::widget::container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let mut tree = Tree::new(&element);
        element.as_widget_mut().diff(&mut tree);
        let viewport = Size::new(self.size.width as f32, self.size.height as f32);
        let limits = layout::Limits::new(Size::ZERO, viewport);
        let node = element
            .as_widget_mut()
            .measure(&mut tree, renderer, &limits);
        let base = <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(theme);
        element.as_widget().draw(
            &tree,
            renderer,
            theme,
            &renderer::Style {
                text_color: base.text_color,
            },
            Layout::of(&node),
            iced::mouse::Cursor::Unavailable,
            &Rectangle::with_size(viewport),
        );
        renderer.screenshot(self.size, 1.0, clear)
    }
}

fn rgb8(c: Color) -> [u8; 3] {
    let f = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [f(c.r), f(c.g), f(c.b)]
}

fn headless_renderer() -> iced::Renderer {
    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer for ColorProbe")
}
