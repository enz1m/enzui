//! Feature-gated GStreamer pipelines with appsink frame-delivery controls and validated remote-source policy.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;

use std::sync::Mutex;

use crate::decode_limits::DecodeLimits;
use crate::frame_plan::{FrameBounds, FrameSeq};
use crate::media::CurlRedirectProbe;
use crate::media::pinned_proxy::{PinnedProxy, PinnedProxyClient};
use crate::media::remote_gate::{
    ApprovedStream, CancelView, Clock, GateEffects, GateRefusal, MediaGate, MonotonicClock,
    SystemResolver,
};
use crate::media::rgba::{
    MAX_EDGE, MAX_NATIVE_RGBA_BYTES, Rgba8Frame, RgbaGeometryError, preflight_rgba8,
};
use crate::media::sink::BoundedFrameSink;
use crate::media::worker::{MediaWorker, StopSignal, WorkerError, WorkerFailure};
use crate::media::{Generation, MediaError, MediaErrorKind, MediaSource};

/// Appsink buffering and drop policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppSinkConfig {
    /// Maximum buffers retained internally by the appsink.
    pub max_buffers: u32,
    /// Whether the appsink drops old buffers instead of blocking.
    pub drop: bool,
}

impl AppSinkConfig {
    /// Latest-frame configuration: one retained appsink buffer with dropping enabled.
    #[allow(clippy::cast_possible_truncation, reason = "`MAX_IN_FLIGHT` is 1")]
    pub const LATEST_FRAME: AppSinkConfig = AppSinkConfig {
        max_buffers: FrameBounds::MAX_IN_FLIGHT as u32,
        drop: true,
    };

    /// Apply this configuration to a live appsink.
    pub fn apply(&self, sink: &gst_app::AppSink) {
        sink.set_max_buffers(self.max_buffers);
        sink.set_drop(self.drop);
    }

    /// Verifies that a live appsink reports this configuration.
    ///
    /// # Errors
    ///
    /// Returns the mismatched property and its expected and actual values.
    pub fn verify_applied(&self, sink: &gst_app::AppSink) -> Result<(), AppSinkConfigError> {
        let actual_max = sink.max_buffers();
        if actual_max != self.max_buffers {
            return Err(AppSinkConfigError::MaxBuffers {
                expected: self.max_buffers,
                actual: actual_max,
            });
        }
        let actual_drop = sink.is_drop();
        if actual_drop != self.drop {
            return Err(AppSinkConfigError::Drop {
                expected: self.drop,
                actual: actual_drop,
            });
        }
        Ok(())
    }
}

/// A mismatch between requested and live appsink properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSinkConfigError {
    /// `max-buffers` does not match.
    MaxBuffers {
        /// What was requested.
        expected: u32,
        /// What the element reports.
        actual: u32,
    },
    /// `drop` does not match.
    Drop {
        /// What was requested.
        expected: bool,
        /// What the element reports.
        actual: bool,
    },
}

impl core::fmt::Display for AppSinkConfigError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AppSinkConfigError::MaxBuffers { expected, actual } => write!(
                f,
                "appsink max-buffers is {actual}, expected {expected}: the \
                 latest-frame bound is not in force"
            ),
            AppSinkConfigError::Drop { expected, actual } => write!(
                f,
                "appsink drop is {actual}, expected {expected}: a slow consumer \
                 will back-pressure the decoder instead of dropping frames"
            ),
        }
    }
}

impl std::error::Error for AppSinkConfigError {}

/// Where a decode pipeline plays the audio of mixed media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AudioOutput {
    /// The desktop's default output (`autoaudiosink`).
    #[default]
    System,
    /// The named sink factory with its `mute` property set: audio is still
    /// converted, negotiated, prerolled and clocked like real playback, but
    /// makes no sound. For tests and headless checks of the playback path.
    Muted(&'static str),
}

/// How the decode worker should run.
#[derive(Debug, Clone, Copy)]
pub struct GstWorkerConfig {
    /// Maximum time allowed for the pipeline to reach `Playing`.
    pub preroll_timeout: Duration,
    /// Interval between stop checks while polling the GStreamer bus.
    pub bus_poll_interval: Duration,
    /// Appsink configuration applied to the pipeline.
    pub appsink: AppSinkConfig,
    /// Destination policy applied to every remote source and redirect.
    pub gate: MediaGate,
    /// Where the audio of mixed media plays. Offline pipelines discard it.
    pub audio: AudioOutput,
}

impl Default for GstWorkerConfig {
    fn default() -> Self {
        GstWorkerConfig {
            preroll_timeout: Duration::from_secs(5),
            bus_poll_interval: Duration::from_millis(50),
            appsink: AppSinkConfig::LATEST_FRAME,
            gate: MediaGate::DEFAULT,
            audio: AudioOutput::System,
        }
    }
}

/// A local source or gate-approved remote stream ready for pipeline construction.
#[derive(Debug, Clone)]
pub struct PreparedSource(Prepared);

#[derive(Debug, Clone)]
enum Prepared {
    Local(std::path::PathBuf),
    Remote(Box<ApprovedStream>),
}

impl PreparedSource {
    /// A local file. No screening applies: nothing leaves the machine.
    #[must_use]
    pub fn local(path: impl Into<std::path::PathBuf>) -> Self {
        PreparedSource(Prepared::Local(path.into()))
    }

    /// A remote stream the gate approved.
    #[must_use]
    pub fn approved_stream(approved: ApprovedStream) -> Self {
        PreparedSource(Prepared::Remote(Box::new(approved)))
    }

    /// Prepares a local source directly or validates a remote source with `gate`.
    ///
    /// # Errors
    ///
    /// Returns the source-policy refusal for an unapproved remote source.
    pub fn prepare(
        source: &MediaSource,
        gate: &MediaGate,
        effects: &GateEffects<'_>,
    ) -> Result<Self, MediaError> {
        match source {
            MediaSource::Local(path) => Ok(PreparedSource::local(path.clone())),
            MediaSource::Remote(_) => {
                let approved = gate.approve_source(source, effects)?;
                match approved {
                    Some(stream) => Ok(PreparedSource::approved_stream(stream)),
                    None => Err(MediaError {
                        kind: MediaErrorKind::Refused,
                        message: "a remote source produced no approval".to_owned(),
                    }),
                }
            }
        }
    }

