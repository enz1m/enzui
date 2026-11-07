//! One stream of drawing samples from a pen or a mouse.
//!
//! A tablet tool arrives twice: as an `iced::Event::Stylus` with pressure and
//! tilt, and as the mouse events the platform emulates from it so ordinary
//! widgets work with a pen. A drawing surface wants each stroke once, with
//! the pen's pressure when there is a pen. [`PenInput::update`] reads both
//! and returns a [`PenSample`] per change: stylus events while a tool is in
//! proximity, mouse events otherwise (left button down is full pressure).
//!
//! Positions are window coordinates, as the events carry them; subtract the
//! widget's bounds to draw.

use iced_core::stylus::{self, ToolKind};
use iced_core::{Event, Point, Vector, mouse};

/// Where a sample is in a stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokePhase {
    /// Nothing touches; the pen or mouse moved over the surface.
    Hover,
    /// The tip touched or the left button went down: a stroke starts.
    Begin,
    /// The stroke goes on.
    Continue,
    /// The tip lifted, the button went up, or the pen left mid-stroke.
    End,
}

/// What made a sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PenSource {
    /// The mouse, or a touchpad.
    Mouse,
    /// A tablet tool.
    Stylus(stylus::Tool),
}

/// One point of input for a drawing surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PenSample {
    /// Where, in window coordinates.
    pub position: Point,
    /// From `0.0` to `1.0`: the pen's pressure, or `1.0` for a held mouse
    /// button; `0.0` while hovering.
    pub pressure: f32,
    /// The pen's tilt in degrees (x right, y towards the user); zero for the
    /// mouse.
    pub tilt: Vector,
    /// Whether this erases: the eraser end of a pen.
    pub eraser: bool,
    /// What made the sample.
    pub source: PenSource,
    /// Where the sample is in a stroke.
    pub phase: StrokePhase,
}

/// Turns stylus and mouse events into [`PenSample`]s; keep one per drawing
/// surface (or window) and feed it every event.
#[derive(Debug, Clone, Copy, Default)]
pub struct PenInput {
    /// A tool is in proximity, so mouse events are its echo.
    stylus_near: bool,
    /// The current stroke, if one is drawing.
    drawing: Option<PenSource>,
    /// The last mouse position.
    cursor: Point,
}

impl PenInput {
    /// A reader with no tool near and no stroke.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a stroke is being drawn.
    #[must_use]
    pub fn is_drawing(&self) -> bool {
        self.drawing.is_some()
    }

    /// The sample `event` makes, if any.
    pub fn update(&mut self, event: &Event) -> Option<PenSample> {
        match event {
            Event::Stylus(stylus) => self.stylus(stylus),
            Event::Mouse(mouse) => self.mouse(mouse),
            _ => None,
        }
    }

    fn stylus(&mut self, event: &stylus::Event) -> Option<PenSample> {
        let source = PenSource::Stylus(event.tool);
        let phase = match event.phase {
            stylus::Phase::ProximityIn => {
                self.stylus_near = true;
                StrokePhase::Hover
            }
            stylus::Phase::Down => {
                self.drawing = Some(source);
                StrokePhase::Begin
            }
            stylus::Phase::Up => {
                self.drawing.take()?;
                StrokePhase::End
            }
            stylus::Phase::ProximityOut => {
                self.stylus_near = false;
                self.drawing.take()?;
                StrokePhase::End
            }
            stylus::Phase::Moved if self.drawing.is_some() => StrokePhase::Continue,
            stylus::Phase::Moved => StrokePhase::Hover,
            stylus::Phase::ButtonPressed(_) | stylus::Phase::ButtonReleased(_) => return None,
        };
        Some(PenSample {
            position: event.position,
            pressure: if phase == StrokePhase::Hover {
                0.0
            } else {
                event.pressure
            },
            tilt: event.tilt,
            eraser: event.tool.kind == ToolKind::Eraser,
            source,
            phase,
        })
    }

    fn mouse(&mut self, event: &mouse::Event) -> Option<PenSample> {
        if self.stylus_near {
            // The platform's echo of the pen, already read with pressure.
            return None;
        }
        let phase = match *event {
            mouse::Event::CursorMoved { position } => {
                self.cursor = position;
                if self.drawing.is_some() {
                    StrokePhase::Continue
                } else {
                    StrokePhase::Hover
                }
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                self.drawing = Some(PenSource::Mouse);
                StrokePhase::Begin
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) | mouse::Event::CursorLeft => {
                self.drawing.take()?;
                StrokePhase::End
            }
            _ => return None,
        };
        Some(PenSample {
            position: self.cursor,
            pressure: if matches!(phase, StrokePhase::Hover) {
                0.0
            } else {
                1.0
            },
            tilt: Vector::ZERO,
            eraser: false,
            source: PenSource::Mouse,
            phase,
        })
    }
}
