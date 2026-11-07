//! Optional media-service downloads through a separately packaged extractor.
//!
//! The adapter never treats a page URL as a media stream. It asks `yt-dlp` for
//! metadata and formats first, then runs the selected format in an isolated
//! staging directory and atomically publishes exactly one regular output file.
//! Every extractor command uses an owned private working directory. User
//! configuration, plugins, self-update, remote components, and ambient
//! environment variables other than the snapshotted `PATH` are disabled.

use enzui_core::cancellation::CancelToken;
use enzui_core::download_destination::ReservedDownloadDestination;
use enzui_security::runtime_dir;
use enzui_security::secure_dir::{SecureRoot, ensure_private_dir};
use enzui_security::tempstage::{DirectoryStage, unique_temp_file_name};
use enzui_services::process_transport::{
    self, OutputStream, ProcessError, ProcessErrorKind, ProcessOutput, ProcessOutputChunk,
    ProcessRequest, StdinPolicy,
};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::File;
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

const DIAGNOSTIC_LIMIT: usize = 512;
const MAX_FORMATS: usize = 256;
const MAX_FIELD_BYTES: usize = 1024;
const PROGRESS_PREFIX: &str = "enzui-progress-v1\t";

/// Installation guidance shown when no compatible extractor can be started.
pub const YT_DLP_INSTALL_HINT: &str = "Install yt-dlp from your system package manager to download media-service URLs. Install ffmpeg to merge separate video and audio streams.";

/// Oldest supported yt-dlp release.
///
/// yt-dlp versions are release dates. `2025.11.12` is the first tagged release
/// whose parser accepts every safety option used by all three commands. Bump
/// this only when an invocation or output contract requires a newer release,
/// and update the packaged helper and parser provenance in the same change.
pub const YT_DLP_MIN_VERSION: &str = "2025.11.12";

/// Installation guidance shown when the discovered yt-dlp is below the floor.
pub const YT_DLP_UPDATE_HINT: &str = "The installed yt-dlp is too old. Update yt-dlp with your system package manager; Files never self-updates helpers.";

/// A validated yt-dlp format identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceFormatId(String);

impl ServiceFormatId {
    /// Validates a format id before it can become a process argument.
    ///
    /// # Errors
    ///
    /// A [`ServiceDownloadError`] when `value` is empty, longer than 128 bytes, or holds anything
    /// but ASCII letters, digits, `.`, `_` and `-`.
    pub fn parse(value: impl Into<String>) -> Result<Self, ServiceDownloadError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            return Err(ServiceDownloadError::new(
                ServiceDownloadErrorKind::Protocol,
                "the extractor returned an invalid format identifier",
            ));
        }
        Ok(Self(value))
    }

    /// The exact identifier passed to the extractor's format selector.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated format selector chosen from inspected metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ServiceFormatSelection {
    /// One exact extractor format id.
    Exact(ServiceFormatId),
    /// Best video and audio streams, merged when separate.
    BestVideoAudio,
}

impl ServiceFormatSelection {
    fn selector(&self) -> &str {
        match self {
            Self::Exact(id) => id.as_str(),
            Self::BestVideoAudio => "bestvideo+bestaudio/best",
        }
    }
}

/// One format choice presented to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFormatChoice {
    /// Typed selector passed back to the extractor.
    pub selection: ServiceFormatSelection,
    /// Human-readable container, resolution, media kind, and size.
    pub label: String,
}

/// One selectable media representation reported by the extractor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceFormat {
    /// Stable extractor format identifier.
    pub id: ServiceFormatId,
    /// Suggested container extension.
    pub extension: String,
    /// Encoded width, when reported.
    pub width: Option<u32>,
    /// Encoded height, when reported.
    pub height: Option<u32>,
    /// Video codec, absent for audio-only formats.
    pub video_codec: Option<String>,
    /// Audio codec, absent for video-only formats.
    pub audio_codec: Option<String>,
    /// Exact or estimated encoded size.
    pub bytes: Option<u64>,
}

