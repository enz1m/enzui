//! Opt-in GIF and APNG posters under the shared image admission policy.
//!
//! Enable `first-frame-previews` and run decoding on an owned worker. Only the
//! first displayed animation frame is returned; later frames are not decoded.
//! APNG's separate default image is decoded and discarded by the codec.
//! PNG header metadata uses the shared 64 KiB minimum working budget.
//! Other stills retain static decoding and orientation rules. Animated WebP
//! remains refused: its codec does not apply working-allocation limits to
//! animated frame decoding. Static and remote resource defaults are unchanged.
//!
//! ```no_run
//! use enzui_core::decode_limits::DecodeLimits;
//! use enzui_resources::first_frame::decode_resized;
//! # fn preview(bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
//! let poster = decode_resized(bytes, &DecodeLimits::DEFAULT, 480, 480, || false)?;
//! assert_eq!(poster.rgba_bytes, u64::from(poster.width) * u64::from(poster.height) * 4);
//! # Ok(()) }
//! ```

use crate::decode::{be_u32, le_u16};
use crate::{DecodeError, DecodedAsset, StaticFormat, decode};
use enzui_core::{
    async_image::EncodedImage,
    decode_limits::{DecodeLimits, RefusedVerdict},
};
use iced_core::image::Handle;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, Limits};
use std::io::{BufReader, Cursor};

/// Content recognized by the explicit poster path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewFormat {
    /// A still decoded through the strict static path.
    Still(StaticFormat),
    /// The first GIF image, composed on its logical canvas.
    Gif,
    /// The first APNG animation frame, excluding a separate default image.
    Apng,
}

/// One admitted poster with exact renderer-byte accounting.
#[derive(Clone, Debug)]
pub struct PreviewAsset {
    /// Renderer handle over validated RGBA storage.
    pub handle: Handle,
    /// Source content and selection semantics.
    pub format: PreviewFormat,
    /// Orientation-adjusted output width.
    pub width: u32,
    /// Orientation-adjusted output height.
    pub height: u32,
    /// Original encoded payload size.
    pub encoded_bytes: u64,
    /// Retained RGBA bytes, excluding decoder scratch storage.
    pub rgba_bytes: u64,
}

impl From<DecodedAsset> for PreviewAsset {
    fn from(asset: DecodedAsset) -> Self {
        Self {
            handle: asset.handle,
            format: PreviewFormat::Still(asset.format),
            width: asset.width,
            height: asset.height,
            encoded_bytes: asset.encoded_bytes,
            rgba_bytes: asset.rgba_bytes,
        }
    }
}

/// Decode the first displayed frame at admitted natural dimensions.
///
/// # Errors
/// Returns shared admission, malformed-content, unsupported-format or
/// cancellation errors. Cancellation is cooperative around synchronous decoding.
pub fn decode(
    bytes: &[u8],
    limits: &DecodeLimits,
    cancelled: impl Fn() -> bool,
) -> Result<PreviewAsset, DecodeError> {
    decode_inner(bytes, limits, None, cancelled)
}

/// Decode one poster and use the shared Lanczos thumbnail fit without enlargement.
///
/// Zero bounds become one pixel, matching `decode_static_resized`. Source geometry
/// must pass admission even when the requested output is small. Decoder scratch
/// can include additional canvases; APNG may refuse a tight working-byte budget.
///
/// # Errors
/// Returns shared admission, malformed-content, unsupported-format or
/// cancellation errors. Later animation frames are not decoded or validated.
pub fn decode_resized(
    bytes: &[u8],
    limits: &DecodeLimits,
    max_width: u32,
    max_height: u32,
    cancelled: impl Fn() -> bool,
) -> Result<PreviewAsset, DecodeError> {
    decode_inner(
        bytes,
        limits,
        Some((max_width.max(1), max_height.max(1))),
        cancelled,
    )
}

fn image_limits(limits: &DecodeLimits) -> Limits {
    let mut policy = Limits::default();
    policy.max_image_width = Some(limits.max_width);
    policy.max_image_height = Some(limits.max_height);
    policy.max_alloc = Some(limits.max_rgba_bytes);
    policy
}

fn pixel_error(error: image::ImageError, limits: &DecodeLimits) -> DecodeError {
    if matches!(error, image::ImageError::Limits(_)) {
        DecodeError::WorkingAllocationLimit {
            max_bytes: limits.max_rgba_bytes,
        }
    } else {
        DecodeError::InvalidImage
    }
}

