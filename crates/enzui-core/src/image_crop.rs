//! Controlled source-pixel crop drafts and identity-bound move/resize gestures.
//!
//! Aspect changes affect only the draft. Applications keep the committed crop
//! separately and adopt the draft only on confirmation. Export uses its exact
//! [`PixelCrop`]; a circular preview does not alter the rectangular pixel recipe.

use crate::identity::OwnerId;
use crate::image_transform::{PixelCrop, TransformError};
use iced_core::{Point, Rectangle, Size, Vector};

/// Application identity of one selected image revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CropIdentity {
    owner: OwnerId,
    generation: u64,
}

impl CropIdentity {
    /// Bind a crop to a stable owner and a caller-managed source generation.
    #[must_use]
    pub fn new(owner: OwnerId, generation: u64) -> Self {
        Self { owner, generation }
    }

    /// Owner namespace for crop and viewer accessibility nodes.
    #[must_use]
    pub fn owner(&self) -> &OwnerId {
        &self.owner
    }

    /// Source revision; increment it whenever the selected image is replaced.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// Constraint on integer crop dimensions.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CropAspect {
    /// Independent width and height, with a one-pixel minimum.
    #[default]
    Free,
    /// Exact rational width-to-height ratio, reduced before use.
    Fixed(Size<u32>),
}

impl CropAspect {
    /// Square avatar selection; export remains rectangular.
    pub const SQUARE: Self = Self::Fixed(Size::new(1, 1));

    fn units(self) -> Result<Option<Size<u32>>, CropError> {
        let Self::Fixed(size) = self else {
            return Ok(None);
        };
        if size.width == 0 || size.height == 0 {
            return Err(CropError::InvalidAspect);
        }
        let (mut a, mut b) = (size.width, size.height);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        Ok(Some(Size::new(size.width / a, size.height / a)))
    }
}

/// Validated draft in orientation-adjusted source coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CropDraft {
    identity: CropIdentity,
    source: Size<u32>,
    crop: PixelCrop,
    aspect: CropAspect,
}

impl CropDraft {
    /// Select the whole source without an aspect constraint.
    ///
    /// # Errors
    /// Rejects empty source dimensions.
    pub fn new(identity: CropIdentity, source: Size<u32>) -> Result<Self, CropError> {
        Ok(Self {
            identity,
            source,
            crop: PixelCrop::new(0, 0, source.width, source.height, source)?,
            aspect: CropAspect::Free,
        })
    }

    /// Select the largest centered square inside the source.
    ///
    /// # Errors
    /// Rejects empty source dimensions.
    pub fn avatar(identity: CropIdentity, source: Size<u32>) -> Result<Self, CropError> {
        Self::new(identity, source)?.with_aspect(CropAspect::SQUARE)
    }

    /// Replace a draft rectangle without changing its source or aspect.
    ///
    /// # Errors
    /// Rejects out-of-source rectangles or dimensions violating the active ratio.
    pub fn with_crop(mut self, crop: PixelCrop) -> Result<Self, CropError> {
        let (x, y, w, h) = crop.bounds();
        PixelCrop::new(x, y, w, h, self.source)?;
        if let Some(units) = self.aspect.units()?
            && u64::from(w) * u64::from(units.height) != u64::from(h) * u64::from(units.width)
        {
            return Err(CropError::InvalidAspect);
        }
        self.crop = crop;
        Ok(self)
    }

    /// Inscribe a new ratio inside the current crop while retaining its center.
    /// Selecting free aspect preserves the rectangle.
    ///
    /// # Errors
    /// Rejects zero ratios or a ratio with no integer-pixel fit in the current crop.
    pub fn with_aspect(mut self, aspect: CropAspect) -> Result<Self, CropError> {
        if let Some(units) = aspect.units()? {
            let (x, y, w, h) = self.crop.bounds();
            let count = (w / units.width).min(h / units.height);
            if count == 0 {
                return Err(CropError::InvalidAspect);
            }
            let (nw, nh) = (count * units.width, count * units.height);
            self.crop = PixelCrop::new(x + (w - nw) / 2, y + (h - nh) / 2, nw, nh, self.source)?;
        }
        self.aspect = aspect;
        Ok(self)
    }

