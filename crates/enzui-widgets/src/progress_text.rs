//! A moving ink highlight over one shaped, accessible status label.
//!
//! The base stays readable throughout the sweep. A shared frame clock updates
//! ink at 30 Hz while visible; reduced motion and inactivity use plain text.
//! Caption and Body use the normal typography policy and Advanced shaping.
//! An external phase handle permits several labels to share one caller-owned clock.
//! Theme-driven reduced motion retains the frame clock's low-rate policy probe.

use crate::frame_clock::{FrameClockPhase, enz_frame_clock, frame_clock_phase};
use crate::text::{TextShaping, TextSize, enz_text};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};

/// A progress label with one text run and a bounded moving highlight.
#[derive(Debug)]
pub struct EnzProgressText {
    content: String,
    size: TextSize,
    active: bool,
    reduced: Option<bool>,
    external_phase: Option<FrameClockPhase>,
    backdrop: Option<Color>,
}

/// Create a Body-size progress label, animated unless motion policy stills it.
#[must_use]
pub fn enz_progress_text(content: impl Into<String>) -> EnzProgressText {
    EnzProgressText {
        content: content.into(),
        size: TextSize::Body,
        active: true,
        reduced: None,
        external_phase: None,
        backdrop: None,
    }
}

impl EnzProgressText {
    /// Use the framework typography rung; defaults to Body.
    #[must_use]
    pub fn size(mut self, size: TextSize) -> Self {
        self.size = size;
        self
    }
    /// Animate while active; false paints plain text and schedules no own frames.
    #[must_use]
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
    /// Override theme reduced motion; true paints plain text.
    #[must_use]
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced = Some(reduced);
        self
    }
    /// Resolve base ink against the caller-painted backdrop.
    /// Without one, use the theme surface or the material's secondary ink on glass.
    #[must_use]
    pub fn backdrop(mut self, backdrop: Color) -> Self {
        self.backdrop = Some(backdrop);
        self
    }

    /// Read a shared elapsed-seconds handle instead of scheduling an internal clock.
    /// The caller owns updates; inactive/reduced labels still paint plain text.
    #[must_use]
    pub fn phase(mut self, phase: FrameClockPhase) -> Self {
        self.external_phase = Some(phase);
        self
    }
    /// Build one shaped text leaf and its optional visible-only clock.
    #[must_use]
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let self_clock = self.external_phase.is_none();
        let phase = self.external_phase.unwrap_or_else(frame_clock_phase);
        let inner = enz_text(self.content)
            .size(self.size)
            .shaping(TextShaping::Advanced)
            .inherit_color(true)
            .into_element();
        let effect: Element<'a, Message> = Element::new(InkWave {
            inner,
            phase: std::rc::Rc::clone(&phase),
            active: self.active,
            reduced: self.reduced,
            backdrop: self.backdrop,
        });
        if self_clock {
            let clock = enz_frame_clock(effect, phase)
                .active(self.active)
                .cadence(iced::time::Duration::from_millis(33));
            match self.reduced {
                Some(reduced) => clock.reduced_motion(reduced).into_element(),
                None => clock.into_element(),
            }
        } else {
            effect
        }
    }
}
impl<'a, Message: 'a> From<EnzProgressText> for Element<'a, Message> {
    fn from(label: EnzProgressText) -> Self {
        label.into_element()
    }
}

struct InkWave<'a, Message> {
    inner: Element<'a, Message>,
    phase: FrameClockPhase,
    active: bool,
    reduced: Option<bool>,
    backdrop: Option<Color>,
}
impl<Message> Widget<Message, iced::Theme, iced::Renderer> for InkWave<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.inner.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.inner.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.inner.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.inner.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.inner
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.inner, tree)
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.inner
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.inner,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let p = crate::theme_lookup::palette(theme);
        let base = match self.backdrop {
            None if p.is_on_glass() => p.text_muted,
            backdrop => p.text_muted_on(backdrop.unwrap_or(p.surface)),
        };
        let base_style = renderer::Style { text_color: base };
        if !self.active
            || self
                .reduced
                .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme))
        {
            self.inner.as_widget().draw(
                tree,
                renderer,
                theme,
                &base_style,
                layout,
                cursor,
                viewport,
            );
            return;
        }
        let bounds = layout.bounds();
        let Some(_) = bounds.intersection(viewport) else {
            return;
        };
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        let time = self.phase.get();
        let phase = if time.is_finite() {
            time.rem_euclid(1.2) / 1.2
        } else {
            0.0
        };
        let width = (bounds.width * 0.6).clamp(24.0, 96.0);
        let left = bounds.x - width + (bounds.width + width) * phase;
        // Disjoint clips avoid compositing antialiased glyph edges twice.
        for (x, width) in [
            (viewport.x, (left - viewport.x).max(0.0)),
            (
                left + width,
                (viewport.x + viewport.width - left - width).max(0.0),
            ),
        ] {
            let region = Rectangle {
                x,
                y: viewport.y,
                width,
                height: viewport.height,
            };
            if let Some(clip) = region.intersection(viewport) {
                renderer.with_layer(clip, |renderer| {
                    self.inner.as_widget().draw(
                        tree,
                        renderer,
                        theme,
                        &base_style,
                        layout,
                        cursor,
                        viewport,
                    );
                });
            }
        }
        const STRIPS: usize = 12;
        for index in 0..STRIPS {
            let strip = Rectangle {
                x: left + index as f32 * width / STRIPS as f32,
                y: viewport.y,
                width: width / STRIPS as f32,
                height: viewport.height,
            };
            let Some(clip) = strip.intersection(viewport) else {
                continue;
            };
            let weight = 1.0 - ((index as f32 + 0.5) / STRIPS as f32 * 2.0 - 1.0).abs();
            let highlight = mix(base, p.text, weight);
            renderer.with_layer(clip, |renderer| {
                self.inner.as_widget().draw(
                    tree,
                    renderer,
                    theme,
                    &renderer::Style {
                        text_color: highlight,
                    },
                    layout,
                    cursor,
                    viewport,
                );
            });
        }
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.inner
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.inner
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}
fn mix(base: Color, high: Color, weight: f32) -> Color {
    Color {
        r: base.r + (high.r - base.r) * weight,
        g: base.g + (high.g - base.g) * weight,
        b: base.b + (high.b - base.b) * weight,
        a: base.a + (high.a - base.a) * weight,
    }
}