    /// The approval, for a remote source.
    #[must_use]
    pub fn approved(&self) -> Option<&ApprovedStream> {
        match &self.0 {
            Prepared::Local(_) => None,
            Prepared::Remote(approved) => Some(approved),
        }
    }

    /// Rejects a remote approval older than `ttl_ms`; local sources remain valid.
    ///
    /// # Errors
    ///
    /// A [`MediaError`] when the approval is older than `ttl_ms`.
    pub fn check_fresh(&self, clock: &dyn Clock, ttl_ms: u64) -> Result<(), MediaError> {
        match self.approved() {
            Some(approved) if approved.is_expired(clock.elapsed_ms(), ttl_ms) => Err(MediaError {
                kind: MediaErrorKind::Refused,
                message: "the destination approval expired before the stream opened".to_owned(),
            }),
            _ => Ok(()),
        }
    }

    /// The redacted form, safe to log.
    #[must_use]
    pub fn redacted(&self) -> String {
        match &self.0 {
            Prepared::Local(path) => {
                let name = path
                    .file_name()
                    .map_or_else(|| "<no filename>".into(), |n| n.to_string_lossy());
                format!("local:{name}")
            }
            Prepared::Remote(approved) => approved.redacted(),
        }
    }
}

pub(crate) fn apply_stream_budget(
    decode: &gst::Element,
    approved: &ApprovedStream,
) -> Result<(), MediaError> {
    let budget = approved.budget();
    require_bool(decode, "use-buffering", true)?;
    require_bool(decode, "download", false)?;

    let buffer_bytes = clamp_i32(budget.buffer_bytes);
    require_property_type(decode, "buffer-size", i32::static_type())?;
    decode.set_property("buffer-size", buffer_bytes);
    if decode.property::<i32>("buffer-size") != buffer_bytes {
        return Err(refused_error("the remote buffer-size budget did not take"));
    }

    let ring = budget.ring_buffer_property();
    require_property_type(decode, "ring-buffer-max-size", u64::static_type())?;
    decode.set_property("ring-buffer-max-size", ring);
    if decode.property::<u64>("ring-buffer-max-size") != ring {
        return Err(refused_error("the remote ring-buffer budget did not take"));
    }
    Ok(())
}

pub(crate) fn configure_remote_source(
    source: &gst::Element,
    proxy: &PinnedProxyClient,
    limits: crate::url_policy::TransportLimits,
    approved_uri: &str,
) -> Result<(), MediaError> {
    configure_remote_source_with_disk(source, proxy, limits, approved_uri, false)
}

pub(crate) fn configure_remote_source_with_disk(
    source: &gst::Element,
    proxy: &PinnedProxyClient,
    limits: crate::url_policy::TransportLimits,
    approved_uri: &str,
    disk: bool,
) -> Result<(), MediaError> {
    if approved_uri.starts_with("http://") || approved_uri.starts_with("https://") {
        let expected_factory = if disk || approved_uri.starts_with("https://") {
            super::bounded_http::FACTORY
        } else {
            "souphttpsrc"
        };
        let selected = source
            .factory()
            .is_some_and(|factory| factory.name() == expected_factory);
        require_property_type(source, "location", String::static_type())?;
        let location = source.property::<Option<String>>("location");
        if !selected || location.as_deref() != Some(approved_uri) {
            source.set_property("location", "");
            return Err(refused_error(
                "the approved HTTP source selection did not take",
            ));
        }
        if expected_factory == "souphttpsrc" {
            require_bool(source, "iradio-mode", false)?;
        }
    }
    configure_http_source(source, proxy, limits)
}

pub(crate) fn configure_http_source(
    source: &gst::Element,
    proxy: &PinnedProxyClient,
    limits: crate::url_policy::TransportLimits,
) -> Result<(), MediaError> {
    let factory = source
        .factory()
        .map(|factory| factory.name().to_string())
        .ok_or_else(|| refused_error("the remote source has no inspectable factory"))?;
    if factory != "souphttpsrc"
        && factory != "curlhttpsrc"
        && factory != super::bounded_http::FACTORY
    {
        return Err(refused_error(
            "the remote source factory is not policy-capable",
        ));
    }
    if factory == super::bounded_http::FACTORY {
        let spec = source
            .find_property("receive-buffer-limit")
            .ok_or_else(|| refused_error("the HTTPS source has no inspectable receive limit"))?;
        if spec.value_type() != u64::static_type()
            || !spec.flags().contains(gst::glib::ParamFlags::READABLE)
            || spec.flags().contains(gst::glib::ParamFlags::WRITABLE)
        {
            return Err(refused_error(
                "the HTTPS receive limit is not fixed and readable",
            ));
        }
        let limit = source.property::<u64>("receive-buffer-limit");
        if limit == 0 || limit > super::bounded_http::RECEIVE_LIMIT {
            return Err(refused_error(
                "the HTTPS source has no bounded receive buffer",
            ));
        }
    }
    // The gate already walked any permitted chain. The media get must never
    // follow another redirect on its own.
    require_bool(source, "automatic-redirect", false)?;
    require_bool(source, "ssl-strict", true)?;
    require_bool(source, "keep-alive", false)?;
    require_property_type(source, "retries", i32::static_type())?;
    source.set_property("retries", 0i32);
    if source.property::<i32>("retries") != 0 {
        return Err(refused_error("the remote source retry bound did not take"));
    }
    require_property_type(source, "proxy", String::static_type())?;
    source.set_property("proxy", proxy.uri());
    let applied: Option<String> = source.property("proxy");
    match applied.as_deref() {
        Some(readback) if proxy_uris_equivalent(proxy.uri(), readback) => {}
        _ => return Err(refused_error("the pinned media proxy setting did not take")),
    }
    // Verify both standard proxy credential properties; setting only the proxy URI can bypass authentication.
    require_property_type(source, "proxy-id", String::static_type())?;
    require_property_type(source, "proxy-pw", String::static_type())?;
    source.set_property("proxy-id", proxy.username());
    source.set_property("proxy-pw", proxy.password());
    let applied_id: Option<String> = source.property("proxy-id");
    let applied_password: Option<String> = source.property("proxy-pw");
    if applied_id.as_deref() != Some(proxy.username())
        || applied_password.as_deref() != Some(proxy.password())
    {
        return Err(refused_error(
            "the pinned media proxy authentication setting did not take",
        ));
    }

    require_property(source, "timeout")?;
    // Curl's whole-transfer timeout must not expire while downstream playback is paused.
    let seconds = if factory == "curlhttpsrc" || factory == super::bounded_http::FACTORY {
        0
    } else {
        timeout_property_secs(limits.read_timeout_ms)
    };
    let value_type = source.property_value("timeout").type_();
    if value_type == u32::static_type() {
        source.set_property("timeout", seconds);
        if source.property::<u32>("timeout") != seconds {
            return Err(refused_error("the remote source timeout did not take"));
        }
    } else if value_type == i32::static_type() {
        let seconds = i32::try_from(seconds).unwrap_or(i32::MAX);
        source.set_property("timeout", seconds);
        if source.property::<i32>("timeout") != seconds {
            return Err(refused_error("the remote source timeout did not take"));
        }
    } else {
        return Err(refused_error(
            "the remote source timeout has an unsupported type",
        ));
    }
    Ok(())
}