impl ServiceFormat {
    /// A compact user-facing description that retains the exact format id.
    #[must_use]
    pub fn label(&self) -> String {
        let kind = match (&self.video_codec, &self.audio_codec) {
            (Some(_), Some(_)) => self.height.map_or_else(
                || "video + audio".to_owned(),
                |h| format!("{h}p video + audio"),
            ),
            (Some(_), None) => self
                .height
                .map_or_else(|| "video only".to_owned(), |h| format!("{h}p video only")),
            (None, Some(_)) => "audio only".to_owned(),
            (None, None) => "media".to_owned(),
        };
        let size = self.bytes.map_or(String::new(), |bytes| {
            format!(" · {:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
        });
        format!("{} · {} · {kind}{size}", self.id.as_str(), self.extension)
    }
}

/// Metadata and bounded selectable formats for one service URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceMedia {
    /// Filename-safe title reported by the extractor.
    pub title: String,
    /// Duration in whole seconds, when reported.
    pub duration_seconds: Option<u64>,
    /// At most 256 formats in extractor order.
    pub formats: Vec<ServiceFormat>,
}

impl ServiceMedia {
    /// Builds bounded choices from this metadata, including safe composite selectors.
    #[must_use]
    pub fn format_choices(&self) -> Vec<ServiceFormatChoice> {
        let has_video = self
            .formats
            .iter()
            .any(|format| format.video_codec.is_some());
        let has_audio = self
            .formats
            .iter()
            .any(|format| format.audio_codec.is_some());
        let mut choices = Vec::with_capacity(self.formats.len() + 1);
        if has_video && has_audio {
            choices.push(ServiceFormatChoice {
                selection: ServiceFormatSelection::BestVideoAudio,
                label: "Best video + audio (recommended)".into(),
            });
        }
        choices.extend(self.formats.iter().map(|format| ServiceFormatChoice {
            selection: ServiceFormatSelection::Exact(format.id.clone()),
            label: format.label(),
        }));
        choices
    }
}

/// Whether a discovered extractor satisfies the framework's invocation contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractorCapability {
    /// The release is at or above [`YT_DLP_MIN_VERSION`].
    Supported,
    /// The release predates [`YT_DLP_MIN_VERSION`].
    TooOld,
}

/// A discovered extractor installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractorInfo {
    /// Version string returned by the executable.
    pub version: String,
}

impl ExtractorInfo {
    /// Classifies a yt-dlp date version against [`YT_DLP_MIN_VERSION`].
    ///
    /// Versions use `YYYY.MM.DD`; an optional suffix starts with `.`, `-`, or
    /// `+` and then contains ASCII letters, digits, `.`, `_`, `-`, or `+`.
    /// Comparison is lexicographic over the numeric date, ignoring that suffix.
    ///
    /// # Errors
    ///
    /// A [`ServiceDownloadError`] when the version cannot be parsed.
    pub fn capability(&self) -> Result<ExtractorCapability, ServiceDownloadError> {
        let discovered = parse_yt_dlp_version(&self.version)?;
        let minimum = parse_yt_dlp_version(YT_DLP_MIN_VERSION)?;
        Ok(if discovered < minimum {
            ExtractorCapability::TooOld
        } else {
            ExtractorCapability::Supported
        })
    }

    /// Refuses an extractor that cannot satisfy the current invocation contract.
    ///
    /// # Errors
    ///
    /// A [`ServiceDownloadError`] when the extractor is too old or its version cannot be parsed.
    pub fn require_supported(self) -> Result<Self, ServiceDownloadError> {
        match self.capability()? {
            ExtractorCapability::Supported => Ok(self),
            ExtractorCapability::TooOld => Err(too_old_error(Some(&self.version))),
        }
    }
}

/// Progress emitted by a running service download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceProgress {
    /// Bytes received so far.
    pub downloaded_bytes: u64,
    /// Exact or estimated total bytes, when available.
    pub total_bytes: Option<u64>,
}

