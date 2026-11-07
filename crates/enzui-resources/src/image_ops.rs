//! Worker-backed crop, flip, rotate, resize and PNG/JPEG/lossless-WebP export.
//!
//! Sources are orientation-adjusted RGBA handles or bounded encoded stills.
//! Recipes use [`ImageTransform`] source coordinates. Encoding clears metadata
//! by default; retention preserves a documented subset and normalizes orientation.
//! The application supplies and owns the bounded worker pool. No picker or dialog
//! is hidden inside an operation.
//!
//! Enable the optional `image-ops` feature. Keep the service in application state
//! and reject results whose generation no longer matches the active request.
//!
//! ```no_run
//! use enzui_core::{blocking_task::BlockingTaskOwner, image_transform::{
//!     ImageResize, ImageTransform, PixelCrop,
//! }};
//! use enzui_resources::image_ops::*;
//! use iced_core::{Size, image::Handle};
//!
//! async fn avatar(
//!     service: &ImageOperations, generation: u64, handle: Handle,
//!     source_size: Size<u32>, crop: PixelCrop,
//! ) -> Result<EncodedImageResult, Box<dyn std::error::Error>> {
//!     let transform = ImageTransform::new(source_size)?.crop(crop)?
//!         .resize(ImageResize::Exact(Size::new(256, 256)))?;
//!     let request = ImageOperationRequest::new(
//!         ImageSource::Decoded { handle, metadata: SourceMetadata::default() },
//!         transform, ImageEncoding::Png,
//!     );
//!     let settled = service.submit(generation, request)?.await;
//!     assert_eq!(settled.generation(), generation);
//!     Ok(settled.into_result()??)
//! }
//!
//! let service = ImageOperations::new(
//!     BlockingTaskOwner::with_defaults("avatar-export")?,
//!     ImageOperationLimits::default(),
//! );
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use enzui_core::blocking_task::{BlockingTask, BlockingTaskOwner};
use enzui_core::decode_limits::{DecodeLimits, RefusedVerdict};
use enzui_core::image_transform::{ImageTransform, QuarterTurn};
use enzui_core::user_file::{self, SaveError, SavePolicy};
use iced_core::{Size, image::Handle};
use image::{ExtendedColorType, ImageDecoder, ImageEncoder, ImageReader, RgbaImage};
use std::io::{self, BufReader, Cursor, Write};
use std::path::Path;
use std::sync::Arc;

/// Encoded metadata accompanying already-decoded pixels.
#[derive(Debug, Clone, Default)]
pub struct SourceMetadata {
    /// Raw TIFF/EXIF bytes, without a JPEG APP1 prefix.
    pub exif: Vec<u8>,
    /// ICC profile bytes; retention copies the profile without color conversion.
    pub icc: Vec<u8>,
}

/// Source storage retained until one generation settles.
#[derive(Debug, Clone)]
pub enum ImageSource {
    /// Orientation-adjusted RGBA pixels; path and encoded handles are refused.
    Decoded {
        /// Shared renderer pixels; the handle's actual dimensions are authoritative.
        handle: Handle,
        /// Metadata previously read by an owned loader.
        metadata: SourceMetadata,
    },
    /// Static encoded bytes decoded on the worker under the existing strict format policy.
    Encoded(Arc<[u8]>),
}

/// Output format with only its applicable options.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageEncoding {
    /// Lossless RGBA with transparency.
    Png,
    /// RGB JPEG; quality must be `1..=100`, and alpha is composited onto this matte.
    Jpeg {
        /// Encoder quality, refused outside `1..=100`.
        quality: u8,
        /// Opaque matte channel values in the unconverted source pixel color space.
        matte: [u8; 3],
    },
    /// Lossless RGBA WebP; lossy quality is not supported by this encoder.
    WebPLossless,
}

/// Metadata retained in transformed output.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum MetadataPolicy {
    /// Drop EXIF and ICC; no color conversion is performed.
    #[default]
    Clear,
    /// Keep ICC and supported camera, date, attribution and comment fields.
    /// Orientation becomes 1 and dimensions become the actual output size.
    /// Maker notes, thumbnails and unknown fields are discarded.
    RetainSupported {
        /// Include supported GPS fields; retained comments may contain private text.
        include_location: bool,
    },
}