    /// Move by source pixels, rounding to integers and clamping the whole crop inside the image.
    ///
    /// # Errors
    /// Rejects nonfinite displacement.
    pub fn move_by(&self, delta: Vector) -> Result<Self, CropError> {
        if !delta.x.is_finite() || !delta.y.is_finite() {
            return Err(CropError::InvalidPoint);
        }
        let (x, y, w, h) = self.crop.bounds();
        let x = (f64::from(x) + f64::from(delta.x))
            .round()
            .clamp(0.0, f64::from(self.source.width - w)) as u32;
        let y = (f64::from(y) + f64::from(delta.y))
            .round()
            .clamp(0.0, f64::from(self.source.height - h)) as u32;
        self.clone()
            .with_crop(PixelCrop::new(x, y, w, h, self.source)?)
    }

    /// Resize one corner while its opposite corner remains fixed.
    ///
    /// # Errors
    /// Rejects a nonfinite point or `Move`, which is not a resize corner.
    pub fn resize_to(&self, handle: CropHandle, point: Point) -> Result<Self, CropError> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return Err(CropError::InvalidPoint);
        }
        let (left, top) = match handle {
            CropHandle::NorthWest => (true, true),
            CropHandle::NorthEast => (false, true),
            CropHandle::SouthEast => (false, false),
            CropHandle::SouthWest => (true, false),
            CropHandle::Move => return Err(CropError::InvalidHandle),
        };
        let (x, y, w, h) = self.crop.bounds();
        let (ax, ay) = (if left { x + w } else { x }, if top { y + h } else { y });
        let (available_w, available_h) = (
            if left { ax } else { self.source.width - ax },
            if top { ay } else { self.source.height - ay },
        );
        let (dx, dy) = (
            if left {
                f64::from(ax) - f64::from(point.x)
            } else {
                f64::from(point.x) - f64::from(ax)
            },
            if top {
                f64::from(ay) - f64::from(point.y)
            } else {
                f64::from(point.y) - f64::from(ay)
            },
        );
        let (w, h) = if let Some(units) = self.aspect.units()? {
            let max_count = (available_w / units.width).min(available_h / units.height);
            let (uw, uh) = (f64::from(units.width), f64::from(units.height));
            let count = ((dx * uw + dy * uh) / (uw * uw + uh * uh))
                .round()
                .clamp(1.0, f64::from(max_count)) as u32;
            (count * units.width, count * units.height)
        } else {
            (
                dx.round().clamp(1.0, f64::from(available_w)) as u32,
                dy.round().clamp(1.0, f64::from(available_h)) as u32,
            )
        };
        self.clone().with_crop(PixelCrop::new(
            if left { ax - w } else { ax },
            if top { ay - h } else { ay },
            w,
            h,
            self.source,
        )?)
    }

    /// Begin an image-space gesture tied to this source revision and aspect.
    ///
    /// # Errors
    /// Rejects nonfinite pointer coordinates.
    pub fn begin(&self, handle: CropHandle, point: Point) -> Result<CropGesture, CropError> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return Err(CropError::InvalidPoint);
        }
        Ok(CropGesture {
            initial: self.clone(),
            handle,
            point,
        })
    }

    /// Accept feedback only while its source revision, dimensions and aspect still match.
    ///
    /// # Errors
    /// Rejects feedback queued before source replacement or an external aspect change.
    pub fn accept(&self, proposed: Self) -> Result<Self, CropError> {
        if self.identity != proposed.identity
            || self.source != proposed.source
            || self.aspect != proposed.aspect
        {
            return Err(CropError::StaleGesture);
        }
        Ok(proposed)
    }

    /// Source revision required by draft feedback and gestures.
    #[must_use]
    pub fn identity(&self) -> &CropIdentity {
        &self.identity
    }
    /// Orientation-adjusted source dimensions.
    #[must_use]
    pub fn source_dimensions(&self) -> Size<u32> {
        self.source
    }
    /// Exact rectangular selection consumed by image export.
    #[must_use]
    pub fn pixels(&self) -> PixelCrop {
        self.crop
    }
    /// Current constraint; ratio changes never mutate image pixels.
    #[must_use]
    pub fn aspect(&self) -> CropAspect {
        self.aspect
    }
    /// Smallest integer width and height satisfying the ratio, or `None` for free aspect.
    #[must_use]
    pub fn aspect_units(&self) -> Option<Size<u32>> {
        self.aspect.units().ok().flatten()
    }
    /// Image-space rectangle for projection onto the viewer.
    #[must_use]
    pub fn image_rect(&self) -> Rectangle {
        let (x, y, w, h) = self.crop.bounds();
        Rectangle::new(
            Point::new(x as f32, y as f32),
            Size::new(w as f32, h as f32),
        )
    }
}

