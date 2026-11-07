//! Validated raster recipes in orientation-adjusted source-pixel coordinates.
//!
//! Recipes crop, flip on the cropped axes, rotate clockwise, then resize.
//! Prediction performs no pixel work. Sources must already have their encoded
//! orientation applied; display selections map through [`ImageProjection`].

use crate::image_space::ImageProjection;
use iced_core::{Point, Rectangle, Size};

/// A nonempty integer crop inside its source image, with exclusive far edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelCrop {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl PixelCrop {
    /// Validate a source-pixel rectangle without clipping or moving it.
    ///
    /// # Errors
    /// Rejects empty rectangles, overflow and any edge outside `source`.
    pub fn new(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        source: Size<u32>,
    ) -> Result<Self, TransformError> {
        let crop = Self {
            x,
            y,
            width,
            height,
        };
        crop.validate(source)?;
        Ok(crop)
    }

    /// Cover a floating image-space selection by rounding near edges down and far edges up.
    ///
    /// # Errors
    /// Rejects nonfinite, empty or out-of-source selections before rounding.
    pub fn from_image_rect(rect: Rectangle, source: Size<u32>) -> Result<Self, TransformError> {
        let x = f64::from(rect.x);
        let y = f64::from(rect.y);
        let right = x + f64::from(rect.width);
        let bottom = y + f64::from(rect.height);
        if ![x, y, right, bottom].iter().all(|v| v.is_finite())
            || x < 0.0
            || y < 0.0
            || rect.width <= 0.0
            || rect.height <= 0.0
            || right > f64::from(source.width)
            || bottom > f64::from(source.height)
        {
            return Err(TransformError::InvalidCrop);
        }
        let x = x.floor() as u32;
        let y = y.floor() as u32;
        Self::new(
            x,
            y,
            right.ceil() as u32 - x,
            bottom.ceil() as u32 - y,
            source,
        )
    }

    /// Map a displayed selection through the viewer's resolved projection before rounding.
    ///
    /// # Errors
    /// Rejects selections mapping outside the source or to a nonfinite/empty rectangle.
    pub fn from_display_rect(
        rect: Rectangle,
        projection: ImageProjection,
        source: Size<u32>,
    ) -> Result<Self, TransformError> {
        let near = projection.display_to_image(rect.position());
        let far =
            projection.display_to_image(Point::new(rect.x + rect.width, rect.y + rect.height));
        Self::from_image_rect(
            Rectangle::new(near, Size::new(far.x - near.x, far.y - near.y)),
            source,
        )
    }

    /// Return `(x, y, width, height)` in source pixels.
    #[must_use]
    pub const fn bounds(self) -> (u32, u32, u32, u32) {
        (self.x, self.y, self.width, self.height)
    }

    /// Dimensions before flips, rotation or resize.
    #[must_use]
    pub const fn dimensions(self) -> Size<u32> {
        Size::new(self.width, self.height)
    }

    fn validate(self, source: Size<u32>) -> Result<(), TransformError> {
        if self.width == 0
            || self.height == 0
            || self
                .x
                .checked_add(self.width)
                .is_none_or(|v| v > source.width)
            || self
                .y
                .checked_add(self.height)
                .is_none_or(|v| v > source.height)
        {
            Err(TransformError::InvalidCrop)
        } else {
            Ok(())
        }
    }
}

/// Clockwise rotation after cropping and flips.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum QuarterTurn {
    /// Preserve the cropped axes.
    #[default]
    None,
    /// Exchange axes with a clockwise quarter turn.
    Clockwise90,
    /// Reverse both axes.
    Clockwise180,
    /// Exchange axes with a counterclockwise quarter turn.
    Clockwise270,
}

/// Resize policy after rotation; every mode refuses enlargement.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ImageResize {
    /// Keep the rotated dimensions.
    #[default]
    Original,
    /// Produce these dimensions; aspect may change.
    Exact(Size<u32>),
    /// Preserve aspect inside these bounds; smaller sources stay unchanged.
    FitWithin(Size<u32>),
    /// Round each rotated dimension to this percentage, with a one-pixel floor.
    /// Valid percentages are `1..=100`.
    Percent(u8),
}

