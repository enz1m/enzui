//! State machine for previewing, committing, and cancelling annotation gestures.
//!
//! Previews have no identity and use display coordinates. Release mints an id
//! and converts committed geometry to image coordinates; cancellation preserves
//! neither identity nor sampler state.

use iced_core::Point;

use crate::image_space::ImageProjection;

use super::ink::{SamplingPolicy, StrokeSampler};
use super::{Annotation, AnnotationIds, ShapeGeometry, ShapeKind, StrokeStyle};

/// Tool selected for the next gesture; `None` leaves pointer handling to the underlying viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AnnotationTool {
    /// Captures no gesture and leaves pointer events unhandled.
    #[default]
    None,
    /// Selects existing annotations without drawing a shape.
    Select,
    /// Drag out an axis-aligned rectangle.
    Rectangle,
    /// Drag out an ellipse inscribed in the dragged box.
    Ellipse,
    /// Drag a straight segment.
    Line,
    /// Drag a straight segment with a head at the release end.
    Arrow,
    /// Free drawing, sampled by [`StrokeSampler`].
    Freehand,
}

impl AnnotationTool {
    /// Returns the shape kind drawn by this tool, or `None` for `None` and `Select`.
    #[must_use]
    pub fn shape_kind(self) -> Option<ShapeKind> {
        match self {
            Self::None | Self::Select => None,
            Self::Rectangle => Some(ShapeKind::Rectangle),
            Self::Ellipse => Some(ShapeKind::Ellipse),
            Self::Line => Some(ShapeKind::Line),
            Self::Arrow => Some(ShapeKind::Arrow),
            Self::Freehand => Some(ShapeKind::Freehand),
        }
    }

    /// Whether this tool draws a shape when dragged.
    #[must_use]
    pub fn draws(self) -> bool {
        self.shape_kind().is_some()
    }
}

/// In-progress display-space geometry and style with no annotation identity.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewShape {
    geometry: ShapeGeometry,
    style: StrokeStyle,
    // Display-pixel width resolved at press time so preview and committed ink agree.
    screen_width: f32,
}

impl PreviewShape {
    /// Returns the geometry in display coordinates.
    #[must_use]
    pub fn geometry(&self) -> &ShapeGeometry {
        &self.geometry
    }

    /// Returns the document-space style; use `screen_width` for preview drawing.
    #[must_use]
    pub fn style(&self) -> StrokeStyle {
        self.style
    }

    /// The stroke width in display pixels.
    #[must_use]
    pub fn screen_width(&self) -> f32 {
        self.screen_width
    }

    /// The kind being drawn.
    #[must_use]
    pub fn kind(&self) -> ShapeKind {
        self.geometry.kind()
    }
}

#[derive(Debug, Clone)]
struct ActiveCapture {
    tool: AnnotationTool,
    style: StrokeStyle,
    // Pin the press-time projection so zoom changes cannot reinterpret a live gesture.
    projection: ImageProjection,
    origin: Point,
    current: Point,
    // Freehand samples are retained in image coordinates; other tools have no sampler.
    sampler: Option<StrokeSampler>,
}

/// The result of releasing the pointer.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureOutcome {
    /// Contains the newly committed annotation and its newly minted identity.
    Committed(Box<Annotation>),
    /// Reports a click without a drag, carrying its display-space press position.
    Clicked(Point),
    /// Nothing was in flight, or the gesture produced no expressible geometry.
    Nothing,
}

/// Owns one overlay’s annotation tool, style, identity source, and live gesture state.
#[derive(Debug, Clone)]
pub struct CaptureSession {
    ids: AnnotationIds,
    tool: AnnotationTool,
    style: StrokeStyle,
    sampling: SamplingPolicy,
    click_threshold: f32,
    active: Option<ActiveCapture>,
}

/// Default display-pixel distance separating clicks from drags.
pub const DEFAULT_CLICK_THRESHOLD: f32 = crate::marquee::DEFAULT_DRAG_THRESHOLD;