/// One application-owned export request, independent of a destination filename.
#[derive(Debug, Clone)]
pub struct ImageOperationRequest {
    /// Source storage; widgets supply handles rather than loading files.
    pub source: ImageSource,
    /// Validated recipe tied to the orientation-adjusted source dimensions.
    pub transform: ImageTransform,
    /// Output codec and its explicit options.
    pub encoding: ImageEncoding,
    /// Privacy policy, applied on the worker before encoding.
    pub metadata: MetadataPolicy,
}

impl ImageOperationRequest {
    /// Create an export request that clears metadata.
    #[must_use]
    pub fn new(source: ImageSource, transform: ImageTransform, encoding: ImageEncoding) -> Self {
        Self {
            source,
            transform,
            encoding,
            metadata: MetadataPolicy::Clear,
        }
    }

    /// Select explicit metadata retention and location policy.
    #[must_use]
    pub fn metadata_policy(mut self, policy: MetadataPolicy) -> Self {
        self.metadata = policy;
        self
    }
}

/// Per-request admission and encoded output ceilings.
#[derive(Debug, Clone, Copy)]
pub struct ImageOperationLimits {
    /// Source encoded bytes, dimensions and each intermediate RGBA allocation.
    pub decode: DecodeLimits,
    /// Maximum encoded result bytes, enforced while the encoder writes.
    pub max_output_bytes: u64,
    /// Ceiling for raw EXIF/ICC bytes, expanded EXIF storage and retained metadata.
    pub max_metadata_bytes: u64,
}

impl Default for ImageOperationLimits {
    fn default() -> Self {
        Self {
            decode: DecodeLimits::default(),
            max_output_bytes: 32 * 1024 * 1024,
            max_metadata_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Encoded image suitable for upload or explicit atomic publication.
#[derive(Debug, Clone)]
pub struct EncodedImageResult {
    /// Encoded bytes bounded by `ImageOperationLimits::max_output_bytes`.
    pub bytes: Arc<[u8]>,
    /// Actual encoded width and height, matching recipe prediction.
    pub dimensions: Size<u32>,
    /// Codec/options used, independent of any filename extension.
    pub encoding: ImageEncoding,
}

impl EncodedImageResult {
    /// Publish a new file through the shared atomic no-clobber writer.
    /// Cancellation is checked while staging and before the writer returns;
    /// after that commit boundary, publication may complete and cannot be rolled back.
    ///
    /// # Errors
    /// Refuses cancellation, destination collisions and shared custody/write failures.
    pub fn publish_new(
        &self,
        path: &Path,
        cancelled: impl Fn() -> bool,
    ) -> Result<(), ImageOperationError> {
        if cancelled() {
            return Err(ImageOperationError::Cancelled);
        }
        user_file::save_with(path, SavePolicy::CreateNew, |writer| {
            for chunk in self.bytes.chunks(64 * 1024) {
                if cancelled() {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "image export cancelled",
                    ));
                }
                writer.write_all(chunk)?;
            }
            if cancelled() {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    "image export cancelled",
                ));
            }
            Ok(())
        })
        .map_err(ImageOperationError::Publication)
    }
}

/// Application-owned operations over an explicitly supplied bounded pool.
pub struct ImageOperations {
    owner: BlockingTaskOwner,
    limits: ImageOperationLimits,
}

impl ImageOperations {
    /// Transfer a pool into this service; its worker and queue capacities are unchanged.
    #[must_use]
    pub fn new(owner: BlockingTaskOwner, limits: ImageOperationLimits) -> Self {
        Self { owner, limits }
    }

    /// Admit a request without scanning or transforming pixels on the calling thread.
    /// Accepted work returns the pool's generation-stamped, cancellable future.
    /// Decode, transform and encode run on its worker; phases and encoder writes
    /// check cancellation. A codec call already running is not forcibly interrupted.
    ///
    /// # Errors
    /// Returns constant-time input/options/limit refusals before queue admission.
    /// Pool saturation and shutdown settle through `BlockingTask` instead.
    pub fn submit(
        &self,
        generation: u64,
        request: ImageOperationRequest,
    ) -> Result<BlockingTask<Result<EncodedImageResult, ImageOperationError>>, ImageOperationError>
    {
        validate_request(&request, self.limits)?;
        let limits = self.limits;
        Ok(self.owner.submit(generation, move |context| {
            execute(request, limits, || context.is_cancelled())
        }))
    }
}

