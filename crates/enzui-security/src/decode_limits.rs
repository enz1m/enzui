//! Inclusive limits and typed verdicts for encoded image size and decoded dimensions.
//!
//! Combined checks examine encoded length before dimensions. Zero limits reject nonzero
//! values, `UNLIMITED` uses each type's maximum, and RGBA byte-count overflow is refused.

/// Per-asset inclusive decode ceilings.
///
/// Zero rejects every nonzero value; use [`DecodeLimits::UNLIMITED`] for size-unbounded checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DecodeLimits {
    /// Largest encoded payload admitted for decoding, in bytes.
    pub max_encoded_bytes: u64,
    /// Largest permitted decoded width, in pixels.
    pub max_width: u32,
    /// Largest permitted decoded height, in pixels.
    pub max_height: u32,
    /// Largest permitted decoded pixel count (`width * height`).
    pub max_pixels: u64,
    /// Largest permitted decoded RGBA8 size in bytes (`width * height * 4`).
    pub max_rgba_bytes: u64,
}

/// Bytes per pixel in the decoded RGBA8 buffer enzUI targets.
pub const RGBA_BYTES_PER_PIXEL: u64 = 4;

impl DecodeLimits {
    /// Defaults to 16 MiB encoded, 16,384 pixels per axis, 64 MiPixels, and 256 MiB RGBA8.
    pub const DEFAULT: DecodeLimits = DecodeLimits {
        max_encoded_bytes: 16 * 1024 * 1024,
        max_width: 16_384,
        max_height: 16_384,
        max_pixels: 64 * 1024 * 1024,
        max_rgba_bytes: 256 * 1024 * 1024,
    };

    /// Uses each field’s maximum so only zero dimensions and arithmetic overflow are refused.
    pub const UNLIMITED: DecodeLimits = DecodeLimits {
        max_encoded_bytes: u64::MAX,
        max_width: u32::MAX,
        max_height: u32::MAX,
        max_pixels: u64::MAX,
        max_rgba_bytes: u64::MAX,
    };

    /// Returns `width * height` widened to `u64`, which cannot overflow for `u32` inputs.
    #[must_use]
    pub const fn pixels(width: u32, height: u32) -> u64 {
        (width as u64) * (height as u64)
    }

    /// Returns the RGBA8 byte count for `pixels`, or `None` on `u64` overflow.
    #[must_use]
    pub const fn rgba_bytes(pixels: u64) -> Option<u64> {
        pixels.checked_mul(RGBA_BYTES_PER_PIXEL)
    }

    /// Checks encoded bytes against the inclusive ceiling for pre-decode admission.
    #[must_use]
    pub const fn check_encoded(&self, encoded_bytes: u64) -> DecodeVerdict {
        if encoded_bytes > self.max_encoded_bytes {
            return DecodeVerdict::EncodedTooLarge {
                bytes: encoded_bytes,
                limit: self.max_encoded_bytes,
            };
        }
        DecodeVerdict::Allow
    }

    /// Checks zero dimensions, width, height, pixel count, and RGBA bytes in that order.
    #[must_use]
    pub const fn check_decoded(&self, width: u32, height: u32) -> DecodeVerdict {
        if width == 0 || height == 0 {
            return DecodeVerdict::ZeroDimension { width, height };
        }
        if width > self.max_width {
            return DecodeVerdict::WidthTooLarge {
                width,
                limit: self.max_width,
            };
        }
        if height > self.max_height {
            return DecodeVerdict::HeightTooLarge {
                height,
                limit: self.max_height,
            };
        }
        let pixels = Self::pixels(width, height);
        if pixels > self.max_pixels {
            return DecodeVerdict::TooManyPixels {
                pixels,
                limit: self.max_pixels,
            };
        }
        match Self::rgba_bytes(pixels) {
            None => DecodeVerdict::RgbaOverflow { pixels },
            Some(rgba) => {
                if rgba > self.max_rgba_bytes {
                    return DecodeVerdict::RgbaTooLarge {
                        bytes: rgba,
                        limit: self.max_rgba_bytes,
                    };
                }
                DecodeVerdict::Allow
            }
        }
    }

    /// Run both gates in order (encoded first, then dimensions), for the case
    /// where the payload size and the declared dimensions are already known.
    #[must_use]
    pub const fn check(&self, encoded_bytes: u64, width: u32, height: u32) -> DecodeVerdict {
        match self.check_encoded(encoded_bytes) {
            DecodeVerdict::Allow => self.check_decoded(width, height),
            refused => refused,
        }
    }
}

impl Default for DecodeLimits {
    fn default() -> Self {
        DecodeLimits::DEFAULT
    }
}

