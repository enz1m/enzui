//! Bounded, magic-driven static raster decoding.
//!
//! The default format set is deliberately short: PNG, JPEG, static WebP and
//! BMP. GIF is excluded with the other animated formats; TIFF and ICO remain
//! excluded because each additional parser is attack surface reached from
//! untrusted remote bytes. The set is not intended to mirror `image`'s broad
//! default feature set.
//!
//! HEIC (HEVC-branded HEIF) is available through the off-by-default `heif` feature. It
//! runtime-loads system libheif so a feature-enabled application still reports
//! [`DecodeError::Unsupported`] when the library or HEVC decoder plugin is
//! absent. See the feature module for the native-code and HEVC licensing costs
//! and why a freedesktop helper subprocess was rejected without a sandbox.
//!
//! AVIF is still intentionally unsupported. `image` 0.25's actual decoder gate
//! is `avif-native` (not `avif`, which enables its encoder) and adds mp4parse
//! plus the C dav1d stack. More importantly, `AvifDecoder::new` decodes the AV1
//! bitstream before exposing dimensions, so adding that feature line would
//! allocate before [`DecodeLimits`] can screen the declared size. AVIF should
//! land only with a header-first admission path, not by weakening the invariant
//! to make the format list longer.
//!
//! The extension is never trusted: the format comes from magic bytes,
//! dimensions are read from the decoder header, and [`DecodeLimits`] runs
//! before full pixel allocation. PNG header/ancillary storage has an allocation
//! ceiling of `max_rgba_bytes` with a 64 KiB working minimum; oversized ICC
//! profiles may be omitted by the codec. The image-crate path also checks the final
//! orientation-adjusted RGBA dimensions before constructing an iced handle.

#[cfg(feature = "heif")]
mod heif;

use enzui_core::async_image::EncodedImage;
use enzui_core::decode_limits::{DecodeLimits, RefusedVerdict};
use iced_core::image::Handle;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits, metadata::Orientation};
use std::fmt;
use std::io::{BufReader, Cursor};

/// The explicitly recognized remote static raster formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StaticFormat {
    /// Portable Network Graphics, excluding APNG (`acTL`).
    Png,
    /// Baseline/progressive JPEG. EXIF orientation is applied by default.
    Jpeg,
    /// Static WebP, excluding `ANIM`/`ANMF` and the VP8X animation flag.
    WebP,
    /// Windows bitmap.
    Bmp,
    /// High Efficiency Image File container carrying an HEVC still image.
    Heif,
}

impl StaticFormat {
    fn image_format(self) -> ImageFormat {
        match self {
            StaticFormat::Png => ImageFormat::Png,
            StaticFormat::Jpeg => ImageFormat::Jpeg,
            StaticFormat::WebP => ImageFormat::WebP,
            StaticFormat::Bmp => ImageFormat::Bmp,
            StaticFormat::Heif => unreachable!("HEIF uses the optional native decoder"),
        }
    }
}

