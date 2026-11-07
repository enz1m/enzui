//! Opt-in local-image providers with header admission and worker-owned execution.
//!
//! Enable `local-images`; the built-in still provider reuses the strict static
//! decoder, including optional runtime HEIC through `heif`. Register additional
//! trusted providers for flattened PSD, developed RAW or motion previews. Picker
//! extensions describe currently available providers, never content admission.
//! Pixels are orientation-adjusted 8-bit SDR RGBA. The built-in still provider
//! converts an embedded RGB ICC profile (Display P3, Adobe RGB) to sRGB; other
//! providers deliver their own color space. HDR display is not promised.
//! Derived rasters do not modify originals, layers or sensor data.
//!
//! Providers must screen headers before allocating, honor all supplied budgets
//! and cancellation, and use reduced decode or embedded previews when safe.
//! Provider code is trusted application code, not a sandbox. Helper-backed
//! providers must separately arrange a read-only source, no network/delegates,
//! and the bounded `enzui_services::process_transport` route. No helper is
//! discovered or executed automatically.

use crate::{DecodeError, StaticFormat, decode, decode_static_resized};
use enzui_core::blocking_task::{BlockingTask, BlockingTaskOwner};
use enzui_core::decode_limits::{DecodeLimits, RefusedVerdict};
use iced_core::{Size, image::Handle};
use std::{fmt, sync::Arc, time::Duration};

/// The source interpretation, independent of its filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// One ordinary raster.
    Still,
    /// A derived composite; source layers are not represented.
    FlattenedLayered,
    /// A provider-developed raster; sensor data is not represented.
    DevelopedRaw,
    /// A bounded sequence, independent of application album grouping.
    Motion,
}

/// Operations an available provider can actually fulfill.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImageCapabilities {
    /// One orientation-adjusted preview raster.
    pub still_preview: bool,
    /// A derived raster suitable for a separate image export service.
    pub flattened_raster: bool,
    /// A bounded sequence with per-frame duration.
    pub motion_preview: bool,
}

/// One provider's stable discovery record.
#[derive(Debug, Clone)]
pub struct ProviderDescriptor {
    /// Unique format/provider key, stable across application sessions.
    pub key: &'static str,
    /// User-facing picker filter label.
    pub label: &'static str,
    /// Lowercase ASCII extensions without a dot; variants are not guaranteed.
    pub extensions: &'static [&'static str],
    /// The interpretation of decoded source data.
    pub kind: MediaKind,
    /// Supported operations when the provider is available.
    pub capabilities: ImageCapabilities,
}

/// Runtime availability is checked independently of content validity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderAvailability {
    /// The provider is ready; individual codec variants may still be unsupported.
    Available,
    /// The feature, library, plugin or explicit helper is absent.
    Unavailable(&'static str),
}

/// Header information obtained before full raster allocation.
#[derive(Debug, Clone)]
pub struct LocalImageMetadata {
    /// The descriptor of the provider selected by content recognition.
    pub provider: ProviderDescriptor,
    /// Orientation-adjusted source size, before preview reduction.
    pub dimensions: Size<u32>,
    /// Original EXIF orientation `1..=8`; HEIC container transforms are already resolved.
    pub orientation: u8,
}

/// Header facts supplied by a trusted provider.
#[derive(Debug, Clone, Copy)]
pub struct ProviderHeader {
    /// Orientation-adjusted source dimensions, not reduced preview dimensions.
    pub dimensions: Size<u32>,
    /// Original EXIF orientation `1..=8`; use 1 for already-resolved container transforms.
    pub orientation: u8,
}

/// Preview bounds and literal admission limits passed to every provider phase.
#[derive(Debug, Clone)]
pub struct LocalImageRequest {
    /// Immutable source bytes retained by the app-owned worker.
    pub bytes: Arc<[u8]>,
    /// Requested operation; animation is never inferred from a still request.
    pub operation: LocalImageOperation,
    /// Source and individual raster ceilings, including encoded bytes.
    pub limits: DecodeLimits,
}