/// Why an admitted image request could not produce or publish an encoded result.
#[derive(Debug)]
pub enum ImageOperationError {
    /// Shared encoded-size or decoded-pixel policy refused admission.
    Limit(RefusedVerdict),
    /// Source is not a resolved RGBA handle or its storage length is invalid.
    InvalidPixels,
    /// Actual orientation-adjusted source dimensions differ from the recipe.
    SourceChanged,
    /// JPEG quality is outside the documented interval.
    InvalidQuality,
    /// Encoded output exceeded its byte ceiling.
    OutputTooLarge,
    /// EXIF/ICC bytes exceeded their combined ceiling.
    MetadataTooLarge,
    /// Supported metadata could not be parsed or serialized.
    InvalidMetadata,
    /// Retention is unavailable for the decoded source format.
    UnsupportedMetadata,
    /// Existing static decode policy refused or failed the source.
    Decode(crate::DecodeError),
    /// Pixel encoder rejected the request.
    Encode(image::ImageError),
    /// Cancellation was observed before a stable result or publication boundary.
    Cancelled,
    /// Shared atomic publication failed; the error reports its publication phase.
    Publication(SaveError),
}

fn admit_dimensions(limits: DecodeLimits, size: Size<u32>) -> Result<(), ImageOperationError> {
    if let Some(refused) = RefusedVerdict::new(limits.check_decoded(size.width, size.height)) {
        return Err(ImageOperationError::Limit(refused));
    }
    Ok(())
}

fn metadata_size(metadata: &SourceMetadata, cap: u64) -> Result<(), ImageOperationError> {
    if (metadata.exif.len() as u64)
        .checked_add(metadata.icc.len() as u64)
        .is_none_or(|n| n > cap)
    {
        return Err(ImageOperationError::MetadataTooLarge);
    }
    Ok(())
}

fn validate_request(
    request: &ImageOperationRequest,
    limits: ImageOperationLimits,
) -> Result<(), ImageOperationError> {
    if matches!(request.encoding, ImageEncoding::Jpeg { quality, .. } if !(1..=100).contains(&quality))
    {
        return Err(ImageOperationError::InvalidQuality);
    }
    if limits.max_output_bytes == 0 {
        return Err(ImageOperationError::OutputTooLarge);
    }
    let source = request.transform.source_dimensions();
    admit_dimensions(limits.decode, source)?;
    let cropped = request.transform.source_crop().dimensions();
    admit_dimensions(limits.decode, cropped)?;
    let rotated = match request.transform.rotation() {
        QuarterTurn::Clockwise90 | QuarterTurn::Clockwise270 => {
            Size::new(cropped.height, cropped.width)
        }
        _ => cropped,
    };
    admit_dimensions(limits.decode, rotated)?;
    admit_dimensions(limits.decode, request.transform.output_dimensions())?;
    match &request.source {
        ImageSource::Encoded(bytes) => {
            if let Some(refused) =
                RefusedVerdict::new(limits.decode.check_encoded(bytes.len() as u64))
            {
                return Err(ImageOperationError::Limit(refused));
            }
        }
        ImageSource::Decoded { handle, metadata } => {
            let Handle::Rgba {
                width,
                height,
                pixels,
                ..
            } = handle
            else {
                return Err(ImageOperationError::InvalidPixels);
            };
            if Size::new(*width, *height) != source {
                return Err(ImageOperationError::SourceChanged);
            }
            let len = u64::from(*width) * u64::from(*height) * 4;
            if len != pixels.len() as u64 {
                return Err(ImageOperationError::InvalidPixels);
            }
            metadata_size(metadata, limits.max_metadata_bytes)?;
        }
    }
    Ok(())
}