/// Why encoded bytes did not become a validated RGBA handle.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// A core size/area/RGBA policy limit refused the payload.
    /// Wrapped in [`RefusedVerdict`] so `DecodeVerdict::Allow` can never
    /// appear inside a refusal.
    Limit(RefusedVerdict),
    /// PNG header or ancillary storage exceeded the configured allocation budget.
    HeaderAllocationLimit {
        /// Working-byte ceiling, with a 64 KiB minimum for small images.
        max_bytes: u64,
    },
    /// Decoder working storage exceeded the requested allocation budget.
    WorkingAllocationLimit {
        /// Configured working-byte ceiling.
        max_bytes: u64,
    },
    /// The content magic is unsupported by the requested decode intent.
    UnsupportedMagic,
    /// A known format has no decoder in this build or on this host.
    Unsupported(StaticFormat),
    /// A recognized container/header is truncated or structurally impossible.
    TruncatedHeader,
    /// The container advertises animation unsupported by the requested decode intent.
    Animated,
    /// The image decoder rejected the payload after its bounded header passed.
    InvalidImage,
    /// A known optional decoder rejected malformed or truncated content.
    DecodeFailed(StaticFormat),
    /// The operation was cancelled before a stable handle was produced.
    Cancelled,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Limit(rv) => write!(f, "decode refused: {}", rv.reason()),
            DecodeError::HeaderAllocationLimit { max_bytes } => {
                write!(f, "PNG header allocation exceeds {max_bytes} bytes")
            }
            DecodeError::WorkingAllocationLimit { max_bytes } => {
                write!(
                    f,
                    "image decode working allocation exceeds {max_bytes} bytes"
                )
            }
            DecodeError::UnsupportedMagic => f.write_str("unsupported image magic"),
            DecodeError::Unsupported(format) => {
                write!(f, "unsupported image decoder: {format:?}")
            }
            DecodeError::TruncatedHeader => f.write_str("truncated image header"),
            DecodeError::Animated => f.write_str("animation is unsupported for this decode intent"),
            DecodeError::InvalidImage => f.write_str("invalid image"),
            DecodeError::DecodeFailed(format) => {
                write!(f, "failed to decode {format:?} image")
            }
            DecodeError::Cancelled => f.write_str("image decode cancelled"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A validated, orientation-adjusted RGBA image and stable iced handle.
///
/// Cloning the handle preserves its iced content ID, so cache hits reuse the
/// renderer identity instead of creating a new upload key for the same pixels.
#[derive(Clone, Debug)]
pub struct DecodedAsset {
    /// Stable iced image handle backed by validated RGBA bytes.
    pub handle: Handle,
    /// Magic-derived static format.
    pub format: StaticFormat,
    /// Final width after orientation.
    pub width: u32,
    /// Final height after orientation.
    pub height: u32,
    /// Encoded source size.
    pub encoded_bytes: u64,
    /// Final RGBA allocation size.
    pub rgba_bytes: u64,
}

impl DecodedAsset {
    /// A synthetic asset of `rgba_bytes`, for cache-accounting tests.
    ///
    /// Only the byte count matters to the decoded LRU, so the handle is a 1x1
    /// placeholder; nothing in the cache path inspects the pixels.
    #[cfg(test)]
    pub(crate) fn synthetic_for_test(rgba_bytes: u64) -> Self {
        Self {
            handle: Handle::from_rgba(1, 1, vec![0u8; 4]),
            format: StaticFormat::Png,
            width: 1,
            height: 1,
            encoded_bytes: rgba_bytes,
            rgba_bytes,
        }
    }
}

/// Decode one static raster using content magic and core limits.
///
/// An embedded RGB ICC profile is converted to sRGB after any resize.
/// `cancelled` is checked before header work, before the expensive full decode,
/// and before handle construction. The decoder itself is synchronous; callers
/// must run this function on the service worker pool, never the UI thread.
///
/// # Errors
///
/// A [`DecodeError`] for an unknown format, a limit breach, a failed decode, or cancellation.
pub fn decode_static(
    bytes: &[u8],
    limits: &DecodeLimits,
    cancelled: impl Fn() -> bool,
) -> Result<DecodedAsset, DecodeError> {
    decode_static_inner(bytes, limits, None, cancelled)
}

/// Decode a static raster and high-quality downsample it to fit within the
/// requested bounds. Smaller images are never enlarged. This is intended for
/// remote icons and thumbnails whose source pixels otherwise rely on a
/// single GPU bilinear sample at draw time, producing visibly stair-stepped
/// edges when a large source is reduced into compact UI chrome.
///
/// # Errors
///
/// As [`decode_static`].
pub fn decode_static_resized(
    bytes: &[u8],
    limits: &DecodeLimits,
    max_width: u32,
    max_height: u32,
    cancelled: impl Fn() -> bool,
) -> Result<DecodedAsset, DecodeError> {
    decode_static_inner(
        bytes,
        limits,
        Some((max_width.max(1), max_height.max(1))),
        cancelled,
    )
}

fn decode_static_inner(
    bytes: &[u8],
    limits: &DecodeLimits,
    target: Option<(u32, u32)>,
    cancelled: impl Fn() -> bool,
) -> Result<DecodedAsset, DecodeError> {
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }

    let encoded_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let encoded_verdict = limits.check_encoded(encoded_bytes);
    if let Some(refused) = RefusedVerdict::new(encoded_verdict) {
        return Err(DecodeError::Limit(refused));
    }

    let format = detect_static_format(bytes)?;
    reject_animation(bytes, format)?;

    if format == StaticFormat::Heif {
        #[cfg(feature = "heif")]
        return finish_static(
            heif::decode(bytes, limits, &cancelled)?,
            format,
            encoded_bytes,
            limits,
            target,
            None,
            &cancelled,
        );
        #[cfg(not(feature = "heif"))]
        return Err(DecodeError::Unsupported(StaticFormat::Heif));
    }

    let (mut decoder, orientation) = open_static_decoder(bytes, format, limits)?;
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }

    let profile = decoder.icc_profile().ok().flatten();
    let mut image = DynamicImage::from_decoder(decoder).map_err(|_| DecodeError::InvalidImage)?;
    image.apply_orientation(orientation);

    finish_static(
        image,
        format,
        encoded_bytes,
        limits,
        target,
        profile.as_deref(),
        &cancelled,
    )
}