/// A local decode intent, separate from remote static-image admission.
#[derive(Debug, Clone, Copy)]
pub enum LocalImageOperation {
    /// Inspect the header without decoding pixels.
    Probe,
    /// Fit one raster inside nonzero bounds without enlargement.
    Preview(Size<u32>),
    /// Obtain a flattened/developed raster for subsequent explicit export.
    FlattenedRaster(Size<u32>),
    /// Decode a bounded sequence; a still-only provider refuses this intent.
    Motion {
        /// Nonzero frame bounds, with no enlargement.
        target: Size<u32>,
        /// Maximum number of frames, literal even when zero.
        max_frames: usize,
        /// Sum of frame durations, literal even when zero.
        max_duration: Duration,
        /// Total RGBA frame bytes, literal even when zero.
        max_rgba_bytes: u64,
    },
}

impl LocalImageRequest {
    /// Create a still-preview request using the default source admission policy.
    ///
    /// # Errors
    /// Returns [`LocalImageError::InvalidRequest`] for zero bounds.
    pub fn preview(bytes: Arc<[u8]>, width: u32, height: u32) -> Result<Self, LocalImageError> {
        if width == 0 || height == 0 {
            return Err(LocalImageError::InvalidRequest);
        }
        Ok(Self {
            bytes,
            operation: LocalImageOperation::Preview(Size::new(width, height)),
            limits: DecodeLimits::default(),
        })
    }
}

/// A provider's resolved pixels; the registry validates exact storage and bounds.
#[derive(Debug, Clone)]
pub struct LocalFrame {
    /// Resolved RGBA only; encoded and filesystem handles are refused.
    pub handle: Handle,
    /// Frame duration; ignored for a still raster, nonzero for a sequence.
    pub duration: Duration,
}

/// Decoded output whose metadata remains tied to the admitted source header.
#[derive(Debug, Clone)]
pub struct LocalImageResult {
    /// Original dimensions/orientation and the selected provider.
    pub metadata: LocalImageMetadata,
    /// Empty for a probe, exactly one for a still, bounded for motion.
    pub frames: Vec<LocalFrame>,
}

/// Typed local-media refusals, independent of filenames or empty placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LocalImageError {
    /// No registered provider recognizes the content.
    UnknownFormat,
    /// Recognized content has no available implementation.
    MissingDecoder {
        /// Recognizing provider key.
        provider: &'static str,
        /// Missing feature, runtime library, plugin or helper.
        reason: &'static str,
    },
    /// The recognizing decoder rejected corrupt content.
    Malformed(&'static str),
    /// The provider does not advertise the requested operation or variant.
    UnsupportedOperation(&'static str),
    /// Source dimensions, area, encoded bytes or per-raster RGBA policy refused admission.
    Policy(RefusedVerdict),
    /// Header/ancillary storage exceeded the static decoder's working ceiling.
    HeaderAllocationLimit(u64),
    /// Zero target dimensions or inconsistent request options.
    InvalidRequest,
    /// A duplicate key or invalid extension cannot enter the registry.
    InvalidProvider,
    /// A provider returned invalid orientation, storage, dimensions or frame count.
    InvalidOutput(&'static str),
    /// Aggregate motion frame count, duration or RGBA exceeded the request.
    SequenceLimit,
    /// Cooperative cancellation retired the request.
    Cancelled,
}
impl fmt::Display for LocalImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "local image: {self:?}")
    }
}
impl std::error::Error for LocalImageError {}