impl CaptureSession {
    /// Creates a session with no active tool and a two-pixel black stroke.
    #[must_use]
    pub fn new(ids: AnnotationIds) -> Self {
        Self {
            ids,
            tool: AnnotationTool::None,
            style: const { StrokeStyle::new(2.0, iced_core::Color::BLACK).expect("2.0 is finite") },
            sampling: SamplingPolicy::DEFAULT,
            click_threshold: DEFAULT_CLICK_THRESHOLD,
            active: None,
        }
    }

    /// Builder: the active tool.
    #[must_use]
    pub fn with_tool(mut self, tool: AnnotationTool) -> Self {
        self.tool = tool;
        self
    }

    /// Builder: the stroke style new shapes are drawn with.
    #[must_use]
    pub fn with_style(mut self, style: StrokeStyle) -> Self {
        self.style = style;
        self
    }

    /// Builder: the freehand sampling policy.
    #[must_use]
    pub fn with_sampling(mut self, sampling: SamplingPolicy) -> Self {
        self.sampling = sampling;
        self
    }

    /// Sets the display-pixel drag threshold.
    ///
    /// Negative or non-finite values use `DEFAULT_CLICK_THRESHOLD`; zero is preserved.
    #[must_use]
    pub fn with_click_threshold(mut self, threshold: f32) -> Self {
        self.click_threshold = if threshold.is_finite() && threshold >= 0.0 {
            threshold
        } else {
            DEFAULT_CLICK_THRESHOLD
        };
        self
    }

    /// Sets the active tool, cancelling a live gesture when the tool changes.
    pub fn set_tool(&mut self, tool: AnnotationTool) {
        if self.tool != tool {
            self.cancel();
            self.tool = tool;
        }
    }

    /// Sets the style for later gestures without changing a live gesture.
    pub fn set_style(&mut self, style: StrokeStyle) {
        self.style = style;
    }

    /// The active tool.
    #[must_use]
    pub fn tool(&self) -> AnnotationTool {
        self.tool
    }

    /// The stroke style new shapes are drawn with.
    #[must_use]
    pub fn style(&self) -> StrokeStyle {
        self.style
    }

    /// The drag distance before a press becomes a shape, in display pixels.
    #[must_use]
    pub fn click_threshold(&self) -> f32 {
        self.click_threshold
    }

    /// Whether a gesture is in flight.
    #[must_use]
    pub fn is_capturing(&self) -> bool {
        self.active.is_some()
    }

    /// Returns the number of annotation identities minted by this session.
    #[must_use]
    pub fn minted(&self) -> u64 {
        self.ids.minted()
    }