// Convert millisecond deadlines to positive whole seconds; GStreamer interprets zero as unbounded.
fn timeout_property_secs(read_timeout_ms: u64) -> u32 {
    if read_timeout_ms == crate::url_policy::NO_LIMIT {
        return 0;
    }
    let seconds = read_timeout_ms.div_ceil(1000).clamp(1, 3600);
    u32::try_from(seconds).unwrap_or(3600)
}

fn bus_poll_millis(interval: Duration) -> u64 {
    crate::monotonic_ms::MonotonicMs::from_duration_saturating(interval)
}

// Compare normalized parsed proxy identities rather than URI spelling.
fn proxy_uris_equivalent(expected: &str, actual: &str) -> bool {
    struct ProxyParts<'a> {
        scheme: &'a str,
        host: &'a str,
        port: &'a str,
        path: &'a str,
        query: Option<&'a str>,
    }

    fn decompose(uri: &str) -> Option<ProxyParts<'_>> {
        let (scheme, rest) = uri.split_once("://")?;
        if scheme.is_empty() {
            return None;
        }
        let rest = rest.split_once('#').map_or(rest, |(a, _)| a);
        let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, after_auth) = rest.split_at(auth_end);
        if authority.contains('@') {
            return None;
        }
        let (host, port) = if let Some(bracket) = authority.strip_prefix('[') {
            let (addr, rest) = bracket.split_once(']')?;
            let port = rest.strip_prefix(':').unwrap_or("");
            (addr, port)
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h, p),
                None => (authority, ""),
            }
        };
        let (path, query) = match after_auth.split_once('?') {
            Some((p, q)) => (p, Some(q)),
            None => (after_auth, None),
        };
        Some(ProxyParts {
            scheme,
            host,
            port,
            path,
            query,
        })
    }

    fn normalize_path(p: &str) -> &str {
        if p.is_empty() { "/" } else { p }
    }

    let (Some(exp), Some(act)) = (decompose(expected), decompose(actual)) else {
        return false;
    };
    exp.scheme.eq_ignore_ascii_case(act.scheme)
        && exp.host.eq_ignore_ascii_case(act.host)
        && exp.port == act.port
        && normalize_path(exp.path) == normalize_path(act.path)
        && exp.query == act.query
}

fn require_property(object: &gst::Element, name: &str) -> Result<gst::glib::ParamSpec, MediaError> {
    let property = object.find_property(name).ok_or_else(|| {
        refused_error(&format!(
            "the remote source lacks required property `{name}`"
        ))
    })?;
    let flags = property.flags();
    if !flags.contains(gst::glib::ParamFlags::READABLE)
        || !flags.contains(gst::glib::ParamFlags::WRITABLE)
    {
        return Err(refused_error(&format!(
            "the remote source property `{name}` is not readable and writable"
        )));
    }
    Ok(property)
}

fn require_property_type(
    object: &gst::Element,
    name: &str,
    expected: gst::glib::Type,
) -> Result<(), MediaError> {
    let property = require_property(object, name)?;
    if property.value_type() != expected {
        return Err(refused_error(&format!(
            "the remote source property `{name}` has an unsupported type"
        )));
    }
    Ok(())
}

fn require_bool(object: &gst::Element, name: &str, value: bool) -> Result<(), MediaError> {
    require_property_type(object, name, bool::static_type())?;
    object.set_property(name, value);
    if object.property::<bool>(name) != value {
        return Err(refused_error(&format!(
            "the remote source property `{name}` did not take"
        )));
    }
    Ok(())
}

// Clamp byte limits to the signed GStreamer property range; the unlimited sentinel maps to `-1`.
fn clamp_i32(bytes: u64) -> i32 {
    if bytes == crate::url_policy::NO_LIMIT {
        -1
    } else {
        i32::try_from(bytes).unwrap_or(i32::MAX)
    }
}

/// Where a remote source's configuration result lands once it is known.
type RemoteConfiguration = Arc<Mutex<Option<Result<(), MediaError>>>>;

/// Typed elements owned by a constructed GStreamer pipeline.
pub struct BuiltPipeline {
    /// The top-level pipeline.
    pub pipeline: gst::Pipeline,
    /// The video appsink used for frame pulls and configuration verification.
    pub video_sink: gst_app::AppSink,
    remote_configuration: Option<RemoteConfiguration>,
    _pinned_proxy: Option<PinnedProxy>,
    // The pipeline's own duplicate of a PipeWire remote, shared with any
    // thread still changing the pipeline's state, so the source never uses a
    // descriptor its caller has closed.
    remote_fd: Option<Arc<std::os::fd::OwnedFd>>,
}

