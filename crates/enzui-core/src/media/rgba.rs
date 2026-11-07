//! Checked SDR RGBA8 video frames and non-owning native-buffer preflight.
//!
//! [`preflight_rgba8`] validates native geometry before allocation or copying, while [`Rgba8Frame::new`] validates the final tightly packed buffer.

use crate::decode_limits::{DecodeLimits, RefusedVerdict};
use crate::frame_plan::FrameSeq;
use crate::media::Generation;

/// The number of bytes one RGBA8 pixel occupies: R, G, B, A at 8 bits each.
pub const BYTES_PER_PIXEL: usize = 4;

/// Maximum accepted frame edge in pixels, used as an allocation-safety bound rather than a supported-resolution claim.
pub const MAX_EDGE: u32 = 16_384;

/// Maximum native RGBA plane extent, independent of the tight-output limit in [`DecodeLimits`].
pub const MAX_NATIVE_RGBA_BYTES: u64 = DecodeLimits::DEFAULT.max_rgba_bytes;

/// A non-owning native RGBA/BGRA layout whose indexing lengths fit `usize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbaGeometry {
    row_bytes: usize,
    stride: usize,
    tight_len: usize,
    readable_len: usize,
}

impl RgbaGeometry {
    /// Tight bytes in one output row (`width * 4`).
    #[must_use]
    pub const fn row_bytes(self) -> usize {
        self.row_bytes
    }

    /// Native bytes between the starts of adjacent rows.
    #[must_use]
    pub const fn stride(self) -> usize {
        self.stride
    }

    /// Bytes in the tightly packed destination (`width * height * 4`).
    #[must_use]
    pub const fn tight_len(self) -> usize {
        self.tight_len
    }

    /// Minimum bytes the borrowed native plane must expose.
    ///
    /// This is the end of the last pixel row,
    /// `(height - 1) * stride + row_bytes`; padding after that row is not read.
    #[must_use]
    pub const fn readable_len(self) -> usize {
        self.readable_len
    }

    /// Validates the borrowed plane length, returning [`RgbaGeometryError::BufferTooShort`] when announced rows do not fit and otherwise returning the layout unchanged.
    ///
    /// # Errors
    ///
    /// [`RgbaGeometryError::BufferTooShort`] when the announced rows do not fit in `actual` bytes.
    pub fn check_byte_len(self, actual: usize) -> Result<Self, RgbaGeometryError> {
        if actual < self.readable_len {
            Err(RgbaGeometryError::BufferTooShort {
                required: self.readable_len,
                actual,
            })
        } else {
            Ok(self)
        }
    }
}

/// A geometry or storage refusal from [`preflight_rgba8`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RgbaGeometryError {
    /// Width, height, pixel count, or tight RGBA bytes exceeded decode policy.
    DecodeLimit(RefusedVerdict),
    /// The native row stride cannot contain one complete pixel row.
    StrideTooSmall {
        /// Announced native stride.
        stride: u64,
        /// Minimum bytes needed for one pixel row.
        row_bytes: u64,
    },
    /// A negative native stride requiring unsupported bottom-up row traversal.
    NegativeStride {
        /// Announced native stride.
        stride: i32,
    },
    /// One native row alone exceeds the native-plane safety ceiling.
    StrideTooLarge {
        /// Announced native stride.
        stride: u64,
        /// Native-plane safety ceiling.
        limit: u64,
    },
    /// `(height - 1) * stride + row_bytes` overflowed `u64`.
    ReadableExtentOverflow {
        /// Announced row count.
        height: u32,
        /// Announced native stride.
        stride: u64,
        /// Bytes read from the final row.
        row_bytes: u64,
    },
    /// The readable native-plane extent exceeds the native safety ceiling.
    ReadableExtentTooLarge {
        /// Required readable bytes (`(height - 1) * stride + row_bytes`).
        bytes: u64,
        /// Native-plane safety ceiling.
        limit: u64,
    },
    /// A complete native backing buffer would exceed the safe mapping ceiling.
    ///
    /// GStreamer can merge every memory block when mapping a buffer, so this
    /// bound covers the complete logical buffer rather than only plane 0.
    BackingExtentTooLarge {
        /// Bytes in the complete native backing buffer.
        bytes: u64,
        /// Native mapping safety ceiling.
        limit: u64,
    },
    /// Full native row storage (`height * stride`) overflowed `u64`.
    StorageExtentOverflow {
        /// Announced row count.
        height: u32,
        /// Announced native stride.
        stride: u64,
    },
    /// Full native row storage exceeds the native safety ceiling.
    StorageExtentTooLarge {
        /// Required full storage bytes (`height * stride`).
        bytes: u64,
        /// Native-plane safety ceiling.
        limit: u64,
    },
    /// A validated length cannot be represented by this target's `usize`.
    PlatformLengthOverflow {
        /// Length which did not fit.
        bytes: u64,
    },
    /// The borrowed plane does not contain all announced rows.
    BufferTooShort {
        /// Minimum source bytes required by the geometry.
        required: usize,
        /// Bytes actually exposed by the native plane.
        actual: usize,
    },
}