/// A selected service download and its portal-authorized final path.
#[derive(Debug, Clone)]
pub struct ServiceDownloadRequest {
    /// Page URL understood by the extractor.
    pub url: String,
    /// Format selected from the preceding metadata response.
    pub format: ServiceFormatSelection,
    /// Portal-derived reservation that receives the completed payload atomically.
    pub destination: ReservedDownloadDestination,
    /// Receive ceiling in KiB/s; zero means unlimited.
    pub download_limit_kib: u32,
}

/// Stable category for extractor failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceDownloadErrorKind {
    /// The extractor executable is not installed or cannot be started.
    Unavailable,
    /// The extractor predates the supported version floor.
    TooOld,
    /// The operation was cancelled.
    Cancelled,
    /// The operation exceeded its deadline.
    Deadline,
    /// The extractor exited unsuccessfully.
    Failed,
    /// The extractor violated the bounded metadata or output protocol.
    Protocol,
    /// Staging or atomic publication failed.
    Io,
}

/// Bounded, user-presentable extractor failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDownloadError {
    kind: ServiceDownloadErrorKind,
    detail: String,
}

impl ServiceDownloadError {
    /// Creates a typed failure for an injected extractor implementation.
    pub fn new(kind: ServiceDownloadErrorKind, detail: impl Into<String>) -> Self {
        let detail = detail.into();
        let detail = redact_diagnostic_urls(&detail);
        Self {
            kind,
            detail: bounded_text(detail, DIAGNOSTIC_LIMIT),
        }
    }

    /// The stable failure category.
    #[must_use]
    pub fn kind(&self) -> ServiceDownloadErrorKind {
        self.kind
    }

    /// A bounded diagnostic safe to show in application chrome.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for ServiceDownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for ServiceDownloadError {}

/// Injectable service-extractor capability.
pub trait ServiceExtractor: fmt::Debug + Send + Sync {
    /// Discovers the executable and reports its version.
    ///
    /// # Errors
    ///
    /// A [`ServiceDownloadError`] when the executable is missing or its version cannot be read.
    fn discover(&self, cancel: &CancelToken) -> Result<ExtractorInfo, ServiceDownloadError>;

    /// Extracts metadata without downloading media bytes.
    ///
    /// # Errors
    ///
    /// A [`ServiceDownloadError`] when extraction fails or is cancelled.
    fn inspect(
        &self,
        url: &str,
        cancel: &CancelToken,
    ) -> Result<ServiceMedia, ServiceDownloadError>;

    /// Downloads the selected format and atomically replaces the reserved destination.
    ///
    /// # Errors
    ///
    /// A [`ServiceDownloadError`] when the download fails or is cancelled; the destination is then
    /// unchanged.
    fn download(
        &self,
        request: &ServiceDownloadRequest,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(ServiceProgress),
    ) -> Result<u64, ServiceDownloadError>;
}

/// Resource and time ceilings for [`YtDlpExtractor`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceExtractorLimits {
    /// Deadline for executable discovery.
    pub probe_deadline: Duration,
    /// Deadline for one metadata request.
    pub metadata_deadline: Duration,
    /// Deadline for one media transfer.
    pub download_deadline: Duration,
    /// Maximum metadata stdout bytes.
    pub metadata_output_bytes: usize,
    /// Maximum bytes retained from each download output pipe.
    pub download_output_bytes: usize,
}

impl Default for ServiceExtractorLimits {
    fn default() -> Self {
        Self {
            probe_deadline: Duration::from_secs(3),
            metadata_deadline: Duration::from_secs(45),
            download_deadline: Duration::from_secs(24 * 60 * 60),
            metadata_output_bytes: 512 * 1024,
            download_output_bytes: 8 * 1024 * 1024,
        }
    }
}

/// `yt-dlp` implementation of [`ServiceExtractor`].
#[derive(Debug, Clone)]
pub struct YtDlpExtractor {
    executable: OsString,
    path: Option<OsString>,
    limits: ServiceExtractorLimits,
}

impl Default for YtDlpExtractor {
    fn default() -> Self {
        Self::new("yt-dlp")
    }
}