impl BuiltPipeline {
    /// Sets the pipeline to `Null` to release element resources.
    pub fn shutdown(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    fn wait_remote_configuration(
        &self,
        timeout: Duration,
        cancel: Option<&dyn CancelView>,
    ) -> Result<(), MediaError> {
        let started = std::time::Instant::now();
        let deadline = crate::monotonic_ms::checked_instant_deadline(
            started,
            timeout,
            crate::monotonic_ms::Policy::Clamp,
        );
        let Some(configuration) = &self.remote_configuration else {
            return Ok(());
        };
        loop {
            if let Some(cancel) = cancel
                && cancel.is_cancelled()
            {
                return Err(refused_error("remote media setup was cancelled"));
            }
            let configured = configuration
                .lock()
                .map_err(|_| refused_error("the remote source configuration result was poisoned"))?
                .clone();
            if let Some(result) = configured {
                return result;
            }
            if let Some(error) = self.remote_transport_error() {
                return Err(error);
            }
            let now = std::time::Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                return Err(refused_error("the live remote source was never configured"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn remote_transport_error(&self) -> Option<MediaError> {
        self._pinned_proxy.as_ref().and_then(PinnedProxy::error)
    }

    // Refuse playback unless the live HTTP source confirms use of the approved proxy.
    fn wait_proxy_traversed(
        &self,
        timeout: Duration,
        cancel: Option<&dyn CancelView>,
    ) -> Result<(), MediaError> {
        let started = std::time::Instant::now();
        let deadline = crate::monotonic_ms::checked_instant_deadline(
            started,
            timeout,
            crate::monotonic_ms::Policy::Clamp,
        );
        let Some(proxy) = &self._pinned_proxy else {
            return Ok(());
        };
        loop {
            if proxy.connections() > 0 {
                return Ok(());
            }
            if let Some(error) = proxy.error() {
                return Err(error);
            }
            if let Some(cancel) = cancel
                && cancel.is_cancelled()
            {
                return Err(refused_error("remote media setup was cancelled"));
            }
            let now = std::time::Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                return Err(refused_error(
                    "the remote media stream never traversed the pinned proxy",
                ));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Builds a typed decode pipeline for a prepared source.
///
/// # Errors
///
/// Returns a typed media error when construction or remote-policy enforcement fails.
pub fn build_pipeline(
    source: &PreparedSource,
    config: &GstWorkerConfig,
) -> Result<BuiltPipeline, MediaError> {
    build_pipeline_inner(source, config, false)
}

pub(crate) fn build_offline_pipeline(
    source: &PreparedSource,
    config: &GstWorkerConfig,
) -> Result<BuiltPipeline, MediaError> {
    build_pipeline_inner(source, config, true)
}

fn build_pipeline_inner(
    source: &PreparedSource,
    config: &GstWorkerConfig,
    offline: bool,
) -> Result<BuiltPipeline, MediaError> {
    let uri = pipeline_uri(source)?;
    let pinned_proxy = source.approved().map(PinnedProxy::start).transpose()?;
    let remote_configuration = pinned_proxy
        .as_ref()
        .map(|_| Arc::new(Mutex::new(None::<Result<(), MediaError>>)));

    let pipeline = gst::Pipeline::new();

    let decode_builder = gst::ElementFactory::make("uridecodebin").property("uri", &uri);
    let decode_builder = if offline {
        decode_builder.name("enzui-offline-decode")
    } else {
        decode_builder
    };
    let decode = decode_builder
        .build()
        .map_err(|_| missing_element("uridecodebin"))?;

    // Apply the approved remote budget and redirect policy to live elements, reading properties back where supported.
    if let Some(approved) = source.approved() {
        apply_stream_budget(&decode, approved)?;
        let limits = approved.limits();
        let approved_uri = approved.transport_url().to_owned();
        let proxy = pinned_proxy
            .as_ref()
            .expect("a remote approval creates a proxy")
            .client_configuration();
        let configuration = Arc::clone(
            remote_configuration
                .as_ref()
                .expect("a remote approval creates a configuration result"),
        );
        decode.connect("source-setup", false, move |values| {
            let result = values
                .get(1)
                .and_then(|value| value.get::<gst::Element>().ok())
                .ok_or_else(|| refused_error("uridecodebin did not expose its live source"))
                .and_then(|src| {
                    let result = configure_remote_source(&src, &proxy, limits, &approved_uri);
                    if result.is_err() {
                        let _ = src.set_state(gst::State::Null);
                    }
                    result
                });
            if let Ok(mut slot) = configuration.lock() {
                *slot = Some(result);
            }
            None
        });
    }

    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|_| missing_element("videoconvert"))?;

    // Pin output to the RGBA format accepted by `Rgba8Frame`.
    let caps = gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgba)
        .width_range(1..=i32::try_from(MAX_EDGE).unwrap_or(i32::MAX))
        .height_range(1..=i32::try_from(MAX_EDGE).unwrap_or(i32::MAX))
        .build();

    let video_sink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(config.appsink.max_buffers)
        .drop(config.appsink.drop)
        .sync(!offline)
        .build();

    config.appsink.apply(&video_sink);

    pipeline
        .add_many([&decode, &convert, video_sink.upcast_ref()])
        .map_err(|_| backend_error("could not assemble the pipeline"))?;
    if offline {
        let scale = gst::ElementFactory::make("videoscale")
            .build()
            .map_err(|_| missing_element("videoscale"))?;
        pipeline
            .add(&scale)
            .map_err(|_| backend_error("could not add offline scaler"))?;
        let filter = gst::ElementFactory::make("capsfilter")
            .name("enzui-offline-output")
            .build()
            .map_err(|_| missing_element("capsfilter"))?;
        pipeline
            .add(&filter)
            .map_err(|_| backend_error("could not add offline output bounds"))?;
        gst::Element::link_many([&convert, &scale, &filter, video_sink.upcast_ref()])
            .map_err(|_| backend_error("could not link offline video output"))?;
    } else {
        convert
            .link(&video_sink)
            .map_err(|_| backend_error("could not link videoconvert to appsink"))?;
    }

    // Link video when the dynamic decode pad appears; keep audio entirely within GStreamer.
    let convert_weak = convert.downgrade();
    let pipeline_weak = pipeline.downgrade();
    let audio = config.audio;
    decode.connect_pad_added(move |_, pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let Some(pipeline) = pipeline_weak.upgrade() else {
            return;
        };
        let Some(media_type) = pad
            .current_caps()
            .and_then(|c| c.structure(0).map(|s| s.name().to_string()))
        else {
            return;
        };

        if media_type.starts_with("video/") {
            if let Some(sink_pad) = convert.static_pad("sink")
                && !sink_pad.is_linked()
            {
                let _ = pad.link(&sink_pad);
            }
        } else if offline {
            if let Ok(audio_sink) = gst::ElementFactory::make("fakesink").build()
                && pipeline.add(&audio_sink).is_ok()
            {
                audio_sink.set_property("sync", false);
                audio_sink.set_property("async", false);
                let _ = audio_sink.sync_state_with_parent();
                if let Some(sink_pad) = audio_sink.static_pad("sink") {
                    let _ = pad.link(&sink_pad);
                }
            }
        } else if media_type.starts_with("audio/") {
            link_audio_output(&pipeline, pad, audio);
        }
    });

    Ok(BuiltPipeline {
        pipeline,
        video_sink,
        remote_configuration,
        _pinned_proxy: pinned_proxy,
        remote_fd: None,
    })
}

/// Plays a decoded audio pad through `queue ! audioconvert ! audioresample ! sink`.
///
/// Decoders emit formats an output cannot take directly (AAC decodes to planar
/// float, which no desktop sink accepts), and a sink linked to them straight
/// never prerolls, so the whole pipeline waited in `Paused` with its video
/// already prerolled. The queue keeps the audio branch from blocking
/// the demuxer's video branch. The chain is linked before its states are synced
/// downstream-first, so no element starts streaming into an unlinked peer.
///
/// A chain that cannot be built or linked posts an element error, so playback
/// fails visibly rather than continuing without its audio.
fn link_audio_output(pipeline: &gst::Pipeline, pad: &gst::Pad, output: AudioOutput) {
    let build = || -> Result<Vec<gst::Element>, &'static str> {
        let make = |factory: &'static str| {
            gst::ElementFactory::make(factory)
                .build()
                .map_err(|_| factory)
        };
        let sink = match output {
            AudioOutput::System => make("autoaudiosink")?,
            AudioOutput::Muted(factory) => {
                let sink = make(factory)?;
                if sink.find_property("mute").is_none() {
                    return Err(factory);
                }
                sink.set_property("mute", true);
                sink
            }
        };
        Ok(vec![
            make("queue")?,
            make("audioconvert")?,
            make("audioresample")?,
            sink,
        ])
    };
    let chain = match build() {
        Ok(chain) => chain,
        Err(factory) => {
            post_audio_error(pipeline, &format!("missing audio element {factory}"));
            return;
        }
    };

    let linked = pipeline.add_many(&chain).is_ok()
        && gst::Element::link_many(&chain).is_ok()
        && chain
            .iter()
            .rev()
            .all(|element| element.sync_state_with_parent().is_ok())
        && chain[0]
            .static_pad("sink")
            .is_some_and(|sink| pad.link(&sink).is_ok());
    if !linked {
        post_audio_error(pipeline, "could not link the audio output");
    }
}

fn post_audio_error(pipeline: &gst::Pipeline, message: &str) {
    let _ = pipeline.post_message(
        gst::message::Error::builder(gst::CoreError::Negotiation, message)
            .src(pipeline)
            .build(),
    );
}

/// Counts the payload bytes a live remote source delivers.
///
/// A transfer that ends without a byte reaches typefind as a stream that does
/// not contain enough data, which reads as unsupported media. The services
/// consult this count so that transport failure is reported as one.
#[derive(Debug, Clone)]
pub(crate) struct PayloadCounter(Arc<AtomicU64>);

impl PayloadCounter {
    const NOT_OBSERVED: u64 = u64::MAX;

    pub(crate) fn new() -> Self {
        Self(Arc::new(AtomicU64::new(Self::NOT_OBSERVED)))
    }

    /// Starts counting the buffers leaving `source`'s `src` pad.
    pub(crate) fn observe(&self, source: &gst::Element) {
        let Some(pad) = source.static_pad("src") else {
            return;
        };
        self.0.store(0, Ordering::Release);
        let counter = Arc::clone(&self.0);
        let _ = pad.add_probe(
            gst::PadProbeType::BUFFER | gst::PadProbeType::BUFFER_LIST,
            move |_, info| {
                let bytes = match &info.data {
                    Some(gst::PadProbeData::Buffer(buffer)) => buffer.size(),
                    Some(gst::PadProbeData::BufferList(list)) => list.calculate_size(),
                    _ => 0,
                };
                counter.fetch_add(bytes as u64, Ordering::AcqRel);
                gst::PadProbeReturn::Ok
            },
        );
    }

    /// The bytes delivered so far, or `None` before a source was observed.
    pub(crate) fn received(&self) -> Option<u64> {
        Some(self.0.load(Ordering::Acquire)).filter(|&bytes| bytes != Self::NOT_OBSERVED)
    }
}

/// Builds an RGBA PipeWire screencast pipeline for the supplied remote fd and node id.
///
/// The pipeline duplicates `fd` and keeps the duplicate for as long as it runs,
/// so the caller may close its own descriptor at any time.
///
/// # Errors
///
/// Returns a media error if the descriptor cannot be duplicated, or if GStreamer
/// initialization or pipeline construction fails.
pub fn build_pipewire_pipeline(
    fd: std::os::fd::BorrowedFd<'_>,
    node_id: u32,
    config: &GstWorkerConfig,
) -> Result<BuiltPipeline, MediaError> {
    use std::os::fd::AsRawFd as _;

    ensure_initialised()?;
    let remote_fd = Arc::new(
        fd.try_clone_to_owned()
            .map_err(|_| backend_error("could not duplicate the pipewire remote descriptor"))?,
    );
    let fd = remote_fd.as_raw_fd();

    let pipeline = gst::Pipeline::new();

    // `pipewiresrc` binds to the portal's remote over `fd`, and to the specific
    // negotiated stream over `path` (the node id, as a string).
    let source = gst::ElementFactory::make("pipewiresrc")
        .property("fd", fd)
        .property("path", node_id.to_string())
        .build()
        .map_err(|_| missing_element("pipewiresrc"))?;

    let convert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|_| missing_element("videoconvert"))?;

    // Reuse the file pipeline’s RGBA contract, with defensive width and height caps for hostile portal metadata.
    let caps = gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgba)
        .width_range(1..=i32::try_from(MAX_EDGE).unwrap_or(i32::MAX))
        .height_range(1..=i32::try_from(MAX_EDGE).unwrap_or(i32::MAX))
        .build();

    let video_sink = gst_app::AppSink::builder()
        .caps(&caps)
        .max_buffers(config.appsink.max_buffers)
        .drop(config.appsink.drop)
        .sync(true)
        .build();
    config.appsink.apply(&video_sink);

    pipeline
        .add_many([&source, &convert, video_sink.upcast_ref()])
        .map_err(|_| backend_error("could not assemble the pipewire pipeline"))?;
    // `pipewiresrc` has an always src pad, so link statically — no pad-added
    // dance like `uridecodebin` needs.
    source
        .link(&convert)
        .map_err(|_| backend_error("could not link pipewiresrc to videoconvert"))?;
    convert
        .link(&video_sink)
        .map_err(|_| backend_error("could not link videoconvert to appsink"))?;

    Ok(BuiltPipeline {
        pipeline,
        video_sink,
        remote_configuration: None,
        _pinned_proxy: None,
        remote_fd: Some(remote_fd),
    })
}

/// Starts the pipeline and returns the first decoded frame, polling until the timeout deadline is observed.
///
/// # Errors
///
/// Returns a typed error when the latest-frame appsink policy is absent, startup or decoding fails, the stream ends before a frame, or the deadline is observed.
pub fn read_one_frame(
    built: &BuiltPipeline,
    generation: Generation,
    timeout: Duration,
) -> Result<Rgba8Frame, MediaError> {
    let started = std::time::Instant::now();
    let deadline = crate::monotonic_ms::checked_instant_deadline(
        started,
        timeout,
        crate::monotonic_ms::Policy::Clamp,
    );

    // The one-frame API requires the latest-frame appsink policy before pulling.
    config_bound_or_err(built)?;

    // pipewiresrc connects during the state change and can block there for
    // its own connection timeout, so the change runs off this thread and the
    // deadline still holds. A change still pending at the deadline tears the
    // pipeline down itself, holding the remote descriptor until it has.
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let pipeline = built.pipeline.clone();
    let remote_fd = built.remote_fd.clone();
    let spawned = std::thread::Builder::new()
        .name("enzui-gst-start".to_owned())
        .spawn(move || {
            let changed = pipeline.set_state(gst::State::Playing);
            if started_tx.send(changed.is_ok()).is_err() {
                let _ = pipeline.set_state(gst::State::Null);
            }
            drop(remote_fd);
        });
    if spawned.is_err() {
        built.shutdown();
        return Err(backend_error(
            "could not start the pipewire pipeline thread",
        ));
    }
    let remaining = deadline.map_or(timeout, |deadline| {
        deadline.saturating_duration_since(std::time::Instant::now())
    });
    match started_rx.recv_timeout(remaining) {
        Ok(true) => {}
        Ok(false) => {
            built.shutdown();
            return Err(backend_error("the pipewire pipeline would not start"));
        }
        Err(_) => {
            return Err(backend_error(
                "the pipewire pipeline did not start within the timeout",
            ));
        }
    }

    let result = loop {
        let now = std::time::Instant::now();
        if deadline.is_some_and(|deadline| now >= deadline) {
            break Err(backend_error(
                "no frame arrived from pipewiresrc within the timeout",
            ));
        }
        match built
            .video_sink
            .try_pull_sample(gst::ClockTime::from_mseconds(100))
        {
            Some(sample) => match sample_to_frame(&sample, generation, FrameSeq(1)) {
                SampleOutcome::Frame(frame) => break Ok(frame),
                // A rejected sample (e.g. wrong format) is a hard error here:
                // the caller asked for one frame and the negotiation failed.
                SampleOutcome::Rejected(e) => break Err(e),
            },
            None => {
                if built.video_sink.is_eos() {
                    break Err(backend_error("pipewiresrc reached EOS before a frame"));
                }
            }
        }
    };

    built.shutdown();
    result
}

fn config_bound_or_err(built: &BuiltPipeline) -> Result<(), MediaError> {
    let sink = &built.video_sink;
    if sink.max_buffers() != AppSinkConfig::LATEST_FRAME.max_buffers
        || sink.is_drop() != AppSinkConfig::LATEST_FRAME.drop
    {
        return Err(backend_error(
            "the appsink bound (max-buffers=1 drop=true) did not take",
        ));
    }
    Ok(())
}

pub(crate) fn pipeline_uri(source: &PreparedSource) -> Result<String, MediaError> {
    let uri = source_to_uri(source)?;
    // Source aliases select policy-capable factories without changing global ranks.
    if let Some(rest) = uri.strip_prefix("http://") {
        Ok(format!("icy://{rest}"))
    } else if let Some(rest) = uri.strip_prefix("https://") {
        Ok(format!("enzuihttps://{rest}"))
    } else {
        Ok(uri)
    }
}

pub(crate) fn source_to_uri(source: &PreparedSource) -> Result<String, MediaError> {
    match &source.0 {
        Prepared::Local(path) => gst::glib::filename_to_uri(path, None)
            .map(Into::into)
            .map_err(|_| {
                // The message must not carry the path — `PreparedSource` redacts,
                // and an error string is exactly where a full path leaks into a log.
                MediaError {
                    kind: MediaErrorKind::SourceNotFound,
                    message: format!("could not form a file URI for {}", source.redacted()),
                }
            }),
        Prepared::Remote(approved) => Ok(approved.transport_url().to_owned()),
    }
}

fn missing_element(name: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::UnsupportedFormat,
        message: format!("the GStreamer element `{name}` is not installed"),
    }
}