fn admit(bytes: &[u8], width: u32, height: u32, limits: &DecodeLimits) -> Result<(), DecodeError> {
    if let Some(refused) =
        RefusedVerdict::new(EncodedImage::new(bytes.len() as u64, width, height).screen(limits))
    {
        return Err(DecodeError::Limit(refused));
    }
    Ok(())
}

fn decode_inner(
    bytes: &[u8],
    limits: &DecodeLimits,
    target: Option<(u32, u32)>,
    cancelled: impl Fn() -> bool,
) -> Result<PreviewAsset, DecodeError> {
    if cancelled() {
        return Err(DecodeError::Cancelled);
    }
    if let Some(refused) = RefusedVerdict::new(limits.check_encoded(bytes.len() as u64)) {
        return Err(DecodeError::Limit(refused));
    }
    let (mut raster, format, orientation) = if bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
    {
        let header = bytes.get(6..10).ok_or(DecodeError::TruncatedHeader)?;
        let width = u32::from(le_u16(header, 0)?);
        let height = u32::from(le_u16(header, 2)?);
        admit(bytes, width, height, limits)?;
        let mut decoder = image::codecs::gif::GifDecoder::new(BufReader::new(Cursor::new(bytes)))
            .map_err(|_| DecodeError::TruncatedHeader)?;
        decoder
            .set_limits(image_limits(limits))
            .map_err(|error| pixel_error(error, limits))?;
        if cancelled() {
            return Err(DecodeError::Cancelled);
        }
        let raster =
            DynamicImage::from_decoder(decoder).map_err(|error| pixel_error(error, limits))?;
        (
            raster,
            PreviewFormat::Gif,
            image::metadata::Orientation::NoTransforms,
        )
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let header = bytes.get(8..33).ok_or(DecodeError::TruncatedHeader)?;
        if header[..8] != *b"\x00\x00\x00\x0dIHDR" {
            return Err(DecodeError::TruncatedHeader);
        }
        let width = be_u32(header, 8)?;
        let height = be_u32(header, 12)?;
        admit(bytes, width, height, limits)?;
        let header_budget = limits.max_rgba_bytes.max(64 * 1024);
        let mut header_limits = image_limits(limits);
        header_limits.max_alloc = Some(header_budget);
        let mut decoder = image::codecs::png::PngDecoder::with_limits(
            BufReader::new(Cursor::new(bytes)),
            header_limits,
        )
        .map_err(|error| {
            if matches!(error, image::ImageError::Limits(_)) {
                DecodeError::HeaderAllocationLimit {
                    max_bytes: header_budget,
                }
            } else {
                DecodeError::TruncatedHeader
            }
        })?;
        let (width, height) = decoder.dimensions();
        admit(bytes, width, height, limits)?;
        decoder
            .set_limits(image_limits(limits))
            .map_err(|error| pixel_error(error, limits))?;
        let orientation = decoder
            .orientation()
            .unwrap_or(image::metadata::Orientation::NoTransforms);
        if cancelled() {
            return Err(DecodeError::Cancelled);
        }
        if decoder
            .is_apng()
            .map_err(|error| pixel_error(error, limits))?
        {
            let frame = decoder
                .apng()
                .map_err(|error| pixel_error(error, limits))?
                .into_frames()
                .next()
                .ok_or(DecodeError::InvalidImage)?
                .map_err(|error| pixel_error(error, limits))?;
            (
                DynamicImage::ImageRgba8(frame.into_buffer()),
                PreviewFormat::Apng,
                orientation,
            )
        } else {
            (
                DynamicImage::from_decoder(decoder).map_err(|error| pixel_error(error, limits))?,
                PreviewFormat::Still(StaticFormat::Png),
                orientation,
            )
        }
    } else {
        let asset = if let Some((width, height)) = target {
            crate::decode_static_resized(bytes, limits, width, height, cancelled)?
        } else {
            crate::decode_static(bytes, limits, cancelled)?
        };
        return Ok(asset.into());
    };
    raster.apply_orientation(orientation);
    let raster =
        decode::finish_raster(raster, bytes.len() as u64, limits, target, None, &cancelled)?;
    Ok(PreviewAsset {
        handle: raster.handle,
        format,
        width: raster.width,
        height: raster.height,
        encoded_bytes: bytes.len() as u64,
        rgba_bytes: raster.rgba_bytes,
    })
}
