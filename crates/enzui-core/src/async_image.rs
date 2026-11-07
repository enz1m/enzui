//! Pure image-request state with URL screening, decode admission, and generation matching.
//!
//! Applications perform I/O and decoding externally, stamp completions with the returned [`Generation`], and feed them to [`EnzAsyncImage::complete`]. A completion is accepted only while the state is loading the same generation.

#[cfg(test)]
mod tests;

use crate::decode_limits::{DecodeLimits, DecodeVerdict, RefusedVerdict};
use crate::url_policy::{SafeUrl, UrlPolicy, UrlVerdict};
use std::fmt;
use std::path::PathBuf;

/// A wrapping counter identifying an image-request generation.
///
/// A completion is accepted only when its value matches the current loading generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// The first generation, carried before any request is made.
    pub const FIRST: Generation = Generation(0);

    /// The next generation, for use when a new source is requested.
    #[must_use]
    pub fn next(self) -> Generation {
        Generation(self.0.wrapping_add(1))
    }

    /// Returns the raw counter value for diagnostics.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A local path or policy-approved remote URL.
///
/// Its `Display` and `Debug` forms show only a local filename and use [`SafeUrl`]'s form, which omits user information and replaces query contents.
#[derive(Clone, PartialEq, Eq)]
pub enum ImageSource {
    /// A local file path. Only the filename appears in `Display`/`Debug`.
    Local(PathBuf),
    /// A remote URL that passed [`UrlPolicy`] validation.
    Remote(SafeUrl),
}

impl ImageSource {
    /// Creates a local source whose displayed form contains only the file name.
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        ImageSource::Local(path.into())
    }

    /// Validates and creates a remote source.
    ///
    /// Rejected URLs are not stored, and the returned [`UrlVerdict`] identifies the policy failure.
    ///
    /// # Errors
    ///
    /// The [`UrlVerdict`] when `policy` refuses `url`.
    pub fn remote(url: &str, policy: &UrlPolicy) -> Result<Self, UrlVerdict> {
        let safe = policy.check_url(url)?;
        Ok(ImageSource::Remote(safe))
    }

    /// Returns the source's redacted display form.
    #[must_use]
    pub fn redacted(&self) -> String {
        match self {
            ImageSource::Local(path) => {
                let name = path
                    .file_name()
                    .map_or_else(|| "<no filename>".into(), |n| n.to_string_lossy());
                format!("local:{name}")
            }
            ImageSource::Remote(safe) => safe.redacted(),
        }
    }
}

impl fmt::Display for ImageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

impl fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ImageSource({})", self.redacted())
    }
}

/// Encoded byte size and declared dimensions used for pre-decode admission.
///
/// Zero values remain literal inputs to [`DecodeLimits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EncodedImage {
    /// The size of the encoded (compressed) payload, in bytes.
    pub encoded_bytes: u64,
    /// The declared decoded width, in pixels, from the header.
    pub width: u32,
    /// The declared decoded height, in pixels, from the header.
    pub height: u32,
}

impl EncodedImage {
    /// Describe an encoded payload of `encoded_bytes` declaring `width`×`height`.
    #[must_use]
    pub fn new(encoded_bytes: u64, width: u32, height: u32) -> Self {
        EncodedImage {
            encoded_bytes,
            width,
            height,
        }
    }

    /// Screens this payload against `limits` and returns the resulting [`DecodeVerdict`].
    #[must_use]
    pub fn screen(&self, limits: &DecodeLimits) -> DecodeVerdict {
        limits.check(self.encoded_bytes, self.width, self.height)
    }
}

/// A reason intended for rendering after sensitive source data has been removed.
///
/// Policy variants contain verdicts; callers must redact a `Service` message before constructing it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FailureReason {
    /// A URL-policy refusal recorded without the rejected URL.
    UrlRefused(UrlVerdict),
    /// A decode-limit refusal whose [`RefusedVerdict`] cannot contain `Allow`.
    DecodeRefused(RefusedVerdict),
    /// A transport or decode error message that the caller has already redacted.
    Service(String),
}

impl FailureReason {
    /// A short, stable tag for a log line or metric label.
    #[must_use]
    pub fn reason(&self) -> &'static str {
        match self {
            FailureReason::UrlRefused(v) => v.reason(),
            FailureReason::DecodeRefused(rv) => rv.reason(),
            FailureReason::Service(_) => "service-error",
        }
    }
}

impl fmt::Display for FailureReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureReason::UrlRefused(v) => write!(f, "url refused: {}", v.reason()),
            FailureReason::DecodeRefused(rv) => write!(f, "decode refused: {}", rv.reason()),
            FailureReason::Service(msg) => write!(f, "service error: {msg}"),
        }
    }
}

/// The generation-keyed lifecycle state of one image request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState<H> {
    /// No request in flight and nothing to show.
    Idle,
    /// A request is in flight under this generation; render a placeholder.
    Loading(Generation),
    /// The decode succeeded and passed all limits; here is the handle.
    Ready(H),
    /// The request failed with a policy verdict or caller-provided service message.
    Failed(FailureReason),
}