fn backend_error(what: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Backend,
        message: what.to_owned(),
    }
}

fn refused_error(what: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Refused,
        message: what.to_owned(),
    }
}

/// Initializes GStreamer and registers the bounded HTTPS media source.
///
/// # Errors
///
/// Returns a backend error if initialization or native source registration fails.
pub fn ensure_initialised() -> Result<(), MediaError> {
    gst::init().map_err(|e| MediaError {
        kind: MediaErrorKind::Backend,
        message: format!("GStreamer failed to initialise: {e}"),
    })?;
    super::bounded_http::register().map_err(backend_error)
}

/// What a decoded sample turned into.
pub enum SampleOutcome {
    /// A validated RGBA8 frame.
    Frame(Rgba8Frame),
    /// The sample could not be used; the reason is redacted and typed.
    Rejected(MediaError),
}

pub(crate) enum SampleConversionError {
    Backend(MediaError),
    Geometry(RgbaGeometryError),
}

impl core::fmt::Display for SampleConversionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Backend(error) => error.fmt(f),
            Self::Geometry(error) => error.fmt(f),
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_COPIED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_DESTINATION_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_NATIVE_MAPPINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static TEST_NATIVE_MAPPING_LIMIT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

fn native_mapping_limit() -> u64 {
    #[cfg(test)]
    if let Some(limit) = TEST_NATIVE_MAPPING_LIMIT.get() {
        return limit.min(MAX_NATIVE_RGBA_BYTES);
    }
    MAX_NATIVE_RGBA_BYTES
}