/// Converts RGBA rows in place from their embedded ICC `profile` to sRGB, so a
/// Display P3 or Adobe RGB photo shows its own colours on an sRGB surface.
/// One row of scratch at a time keeps the working memory at one row. An sRGB
/// profile, an unreadable one or a non-RGB one leaves the pixels as decoded.
fn convert_to_srgb(pixels: &mut [u8], width: u32, profile: &[u8]) {
    use moxcms::{ColorProfile, DataColorSpace, Layout, TransformOptions};
    let Ok(source) = ColorProfile::new_from_slice(profile) else {
        return;
    };
    let srgb = ColorProfile::new_srgb();
    let near = |a: moxcms::Xyzd, b: moxcms::Xyzd| {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3 && (a.z - b.z).abs() < 1e-3
    };
    let already_srgb = near(source.red_colorant, srgb.red_colorant)
        && near(source.green_colorant, srgb.green_colorant)
        && near(source.blue_colorant, srgb.blue_colorant);
    if source.color_space != DataColorSpace::Rgb || already_srgb {
        return;
    }
    let Ok(transform) = source.create_transform_8bit(
        Layout::Rgba,
        &srgb,
        Layout::Rgba,
        TransformOptions::default(),
    ) else {
        return;
    };
    let row_bytes = width as usize * 4;
    if row_bytes == 0 {
        return;
    }
    let mut converted = vec![0; row_bytes];
    for row in pixels.chunks_exact_mut(row_bytes) {
        if transform.transform(row, &mut converted).is_err() {
            return;
        }
        row.copy_from_slice(&converted);
    }
}

fn open_static_decoder<'a>(
    bytes: &'a [u8],
    format: StaticFormat,
    limits: &DecodeLimits,
) -> Result<(Box<dyn ImageDecoder + 'a>, Orientation), DecodeError> {
    let encoded_bytes = bytes.len() as u64;
    let header_budget = limits.max_rgba_bytes.max(64 * 1024);
    if format == StaticFormat::Png
        && let Some(header) = bytes.get(8..33)
        && header[..8] == *b"\x00\x00\x00\x0dIHDR"
    {
        let width = be_u32(header, 8)?;
        let height = be_u32(header, 12)?;
        let declared = EncodedImage::new(encoded_bytes, width, height).screen(limits);
        if let Some(refused) = RefusedVerdict::new(declared) {
            return Err(DecodeError::Limit(refused));
        }
    }
    let mut reader =
        ImageReader::with_format(BufReader::new(Cursor::new(bytes)), format.image_format());
    if format == StaticFormat::Png {
        // PNG constructs ancillary storage before exposing decoder dimensions.
        let mut header_limits = Limits::default();
        header_limits.max_alloc = Some(header_budget);
        reader.limits(header_limits);
    } else {
        reader.no_limits();
    }
    let mut decoder = reader.into_decoder().map_err(|error| {
        if format == StaticFormat::Png && matches!(error, image::ImageError::Limits(_)) {
            DecodeError::HeaderAllocationLimit {
                max_bytes: header_budget,
            }
        } else {
            DecodeError::TruncatedHeader
        }
    })?;
    let (width, height) = decoder.dimensions();
    let before = EncodedImage::new(encoded_bytes, width, height).screen(limits);
    if let Some(refused) = RefusedVerdict::new(before) {
        return Err(DecodeError::Limit(refused));
    }

    // Pixel allocation must remain bounded after header admission and orientation.
    let mut image_limits = Limits::default();
    image_limits.max_image_width = Some(limits.max_width);
    image_limits.max_image_height = Some(limits.max_height);
    image_limits.max_alloc = Some(limits.max_rgba_bytes);
    decoder
        .set_limits(image_limits)
        .map_err(|_| DecodeError::InvalidImage)?;

    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    Ok((Box::new(decoder), orientation))
}