impl<H> ImageState<H> {
    /// Whether a request is currently in flight.
    #[must_use]
    pub fn is_loading(&self) -> bool {
        matches!(self, ImageState::Loading(_))
    }

    /// The ready handle, if the request has completed successfully.
    #[must_use]
    pub fn handle(&self) -> Option<&H> {
        match self {
            ImageState::Ready(h) => Some(h),
            _ => None,
        }
    }

    /// The failure reason, if the request failed.
    #[must_use]
    pub fn failure(&self) -> Option<&FailureReason> {
        match self {
            ImageState::Failed(r) => Some(r),
            _ => None,
        }
    }
}

/// A service result stamped with the request generation it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageCompletion<H> {
    /// Which request-session this completion belongs to.
    pub generation: Generation,
    /// The delivered decoded candidate or service failure.
    pub outcome: CompletionOutcome<H>,
}

/// The two things a service completion can carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionOutcome<H> {
    /// A candidate handle and header description awaiting decode-limit admission.
    Decoded {
        /// The candidate decoded handle.
        handle: H,
        /// Header metadata screened before the candidate handle is accepted.
        encoded: EncodedImage,
    },
    /// A service failure; the caller must redact the message before constructing this variant.
    Failed(String),
}

/// Pure generation-keyed state for image requests and completions.
///
/// The application performs fetch and decode work, stamps results with the generation from [`Self::request`], and reads the resulting [`Self::state`].
#[derive(Debug, Clone)]
pub struct EnzAsyncImage<H> {
    generation: Generation,
    source: Option<ImageSource>,
    state: ImageState<H>,
    limits: DecodeLimits,
}

impl<H: Clone> EnzAsyncImage<H> {
    /// A fresh view with the default decode limits and no request.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(DecodeLimits::DEFAULT)
    }

    /// A fresh view with explicit decode limits.
    #[must_use]
    pub fn with_limits(limits: DecodeLimits) -> Self {
        EnzAsyncImage {
            generation: Generation::FIRST,
            source: None,
            state: ImageState::Idle,
            limits,
        }
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The current lifecycle state, for the view layer.
    #[must_use]
    pub fn state(&self) -> &ImageState<H> {
        &self.state
    }

    /// The decode limits in force.
    #[must_use]
    pub fn limits(&self) -> &DecodeLimits {
        &self.limits
    }

    /// The current source, if any.
    #[must_use]
    pub fn source(&self) -> Option<&ImageSource> {
        self.source.as_ref()
    }

    /// Starts a request, advances the generation, and enters [`ImageState::Loading`].
    ///
    /// Stamp the service completion with the returned generation.
    pub fn request(&mut self, source: ImageSource) -> Generation {
        self.generation = self.generation.next();
        self.source = Some(source);
        self.state = ImageState::Loading(self.generation);
        self.generation
    }

    /// Validates and starts a remote request.
    ///
    /// On refusal, advances the generation and enters `Failed` with only the [`UrlVerdict`], ensuring prior completions are stale.
    ///
    /// # Errors
    ///
    /// The [`UrlVerdict`] when `policy` refuses `url`.
    pub fn request_remote(
        &mut self,
        url: &str,
        policy: &UrlPolicy,
    ) -> Result<Generation, UrlVerdict> {
        match ImageSource::remote(url, policy) {
            Ok(source) => Ok(self.request(source)),
            Err(verdict) => {
                self.generation = self.generation.next();
                self.source = None;
                self.state = ImageState::Failed(FailureReason::UrlRefused(verdict));
                Err(verdict)
            }
        }
    }

    /// Applies a completion only while loading its exact generation.
    ///
    /// Decoded candidates pass through [`DecodeLimits`]. Returns `false` for stale or duplicate terminal completions without changing state.
    pub fn complete(&mut self, completion: ImageCompletion<H>) -> bool {
        let accepts = matches!(
            self.state,
            ImageState::Loading(active)
                if active == self.generation && active == completion.generation
        );
        if !accepts {
            return false;
        }
        match completion.outcome {
            CompletionOutcome::Decoded { handle, encoded } => {
                let verdict = encoded.screen(&self.limits);
                if let Some(refused) = RefusedVerdict::new(verdict) {
                    self.state = ImageState::Failed(FailureReason::DecodeRefused(refused));
                } else {
                    self.state = ImageState::Ready(handle);
                }
            }
            CompletionOutcome::Failed(message) => {
                self.state = ImageState::Failed(FailureReason::Service(message));
            }
        }
        true
    }

    /// Reset to [`ImageState::Idle`], advancing the generation so any in-flight
    /// completion is discarded.
    pub fn reset(&mut self) -> Generation {
        self.generation = self.generation.next();
        self.source = None;
        self.state = ImageState::Idle;
        self.generation
    }
}

impl<H: Clone> Default for EnzAsyncImage<H> {
    fn default() -> Self {
        Self::new()
    }
}