fn copy_rgba_plane(
    width: u32,
    height: u32,
    stride: u64,
    plane: &[u8],
    limits: &DecodeLimits,
) -> Result<Vec<u8>, RgbaGeometryError> {
    // Validate geometry and plane length before allocating or reading source bytes.
    let layout = preflight_rgba8(width, height, stride, limits)?.check_byte_len(plane.len())?;
    #[cfg(test)]
    TEST_DESTINATION_ALLOCATIONS.set(TEST_DESTINATION_ALLOCATIONS.get() + 1);
    let mut pixels = Vec::with_capacity(layout.tight_len());
    for row in 0..height as usize {
        let start = row * layout.stride();
        pixels.extend_from_slice(&plane[start..start + layout.row_bytes()]);
    }
    #[cfg(test)]
    TEST_COPIED_BYTES.set(TEST_COPIED_BYTES.get() + pixels.len());
    Ok(pixels)
}

/// Converts one appsink sample to a checked RGBA8 frame.
///
/// Invalid caps, layout, or readable plane bounds produce [`SampleOutcome::Rejected`].
pub fn sample_to_frame(
    sample: &gst::Sample,
    generation: Generation,
    seq: FrameSeq,
) -> SampleOutcome {
    match sample_to_frame_with_limits(sample, generation, seq, &DecodeLimits::DEFAULT) {
        Ok(frame) => SampleOutcome::Frame(frame),
        Err(error) => SampleOutcome::Rejected(match error {
            SampleConversionError::Backend(error) => error,
            SampleConversionError::Geometry(error) => MediaError {
                kind: MediaErrorKind::UnsupportedFormat,
                message: format!("a decoded frame failed geometry preflight: {error}"),
            },
        }),
    }
}