/// A decode decision that preserves the specific failed limit and measured value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DecodeVerdict {
    /// The asset satisfies every limit.
    Allow,
    /// The compressed payload exceeded [`DecodeLimits::max_encoded_bytes`].
    EncodedTooLarge {
        /// The observed encoded size in bytes.
        bytes: u64,
        /// The configured inclusive byte limit.
        limit: u64,
    },
    /// A declared dimension is zero.
    ZeroDimension {
        /// The declared width.
        width: u32,
        /// The declared height.
        height: u32,
    },
    /// The declared width exceeded [`DecodeLimits::max_width`].
    WidthTooLarge {
        /// The width that was refused.
        width: u32,
        /// The configured inclusive width limit.
        limit: u32,
    },
    /// The declared height exceeded [`DecodeLimits::max_height`].
    HeightTooLarge {
        /// The height that was refused.
        height: u32,
        /// The configured inclusive height limit.
        limit: u32,
    },
    /// The decoded area exceeds [`DecodeLimits::max_pixels`].
    TooManyPixels {
        /// The computed pixel count.
        pixels: u64,
        /// The configured inclusive pixel limit.
        limit: u64,
    },
    /// The decoded RGBA buffer would exceed [`DecodeLimits::max_rgba_bytes`].
    RgbaTooLarge {
        /// The computed RGBA8 size in bytes.
        bytes: u64,
        /// The configured inclusive RGBA8 byte limit.
        limit: u64,
    },
    /// The RGBA8 byte count overflowed `u64`.
    RgbaOverflow {
        /// The pixel count whose RGBA size could not be represented.
        pixels: u64,
    },
}

impl DecodeVerdict {
    /// Whether the decode may proceed.
    #[must_use]
    pub const fn is_allowed(self) -> bool {
        matches!(self, DecodeVerdict::Allow)
    }

    /// Returns whether the decode was refused.
    #[must_use]
    pub const fn is_refused(self) -> bool {
        !self.is_allowed()
    }

    /// A short, stable, allocation-free tag naming the limit that refused —
    /// suitable for a log line or a metric label. `"allow"` when permitted.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            DecodeVerdict::Allow => "allow",
            DecodeVerdict::EncodedTooLarge { .. } => "encoded-too-large",
            DecodeVerdict::ZeroDimension { .. } => "zero-dimension",
            DecodeVerdict::WidthTooLarge { .. } => "width-too-large",
            DecodeVerdict::HeightTooLarge { .. } => "height-too-large",
            DecodeVerdict::TooManyPixels { .. } => "too-many-pixels",
            DecodeVerdict::RgbaTooLarge { .. } => "rgba-too-large",
            DecodeVerdict::RgbaOverflow { .. } => "rgba-overflow",
        }
    }
}

/// A [`DecodeVerdict`] guaranteed not to be [`DecodeVerdict::Allow`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RefusedVerdict(DecodeVerdict);

impl RefusedVerdict {
    /// Wraps a refused verdict. Returns `None` for [`DecodeVerdict::Allow`].
    #[must_use]
    pub fn new(verdict: DecodeVerdict) -> Option<Self> {
        if verdict.is_allowed() {
            None
        } else {
            Some(Self(verdict))
        }
    }

    /// Constructs a [`DecodeVerdict::RgbaTooLarge`] refusal.
    #[must_use]
    pub const fn rgba_too_large(bytes: u64, limit: u64) -> Self {
        Self(DecodeVerdict::RgbaTooLarge { bytes, limit })
    }

    /// Constructs a [`DecodeVerdict::RgbaOverflow`] refusal.
    #[must_use]
    pub const fn rgba_overflow(pixels: u64) -> Self {
        Self(DecodeVerdict::RgbaOverflow { pixels })
    }

    /// The inner [`DecodeVerdict`], guaranteed not to be `Allow`.
    #[must_use]
    pub fn verdict(self) -> DecodeVerdict {
        self.0
    }

    /// Forwarded from [`DecodeVerdict::reason`] — a short, stable,
    /// allocation-free tag. Never `"allow"`.
    #[must_use]
    pub fn reason(self) -> &'static str {
        self.0.reason()
    }
}

impl TryFrom<DecodeVerdict> for RefusedVerdict {
    type Error = ();

    /// Succeeds for every refused verdict; fails for [`DecodeVerdict::Allow`].
    fn try_from(verdict: DecodeVerdict) -> Result<Self, ()> {
        Self::new(verdict).ok_or(())
    }
}

impl core::fmt::Display for RefusedVerdict {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.reason())
    }
}

#[cfg(test)]
mod tests;