impl YtDlpExtractor {
    /// Uses one direct executable and snapshots `PATH`; no other environment or cwd is inherited.
    #[must_use]
    pub fn new(executable: impl Into<OsString>) -> Self {
        Self {
            executable: executable.into(),
            path: std::env::var_os("PATH"),
            limits: ServiceExtractorLimits::default(),
        }
    }

    /// Overrides resource ceilings, primarily for hosts with stricter policies.
    #[must_use]
    pub fn with_limits(mut self, limits: ServiceExtractorLimits) -> Self {
        self.limits = limits;
        self
    }

    fn request(
        &self,
        stdin: StdinPolicy,
        deadline: Duration,
        stdout_limit: usize,
        stderr_limit: usize,
        current_dir: &Path,
    ) -> ProcessRequest {
        let request = ProcessRequest::new(
            self.executable.clone(),
            stdin,
            stdout_limit,
            stderr_limit,
            deadline,
        )
        .current_dir(current_dir);
        match &self.path {
            Some(path) => request.env("PATH", path.clone()),
            None => request,
        }
    }
}

impl ServiceExtractor for YtDlpExtractor {
    fn discover(&self, cancel: &CancelToken) -> Result<ExtractorInfo, ServiceDownloadError> {
        let working = OwnedStage::create()?;
        let output = process_transport::run(
            self.request(
                StdinPolicy::Null,
                self.limits.probe_deadline,
                128,
                4096,
                working.path(),
            )
            .args([
                "--ignore-config",
                "--no-plugin-dirs",
                "--no-update",
                "--version",
            ]),
            cancel,
        )
        .map_err(map_process_error)?;
        if !output.status.success() && rejected_required_discovery_flag(&output.stderr) {
            return Err(too_old_error(None));
        }
        require_success(output).and_then(|output| {
            let version = parse_short_field(&output.stdout, "extractor version")?;
            ExtractorInfo { version }.require_supported()
        })
    }

    fn inspect(
        &self,
        url: &str,
        cancel: &CancelToken,
    ) -> Result<ServiceMedia, ServiceDownloadError> {
        validate_url(url)?;
        let working = OwnedStage::create()?;
        let mut request = self.request(
            source_url_stdin(url),
            self.limits.metadata_deadline,
            self.limits.metadata_output_bytes,
            32 * 1024,
            working.path(),
        );
        for template in metadata_templates() {
            request = request.arg("--print").arg(template);
        }
        let output = process_transport::run(
            request.args([
                "--ignore-config",
                "--no-plugin-dirs",
                "--no-remote-components",
                "--no-update",
                "--no-cache-dir",
                "--no-playlist",
                "--skip-download",
                "--no-warnings",
                "--socket-timeout",
                "10",
                "--format",
                "all",
                "--batch-file",
                "-",
            ]),
            cancel,
        )
        .map_err(map_process_error)?;
        parse_metadata(&require_success(output)?.stdout)
    }