pub(crate) fn sample_to_frame_with_limits(
    sample: &gst::Sample,
    generation: Generation,
    seq: FrameSeq,
    limits: &DecodeLimits,
) -> Result<Rgba8Frame, SampleConversionError> {
    let Some(buffer) = sample.buffer() else {
        return Err(SampleConversionError::Backend(backend_error(
            "an appsink sample carried no buffer",
        )));
    };
    let Some(caps) = sample.caps() else {
        return Err(SampleConversionError::Backend(backend_error(
            "an appsink sample carried no caps",
        )));
    };
    let Ok(info) = gst_video::VideoInfo::from_caps(caps) else {
        return Err(SampleConversionError::Backend(backend_error(
            "an appsink sample's caps were not video caps",
        )));
    };

    if info.format() != gst_video::VideoFormat::Rgba {
        // The capsfilter should make this unreachable; refusing rather than
        // reinterpreting means a negotiation bug cannot become wrong pixels.
        return Err(SampleConversionError::Backend(MediaError {
            kind: MediaErrorKind::UnsupportedFormat,
            message: format!(
                "expected RGBA, negotiated {:?}: only checked SDR RGBA8 is accepted",
                info.format()
            ),
        }));
    }

    // Prefer VideoMeta layout over caps when present, without creating a slice before validation.
    let (width, height, signed_stride, plane_offset) =
        if let Some(meta) = buffer.meta::<gst_video::VideoMeta>() {
            if meta.format() != gst_video::VideoFormat::Rgba
                || meta.width() != info.width()
                || meta.height() != info.height()
                || meta.n_planes() != 1
            {
                return Err(SampleConversionError::Backend(backend_error(
                    "an RGBA frame's video metadata disagreed with its caps",
                )));
            }
            let Some(&stride) = meta.stride().first() else {
                return Err(SampleConversionError::Backend(backend_error(
                    "an RGBA frame had no plane 0 stride",
                )));
            };
            let Some(&offset) = meta.offset().first() else {
                return Err(SampleConversionError::Backend(backend_error(
                    "an RGBA frame had no plane 0 offset",
                )));
            };
            (meta.width(), meta.height(), stride, offset)
        } else {
            let Some(&stride) = info.stride().first() else {
                return Err(SampleConversionError::Backend(backend_error(
                    "an RGBA frame had no plane 0 stride",
                )));
            };
            let Some(&offset) = info.offset().first() else {
                return Err(SampleConversionError::Backend(backend_error(
                    "an RGBA frame had no plane 0 offset",
                )));
            };
            (info.width(), info.height(), stride, offset)
        };

    if signed_stride < 0 {
        return Err(SampleConversionError::Geometry(
            RgbaGeometryError::NegativeStride {
                stride: signed_stride,
            },
        ));
    }
    let stride = u64::try_from(signed_stride).map_err(|_| {
        SampleConversionError::Geometry(RgbaGeometryError::NegativeStride {
            stride: signed_stride,
        })
    })?;
    let layout =
        preflight_rgba8(width, height, stride, limits).map_err(SampleConversionError::Geometry)?;
    // Bound the complete merged GstMemory map, including bytes before the plane offset.
    let backing_len = u64::try_from(buffer.size()).unwrap_or(u64::MAX);
    let mapping_limit = native_mapping_limit();
    if backing_len > mapping_limit {
        return Err(SampleConversionError::Geometry(
            RgbaGeometryError::BackingExtentTooLarge {
                bytes: backing_len,
                limit: mapping_limit,
            },
        ));
    }
    // Validate the plane against both BufferRef size and the mapped slice length.
    let available = buffer.size().saturating_sub(plane_offset);
    layout
        .check_byte_len(available)
        .map_err(SampleConversionError::Geometry)?;

    #[cfg(test)]
    TEST_NATIVE_MAPPINGS.set(TEST_NATIVE_MAPPINGS.get() + 1);
    let map = buffer.map_readable().map_err(|_| {
        SampleConversionError::Backend(backend_error("an appsink buffer could not be mapped"))
    })?;
    let plane = map.as_slice().get(plane_offset..).ok_or_else(|| {
        SampleConversionError::Geometry(RgbaGeometryError::BufferTooShort {
            required: layout.readable_len(),
            actual: 0,
        })
    })?;
    layout
        .check_byte_len(plane.len())
        .map_err(SampleConversionError::Geometry)?;
    let pixels = copy_rgba_plane(width, height, stride, plane, limits)
        .map_err(SampleConversionError::Geometry)?;

    Rgba8Frame::new(generation, seq, width, height, pixels).map_err(|error| {
        SampleConversionError::Backend(MediaError {
            kind: MediaErrorKind::UnsupportedFormat,
            message: format!("a decoded frame failed validation: {error}"),
        })
    })
}