/// Movable body or an opposite-corner-anchored resize handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropHandle {
    /// Move the whole rectangle.
    Move,
    /// Resize from the upper-left corner.
    NorthWest,
    /// Resize from the upper-right corner.
    NorthEast,
    /// Resize from the lower-right corner.
    SouthEast,
    /// Resize from the lower-left corner.
    SouthWest,
}

/// Snapshot for one continuous gesture; pointer feedback is measured from its start.
#[derive(Debug, Clone)]
pub struct CropGesture {
    initial: CropDraft,
    handle: CropHandle,
    point: Point,
}

impl CropGesture {
    /// Resolve a pointer sample against its initial crop, allowing normal draft feedback.
    ///
    /// # Errors
    /// Rejects source/owner/generation replacement, aspect changes and invalid points.
    pub fn drag(&self, current: &CropDraft, point: Point) -> Result<CropDraft, CropError> {
        if current.identity != self.initial.identity
            || current.source != self.initial.source
            || current.aspect != self.initial.aspect
        {
            return Err(CropError::StaleGesture);
        }
        match self.handle {
            CropHandle::Move => self.initial.move_by(point - self.point),
            corner => {
                let (x, y, w, h) = self.initial.crop.bounds();
                let start = match corner {
                    CropHandle::NorthWest => Point::new(x as f32, y as f32),
                    CropHandle::NorthEast => Point::new((x + w) as f32, y as f32),
                    CropHandle::SouthEast => Point::new((x + w) as f32, (y + h) as f32),
                    CropHandle::SouthWest => Point::new(x as f32, (y + h) as f32),
                    CropHandle::Move => unreachable!(),
                };
                self.initial.resize_to(corner, start + (point - self.point))
            }
        }
    }
}

/// Why a crop draft or gesture was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropError {
    /// Shared source-pixel rectangle validation failed.
    Pixels(TransformError),
    /// Ratio is empty, cannot fit, or disagrees with the rectangle.
    InvalidAspect,
    /// Coordinates are nonfinite.
    InvalidPoint,
    /// A move body was supplied to corner resizing.
    InvalidHandle,
    /// Gesture identity, source dimensions or aspect no longer matches.
    StaleGesture,
}

impl From<TransformError> for CropError {
    fn from(error: TransformError) -> Self {
        Self::Pixels(error)
    }
}
impl std::fmt::Display for CropError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pixels(error) => error.fmt(f),
            Self::InvalidAspect => f.write_str("crop aspect is invalid or cannot fit"),
            Self::InvalidPoint => f.write_str("crop point is nonfinite"),
            Self::InvalidHandle => f.write_str("crop resize requires a corner handle"),
            Self::StaleGesture => {
                f.write_str("crop gesture belongs to a replaced source or aspect")
            }
        }
    }
}
impl std::error::Error for CropError {}