    fn download(
        &self,
        request: &ServiceDownloadRequest,
        cancel: &CancelToken,
        progress: &mut dyn FnMut(ServiceProgress),
    ) -> Result<u64, ServiceDownloadError> {
        validate_url(&request.url)?;
        let stage = OwnedStage::create()?;
        let output_template = stage.path().join("payload.%(ext)S");
        let temp_path = format!("temp:{}", stage.path().display());
        let mut process = self
            .request(
                source_url_stdin(&request.url),
                self.limits.download_deadline,
                self.limits.download_output_bytes,
                self.limits.download_output_bytes,
                stage.path(),
            )
            .args([
                "--ignore-config",
                "--no-plugin-dirs",
                "--no-remote-components",
                "--no-update",
                "--no-cache-dir",
                "--no-playlist",
                "--no-overwrites",
                "--no-write-info-json",
                "--no-write-playlist-metafiles",
                "--no-write-subs",
                "--no-write-auto-subs",
                "--no-write-thumbnail",
                "--newline",
                "--progress",
                "--progress-delta",
                "1.0",
                "--progress-template",
                "download:enzui-progress-v1\t%(progress.downloaded_bytes)s\t%(progress.total_bytes,progress.total_bytes_estimate|NA)s",
                "--format",
            ])
            .arg(request.format.selector())
            .arg("--paths")
            .arg(temp_path)
            .arg("--output")
            .arg(output_template.as_os_str())
            // A media download is sustained network and disk work nobody
            // watches frame by frame.
            .background(process_transport::BackgroundScope::worker(
                "enzui-service-download",
            ));
        if matches!(request.format, ServiceFormatSelection::BestVideoAudio) {
            process = process.args(["--merge-output-format", "mkv"]);
        }
        if request.download_limit_kib > 0 {
            process = process
                .arg("--limit-rate")
                .arg(format!("{}K", request.download_limit_kib));
        }
        process = process.arg("--batch-file").arg("-");

        let (sender, receiver) = mpsc::sync_channel(16);
        let mut decoder = ProgressDecoder::default();
        let result = std::thread::scope(|scope| {
            let worker = scope.spawn(|| process_transport::run_streaming(process, cancel, sender));
            loop {
                match receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(chunk) => decoder.push(chunk, progress),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            worker.join().map_err(|_| {
                ServiceDownloadError::new(
                    ServiceDownloadErrorKind::Failed,
                    "the extractor worker stopped unexpectedly",
                )
            })
        })?;
        let output = require_success(result.map_err(map_process_error)?)?;
        decoder.finish(progress);
        let (mut source, bytes) = stage.single_output()?;
        request
            .destination
            .publish_with(|target| {
                source.seek(SeekFrom::Start(0))?;
                let copied = std::io::copy(&mut source, target)?;
                if copied != bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "staged media changed while it was being published",
                    ));
                }
                Ok(())
            })
            .map_err(|error| {
                ServiceDownloadError::new(
                    ServiceDownloadErrorKind::Io,
                    format!("could not publish the selected media: {error}"),
                )
            })?;
        let _ = output;
        Ok(bytes)
    }
}

fn source_url_stdin(url: &str) -> StdinPolicy {
    let mut input = Vec::with_capacity(url.len() + 1);
    input.extend_from_slice(url.as_bytes());
    input.push(b'\n');
    StdinPolicy::Bytes(input)
}

fn metadata_templates() -> [&'static str; 1] {
    [
        "enzui-format-v1\t%(title)S\t%(duration|NA)s\t%(format_id)s\t%(ext)s\t%(width|NA)s\t%(height|NA)s\t%(vcodec|NA)s\t%(acodec|NA)s\t%(filesize,filesize_approx|NA)s",
    ]
}

fn parse_metadata(bytes: &[u8]) -> Result<ServiceMedia, ServiceDownloadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| protocol("metadata was not UTF-8"))?;
    let mut title: Option<String> = None;
    let mut duration: Option<Option<u64>> = None;
    let mut seen = HashSet::new();
    let mut formats = Vec::new();
    for line in text.lines() {
        if line.len() > MAX_FIELD_BYTES {
            return Err(protocol("metadata field exceeded its size limit"));
        }
        let fields: Vec<_> = line.split('\t').collect();
        let [
            "enzui-format-v1",
            row_title,
            row_duration_raw,
            id,
            extension,
            width,
            height,
            video_codec,
            audio_codec,
            size,
        ] = fields.as_slice()
        else {
            return Err(protocol("metadata format record was malformed"));
        };
        if row_title.is_empty() {
            return Err(protocol("metadata title was missing"));
        }
        match &title {
            Some(title) if title != row_title => {
                return Err(protocol("metadata rows disagreed about the title"));
            }
            None => title = Some((*row_title).to_owned()),
            _ => {}
        }
        let row_duration = parse_optional_u64(row_duration_raw)?;
        match duration {
            Some(previous) if previous != row_duration => {
                return Err(protocol("metadata rows disagreed about the duration"));
            }
            None => duration = Some(row_duration),
            _ => {}
        }
        let id = ServiceFormatId::parse((*id).to_owned())?;
        if !seen.insert(id.clone()) {
            return Err(protocol("metadata repeated a format identifier"));
        }
        let video_codec = parse_codec(video_codec)?;
        let audio_codec = parse_codec(audio_codec)?;
        if video_codec.is_none() && audio_codec.is_none() {
            continue;
        }
        formats.push(ServiceFormat {
            id,
            extension: parse_token(extension, "container extension", 16)?,
            width: parse_optional_u32(width)?,
            height: parse_optional_u32(height)?,
            video_codec,
            audio_codec,
            bytes: parse_optional_u64(size)?,
        });
        if formats.len() > MAX_FORMATS {
            return Err(protocol("metadata exceeded the format limit"));
        }
    }
    if formats.is_empty() {
        return Err(protocol("metadata returned no playable formats"));
    }
    Ok(ServiceMedia {
        title: title.expect("a format established the title"),
        duration_seconds: duration.unwrap_or(None),
        formats,
    })
}

