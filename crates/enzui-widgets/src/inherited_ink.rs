//! Measure whether changing inherited text ink changes rendered pixels.
//!
//! Container text colour reaches descendants that use the renderer's inherited
//! text-colour channel, such as uncoloured iced text. [`EnzText`](crate::enz_text::EnzText)
//! normally chooses its own ink; [`inherit_color`](crate::enz_text::EnzText::inherit_color)
//! opts into inheritance. SVG icons use a separate tint channel.
//!
//! [`InkProbe`] renders freshly built subtrees with ink A, ink B, then ink A
//! again. It rejects differing A frames as nondeterministic and separates an
//! unpainted reference from a painted but unchanged one. It observes the whole
//! canvas, rather than identifying which descendant or ink path changed.
//! Keep builds identical apart from the supplied ink; independent animation
//! or builder state can affect the result. The A control does not establish
//! determinism for ink B. Use distinct inks and a background that lets the
//! reference content produce visible pixels.
//!
//! The default canvas is 240×80 pixels at scale factor 1, with a mid-grey
//! background, red/blue inks and the dark iced theme. Each frame creates a
//! headless renderer; use the probe in tests or audits rather than a view.
//!
//! ```no_run
//! use enzui_widgets::enz_text::enz_text;
//! use enzui_widgets::inherited_ink::{InkProbe, InkReach};
//!
//! # #[derive(Debug, Clone)] enum Msg {}
//! let reach = InkProbe::new().run(|_ink| -> iced::Element<'static, Msg> {
//!     enz_text("Today").into()
//! });
//! assert!(matches!(reach, InkReach::Inert { .. }));
//!
//! let reach = InkProbe::new().run(|_ink| -> iced::Element<'static, Msg> {
//!     iced::widget::text("Today").into()
//! });
//! assert!(matches!(reach, InkReach::Reaches { .. }));
//! ```
//!

use iced::advanced::layout::Measure as _;
use iced::advanced::renderer::Headless;
use iced::advanced::widget::Tree;
use iced::advanced::{Layout, layout, renderer};
use iced::widget::container;
use iced::{Color, Element, Length, Size};

/// Default first probe ink: vivid red.
pub const PROBE_INK_A: Color = Color::from_rgb(1.0, 0.0, 0.0);

/// The second probe ink — vivid blue.
pub const PROBE_INK_B: Color = Color::from_rgb(0.0, 0.0, 1.0);

/// The observed pixel response to the probe inks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InkReach {
    /// The reference frame matches the clear colour exactly.
    ///
    /// This provides no painted-content measurement; it is not a measured no-op.
    NothingPainted,
    /// The frames differ under the two probe inks.
    Reaches {
        /// Pixels that differ between the two renders.
        changed: usize,
        /// Pixels in the reference render that differ from the clear colour.
        painted: usize,
    },
    /// The reference painted pixels, but changing inks changed none.
    ///
    /// This is a measured no-op for these inputs, not proof that every descendant
    /// owns its ink.
    Inert {
        /// Pixels in the reference render that differ from the clear colour.
        painted: usize,
    },
    /// The two ink-A frames differ; no reachability verdict is returned.
    Nondeterministic {
        /// Pixels that differ between the two identical-input renders.
        differing: usize,
    },
}

impl InkReach {
    /// `true` for [`InkReach::Inert`] only — a *measured* no-op.
    ///
    /// [`InkReach::NothingPainted`] answers `false`, so a probe that saw
    /// nothing can never be mistaken for a finding.
    pub fn is_dead_declaration(&self) -> bool {
        matches!(self, InkReach::Inert { .. })
    }

    /// Pixels the reference render put on the canvas (0 when there is no
    /// usable measurement).
    pub fn painted(&self) -> usize {
        match self {
            InkReach::NothingPainted | InkReach::Nondeterministic { .. } => 0,
            InkReach::Reaches { painted, .. } | InkReach::Inert { painted } => *painted,
        }
    }
}

/// Compare whole-canvas pixels under two inherited ink declarations.
///
/// See the [module documentation](self) for the three-frame control and limits.
#[derive(Debug, Clone)]
#[must_use]
pub struct InkProbe {
    size: Size<u32>,
    background: Color,
    inks: (Color, Color),
    theme: iced::Theme,
}