impl core::fmt::Display for RgbaGeometryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DecodeLimit(verdict) => write!(f, "RGBA geometry refused: {}", verdict.reason()),
            Self::StrideTooSmall { stride, row_bytes } => write!(
                f,
                "RGBA stride {stride} is smaller than the {row_bytes}-byte pixel row"
            ),
            Self::NegativeStride { stride } => {
                write!(f, "negative RGBA stride {stride} is unsupported")
            }
            Self::StrideTooLarge { stride, limit } => write!(
                f,
                "RGBA stride {stride} exceeds the {limit}-byte native-plane limit"
            ),
            Self::ReadableExtentOverflow {
                height,
                stride,
                row_bytes,
            } => write!(
                f,
                "RGBA readable extent overflows for {height} rows at stride {stride} and {row_bytes} bytes in the final row"
            ),
            Self::ReadableExtentTooLarge { bytes, limit } => write!(
                f,
                "RGBA readable source extent {bytes} exceeds the {limit}-byte native-plane limit"
            ),
            Self::BackingExtentTooLarge { bytes, limit } => write!(
                f,
                "RGBA native backing extent {bytes} exceeds the {limit}-byte mapping limit"
            ),
            Self::StorageExtentOverflow { height, stride } => write!(
                f,
                "RGBA full storage extent overflows for {height} rows at stride {stride}"
            ),
            Self::StorageExtentTooLarge { bytes, limit } => write!(
                f,
                "RGBA full storage extent {bytes} exceeds the {limit}-byte native-plane limit"
            ),
            Self::PlatformLengthOverflow { bytes } => {
                write!(f, "RGBA byte length {bytes} does not fit this platform")
            }
            Self::BufferTooShort { required, actual } => write!(
                f,
                "RGBA source has {actual} bytes, fewer than the {required} needed to reach its final pixel row"
            ),
        }
    }
}

impl std::error::Error for RgbaGeometryError {}

/// Validates borrowed RGBA/BGRA geometry before destination allocation or copying.
///
/// The result proves tight-output limits and `usize` representability. Call [`RgbaGeometry::check_byte_len`] before reading the plane; native padding is checked against [`MAX_NATIVE_RGBA_BYTES`] separately.
///
/// # Errors
///
/// An [`RgbaGeometryError`] when the geometry breaks the decode limits, the stride is too small or
/// too large, or the readable extent overflows.
#[allow(
    clippy::missing_panics_doc,
    reason = "panics only on a broken invariant: the decode limits already accepted this byte \
              product"
)]
pub fn preflight_rgba8(
    width: u32,
    height: u32,
    stride: u64,
    limits: &DecodeLimits,
) -> Result<RgbaGeometry, RgbaGeometryError> {
    if let Some(refused) = RefusedVerdict::new(limits.check_decoded(width, height)) {
        return Err(RgbaGeometryError::DecodeLimit(refused));
    }

    let row_bytes = u64::from(width) * BYTES_PER_PIXEL as u64;
    if stride < row_bytes {
        return Err(RgbaGeometryError::StrideTooSmall { stride, row_bytes });
    }
    // Native stride uses the independent native-plane ceiling, not the caller's tight-output limit.
    if stride > MAX_NATIVE_RGBA_BYTES {
        return Err(RgbaGeometryError::StrideTooLarge {
            stride,
            limit: MAX_NATIVE_RGBA_BYTES,
        });
    }
    let readable_len = stride
        .checked_mul(u64::from(height - 1))
        .and_then(|last_row_offset| last_row_offset.checked_add(row_bytes))
        .ok_or(RgbaGeometryError::ReadableExtentOverflow {
            height,
            stride,
            row_bytes,
        })?;
    if readable_len > MAX_NATIVE_RGBA_BYTES {
        return Err(RgbaGeometryError::ReadableExtentTooLarge {
            bytes: readable_len,
            limit: MAX_NATIVE_RGBA_BYTES,
        });
    }
    let tight_len = DecodeLimits::rgba_bytes(DecodeLimits::pixels(width, height))
        .expect("check_decoded accepted the RGBA byte product");

    let to_usize = |bytes| {
        usize::try_from(bytes).map_err(|_| RgbaGeometryError::PlatformLengthOverflow { bytes })
    };
    Ok(RgbaGeometry {
        row_bytes: to_usize(row_bytes)?,
        stride: to_usize(stride)?,
        tight_len: to_usize(tight_len)?,
        readable_len: to_usize(readable_len)?,
    })
}