fn too_old_error(version: Option<&str>) -> ServiceDownloadError {
    let installed = version.map_or_else(
        || "The installed yt-dlp does not support the required safety options".to_owned(),
        |version| format!("yt-dlp {version} is too old"),
    );
    ServiceDownloadError::new(
        ServiceDownloadErrorKind::TooOld,
        format!(
            "{installed}; version {YT_DLP_MIN_VERSION} or newer is required. {YT_DLP_UPDATE_HINT}"
        ),
    )
}

fn rejected_required_discovery_flag(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    ["no such option:", "unrecognized arguments:"]
        .into_iter()
        .any(|marker| stderr.contains(marker) && stderr.contains("--no-plugin-dirs"))
}

fn parse_yt_dlp_version(value: &str) -> Result<(u32, u8, u8), ServiceDownloadError> {
    let mut parts = value.splitn(3, '.');
    let year = parts.next().unwrap_or_default();
    let month = parts.next().unwrap_or_default();
    let day_and_suffix = parts.next().unwrap_or_default();
    if year.len() != 4
        || month.len() != 2
        || day_and_suffix.len() < 2
        || !year.bytes().all(|byte| byte.is_ascii_digit())
        || !month.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(protocol("extractor version was not a yt-dlp date version"));
    }
    let (day, suffix) = day_and_suffix.split_at(2);
    if !day.bytes().all(|byte| byte.is_ascii_digit())
        || (!suffix.is_empty()
            && (!matches!(suffix.as_bytes()[0], b'.' | b'-' | b'+')
                || suffix.len() == 1
                || !suffix.bytes().skip(1).all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
                })))
    {
        return Err(protocol("extractor version was not a yt-dlp date version"));
    }
    let year = year
        .parse::<u32>()
        .map_err(|_| protocol("extractor version year was invalid"))?;
    let month = month
        .parse::<u8>()
        .map_err(|_| protocol("extractor version month was invalid"))?;
    let day = day
        .parse::<u8>()
        .map_err(|_| protocol("extractor version day was invalid"))?;
    let leap_year =
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap_year => 29,
        2 => 28,
        _ => 0,
    };
    if day == 0 || day > maximum_day {
        return Err(protocol("extractor version date was invalid"));
    }
    Ok((year, month, day))
}

fn parse_short_field(bytes: &[u8], label: &str) -> Result<String, ServiceDownloadError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| protocol(format!("{label} was not UTF-8")))?
        .trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err(protocol(format!("{label} was missing or invalid")))
    } else {
        Ok(value.to_owned())
    }
}

fn parse_optional_u64(value: &str) -> Result<Option<u64>, ServiceDownloadError> {
    if matches!(value, "" | "NA" | "None" | "null") {
        return Ok(None);
    }
    value
        .split_once('.')
        .map_or(value, |(whole, _)| whole)
        .parse::<u64>()
        .map(Some)
        .map_err(|_| protocol("metadata contained an invalid number"))
}

fn parse_optional_u32(value: &str) -> Result<Option<u32>, ServiceDownloadError> {
    parse_optional_u64(value)?
        .map(u32::try_from)
        .transpose()
        .map_err(|_| protocol("metadata number exceeded its limit"))
}