fn read_metadata(
    bytes: &[u8],
    limits: ImageOperationLimits,
) -> Result<SourceMetadata, ImageOperationError> {
    let mut reader = ImageReader::new(BufReader::new(Cursor::new(bytes)))
        .with_guessed_format()
        .map_err(|_| ImageOperationError::InvalidMetadata)?;
    let mut allocation = image::Limits::default();
    allocation.max_image_width = Some(limits.decode.max_width);
    allocation.max_image_height = Some(limits.decode.max_height);
    allocation.max_alloc = Some(limits.max_metadata_bytes.min(limits.decode.max_rgba_bytes));
    reader.limits(allocation);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| ImageOperationError::UnsupportedMetadata)?;
    let metadata = SourceMetadata {
        exif: decoder
            .exif_metadata()
            .map_err(|_| ImageOperationError::InvalidMetadata)?
            .unwrap_or_default(),
        icc: decoder
            .icc_profile()
            .map_err(|_| ImageOperationError::InvalidMetadata)?
            .unwrap_or_default(),
    };
    metadata_size(&metadata, limits.max_metadata_bytes)?;
    Ok(metadata)
}

fn execute(
    request: ImageOperationRequest,
    limits: ImageOperationLimits,
    cancelled: impl Fn() -> bool,
) -> Result<EncodedImageResult, ImageOperationError> {
    if cancelled() {
        return Err(ImageOperationError::Cancelled);
    }
    let (handle, metadata) = match request.source {
        ImageSource::Decoded { handle, metadata } => (handle, metadata),
        ImageSource::Encoded(bytes) => {
            let asset = crate::decode_static(&bytes, &limits.decode, &cancelled)
                .map_err(ImageOperationError::Decode)?;
            let metadata = match request.metadata {
                MetadataPolicy::Clear => SourceMetadata::default(),
                MetadataPolicy::RetainSupported { .. } => read_metadata(&bytes, limits)?,
            };
            (asset.handle, metadata)
        }
    };
    let Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = &handle
    else {
        return Err(ImageOperationError::InvalidPixels);
    };
    if Size::new(*width, *height) != request.transform.source_dimensions() {
        return Err(ImageOperationError::SourceChanged);
    }
    let source =
        image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(*width, *height, pixels.as_ref())
            .ok_or(ImageOperationError::InvalidPixels)?;
    let (x, y, w, h) = request.transform.source_crop().bounds();
    if cancelled() {
        return Err(ImageOperationError::Cancelled);
    }
    let mut cropped = RgbaImage::from_fn(w, h, |px, py| *source.get_pixel(x + px, y + py));
    let (horizontal, vertical) = request.transform.flips();
    if horizontal {
        image::imageops::flip_horizontal_in_place(&mut cropped);
    }
    if vertical {
        image::imageops::flip_vertical_in_place(&mut cropped);
    }
    if cancelled() {
        return Err(ImageOperationError::Cancelled);
    }
    cropped = match request.transform.rotation() {
        QuarterTurn::None => cropped,
        QuarterTurn::Clockwise90 => image::imageops::rotate90(&cropped),
        QuarterTurn::Clockwise180 => image::imageops::rotate180(&cropped),
        QuarterTurn::Clockwise270 => image::imageops::rotate270(&cropped),
    };
    let dimensions = request.transform.output_dimensions();
    if cancelled() {
        return Err(ImageOperationError::Cancelled);
    }
    if cropped.dimensions() != (dimensions.width, dimensions.height) {
        cropped = image::imageops::resize(
            &cropped,
            dimensions.width,
            dimensions.height,
            image::imageops::FilterType::Lanczos3,
        );
    }
    if cancelled() {
        return Err(ImageOperationError::Cancelled);
    }
    let metadata = normalize_metadata(
        metadata,
        request.metadata,
        dimensions,
        limits.max_metadata_bytes,
    )?;
    let mut output = BoundedOutput {
        bytes: Vec::new(),
        cap: limits.max_output_bytes,
        cancelled: &cancelled,
        exceeded: false,
    };
    let encoded = match request.encoding {
        ImageEncoding::Png => {
            configure(image::codecs::png::PngEncoder::new(&mut output), &metadata)?.write_image(
                &cropped,
                dimensions.width,
                dimensions.height,
                ExtendedColorType::Rgba8,
            )
        }
        ImageEncoding::WebPLossless => configure(
            image::codecs::webp::WebPEncoder::new_lossless(&mut output),
            &metadata,
        )?
        .write_image(
            &cropped,
            dimensions.width,
            dimensions.height,
            ExtendedColorType::Rgba8,
        ),
        ImageEncoding::Jpeg { quality, matte } => {
            let rgb = flatten(&cropped, matte);
            configure(
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut output, quality),
                &metadata,
            )?
            .write_image(
                &rgb,
                dimensions.width,
                dimensions.height,
                ExtendedColorType::Rgb8,
            )
        }
    };
    if cancelled() {
        return Err(ImageOperationError::Cancelled);
    }
    if output.exceeded {
        return Err(ImageOperationError::OutputTooLarge);
    }
    encoded.map_err(ImageOperationError::Encode)?;
    Ok(EncodedImageResult {
        bytes: output.bytes.into(),
        dimensions,
        encoding: request.encoding,
    })
}