/// Validates complete native row storage, including padding after the final row.
///
/// # Errors
///
/// As [`preflight_rgba8`], and when the full row storage overflows, exceeds its bound or does not
/// fit in `usize`.
pub fn preflight_rgba8_storage(
    width: u32,
    height: u32,
    stride: u64,
    limits: &DecodeLimits,
) -> Result<(RgbaGeometry, usize), RgbaGeometryError> {
    let layout = preflight_rgba8(width, height, stride, limits)?;
    let storage_len = u64::from(height)
        .checked_mul(stride)
        .ok_or(RgbaGeometryError::StorageExtentOverflow { height, stride })?;
    if storage_len > MAX_NATIVE_RGBA_BYTES {
        return Err(RgbaGeometryError::StorageExtentTooLarge {
            bytes: storage_len,
            limit: MAX_NATIVE_RGBA_BYTES,
        });
    }
    let storage_len = usize::try_from(storage_len)
        .map_err(|_| RgbaGeometryError::PlatformLengthOverflow { bytes: storage_len })?;
    Ok((layout, storage_len))
}

/// A reason an owned RGBA8 buffer was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameFormatError {
    /// A zero width or height, which cannot represent a video frame.
    ZeroDimension {
        /// The claimed width.
        width: u32,
        /// The claimed height.
        height: u32,
    },
    /// An edge exceeding [`MAX_EDGE`].
    TooLarge {
        /// The claimed width.
        width: u32,
        /// The claimed height.
        height: u32,
    },
    /// The buffer length disagrees with `width * height * 4`.
    BufferSizeMismatch {
        /// The length `width * height * 4` requires.
        expected: usize,
        /// The length the buffer actually has.
        actual: usize,
    },
}

impl core::fmt::Display for FrameFormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameFormatError::ZeroDimension { width, height } => {
                write!(f, "zero-area frame: {width}x{height}")
            }
            FrameFormatError::TooLarge { width, height } => {
                write!(
                    f,
                    "frame {width}x{height} exceeds the {MAX_EDGE}px edge bound"
                )
            }
            FrameFormatError::BufferSizeMismatch { expected, actual } => {
                write!(
                    f,
                    "RGBA8 buffer length {actual} does not match the {expected} \
                     bytes the claimed geometry requires"
                )
            }
        }
    }
}

impl std::error::Error for FrameFormatError {}

/// An owned SDR RGBA8 frame whose buffer length matches its dimensions.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgba8Frame {
    generation: Generation,
    seq: FrameSeq,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Rgba8Frame {
    /// Validates a tightly packed SDR RGBA8 buffer.
    ///
    /// Zero-area frames, edges above [`MAX_EDGE`], and lengths other than `width * height * 4` are refused. Adapter-specific [`DecodeLimits`] apply separately before native allocation or copying.
    ///
    /// # Errors
    ///
    /// A [`FrameFormatError`] for a zero-area frame, an edge above [`MAX_EDGE`], or a buffer of the
    /// wrong length.
    pub fn new(
        generation: Generation,
        seq: FrameSeq,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    ) -> Result<Rgba8Frame, FrameFormatError> {
        if width == 0 || height == 0 {
            return Err(FrameFormatError::ZeroDimension { width, height });
        }
        if width > MAX_EDGE || height > MAX_EDGE {
            return Err(FrameFormatError::TooLarge { width, height });
        }
        // usize math on already-bounded edges: MAX_EDGE^2 * 4 fits in u64 and
        // in usize on every target this runs on, so this cannot wrap.
        let expected = width as usize * height as usize * BYTES_PER_PIXEL;
        if pixels.len() != expected {
            return Err(FrameFormatError::BufferSizeMismatch {
                expected,
                actual: pixels.len(),
            });
        }
        Ok(Rgba8Frame {
            generation,
            seq,
            width,
            height,
            pixels,
        })
    }

    /// The generation of the source that produced this frame.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// This frame's sequence number within its generation.
    #[must_use]
    pub fn seq(&self) -> FrameSeq {
        self.seq
    }

    /// The frame width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The frame height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA8 pixel bytes, `width * height * 4` long.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns the pixel buffer length in bytes.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.pixels.len()
    }

    /// Consumes the frame and returns its pixel buffer.
    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

/// `Debug` omits pixel bytes and reports their length to avoid logging large frame buffers.
impl core::fmt::Debug for Rgba8Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Rgba8Frame")
            .field("generation", &self.generation)
            .field("seq", &self.seq)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gen_first() -> Generation {
        Generation::FIRST
    }

    #[test]
    fn a_buffer_one_byte_short_is_refused() {
        let err = Rgba8Frame::new(gen_first(), FrameSeq(1), 4, 2, vec![0u8; 31])
            .expect_err("31 bytes cannot be a 4x2 RGBA8 frame");
        assert_eq!(
            err,
            FrameFormatError::BufferSizeMismatch {
                expected: 32,
                actual: 31
            }
        );
    }
}