#[cfg(feature = "local-images")]
pub(crate) fn probe_static(
    bytes: &[u8],
    limits: &DecodeLimits,
    cancelled: &impl Fn() -> bool,
) -> Result<(StaticFormat, (u32, u32), u8), DecodeError> {
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }
    if let Some(refused) = RefusedVerdict::new(limits.check_encoded(bytes.len() as u64)) {
        return Err(DecodeError::Limit(refused));
    }
    let format = detect_static_format(bytes)?;
    reject_animation(bytes, format)?;
    if format == StaticFormat::Heif {
        #[cfg(feature = "heif")]
        return heif::probe(bytes, limits, cancelled).map(|size| (format, size, 1));
        #[cfg(not(feature = "heif"))]
        return Err(DecodeError::Unsupported(format));
    }
    let (decoder, orientation) = open_static_decoder(bytes, format, limits)?;
    let (width, height) = decoder.dimensions();
    let size = match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (height, width),
        _ => (width, height),
    };
    if let Some(refused) = RefusedVerdict::new(limits.check_decoded(size.0, size.1)) {
        return Err(DecodeError::Limit(refused));
    }
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }
    Ok((format, size, orientation.to_exif()))
}

#[cfg(feature = "local-images")]
pub(crate) fn static_available(format: StaticFormat) -> bool {
    if format != StaticFormat::Heif {
        return true;
    }
    #[cfg(feature = "heif")]
    {
        heif::available()
    }
    #[cfg(not(feature = "heif"))]
    {
        false
    }
}

fn finish_static(
    image: DynamicImage,
    format: StaticFormat,
    encoded_bytes: u64,
    limits: &DecodeLimits,
    target: Option<(u32, u32)>,
    profile: Option<&[u8]>,
    cancelled: &impl Fn() -> bool,
) -> Result<DecodedAsset, DecodeError> {
    let raster = finish_raster(image, encoded_bytes, limits, target, profile, cancelled)?;
    Ok(DecodedAsset {
        handle: raster.handle,
        format,
        width: raster.width,
        height: raster.height,
        encoded_bytes,
        rgba_bytes: raster.rgba_bytes,
    })
}

pub(crate) struct FinishedRaster {
    pub(crate) handle: Handle,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba_bytes: u64,
}

/// Resizes to `target`, converts from the embedded ICC `profile` to sRGB and
/// admits the result.
pub(crate) fn finish_raster(
    mut image: DynamicImage,
    encoded_bytes: u64,
    limits: &DecodeLimits,
    target: Option<(u32, u32)>,
    profile: Option<&[u8]>,
    cancelled: &impl Fn() -> bool,
) -> Result<FinishedRaster, DecodeError> {
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }

    if let Some((max_width, max_height)) = target {
        let (width, height) = (image.width(), image.height());
        if width > max_width || height > max_height {
            image = image.resize(max_width, max_height, image::imageops::FilterType::Lanczos3);
        }
    }

    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    let after = EncodedImage::new(encoded_bytes, width, height).screen(limits);
    if let Some(refused) = RefusedVerdict::new(after) {
        return Err(DecodeError::Limit(refused));
    }

    let pixels = rgba.into_raw();
    let rgba_bytes = u64::try_from(pixels.len()).unwrap_or(u64::MAX);
    if rgba_bytes > limits.max_rgba_bytes {
        return Err(DecodeError::Limit(RefusedVerdict::rgba_too_large(
            rgba_bytes,
            limits.max_rgba_bytes,
        )));
    }

    if cancelled() {
        return Err(DecodeError::Cancelled);
    }

    let mut pixels = pixels;
    if let Some(profile) = profile {
        convert_to_srgb(&mut pixels, width, profile);
    }
    let handle = Handle::from_rgba(width, height, pixels);
    Ok(FinishedRaster {
        handle,
        width,
        height,
        rgba_bytes,
    })
}

pub(crate) fn detect_static_format(bytes: &[u8]) -> Result<StaticFormat, DecodeError> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(StaticFormat::Png)
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok(StaticFormat::Jpeg)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Ok(StaticFormat::WebP)
    } else if bytes.starts_with(b"BM") {
        Ok(StaticFormat::Bmp)
    } else if is_heif(bytes) {
        Ok(StaticFormat::Heif)
    } else {
        Err(DecodeError::UnsupportedMagic)
    }
}