fn flatten(pixels: &RgbaImage, matte: [u8; 3]) -> image::RgbImage {
    image::RgbImage::from_fn(pixels.width(), pixels.height(), |x, y| {
        let p = pixels.get_pixel(x, y);
        let alpha = u32::from(p[3]);
        image::Rgb(std::array::from_fn(|i| {
            u8::try_from(
                (u32::from(p[i]) * alpha + u32::from(matte[i]) * (255 - alpha) + 127) / 255,
            )
            .unwrap_or(u8::MAX)
        }))
    })
}

fn configure<E: ImageEncoder>(
    mut encoder: E,
    metadata: &SourceMetadata,
) -> Result<E, ImageOperationError> {
    if !metadata.exif.is_empty() {
        encoder
            .set_exif_metadata(metadata.exif.clone())
            .map_err(|_| ImageOperationError::UnsupportedMetadata)?;
    }
    if !metadata.icc.is_empty() {
        encoder
            .set_icc_profile(metadata.icc.clone())
            .map_err(|_| ImageOperationError::UnsupportedMetadata)?;
    }
    Ok(encoder)
}

struct BoundedOutput<'a> {
    bytes: Vec<u8>,
    cap: u64,
    cancelled: &'a dyn Fn() -> bool,
    exceeded: bool,
}

impl Write for BoundedOutput<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if (self.cancelled)() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "image encode cancelled",
            ));
        }
        let len = self
            .bytes
            .len()
            .checked_add(bytes.len())
            .filter(|n| *n as u64 <= self.cap);
        let Some(len) = len else {
            self.exceeded = true;
            return Err(io::Error::other(
                "image encoded output exceeds its byte limit",
            ));
        };
        if len > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(len)
                .min(usize::try_from(self.cap).unwrap_or(usize::MAX));
            self.bytes
                .try_reserve_exact(capacity - self.bytes.len())
                .map_err(io::Error::other)?;
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn normalize_metadata(
    metadata: SourceMetadata,
    policy: MetadataPolicy,
    size: Size<u32>,
    cap: u64,
) -> Result<SourceMetadata, ImageOperationError> {
    let MetadataPolicy::RetainSupported { include_location } = policy else {
        return Ok(SourceMetadata::default());
    };
    metadata_size(&metadata, cap)?;
    if metadata.exif.is_empty() {
        return Ok(metadata);
    }
    let raw = metadata
        .exif
        .strip_prefix(b"Exif\0\0")
        .unwrap_or(&metadata.exif);
    screen_exif(raw, cap)?;
    let exif = exif::Reader::new()
        .read_raw(raw.to_vec())
        .map_err(|_| ImageOperationError::InvalidMetadata)?;
    let mut fields: Vec<exif::Field> = exif
        .fields()
        .filter(|field| {
            field.ifd_num == exif::In::PRIMARY
                && (matches!(
                    field.tag,
                    exif::Tag::Make
                        | exif::Tag::Model
                        | exif::Tag::Software
                        | exif::Tag::Artist
                        | exif::Tag::Copyright
                        | exif::Tag::DateTime
                        | exif::Tag::DateTimeOriginal
                        | exif::Tag::DateTimeDigitized
                        | exif::Tag::UserComment
                ) || include_location
                    && matches!(
                        field.tag,
                        exif::Tag::GPSVersionID
                            | exif::Tag::GPSLatitudeRef
                            | exif::Tag::GPSLatitude
                            | exif::Tag::GPSLongitudeRef
                            | exif::Tag::GPSLongitude
                            | exif::Tag::GPSAltitudeRef
                            | exif::Tag::GPSAltitude
                            | exif::Tag::GPSTimeStamp
                            | exif::Tag::GPSDateStamp
                    ))
        })
        .cloned()
        .collect();
    for (tag, value) in [
        (exif::Tag::Orientation, exif::Value::Short(vec![1])),
        (exif::Tag::ImageWidth, exif::Value::Long(vec![size.width])),
        (exif::Tag::ImageLength, exif::Value::Long(vec![size.height])),
        (
            exif::Tag::PixelXDimension,
            exif::Value::Long(vec![size.width]),
        ),
        (
            exif::Tag::PixelYDimension,
            exif::Value::Long(vec![size.height]),
        ),
    ] {
        fields.push(exif::Field {
            tag,
            ifd_num: exif::In::PRIMARY,
            value,
        });
    }
    let mut writer = exif::experimental::Writer::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut out = MetadataOutput {
        cursor: Cursor::new(Vec::new()),
        cap: cap.saturating_sub(metadata.icc.len() as u64),
        exceeded: false,
    };
    let result = writer.write(&mut out, true);
    if out.exceeded {
        return Err(ImageOperationError::MetadataTooLarge);
    }
    result.map_err(|_| ImageOperationError::InvalidMetadata)?;
    Ok(SourceMetadata {
        exif: out.cursor.into_inner(),
        icc: metadata.icc,
    })
}