impl ImageResize {
    fn dimensions(self, source: Size<u32>) -> Result<Size<u32>, TransformError> {
        let output = match self {
            Self::Original => source,
            Self::Exact(size) => size,
            Self::FitWithin(size) => {
                if size.width == 0 || size.height == 0 {
                    return Err(TransformError::InvalidResize);
                }
                if source.width <= size.width && source.height <= size.height {
                    return Ok(source);
                }
                if u64::from(size.width) * u64::from(source.height)
                    <= u64::from(size.height) * u64::from(source.width)
                {
                    Size::new(
                        size.width,
                        ((u64::from(source.height) * u64::from(size.width))
                            / u64::from(source.width))
                        .max(1) as u32,
                    )
                } else {
                    Size::new(
                        ((u64::from(source.width) * u64::from(size.height))
                            / u64::from(source.height))
                        .max(1) as u32,
                        size.height,
                    )
                }
            }
            Self::Percent(percent) => {
                if !(1..=100).contains(&percent) {
                    return Err(TransformError::InvalidResize);
                }
                let scale = |n| ((u64::from(n) * u64::from(percent) + 50) / 100).max(1) as u32;
                Size::new(scale(source.width), scale(source.height))
            }
        };
        if output.width == 0 || output.height == 0 {
            return Err(TransformError::InvalidResize);
        }
        if output.width > source.width || output.height > source.height {
            return Err(TransformError::Enlargement);
        }
        Ok(output)
    }
}

/// A validated recipe tied to the dimensions of its orientation-adjusted source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageTransform {
    source: Size<u32>,
    crop: PixelCrop,
    rotation: QuarterTurn,
    flips: (bool, bool),
    resize: ImageResize,
    output: Size<u32>,
}

impl ImageTransform {
    /// Start with the whole source and no pixel changes.
    ///
    /// # Errors
    /// Rejects zero source dimensions.
    pub fn new(source: Size<u32>) -> Result<Self, TransformError> {
        if source.width == 0 || source.height == 0 {
            return Err(TransformError::EmptySource);
        }
        Ok(Self {
            source,
            crop: PixelCrop::new(0, 0, source.width, source.height, source)?,
            rotation: QuarterTurn::None,
            flips: (false, false),
            resize: ImageResize::Original,
            output: source,
        })
    }

    /// Replace the crop while preserving rotation, flips and resize policy.
    ///
    /// # Errors
    /// Rejects an out-of-source crop or a resulting resize enlargement.
    pub fn crop(mut self, crop: PixelCrop) -> Result<Self, TransformError> {
        crop.validate(self.source)?;
        self.crop = crop;
        self.resolve()
    }

    /// Replace clockwise rotation, retaining the crop and resize policy.
    ///
    /// # Errors
    /// Rejects a retained exact resize that would enlarge a rotated axis.
    pub fn rotate(mut self, rotation: QuarterTurn) -> Result<Self, TransformError> {
        self.rotation = rotation;
        self.resolve()
    }

    /// Flip on cropped source axes before rotation.
    #[must_use]
    pub fn flip(mut self, horizontal: bool, vertical: bool) -> Self {
        self.flips = (horizontal, vertical);
        self
    }

    /// Replace the final resize policy.
    ///
    /// # Errors
    /// Rejects empty sizes, invalid percentages or enlargement.
    pub fn resize(mut self, resize: ImageResize) -> Result<Self, TransformError> {
        self.resize = resize;
        self.resolve()
    }

    /// Source dimensions required by this recipe.
    #[must_use]
    pub const fn source_dimensions(self) -> Size<u32> {
        self.source
    }
    /// Integer source rectangle to copy before other operations.
    #[must_use]
    pub const fn source_crop(self) -> PixelCrop {
        self.crop
    }
    /// Rotation applied after cropped-axis flips.
    #[must_use]
    pub const fn rotation(self) -> QuarterTurn {
        self.rotation
    }
    /// Horizontal and vertical flip flags, in cropped source axes.
    #[must_use]
    pub const fn flips(self) -> (bool, bool) {
        self.flips
    }
    /// Predicted final dimensions, without decoding or allocation.
    #[must_use]
    pub const fn output_dimensions(self) -> Size<u32> {
        self.output
    }

    fn resolve(mut self) -> Result<Self, TransformError> {
        let size = self.crop.dimensions();
        let rotated = match self.rotation {
            QuarterTurn::Clockwise90 | QuarterTurn::Clockwise270 => {
                Size::new(size.height, size.width)
            }
            _ => size,
        };
        self.output = self.resize.dimensions(rotated)?;
        Ok(self)
    }
}

/// Why a recipe could not be validated without pixel work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformError {
    /// Source dimensions are zero.
    EmptySource,
    /// Crop is empty, nonfinite or outside its source.
    InvalidCrop,
    /// Resize dimensions or percentage are invalid.
    InvalidResize,
    /// A resize would increase at least one rotated dimension.
    Enlargement,
}

impl std::fmt::Display for TransformError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::EmptySource => "image source is empty",
            Self::InvalidCrop => "crop is empty or outside its source",
            Self::InvalidResize => "invalid image resize dimensions or percentage",
            Self::Enlargement => "image resize would enlarge a rotated axis",
        })
    }
}

impl std::error::Error for TransformError {}