/// Runs the decode loop until cancellation, end of stream, frame delivery stops, or an error occurs.
///
/// # Errors
///
/// Returns a worker failure for source-policy, setup, or playback errors.
pub fn run_gst_worker<F>(
    source: MediaSource,
    generation: Generation,
    config: GstWorkerConfig,
    signal: StopSignal,
    progress: Arc<AtomicU64>,
    mut on_frame: F,
) -> Result<u64, WorkerFailure>
where
    F: FnMut(Rgba8Frame) -> bool + Send,
{
    ensure_initialised()?;
    // Screen the destination and every redirect before constructing any pipeline element.
    let clock = MonotonicClock::start();
    let redirect_probe = CurlRedirectProbe;
    let effects = GateEffects {
        resolver: &SystemResolver,
        probe: Some(&redirect_probe),
        clock: &clock,
        cancel: &signal,
    };
    let prepared = match &source {
        MediaSource::Local(path) => PreparedSource::local(path.clone()),
        MediaSource::Remote(remote) => PreparedSource::approved_stream(
            config
                .gate
                .approve(remote, &effects)
                .map_err(WorkerFailure::RemotePolicy)?,
        ),
    };
    if signal.should_stop() {
        return Err(GateRefusal::Cancelled.into());
    }
    prepared.check_fresh(&clock, config.gate.approval_ttl_ms)?;
    let built = build_pipeline(&prepared, &config)?;
    if signal.should_stop() {
        built.shutdown();
        return Err(GateRefusal::Cancelled.into());
    }
    if let Err(error) = config.appsink.verify_applied(&built.video_sink) {
        built.shutdown();
        return Err(backend_error(&error.to_string()).into());
    }

    if built.pipeline.set_state(gst::State::Playing).is_err() {
        let configuration_error = if built.remote_configuration.is_some() {
            built
                .wait_remote_configuration(Duration::from_millis(20), Some(&signal))
                .err()
        } else {
            None
        };
        built.shutdown();
        return Err(configuration_error
            .unwrap_or_else(|| backend_error("the media pipeline would not start"))
            .into());
    }
    if let Err(error) = built.wait_remote_configuration(config.preroll_timeout, Some(&signal)) {
        built.shutdown();
        return Err(error.into());
    }
    if let Err(error) = built.wait_proxy_traversed(config.preroll_timeout, Some(&signal)) {
        built.shutdown();
        return Err(error.into());
    }

    let bus = built.pipeline.bus();
    let mut frames = 0u64;
    let mut seq = FrameSeq(0);

    while !signal.should_stop() {
        // Bounded pull: never waits longer than the poll interval, so the stop
        // flag is observed promptly and this loop cannot hang.
        match built
            .video_sink
            .try_pull_sample(gst::ClockTime::from_mseconds(bus_poll_millis(
                config.bus_poll_interval,
            ))) {
            Some(sample) => {
                seq = seq.next();
                if let SampleOutcome::Frame(frame) = sample_to_frame(&sample, generation, seq) {
                    frames += 1;
                    progress.store(frames, Ordering::Release);
                    if !on_frame(frame) {
                        break;
                    }
                }
            }
            None => {
                if built.video_sink.is_eos() {
                    if let Some(error) = built.remote_transport_error() {
                        built.shutdown();
                        return Err(error.into());
                    }
                    break;
                }
            }
        }

        if let Some(error) = built.remote_transport_error() {
            built.shutdown();
            return Err(error.into());
        }

        if let Some(bus) = &bus
            && let Some(msg) = bus.timed_pop(gst::ClockTime::ZERO)
            && matches!(msg.view(), gst::MessageView::Error(_))
        {
            built.shutdown();
            return Err(backend_error("the media pipeline reported a playback error").into());
        }
    }

    // Request the `Null` state before returning so worker completion initiates pipeline teardown.
    let transport_error = built.remote_transport_error();
    built.shutdown();
    match transport_error {
        Some(error) => Err(error.into()),
        None => Ok(frames),
    }
}

/// Spawns a decode worker that delivers frames to a bounded sink.
///
/// Decode and policy failures are reported through the worker's join outcome.
///
/// # Errors
///
/// Returns [`WorkerError::SpawnFailed`] if the worker thread cannot be created.
pub fn spawn_gst_worker(
    source: MediaSource,
    generation: Generation,
    config: GstWorkerConfig,
    sink: Arc<Mutex<BoundedFrameSink>>,
) -> Result<MediaWorker, WorkerError> {
    MediaWorker::spawn_fallible(generation, move |signal, progress| {
        run_gst_worker(source, generation, config, signal, progress, move |frame| {
            // End delivery on a poisoned sink lock instead of panicking on the worker thread.
            match sink.lock() {
                Ok(mut sink) => {
                    sink.deliver(frame);
                    true
                }
                Err(_) => false,
            }
        })
    })
}

#[cfg(test)]
mod tests;