fn parse_token(value: &str, label: &str, limit: usize) -> Result<String, ServiceDownloadError> {
    if value.is_empty()
        || value.len() > limit
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        Err(protocol(format!("metadata contained an invalid {label}")))
    } else {
        Ok(value.to_owned())
    }
}

fn parse_codec(value: &str) -> Result<Option<String>, ServiceDownloadError> {
    if matches!(value, "" | "NA" | "None" | "null" | "none") {
        Ok(None)
    } else {
        parse_token(value, "codec", 64).map(Some)
    }
}

fn validate_url(url: &str) -> Result<(), ServiceDownloadError> {
    if url.len() > 8192
        || url.chars().any(char::is_control)
        || !(url.starts_with("https://") || url.starts_with("http://"))
    {
        Err(ServiceDownloadError::new(
            ServiceDownloadErrorKind::Protocol,
            "service downloads require a valid HTTP or HTTPS URL",
        ))
    } else {
        Ok(())
    }
}

fn protocol(detail: impl Into<String>) -> ServiceDownloadError {
    ServiceDownloadError::new(ServiceDownloadErrorKind::Protocol, detail)
}

fn require_success(output: ProcessOutput) -> Result<ProcessOutput, ServiceDownloadError> {
    if output.status.success() {
        Ok(output)
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        Err(ServiceDownloadError::new(
            ServiceDownloadErrorKind::Failed,
            if detail.is_empty() {
                "the extractor exited unsuccessfully".to_owned()
            } else {
                format!("the extractor failed: {detail}")
            },
        ))
    }
}

fn map_process_error(error: ProcessError) -> ServiceDownloadError {
    let kind = match error.kind() {
        ProcessErrorKind::Spawn | ProcessErrorKind::ExecutableAuthority => {
            ServiceDownloadErrorKind::Unavailable
        }
        ProcessErrorKind::Cancelled(_) => ServiceDownloadErrorKind::Cancelled,
        ProcessErrorKind::Deadline => ServiceDownloadErrorKind::Deadline,
        ProcessErrorKind::OutputLimitExceeded(_) => ServiceDownloadErrorKind::Protocol,
        _ => ServiceDownloadErrorKind::Failed,
    };
    let detail = match kind {
        ServiceDownloadErrorKind::Unavailable => YT_DLP_INSTALL_HINT.to_owned(),
        ServiceDownloadErrorKind::Cancelled => "the service download was cancelled".to_owned(),
        ServiceDownloadErrorKind::Deadline => "the extractor exceeded its deadline".to_owned(),
        ServiceDownloadErrorKind::Protocol => "the extractor output exceeded its limit".to_owned(),
        _ => error
            .complete_capture(OutputStream::Stderr)
            .filter(|bytes| !bytes.is_empty())
            .map_or_else(
                || error.to_string(),
                |bytes| format!("the extractor failed: {}", String::from_utf8_lossy(bytes)),
            ),
    };
    ServiceDownloadError::new(kind, detail)
}

fn redact_diagnostic_urls(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remainder = value;
    while let Some((start, scheme_len)) = find_http_url(remainder) {
        let (before, candidate) = remainder.split_at(start);
        output.push_str(before);
        let end = candidate
            .char_indices()
            .find_map(|(index, ch)| (index > 0 && ch.is_whitespace()).then_some(index))
            .unwrap_or(candidate.len());
        let (url, rest) = candidate.split_at(end);
        output.push_str(redact_http_url(url, scheme_len));
        remainder = rest;
    }
    output.push_str(remainder);
    output
}