impl Default for InkProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl InkProbe {
    /// Use a 240×80 canvas, mid-grey background, red/blue inks and the dark iced theme.
    pub fn new() -> Self {
        Self {
            size: Size::new(240, 80),
            background: Color::from_rgb(0.5, 0.5, 0.5),
            inks: (PROBE_INK_A, PROBE_INK_B),
            theme: iced::Theme::Dark,
        }
    }

    /// Canvas size in physical pixels.
    pub fn size(mut self, width: u32, height: u32) -> Self {
        self.size = Size::new(width, height);
        self
    }

    /// The clear colour the screenshot starts from, and the reference for the
    /// `painted` positive control.
    pub fn background(mut self, background: Color) -> Self {
        self.background = background;
        self
    }

    /// Set the probe inks; choose distinct values to measure a response.
    ///
    /// Equal inks are accepted and can report Inert without exercising a change.
    pub fn inks(mut self, a: Color, b: Color) -> Self {
        self.inks = (a, b);
        self
    }

    /// Set the theme shared by all three renders.
    pub fn theme(mut self, theme: iced::Theme) -> Self {
        self.theme = theme;
        self
    }

    /// Build and render with ink A, ink B, then ink A again.
    ///
    /// Each subtree is wrapped in a container declaring that ink. `build` also
    /// receives the ink, so explicit consumption can change pixels independently
    /// of inheritance. A differing A repeat returns Nondeterministic. Otherwise
    /// the probe measures painted reference pixels and A/B pixel differences.
    ///
    /// # Panics
    ///
    /// Panics if a headless renderer cannot be initialized.
    pub fn run<'a, Message: 'a>(
        &self,
        mut build: impl FnMut(Color) -> Element<'a, Message>,
    ) -> InkReach {
        let frame_a = self.frame(build(self.inks.0), self.inks.0);
        let frame_b = self.frame(build(self.inks.1), self.inks.1);

        // Reject an unstable reference before treating a cross-ink difference as a response.
        let frame_a2 = self.frame(build(self.inks.0), self.inks.0);

        let differing = frame_a
            .as_chunks::<4>()
            .0
            .iter()
            .zip(frame_a2.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count();
        if differing > 0 {
            return InkReach::Nondeterministic { differing };
        }

        let clear = rgba8(self.background);
        let painted = frame_a
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|px| *px != clear.as_slice())
            .count();
        if painted == 0 {
            return InkReach::NothingPainted;
        }

        let changed = frame_a
            .as_chunks::<4>()
            .0
            .iter()
            .zip(frame_b.as_chunks::<4>().0.iter())
            .filter(|(a, b)| a != b)
            .count();

        if changed == 0 {
            InkReach::Inert { painted }
        } else {
            InkReach::Reaches { changed, painted }
        }
    }

    fn frame<'a, Message: 'a>(&self, content: Element<'a, Message>, ink: Color) -> Vec<u8> {
        // Fresh renderers prevent retained layers from contaminating the next screenshot.
        let mut renderer = headless_renderer();
        let renderer = &mut renderer;

        let mut element: Element<'a, Message> = container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_theme: &iced::Theme| container::Style {
                text_color: Some(ink),
                ..container::Style::default()
            })
            .into();

        let mut tree = Tree::new(&element);
        element.as_widget_mut().diff(&mut tree);

        let viewport = Size::new(self.size.width as f32, self.size.height as f32);
        let limits = layout::Limits::new(Size::ZERO, viewport);
        let node = element
            .as_widget_mut()
            .measure(&mut tree, renderer, &limits);

        element.as_widget().draw(
            &tree,
            renderer,
            &self.theme,
            &renderer::Style {
                text_color: Color::from_rgb(0.0, 1.0, 0.0),
            },
            Layout::of(&node),
            iced::mouse::Cursor::Unavailable,
            &iced::Rectangle::with_size(viewport),
        );

        renderer.screenshot(self.size, 1.0, self.background)
    }
}

fn rgba8(c: Color) -> [u8; 4] {
    let f = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [f(c.r), f(c.g), f(c.b), f(c.a)]
}

fn headless_renderer() -> iced::Renderer {
    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
        renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer for InkProbe")
}