/// Trusted synchronous codec adapter; all methods run on the owning worker.
///
/// `recognizes` must inspect content, never a filename. `probe` must bound its
/// own header storage and avoid full pixel decode. `decode` receives an admitted
/// header and must honor source/pixel/sequence budgets before allocation, check
/// cancellation between phases, and return orientation-adjusted SDR RGBA. An
/// unavailable adapter still recognizes its format so absence stays distinct
/// from malformed content. Implementations must be safe for concurrent calls.
pub trait LocalImageProvider: Send + Sync {
    /// Stable discovery information without implicit helper installation.
    fn descriptor(&self) -> ProviderDescriptor;
    /// Runtime availability; discovery invokes this on a worker too.
    fn availability(&self) -> DecoderAvailability;
    /// Content recognition performed after encoded-byte admission.
    fn recognizes(&self, bytes: &[u8]) -> bool;
    /// Bounded header inspection, before the registry admits dimensions.
    ///
    /// # Errors
    ///
    /// A [`LocalImageError`] when the header is unreadable, unsupported or over its limits, or on
    /// cancellation.
    fn probe(
        &self,
        request: &LocalImageRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ProviderHeader, LocalImageError>;
    /// Bounded output for the explicit operation and requested target.
    ///
    /// # Errors
    ///
    /// A [`LocalImageError`] when the decode fails or exceeds its limits, or on cancellation.
    fn decode(
        &self,
        request: &LocalImageRequest,
        header: ProviderHeader,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<LocalFrame>, LocalImageError>;
}

/// Ordered provider registry; built-ins precede application adapters.
///
/// Default construction supplies PNG/JPEG/static WebP/BMP and a recognizing
/// HEIC adapter, whose availability requires `heif` plus the runtime decoder.
/// A provider's extension list never widens the remote decoder's format set.
#[derive(Clone)]
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn LocalImageProvider>>,
}
impl Default for ProviderRegistry {
    fn default() -> Self {
        Self {
            providers: [
                StaticFormat::Png,
                StaticFormat::Jpeg,
                StaticFormat::WebP,
                StaticFormat::Bmp,
                StaticFormat::Heif,
            ]
            .into_iter()
            .map(|format| Arc::new(StaticProvider(format)) as Arc<dyn LocalImageProvider>)
            .collect(),
        }
    }
}
impl ProviderRegistry {
    /// Add one explicit application adapter, preserving content-recognition order.
    ///
    /// # Errors
    /// Returns [`LocalImageError::InvalidProvider`] for duplicate/empty keys or
    /// empty, non-lowercase-ASCII-alphanumeric extensions.
    pub fn register(
        &mut self,
        provider: Arc<dyn LocalImageProvider>,
    ) -> Result<(), LocalImageError> {
        let descriptor = provider.descriptor();
        if descriptor.key.is_empty()
            || descriptor.label.is_empty()
            || descriptor.extensions.is_empty()
            || descriptor.extensions.iter().any(|extension| {
                extension.is_empty()
                    || !extension
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            })
            || self
                .providers
                .iter()
                .any(|old| old.descriptor().key == descriptor.key)
        {
            return Err(LocalImageError::InvalidProvider);
        }
        self.providers.push(provider);
        Ok(())
    }
    /// Enumerate currently available picker filters; call on a worker.
    ///
    /// Codec variants and camera models still require content admission.
    pub fn available_formats(&self) -> Vec<ProviderDescriptor> {
        self.providers
            .iter()
            .filter(|provider| provider.availability() == DecoderAvailability::Available)
            .map(|provider| provider.descriptor())
            .collect()
    }
    /// Inspect or decode admitted local bytes synchronously on the caller's worker.
    ///
    /// Prefer [`LocalImages::submit`] unless an existing application pool already
    /// owns this call. No filesystem reads or helper discovery are implicit.
    ///
    /// # Errors
    /// Returns a typed availability, policy, format, output or cancellation refusal.
    pub fn decode(
        &self,
        request: &LocalImageRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<LocalImageResult, LocalImageError> {
        if cancelled() {
            return Err(LocalImageError::Cancelled);
        }
        if let Some(refused) =
            RefusedVerdict::new(request.limits.check_encoded(request.bytes.len() as u64))
        {
            return Err(LocalImageError::Policy(refused));
        }
        let target = match request.operation {
            LocalImageOperation::Probe => None,
            LocalImageOperation::Preview(target)
            | LocalImageOperation::FlattenedRaster(target)
            | LocalImageOperation::Motion { target, .. } => Some(target),
        };
        if target.is_some_and(|target| target.width == 0 || target.height == 0) {
            return Err(LocalImageError::InvalidRequest);
        }
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.recognizes(&request.bytes))
            .ok_or(LocalImageError::UnknownFormat)?;
        let descriptor = provider.descriptor();
        if let DecoderAvailability::Unavailable(reason) = provider.availability() {
            return Err(LocalImageError::MissingDecoder {
                provider: descriptor.key,
                reason,
            });
        }
        let caps = descriptor.capabilities;
        let supported = match request.operation {
            LocalImageOperation::Probe => true,
            LocalImageOperation::Preview(_) => caps.still_preview,
            LocalImageOperation::FlattenedRaster(_) => caps.flattened_raster,
            LocalImageOperation::Motion { .. } => caps.motion_preview,
        };
        if !supported {
            return Err(LocalImageError::UnsupportedOperation(descriptor.key));
        }
        let header = provider.probe(request, cancelled)?;
        if !(1..=8).contains(&header.orientation) {
            return Err(LocalImageError::InvalidOutput(descriptor.key));
        }
        if let Some(refused) = RefusedVerdict::new(
            request
                .limits
                .check_decoded(header.dimensions.width, header.dimensions.height),
        ) {
            return Err(LocalImageError::Policy(refused));
        }
        if cancelled() {
            return Err(LocalImageError::Cancelled);
        }
        let frames = if target.is_some() {
            provider.decode(request, header, cancelled)?
        } else {
            Vec::new()
        };
        if let Some(target) = target {
            if frames.is_empty() {
                return Err(LocalImageError::InvalidOutput(descriptor.key));
            }
            let mut bytes = 0u64;
            let mut duration = Duration::ZERO;
            for frame in &frames {
                let Handle::Rgba {
                    width,
                    height,
                    pixels,
                    ..
                } = &frame.handle
                else {
                    return Err(LocalImageError::InvalidOutput(descriptor.key));
                };
                if *width == 0
                    || *height == 0
                    || *width > target.width
                    || *height > target.height
                    || *width > header.dimensions.width
                    || *height > header.dimensions.height
                    || u64::from(*width) * u64::from(*height) * 4 != pixels.len() as u64
                {
                    return Err(LocalImageError::InvalidOutput(descriptor.key));
                }
                if let Some(refused) =
                    RefusedVerdict::new(request.limits.check_decoded(*width, *height))
                {
                    return Err(LocalImageError::Policy(refused));
                }
                bytes = bytes
                    .checked_add(pixels.len() as u64)
                    .ok_or(LocalImageError::SequenceLimit)?;
                duration = duration
                    .checked_add(frame.duration)
                    .ok_or(LocalImageError::SequenceLimit)?;
            }
            match request.operation {
                LocalImageOperation::Motion {
                    max_frames,
                    max_duration,
                    max_rgba_bytes,
                    ..
                } => {
                    let exceeds_limits = frames.len() > max_frames
                        || duration > max_duration
                        || bytes > max_rgba_bytes
                        || frames.iter().any(|frame| frame.duration.is_zero());
                    if exceeds_limits {
                        return Err(LocalImageError::SequenceLimit);
                    }
                }
                _ if frames.len() != 1 => {
                    return Err(LocalImageError::InvalidOutput(descriptor.key));
                }
                _ => {}
            }
        }
        if cancelled() {
            return Err(LocalImageError::Cancelled);
        }
        Ok(LocalImageResult {
            metadata: LocalImageMetadata {
                provider: descriptor,
                dimensions: header.dimensions,
                orientation: header.orientation,
            },
            frames,
        })
    }
}

/// Application-owned bounded execution for discovery, probing and preview.
///
/// Settlements preserve the submitted generation. The application must reject
/// completions that no longer match its active source/request generation.
pub struct LocalImages {
    owner: BlockingTaskOwner,
    registry: Arc<ProviderRegistry>,
}
impl LocalImages {
    /// Retain the supplied worker owner and explicit provider registry.
    pub fn new(owner: BlockingTaskOwner, registry: ProviderRegistry) -> Self {
        Self {
            owner,
            registry: Arc::new(registry),
        }
    }
    /// Discover picker filters through the same bounded worker custody.
    pub fn discover(&self, generation: u64) -> BlockingTask<Vec<ProviderDescriptor>> {
        let registry = Arc::clone(&self.registry);
        self.owner
            .submit(generation, move |_| registry.available_formats())
    }
    /// Submit immutable local bytes; no filename or filesystem read is implicit.
    pub fn submit(
        &self,
        generation: u64,
        request: LocalImageRequest,
    ) -> BlockingTask<Result<LocalImageResult, LocalImageError>> {
        let registry = Arc::clone(&self.registry);
        self.owner.submit(generation, move |context| {
            registry.decode(&request, &|| context.is_cancelled())
        })
    }
}

struct StaticProvider(StaticFormat);
impl StaticProvider {
    fn error(&self, error: DecodeError) -> LocalImageError {
        let key = self.descriptor().key;
        match error {
            DecodeError::Limit(refused) => LocalImageError::Policy(refused),
            DecodeError::HeaderAllocationLimit { max_bytes } => {
                LocalImageError::HeaderAllocationLimit(max_bytes)
            }
            DecodeError::Unsupported(_) => LocalImageError::MissingDecoder {
                provider: key,
                reason: "feature, runtime library or codec plugin unavailable",
            },
            DecodeError::UnsupportedMagic => LocalImageError::UnknownFormat,
            DecodeError::Animated => LocalImageError::UnsupportedOperation(key),
            DecodeError::Cancelled => LocalImageError::Cancelled,
            _ => LocalImageError::Malformed(key),
        }
    }
}
impl LocalImageProvider for StaticProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        let (key, label, extensions): (_, _, &'static [&'static str]) = match self.0 {
            StaticFormat::Png => ("png", "PNG", &["png"]),
            StaticFormat::Jpeg => ("jpeg", "JPEG", &["jpg", "jpeg"]),
            StaticFormat::WebP => ("webp", "Static WebP", &["webp"]),
            StaticFormat::Bmp => ("bmp", "Bitmap", &["bmp"]),
            StaticFormat::Heif => ("heic", "HEIC", &["heic", "heif"]),
        };
        ProviderDescriptor {
            key,
            label,
            extensions,
            kind: MediaKind::Still,
            capabilities: ImageCapabilities {
                still_preview: true,
                flattened_raster: true,
                motion_preview: false,
            },
        }
    }
    fn availability(&self) -> DecoderAvailability {
        if decode::static_available(self.0) {
            DecoderAvailability::Available
        } else {
            DecoderAvailability::Unavailable("feature, runtime library or HEVC plugin unavailable")
        }
    }
    fn recognizes(&self, bytes: &[u8]) -> bool {
        decode::detect_static_format(bytes) == Ok(self.0)
    }
    fn probe(
        &self,
        request: &LocalImageRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ProviderHeader, LocalImageError> {
        let (_, (width, height), orientation) =
            decode::probe_static(&request.bytes, &request.limits, &cancelled)
                .map_err(|error| self.error(error))?;
        Ok(ProviderHeader {
            dimensions: Size::new(width, height),
            orientation,
        })
    }
    fn decode(
        &self,
        request: &LocalImageRequest,
        _: ProviderHeader,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<LocalFrame>, LocalImageError> {
        let (LocalImageOperation::Preview(target) | LocalImageOperation::FlattenedRaster(target)) =
            request.operation
        else {
            return Err(LocalImageError::UnsupportedOperation(self.descriptor().key));
        };
        let asset = decode_static_resized(
            &request.bytes,
            &request.limits,
            target.width,
            target.height,
            cancelled,
        )
        .map_err(|error| self.error(error))?;
        Ok(vec![LocalFrame {
            handle: asset.handle,
            duration: Duration::ZERO,
        }])
    }
}