fn find_http_url(value: &str) -> Option<(usize, usize)> {
    value.char_indices().find_map(|(index, _)| {
        let candidate = value.get(index..)?;
        if candidate
            .get(.."https://".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
        {
            Some((index, "https://".len()))
        } else if candidate
            .get(.."http://".len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        {
            Some((index, "http://".len()))
        } else {
            None
        }
    })
}

fn redact_http_url(url: &str, scheme_len: usize) -> &str {
    let after_scheme = url.get(scheme_len..).unwrap_or_default();
    let (authority, suffix) = after_scheme.split_at(
        after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len()),
    );
    if authority.contains('@') || suffix.contains(['?', '#']) {
        "[URL]"
    } else {
        url
    }
}

fn bounded_text(value: String, limit: usize) -> String {
    let mut filtered: String = value
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    if filtered.len() <= limit {
        return filtered;
    }
    let mut end = limit;
    while !filtered.is_char_boundary(end) {
        end -= 1;
    }
    filtered.truncate(end);
    filtered
}

#[derive(Default)]
struct ProgressDecoder {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl ProgressDecoder {
    fn push(&mut self, chunk: ProcessOutputChunk, emit: &mut dyn FnMut(ServiceProgress)) {
        let pending = match chunk.stream {
            OutputStream::Stdout => &mut self.stdout,
            OutputStream::Stderr => &mut self.stderr,
        };
        pending.extend_from_slice(&chunk.bytes);
        while let Some(end) = pending
            .iter()
            .position(|byte| matches!(byte, b'\n' | b'\r'))
        {
            let line = pending.drain(..=end).collect::<Vec<_>>();
            if let Some(progress) = parse_progress(&line[..line.len() - 1]) {
                emit(progress);
            }
        }
        if pending.len() > MAX_FIELD_BYTES {
            pending.clear();
        }
    }

    fn finish(&mut self, emit: &mut dyn FnMut(ServiceProgress)) {
        for pending in [&mut self.stdout, &mut self.stderr] {
            if let Some(progress) = parse_progress(pending) {
                emit(progress);
            }
            pending.clear();
        }
    }
}

fn parse_progress(line: &[u8]) -> Option<ServiceProgress> {
    let line = std::str::from_utf8(line).ok()?;
    let body = line.strip_prefix(PROGRESS_PREFIX)?;
    let (downloaded, total) = body.split_once('\t')?;
    let downloaded_bytes = downloaded.parse().ok()?;
    let total_bytes = parse_optional_u64(total).ok()?;
    Some(ServiceProgress {
        downloaded_bytes,
        total_bytes,
    })
}

struct OwnedStage {
    stage: DirectoryStage,
}

impl OwnedStage {
    fn create() -> Result<Self, ServiceDownloadError> {
        let parent = runtime_dir::runtime_dir()
            .map_err(|error| {
                ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
            })?
            .join("enzui-service-downloads");
        ensure_private_dir(&parent).map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        let parent = SecureRoot::open(&parent).map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        for _ in 0..16 {
            let name = unique_temp_file_name(OsStr::new("transfer"));
            match DirectoryStage::create(&parent, &name) {
                Ok(stage) => return Ok(Self { stage }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(ServiceDownloadError::new(
                        ServiceDownloadErrorKind::Io,
                        format!("could not create a service-download stage: {error}"),
                    ));
                }
            }
        }
        Err(ServiceDownloadError::new(
            ServiceDownloadErrorKind::Io,
            "could not allocate a unique service-download stage",
        ))
    }

    fn path(&self) -> &Path {
        self.stage.path()
    }

    fn single_output(&self) -> Result<(File, u64), ServiceDownloadError> {
        let root = self.stage.root();
        root.revalidate().map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        let entries = root.entry_names().map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        let [entry] = entries.as_slice() else {
            return Err(protocol(
                "extractor did not produce exactly one output file",
            ));
        };
        let mut file = root.open_entry_read(&entry.name).map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        let metadata = file.metadata().map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if !metadata.is_file() || metadata.nlink() != 1 {
                return Err(protocol(
                    "extractor output was not one privately owned regular file",
                ));
            }
        }
        #[cfg(not(unix))]
        if !metadata.is_file() {
            return Err(protocol("extractor output was not a regular file"));
        }
        file.seek(SeekFrom::Start(0)).map_err(|error| {
            ServiceDownloadError::new(ServiceDownloadErrorKind::Io, error.to_string())
        })?;
        Ok((file, metadata.len()))
    }
}

#[cfg(test)]
mod tests;