    /// Reserves identities already present in an adopted document.
    ///
    /// Call this once when the session adopts a loaded document so later commits
    /// cannot mint colliding identities.
    pub fn reserve_existing<'a>(&mut self, annotations: impl IntoIterator<Item = &'a Annotation>) {
        self.ids.reserve_existing(annotations);
    }

    /// Starts a gesture at a display-space point under a fixed projection.
    ///
    /// Returns `false` for `None` or a non-finite point. `Select` starts click/drag
    /// tracking but never creates a preview or committed annotation.
    pub fn press(&mut self, at: Point, projection: ImageProjection) -> bool {
        if self.tool == AnnotationTool::None || !finite(at) {
            return false;
        }
        let mut sampler = (self.tool == AnnotationTool::Freehand).then(|| {
            // Convert the sampling policy at press time so visible point spacing stays uniform across zoom.
            let policy = SamplingPolicy::from_screen(
                self.sampling.min_distance(),
                self.sampling.capacity(),
                projection,
            )
            .unwrap_or(self.sampling);
            StrokeSampler::new(policy)
        });
        if let Some(s) = sampler.as_mut() {
            s.push(projection.display_to_image(at));
        }
        self.active = Some(ActiveCapture {
            tool: self.tool,
            style: self.style,
            projection,
            origin: at,
            current: at,
            sampler,
        });
        true
    }

    /// Extends a live gesture to a display-space point.
    ///
    /// Returns whether a gesture existed; non-finite points are ignored without
    /// cancelling it.
    pub fn pointer_moved(&mut self, at: Point) -> bool {
        let Some(active) = self.active.as_mut() else {
            return false;
        };
        if !finite(at) {
            return true;
        }
        active.current = at;
        if let Some(sampler) = active.sampler.as_mut() {
            // Samples remain in image space so preview and commit use the same retained point set.
            sampler.push(active.projection.display_to_image(at));
        }
        true
    }

    /// Returns display-space preview geometry after the drag threshold is crossed.
    #[must_use]
    pub fn preview(&self) -> Option<PreviewShape> {
        let active = self.active.as_ref()?;
        if !self.is_drag(active) {
            return None;
        }
        let geometry = self.display_geometry(active)?;
        Some(PreviewShape {
            geometry,
            style: active.style,
            screen_width: active.style.screen_width(active.projection),
        })
    }

    /// Ends the gesture, minting an identity and converting to image space only when a shape commits.
    pub fn release(&mut self) -> CaptureOutcome {
        let Some(active) = self.active.take() else {
            return CaptureOutcome::Nothing;
        };
        if !self.is_drag(&active) {
            return CaptureOutcome::Clicked(active.origin);
        }
        let Some(geometry) = self.display_geometry(&active) else {
            return CaptureOutcome::Nothing;
        };
        let annotation =
            Annotation::from_display(self.ids.mint(), &geometry, active.style, active.projection);
        CaptureOutcome::Committed(Box::new(annotation))
    }

    /// Cancels the live gesture and returns whether one existed.
    ///
    /// Cancellation retains no geometry, sampler state, or minted identity.
    pub fn cancel(&mut self) -> bool {
        self.active.take().is_some()
    }

    // Freehand uses path length so a loop returning near its origin is not discarded as a click.
    fn is_drag(&self, active: &ActiveCapture) -> bool {
        if active.tool == AnnotationTool::Freehand {
            let Some(sampler) = active.sampler.as_ref() else {
                return false;
            };
            let points = sampler.points();
            if points.len() < 2 {
                return false;
            }
            let scale = active.projection.scale();
            let mean = (scale.x.abs() * scale.y.abs()).sqrt();
            let length: f32 = points
                .windows(2)
                .map(|w| {
                    let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
                    (dx * dx + dy * dy).sqrt()
                })
                .sum();
            // Convert sampled image-space path length to display distance before comparing the threshold.
            return length * mean >= self.click_threshold;
        }
        let (dx, dy) = (
            active.current.x - active.origin.x,
            active.current.y - active.origin.y,
        );
        (dx * dx + dy * dy).sqrt() >= self.click_threshold
    }

    fn display_geometry(&self, active: &ActiveCapture) -> Option<ShapeGeometry> {
        match active.tool {
            AnnotationTool::None | AnnotationTool::Select => None,
            AnnotationTool::Rectangle => {
                ShapeGeometry::rectangle_from_corners(active.origin, active.current)
            }
            AnnotationTool::Ellipse => {
                ShapeGeometry::ellipse_from_corners(active.origin, active.current)
            }
            AnnotationTool::Line => ShapeGeometry::line(active.origin, active.current),
            AnnotationTool::Arrow => ShapeGeometry::arrow(active.origin, active.current),
            AnnotationTool::Freehand => {
                let sampler = active.sampler.as_ref()?;
                // Project retained image samples for preview so preview and commit use the same point set.
                let points: Vec<Point> = sampler
                    .points()
                    .iter()
                    .map(|p| active.projection.image_to_display(*p))
                    .collect();
                ShapeGeometry::freehand(points)
            }
        }
    }
}

fn finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}