fn is_heif(bytes: &[u8]) -> bool {
    if bytes.len() < 16 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let Ok(box_size) = be_u32(bytes, 0) else {
        return false;
    };
    let box_size = box_size as usize;
    if box_size < 16 || box_size > bytes.len() {
        return false;
    }

    // `ftyp`: size + type + major_brand + minor_version + compatible brands.
    // Search only brand slots inside this box, never coincidental payload bytes.
    std::iter::once(&bytes[8..12])
        .chain(
            bytes[16..box_size]
                .as_chunks::<4>()
                .0
                .iter()
                .map(<[u8; 4]>::as_slice),
        )
        .any(|brand| matches!(brand, b"heic" | b"heix" | b"hevc" | b"hevx"))
}

fn reject_animation(bytes: &[u8], format: StaticFormat) -> Result<(), DecodeError> {
    match format {
        StaticFormat::Png => reject_apng(bytes),
        StaticFormat::WebP => reject_animated_webp(bytes),
        StaticFormat::Jpeg | StaticFormat::Bmp | StaticFormat::Heif => Ok(()),
    }
}

/// The big-endian `u32` at `at`, or [`DecodeError::TruncatedHeader`] past the end.
pub(crate) fn be_u32(bytes: &[u8], at: usize) -> Result<u32, DecodeError> {
    word(bytes, at).map(u32::from_be_bytes)
}

/// The little-endian `u32` at `at`, or [`DecodeError::TruncatedHeader`] past the end.
pub(crate) fn le_u32(bytes: &[u8], at: usize) -> Result<u32, DecodeError> {
    word(bytes, at).map(u32::from_le_bytes)
}

/// The little-endian `u16` at `at`, or [`DecodeError::TruncatedHeader`] past the end.
#[cfg(feature = "first-frame-previews")]
pub(crate) fn le_u16(bytes: &[u8], at: usize) -> Result<u16, DecodeError> {
    word(bytes, at).map(u16::from_le_bytes)
}

fn word<const N: usize>(bytes: &[u8], at: usize) -> Result<[u8; N], DecodeError> {
    at.checked_add(N)
        .and_then(|end| bytes.get(at..end))
        .and_then(|slice| slice.try_into().ok())
        .ok_or(DecodeError::TruncatedHeader)
}

fn reject_apng(bytes: &[u8]) -> Result<(), DecodeError> {
    // PNG: signature, then [length:4][type:4][data:length][crc:4]. Deriving the
    // scan from the grammar means unknown chunks remain valid instead of being
    // accepted/rejected by a brittle list.
    let mut offset = 8usize;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(DecodeError::TruncatedHeader)?;
        if header_end > bytes.len() {
            return Err(DecodeError::TruncatedHeader);
        }
        let length = be_u32(bytes, offset)? as usize;
        let kind = &bytes[offset + 4..offset + 8];
        let end = header_end
            .checked_add(length)
            .and_then(|v| v.checked_add(4))
            .ok_or(DecodeError::TruncatedHeader)?;
        if end > bytes.len() {
            return Err(DecodeError::TruncatedHeader);
        }
        if kind == b"acTL" {
            return Err(DecodeError::Animated);
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return Ok(());
        }
        offset = end;
    }
    Err(DecodeError::TruncatedHeader)
}

fn reject_animated_webp(bytes: &[u8]) -> Result<(), DecodeError> {
    let mut offset = 12usize;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).ok_or(DecodeError::TruncatedHeader)?;
        if header_end > bytes.len() {
            return Err(DecodeError::TruncatedHeader);
        }
        let kind = &bytes[offset..offset + 4];
        let length = le_u32(bytes, offset + 4)? as usize;
        let data_end = header_end
            .checked_add(length)
            .ok_or(DecodeError::TruncatedHeader)?;
        if data_end > bytes.len() {
            return Err(DecodeError::TruncatedHeader);
        }
        if kind == b"ANIM" || kind == b"ANMF" {
            return Err(DecodeError::Animated);
        }
        if kind == b"VP8X" {
            let flags = bytes.get(header_end).ok_or(DecodeError::TruncatedHeader)?;
            if flags & 0x02 != 0 {
                return Err(DecodeError::Animated);
            }
        }
        // RIFF chunks are padded to an even byte boundary.
        offset = data_end
            .checked_add(length & 1)
            .ok_or(DecodeError::TruncatedHeader)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