// Aliased payloads and ASCII string arrays can expand beyond their encoded EXIF size.
fn screen_exif(raw: &[u8], cap: u64) -> Result<(), ImageOperationError> {
    let little = match raw.get(..2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return Err(ImageOperationError::InvalidMetadata),
    };
    let u16_at = |offset: usize| -> Result<u16, ImageOperationError> {
        let bytes: [u8; 2] = raw
            .get(
                offset
                    ..offset
                        .checked_add(2)
                        .ok_or(ImageOperationError::InvalidMetadata)?,
            )
            .ok_or(ImageOperationError::InvalidMetadata)?
            .try_into()
            .map_err(|_| ImageOperationError::InvalidMetadata)?;
        Ok(if little {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        })
    };
    let u32_at = |offset: usize| -> Result<u32, ImageOperationError> {
        let bytes: [u8; 4] = raw
            .get(
                offset
                    ..offset
                        .checked_add(4)
                        .ok_or(ImageOperationError::InvalidMetadata)?,
            )
            .ok_or(ImageOperationError::InvalidMetadata)?
            .try_into()
            .map_err(|_| ImageOperationError::InvalidMetadata)?;
        Ok(if little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    };
    if u16_at(2)? != 42 {
        return Err(ImageOperationError::InvalidMetadata);
    }
    let mut pending = vec![u32_at(4)? as usize];
    let mut seen = std::collections::BTreeSet::new();
    let mut fields = 0usize;
    let mut expanded = 0u64;
    while let Some(offset) = pending.pop() {
        if offset == 0 {
            continue;
        }
        if !seen.insert(offset) {
            return Err(ImageOperationError::InvalidMetadata);
        }
        let count = usize::from(u16_at(offset)?);
        fields = fields
            .checked_add(count)
            .ok_or(ImageOperationError::MetadataTooLarge)?;
        if fields > 128 || seen.len() > 128 {
            return Err(ImageOperationError::MetadataTooLarge);
        }
        let start = offset
            .checked_add(2)
            .ok_or(ImageOperationError::InvalidMetadata)?;
        let end = start
            .checked_add(count * 12)
            .ok_or(ImageOperationError::InvalidMetadata)?;
        let next = u32_at(end)?;
        if next != 0 {
            pending.push(next as usize);
        }
        for entry in (start..end).step_by(12) {
            let tag = u16_at(entry)?;
            let field_type = u16_at(entry + 2)?;
            let unit = match field_type {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return Err(ImageOperationError::InvalidMetadata),
            };
            let count = u64::from(u32_at(entry + 4)?);
            let bytes = count
                .checked_mul(unit)
                .ok_or(ImageOperationError::MetadataTooLarge)?;
            expanded = expanded
                .checked_add(bytes)
                .and_then(|n| n.checked_add(64))
                .ok_or(ImageOperationError::MetadataTooLarge)?;
            if expanded > cap {
                return Err(ImageOperationError::MetadataTooLarge);
            }
            let value = u32_at(entry + 8)?;
            if bytes > 4
                && u64::from(value)
                    .checked_add(bytes)
                    .is_none_or(|n| n > raw.len() as u64)
            {
                return Err(ImageOperationError::InvalidMetadata);
            }
            if field_type == 2 {
                let offset = if bytes <= 4 {
                    entry + 8
                } else {
                    value as usize
                };
                let end = offset
                    .checked_add(
                        usize::try_from(bytes).map_err(|_| ImageOperationError::InvalidMetadata)?,
                    )
                    .ok_or(ImageOperationError::InvalidMetadata)?;
                let payload = raw
                    .get(offset..end)
                    .ok_or(ImageOperationError::InvalidMetadata)?;
                let strings = payload.iter().filter(|&&byte| byte == 0).count() as u64 + 1;
                let slots = strings
                    .checked_mul(2)
                    .map(|n| n.max(4))
                    .ok_or(ImageOperationError::MetadataTooLarge)?;
                expanded = slots
                    .checked_mul(std::mem::size_of::<Vec<u8>>() as u64)
                    .and_then(|headers| expanded.checked_add(headers))
                    .ok_or(ImageOperationError::MetadataTooLarge)?;
                if expanded > cap {
                    return Err(ImageOperationError::MetadataTooLarge);
                }
            }
            if matches!(tag, 0x8769 | 0x8825 | 0xa005) {
                if count != 1 || unit != 4 {
                    return Err(ImageOperationError::InvalidMetadata);
                }
                pending.push(value as usize);
            }
        }
    }
    Ok(())
}

struct MetadataOutput {
    cursor: Cursor<Vec<u8>>,
    cap: u64,
    exceeded: bool,
}
impl Write for MetadataOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self
            .cursor
            .position()
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.cap)
        {
            self.exceeded = true;
            return Err(io::Error::other(
                "normalized image metadata exceeds its limit",
            ));
        }
        self.cursor.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl io::Seek for MetadataOutput {
    fn seek(&mut self, offset: io::SeekFrom) -> io::Result<u64> {
        let target = match offset {
            io::SeekFrom::Start(n) => i128::from(n),
            io::SeekFrom::Current(n) => i128::from(self.cursor.position()) + i128::from(n),
            io::SeekFrom::End(n) => self.cursor.get_ref().len() as i128 + i128::from(n),
        };
        if target < 0 || target > i128::from(self.cap) {
            self.exceeded = true;
            return Err(io::Error::other(
                "normalized image metadata seek exceeds its limit",
            ));
        }
        let target = u64::try_from(target).map_err(io::Error::other)?;
        self.cursor.set_position(target);
        Ok(target)
    }
}

impl std::fmt::Display for ImageOperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Limit(reason) => write!(f, "image operation refused: {}", reason.reason()),
            Self::InvalidPixels => f.write_str("image source must contain complete RGBA pixels"),
            Self::SourceChanged => f.write_str("image source dimensions changed"),
            Self::InvalidQuality => f.write_str("JPEG quality must be 1 through 100"),
            Self::OutputTooLarge => f.write_str("encoded image exceeds output byte limit"),
            Self::MetadataTooLarge => f.write_str("image metadata exceeds its byte or field limit"),
            Self::InvalidMetadata => {
                f.write_str("image metadata is malformed or cannot be normalized")
            }
            Self::UnsupportedMetadata => {
                f.write_str("metadata retention is unsupported for this source or encoder")
            }
            Self::Decode(error) => write!(f, "{error}"),
            Self::Encode(error) => write!(f, "image encoding failed: {error}"),
            Self::Cancelled => f.write_str("image operation cancelled"),
            Self::Publication(error) => write!(f, "image publication failed: {error}"),
        }
    }
}
impl std::error::Error for ImageOperationError {}
