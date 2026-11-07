//! Bounded archive extraction through the system's libarchive.
//!
//! [`extract`] reads ZIP, TAR (plain, gzip, bzip2, xz, zstd), 7z, RAR4/RAR5
//! and standalone gzip/bzip2/xz/zstd streams, including split 7z/ZIP/TAR
//! sets (`name.7z.001`) and multi-part RAR (`name.part1.rar`,
//! `name.rar` + `name.r00`). libarchive is loaded at run time, so a host
//! without it, or without one codec, answers [`ArchiveError::CodecUnavailable`]
//! instead of failing to start; [`capabilities`] says up front what this host
//! reads. A codec libarchive would run as an external program is reported
//! missing and never registered, so extraction never starts a process.
//!
//! The native reader only decodes. This module owns every decision about the
//! filesystem:
//!
//! * Entry names stay native bytes. A name that is absolute, climbs with
//!   `..`, or names nothing is skipped, and so is every symlink, hardlink,
//!   device, FIFO or socket entry: nothing in the output can point anywhere.
//! * Data streams straight from the decoder into the output file, one block
//!   at a time; sparse entries keep their holes. [`Limits`] bounds the entry
//!   count, the bytes actually written, and each file's apparent size, all
//!   checked as data arrives, never trusted from the archive's own header.
//! * A second entry for a path already written follows [`Duplicates`].
//! * Output goes into a fresh private staging folder beside the destination,
//!   which is published only after every entry is written, with a
//!   no-replace rename to a folder named after the archive (`name`,
//!   `name (2)`, ...). Failure, cancellation or a panic removes the staging
//!   folder; nothing partial is ever visible under the archive's name.
//!
//! Encrypted ZIP entries extract with a passphrase given as a
//! [`SecretText`]: it reaches libarchive in memory, never through argv or the
//! environment. Run extraction on [`ArchiveWorker`], which bounds concurrency
//! and reports [`ExtractProgress`] for an activity row.

mod native;
mod volumes;
mod worker;

use enzui_security::secret::SecretText;
use enzui_security::secure_dir::{MoveCustody, RetainedDir};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Seek, Write};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub use worker::{ArchiveWorker, ExtractOutcome};

/// An archive or compression format, named by its file extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ArchiveFormat {
    /// `.zip`
    Zip,
    /// `.tar`
    Tar,
    /// `.tar.gz`, `.tgz`
    TarGz,
    /// `.tar.bz2`, `.tbz2`, `.tbz`
    TarBz2,
    /// `.tar.xz`, `.txz`
    TarXz,
    /// `.tar.zst`, `.tzst`
    TarZstd,
    /// `.7z`
    SevenZip,
    /// `.rar`, RAR4 or RAR5
    Rar,
    /// A single gzip-compressed file, `.gz`
    Gzip,
    /// A single bzip2-compressed file, `.bz2`
    Bzip2,
    /// A single xz-compressed file, `.xz`
    Xz,
    /// A single zstd-compressed file, `.zst`
    Zstd,
}

/// Longest suffix first, so `.tar.gz` wins over `.gz`.
const SUFFIXES: &[(&str, ArchiveFormat)] = &[
    (".tar.bz2", ArchiveFormat::TarBz2),
    (".tar.zst", ArchiveFormat::TarZstd),
    (".tar.gz", ArchiveFormat::TarGz),
    (".tar.xz", ArchiveFormat::TarXz),
    (".tbz2", ArchiveFormat::TarBz2),
    (".tzst", ArchiveFormat::TarZstd),
    (".tbz", ArchiveFormat::TarBz2),
    (".tgz", ArchiveFormat::TarGz),
    (".txz", ArchiveFormat::TarXz),
    (".tar", ArchiveFormat::Tar),
    (".zip", ArchiveFormat::Zip),
    (".rar", ArchiveFormat::Rar),
    (".bz2", ArchiveFormat::Bzip2),
    (".zst", ArchiveFormat::Zstd),
    (".7z", ArchiveFormat::SevenZip),
    (".gz", ArchiveFormat::Gzip),
    (".xz", ArchiveFormat::Xz),
];

impl ArchiveFormat {
    /// Every format this module knows.
    pub const ALL: [Self; 12] = [
        Self::Zip,
        Self::Tar,
        Self::TarGz,
        Self::TarBz2,
        Self::TarXz,
        Self::TarZstd,
        Self::SevenZip,
        Self::Rar,
        Self::Gzip,
        Self::Bzip2,
        Self::Xz,
        Self::Zstd,
    ];

    /// The format `path`'s name says, volume suffixes included
    /// (`name.7z.001`, `name.part2.rar`, `name.r00`).
    #[must_use]
    pub fn detect(path: &Path) -> Option<Self> {
        let name = path.file_name()?;
        let set = volumes::parse(name.as_bytes());
        Self::from_suffix(&set.base_name).map(|(format, _)| format)
    }

    fn from_suffix(name: &[u8]) -> Option<(Self, usize)> {
        SUFFIXES.iter().find_map(|(suffix, format)| {
            let suffix = suffix.as_bytes();
            (name.len() > suffix.len()
                && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix))
            .then_some((*format, suffix.len()))
        })
    }

    /// Whether this is one compressed file rather than an archive of entries.
    #[must_use]
    pub const fn is_stream(self) -> bool {
        matches!(self, Self::Gzip | Self::Bzip2 | Self::Xz | Self::Zstd)
    }

    /// Whether this format can carry entries that need a passphrase which
    /// [`extract`] can use. Encrypted 7z and RAR are reported as
    /// [`ArchiveError::EncryptionUnsupported`].
    #[must_use]
    pub const fn decrypts(self) -> bool {
        matches!(self, Self::Zip)
    }

    fn filter(self) -> Option<native::Filter> {
        match self {
            Self::TarGz | Self::Gzip => Some(native::Filter::Gzip),
            Self::TarBz2 | Self::Bzip2 => Some(native::Filter::Bzip2),
            Self::TarXz | Self::Xz => Some(native::Filter::Xz),
            Self::TarZstd | Self::Zstd => Some(native::Filter::Zstd),
            Self::Zip | Self::Tar | Self::SevenZip | Self::Rar => None,
        }
    }

    fn reader(self) -> native::Format {
        match self {
            Self::Zip => native::Format::Zip,
            Self::Tar | Self::TarGz | Self::TarBz2 | Self::TarXz | Self::TarZstd => {
                native::Format::Tar
            }
            Self::SevenZip => native::Format::SevenZip,
            Self::Rar => native::Format::Rar,
            Self::Gzip | Self::Bzip2 | Self::Xz | Self::Zstd => native::Format::Raw,
        }
    }

    /// The magic bytes a standalone stream must start with: the raw reader
    /// accepts anything, so the signature is checked here.
    fn stream_magic(self) -> Option<&'static [u8]> {
        match self {
            Self::Gzip => Some(&[0x1f, 0x8b]),
            Self::Bzip2 => Some(b"BZh"),
            Self::Xz => Some(b"\xfd7zXZ\0"),
            Self::Zstd => Some(&[0x28, 0xb5, 0x2f, 0xfd]),
            _ => None,
        }
    }
}

/// What this host's libarchive reads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Capabilities {
    formats: Vec<ArchiveFormat>,
    library: Option<String>,
}

impl Capabilities {
    /// Whether [`extract`] can read `format` here.
    #[must_use]
    pub fn reads(&self, format: ArchiveFormat) -> bool {
        self.formats.contains(&format)
    }

    /// Every readable format, in [`ArchiveFormat::ALL`] order.
    #[must_use]
    pub fn formats(&self) -> &[ArchiveFormat] {
        &self.formats
    }

    /// The loaded libarchive's version string, or `None` when it is absent.
    #[must_use]
    pub fn library(&self) -> Option<&str> {
        self.library.as_deref()
    }
}

/// What this host reads, probed once per process.
#[must_use]
pub fn capabilities() -> &'static Capabilities {
    static PROBED: std::sync::OnceLock<Capabilities> = std::sync::OnceLock::new();
    PROBED.get_or_init(|| match native::codecs() {
        Ok(codecs) => Capabilities {
            formats: ArchiveFormat::ALL
                .into_iter()
                .filter(|format| {
                    codecs.has_format(format.reader())
                        && format
                            .filter()
                            .is_none_or(|filter| codecs.has_filter(filter))
                })
                .collect(),
            library: Some(codecs.version()),
        },
        Err(_) => Capabilities::default(),
    })
}

/// Bounds applied while an archive is read, all checked as data arrives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Limits {
    /// Entries read, skipped ones included.
    pub max_entries: u64,
    /// Bytes actually written, summed over every file.
    pub max_output_bytes: u64,
    /// One file's apparent size, holes included.
    pub max_file_size: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_entries: 200_000,
            max_output_bytes: 64 << 30,
            max_file_size: 64 << 30,
        }
    }
}

/// Which [`Limits`] bound an archive crossed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Limit {
    /// [`Limits::max_entries`]
    Entries,
    /// [`Limits::max_output_bytes`]
    OutputBytes,
    /// [`Limits::max_file_size`]
    FileSize,
}

/// What to do with a second entry for a path already written.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Duplicates {
    /// Keep the first entry and count the later one as skipped.
    #[default]
    KeepFirst,
    /// Fail with [`ArchiveError::DuplicateEntry`].
    Fail,
}

/// One extraction to run.
#[derive(Clone, Debug)]
pub struct ExtractRequest {
    /// The archive, or any volume of a split set.
    pub archive: PathBuf,
    /// The folder the archive-named output folder is created in.
    pub destination: PathBuf,
    /// Bounds on what the archive may produce.
    pub limits: Limits,
    /// What a repeated entry does.
    pub duplicates: Duplicates,
    /// The passphrase for encrypted entries, after
    /// [`ArchiveError::PasswordRequired`] asked for one.
    pub passphrase: Option<SecretText>,
}

impl ExtractRequest {
    /// Extract `archive` into `destination` with the default limits.
    #[must_use]
    pub fn new(archive: impl Into<PathBuf>, destination: impl Into<PathBuf>) -> Self {
        Self {
            archive: archive.into(),
            destination: destination.into(),
            limits: Limits::default(),
            duplicates: Duplicates::default(),
            passphrase: None,
        }
    }

    /// The same request with `passphrase`.
    #[must_use]
    pub fn with_passphrase(mut self, passphrase: SecretText) -> Self {
        self.passphrase = Some(passphrase);
        self
    }
}

/// Why an extraction did not publish.
#[derive(Clone, Debug)]
pub enum ArchiveError {
    /// Cancellation was requested; nothing was published.
    Cancelled,
    /// The name is not one of the [`ArchiveFormat`]s.
    Unsupported,
    /// This host cannot read the format: libarchive or its codec is absent.
    CodecUnavailable(ArchiveFormat),
    /// The archive uses a compression method or feature libarchive lacks.
    UnsupportedCodec(String),
    /// An entry is encrypted; retry with [`ExtractRequest::passphrase`].
    PasswordRequired,
    /// The passphrase given does not decrypt the entries.
    IncorrectPassword,
    /// The archive is encrypted in a way libarchive cannot decrypt
    /// (encrypted 7z or RAR).
    EncryptionUnsupported,
    /// A split archive ended before its last part; this one is absent.
    MissingVolume(PathBuf),
    /// The archive crossed one of the [`Limits`].
    LimitExceeded(Limit),
    /// [`Duplicates::Fail`] met a second entry for this path.
    DuplicateEntry(PathBuf),
    /// The archive is damaged or not what its name says.
    Corrupt(String),
    /// The filesystem refused.
    Io(Arc<io::Error>),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Extraction was cancelled"),
            Self::Unsupported => f.write_str("This is not an archive format that can be opened"),
            Self::CodecUnavailable(format) => write!(
                f,
                "This system cannot read {format:?} archives (libarchive or its codec is missing)"
            ),
            Self::UnsupportedCodec(detail) => {
                write!(f, "The archive uses an unsupported method ({detail})")
            }
            Self::PasswordRequired => f.write_str("The archive needs a password"),
            Self::IncorrectPassword => f.write_str("The password is incorrect"),
            Self::EncryptionUnsupported => {
                f.write_str("Encrypted archives of this type cannot be extracted")
            }
            Self::MissingVolume(path) => write!(
                f,
                "A part of the archive is missing: {}",
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
            ),
            Self::LimitExceeded(Limit::Entries) => f.write_str("The archive has too many entries"),
            Self::LimitExceeded(Limit::OutputBytes) => {
                f.write_str("The archive expands to more data than allowed")
            }
            Self::LimitExceeded(Limit::FileSize) => {
                f.write_str("A file in the archive is larger than allowed")
            }
            Self::DuplicateEntry(path) => write!(
                f,
                "The archive contains {} more than once",
                path.to_string_lossy()
            ),
            Self::Corrupt(detail) => write!(f, "The archive is damaged ({detail})"),
            Self::Io(error) => {
                use enzui_core::io_class::IoFailureCategory;
                f.write_str(match IoFailureCategory::classify(error) {
                    IoFailureCategory::Permission => "Permission was denied",
                    IoFailureCategory::NotFound | IoFailureCategory::NotADirectory => {
                        "A file or folder was not found"
                    }
                    IoFailureCategory::StorageExhausted => "There is no space left",
                    IoFailureCategory::Unknown => "A filesystem error occurred",
                })
            }
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<io::Error> for ArchiveError {
    fn from(error: io::Error) -> Self {
        Self::Io(Arc::new(error))
    }
}

/// Entries an extraction left out, by reason.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Skipped {
    /// Absolute, `..`-climbing or empty names.
    pub unsafe_paths: u64,
    /// Symlinks and hardlinks.
    pub links: u64,
    /// Devices, FIFOs and sockets.
    pub special: u64,
    /// Later entries for a path already written ([`Duplicates::KeepFirst`]).
    pub duplicates: u64,
}

impl Skipped {
    /// Every skipped entry.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.unsafe_paths + self.links + self.special + self.duplicates
    }
}

/// What a completed extraction published.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractReport {
    /// The published folder.
    pub output: PathBuf,
    /// Regular files written.
    pub files: u64,
    /// Folders created.
    pub folders: u64,
    /// Bytes written, holes excluded.
    pub bytes: u64,
    /// Entries left out.
    pub skipped: Skipped,
}

/// Live counters of one extraction, readable from any thread.
#[derive(Clone, Debug, Default)]
pub struct ExtractProgress(Arc<ProgressCounters>);

#[derive(Debug, Default)]
struct ProgressCounters {
    entries: AtomicU64,
    written: AtomicU64,
    consumed: AtomicU64,
    input: AtomicU64,
}

impl ExtractProgress {
    /// Entries read so far.
    #[must_use]
    pub fn entries(&self) -> u64 {
        self.0.entries.load(Ordering::Relaxed)
    }

    /// Bytes written so far.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.0.written.load(Ordering::Relaxed)
    }

    /// Archive bytes consumed out of the input's total, for an activity row.
    #[must_use]
    pub fn activity(&self) -> enzui_core::operation_activity::ActivityProgress {
        let total = self.0.input.load(Ordering::Relaxed);
        enzui_core::operation_activity::ActivityProgress::Bytes {
            done: self.0.consumed.load(Ordering::Relaxed).min(total),
            total: (total > 0).then_some(total),
        }
    }
}

/// Extracts the archive into a new folder named after it in the request's
/// destination, never replacing anything there.
///
/// # Errors
///
/// See [`ArchiveError`]; on any error nothing is published.
pub fn extract(
    request: &ExtractRequest,
    progress: &ExtractProgress,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExtractReport, ArchiveError> {
    let name = request
        .archive
        .file_name()
        .ok_or(ArchiveError::Unsupported)?;
    let mut set = volumes::parse(name.as_bytes());
    let (format, suffix) =
        ArchiveFormat::from_suffix(&set.base_name).ok_or(ArchiveError::Unsupported)?;
    if !capabilities().reads(format) {
        return Err(ArchiveError::CodecUnavailable(format));
    }
    let dir = request.archive.parent().unwrap_or_else(|| Path::new("."));
    let volumes = set.resolve(dir)?;
    for volume in &volumes {
        if !fs::metadata(volume)?.is_file() {
            return Err(ArchiveError::Unsupported);
        }
    }
    let sizes: Vec<u64> = volumes
        .iter()
        .map(|volume| fs::metadata(volume).map(|metadata| metadata.len()))
        .collect::<io::Result<_>>()?;
    progress
        .0
        .input
        .store(sizes.iter().sum(), Ordering::Relaxed);
    // Byte splits are equal-sized but for the last: a full-sized last part
    // means more were expected.
    let full_tail = set.is_joined() && sizes.first() == sizes.last();
    if let Some(magic) = format.stream_magic() {
        let mut head = vec![0; magic.len()];
        io::Read::read_exact(&mut File::open(&volumes[0])?, &mut head)
            .map_err(|_| ArchiveError::Corrupt("the file is too short".into()))?;
        if head != magic {
            return Err(ArchiveError::Corrupt(
                "invalid compression signature".into(),
            ));
        }
    }

    let stem = &set.base_name[..set.base_name.len() - suffix];
    let base = OsString::from_vec(stem.to_vec());
    let staging = Staging::create(&request.destination, &base)?;
    let mut run = Run {
        request,
        format,
        stream_name: format.is_stream().then(|| PathBuf::from(&base)),
        staging: staging.root.clone(),
        full_tail,
        progress,
        cancelled,
        report: ExtractReport {
            output: PathBuf::new(),
            files: 0,
            folders: 0,
            bytes: 0,
            skipped: Skipped::default(),
        },
    };
    let mut reader = native::Reader::open(
        &volumes,
        set.is_joined(),
        format.reader(),
        format.filter(),
        request.passphrase.as_ref(),
    )
    .map_err(|error| run.classify(error.into(), None, &set, dir))?;
    run.entries(&mut reader)
        .map_err(|error| run.classify(error, Some(&reader), &set, dir))?;
    drop(reader);
    if cancelled() {
        return Err(ArchiveError::Cancelled);
    }
    let mut report = run.report;
    report.output = staging.publish(&base)?;
    Ok(report)
}

/// One extraction's state while entries are read.
struct Run<'a> {
    request: &'a ExtractRequest,
    format: ArchiveFormat,
    /// A standalone stream's single entry takes the archive's name.
    stream_name: Option<PathBuf>,
    /// The staging folder, held by descriptor: a rename of it or of its
    /// ancestors cannot redirect the writes.
    staging: RetainedDir,
    /// A byte-split set whose last present part is full-sized.
    full_tail: bool,
    progress: &'a ExtractProgress,
    cancelled: &'a dyn Fn() -> bool,
    report: ExtractReport,
}

/// A read failure before it is classified for the user.
enum Failure {
    Archive(ArchiveError),
    Native(native::NativeError),
}

impl From<ArchiveError> for Failure {
    fn from(error: ArchiveError) -> Self {
        Self::Archive(error)
    }
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self::Archive(error.into())
    }
}

impl From<native::NativeError> for Failure {
    fn from(error: native::NativeError) -> Self {
        Self::Native(error)
    }
}

impl Run<'_> {
    fn entries(&mut self, reader: &mut native::Reader) -> Result<(), Failure> {
        let mut entries = 0u64;
        loop {
            if (self.cancelled)() {
                return Err(ArchiveError::Cancelled.into());
            }
            let Some(entry) = reader.next()? else {
                return Ok(());
            };
            entries += 1;
            self.progress.0.entries.store(entries, Ordering::Relaxed);
            self.progress
                .0
                .consumed
                .store(reader.consumed(), Ordering::Relaxed);
            if entries > self.request.limits.max_entries {
                return Err(ArchiveError::LimitExceeded(Limit::Entries).into());
            }
            let name = self
                .stream_name
                .clone()
                .unwrap_or_else(|| entry.path.clone());
            let Some(rel) = safe_relative(&name) else {
                self.report.skipped.unsafe_paths += 1;
                continue;
            };
            if entry.link {
                self.report.skipped.links += 1;
                continue;
            }
            match entry.kind {
                native::Kind::Directory => {
                    if self
                        .make_parents(&rel, true)?
                        .is_some_and(|(_, created)| created)
                    {
                        self.report.folders += 1;
                    }
                }
                native::Kind::File => {
                    if entry.encrypted {
                        if !self.format.decrypts() {
                            return Err(ArchiveError::EncryptionUnsupported.into());
                        }
                        if self.request.passphrase.is_none() {
                            return Err(ArchiveError::PasswordRequired.into());
                        }
                    }
                    self.file(reader, &rel, &entry)?;
                }
                native::Kind::Special => self.report.skipped.special += 1,
            }
        }
    }

    /// Creates `rel`'s folders (and `rel` itself for a folder entry) under
    /// staging, descriptor by descriptor. The folder that receives `rel`'s
    /// last component and whether anything was created, or `None` when the
    /// path is already taken by a file, which counts as a duplicate.
    fn make_parents(
        &mut self,
        rel: &Path,
        including_self: bool,
    ) -> Result<Option<(RetainedDir, bool)>, Failure> {
        let mut directory = self.staging.clone();
        let components: Vec<_> = rel.components().collect();
        let last = components.len() - usize::from(!including_self);
        let mut created = false;
        for component in &components[..last] {
            let name = component.as_os_str();
            directory = match directory.create_subroot(name) {
                Ok(child) => {
                    created = true;
                    child
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    match directory.open_subroot(name) {
                        Ok(child) => child,
                        Err(_) => {
                            self.duplicate(rel)?;
                            return Ok(None);
                        }
                    }
                }
                Err(error) => return Err(error.into()),
            };
        }
        Ok(Some((directory, created)))
    }

    fn duplicate(&mut self, rel: &Path) -> Result<(), ArchiveError> {
        match self.request.duplicates {
            Duplicates::KeepFirst => {
                self.report.skipped.duplicates += 1;
                Ok(())
            }
            Duplicates::Fail => Err(ArchiveError::DuplicateEntry(rel.to_path_buf())),
        }
    }

    fn file(
        &mut self,
        reader: &mut native::Reader,
        rel: &Path,
        entry: &native::Entry,
    ) -> Result<(), Failure> {
        let Some((directory, _)) = self.make_parents(rel, false)? else {
            return Ok(());
        };
        let limits = self.request.limits;
        if entry.size.is_some_and(|size| size > limits.max_file_size) {
            return Err(ArchiveError::LimitExceeded(Limit::FileSize).into());
        }
        let Some(name) = rel.file_name() else {
            return Ok(());
        };
        let mut file = match directory.create_file_new(name) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                self.duplicate(rel)?;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let mut end = 0u64;
        while let Some((offset, data)) = reader.block()? {
            if (self.cancelled)() {
                return Err(ArchiveError::Cancelled.into());
            }
            if offset < end {
                return Err(ArchiveError::Corrupt("overlapping file data".into()).into());
            }
            let block_end = offset.saturating_add(data.len() as u64);
            if block_end > limits.max_file_size {
                return Err(ArchiveError::LimitExceeded(Limit::FileSize).into());
            }
            self.report.bytes = self.report.bytes.saturating_add(data.len() as u64);
            if self.report.bytes > limits.max_output_bytes {
                return Err(ArchiveError::LimitExceeded(Limit::OutputBytes).into());
            }
            if offset > end {
                // A hole: seek over it so the output stays sparse.
                file.seek(io::SeekFrom::Start(offset))?;
            }
            file.write_all(data)?;
            end = block_end;
            self.progress
                .0
                .written
                .store(self.report.bytes, Ordering::Relaxed);
            self.progress
                .0
                .consumed
                .store(reader.consumed(), Ordering::Relaxed);
        }
        if let Some(size) = entry.size
            && size > end
        {
            // A trailing hole.
            file.set_len(size)?;
        }
        if entry.mode & 0o111 != 0 {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o755))?;
        }
        self.report.files += 1;
        Ok(())
    }

    /// Names a native failure for the user: encryption, an absent volume of
    /// a split set, an unsupported method, or damage.
    fn classify(
        &self,
        failure: Failure,
        reader: Option<&native::Reader>,
        set: &volumes::VolumeSet,
        dir: &Path,
    ) -> ArchiveError {
        let error = match failure {
            Failure::Archive(error) => return error,
            Failure::Native(error) => error,
        };
        let lower = error.message.to_ascii_lowercase();
        if lower.contains("passphrase") || lower.contains("encrypt") || lower.contains("password") {
            return if !self.format.decrypts() {
                ArchiveError::EncryptionUnsupported
            } else if self.request.passphrase.is_some() {
                ArchiveError::IncorrectPassword
            } else {
                ArchiveError::PasswordRequired
            };
        }
        // A split set read to the end of its last present part ran out of
        // parts, not of good data.
        if let Some(next) = set.next_missing(dir)
            && (set.has_part_after(dir, &next)
                || self.full_tail
                || reader.is_none_or(|reader| reader.consumed() >= self.progress.input()))
        {
            return ArchiveError::MissingVolume(next);
        }
        if lower.contains("unsupported") || lower.contains("not supported") {
            return ArchiveError::UnsupportedCodec(error.message);
        }
        ArchiveError::Corrupt(error.message)
    }
}

impl ExtractProgress {
    fn input(&self) -> u64 {
        self.0.input.load(Ordering::Relaxed)
    }
}

/// The archive path as normal components only, or `None` for a path that is
/// absolute, climbs with `..`, or names nothing.
fn safe_relative(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// A private hidden folder beside the destination, removed unless published.
///
/// Both folders are held by descriptor from creation to cleanup: publication
/// renames the staging folder only while its name still denotes it, and
/// cleanup removes only what was created there, so a rename or replacement
/// of either by another process never redirects them.
struct Staging {
    parent: RetainedDir,
    root: RetainedDir,
    name: OsString,
    published: bool,
}

impl Staging {
    fn create(destination: &Path, base: &OsStr) -> io::Result<Self> {
        let parent = RetainedDir::open(destination)?;
        let mut short = base.as_bytes();
        if short.len() > 96 {
            short = &short[..96];
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        for attempt in 0..64u32 {
            let mut name = b".".to_vec();
            name.extend_from_slice(short);
            name.extend_from_slice(
                format!(".extracting-{}-{nanos}-{attempt}", std::process::id()).as_bytes(),
            );
            let name = OsString::from_vec(name);
            match parent.create_subroot(&name) {
                Ok(root) => {
                    return Ok(Self {
                        parent,
                        root,
                        name,
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free staging name",
        ))
    }

    /// Renames staging to the first free `base`, `base (2)`, ... without
    /// ever replacing an entry another process created meanwhile.
    fn publish(mut self, base: &OsStr) -> Result<PathBuf, ArchiveError> {
        self.root.set_mode(0o755)?;
        let mut entry = self.parent.retain_entry(&self.name)?;
        if entry.identity() != self.root.identity() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the staging folder was replaced",
            )
            .into());
        }
        for n in 1..=1000u32 {
            let mut name = base.to_os_string();
            if n > 1 {
                name.push(format!(" ({n})"));
            }
            match entry.rename_noreplace(&self.parent, &name) {
                Ok(_) => {
                    self.published = true;
                    return Ok(self.parent.path().join(name));
                }
                Err(failure) => match *failure.custody {
                    MoveCustody::Source(source)
                        if failure.error.kind() == io::ErrorKind::AlreadyExists =>
                    {
                        entry = source;
                    }
                    // Renamed but not confirmed: the folder is no longer
                    // staging's to remove.
                    MoveCustody::Source(_) => return Err(failure.error.into()),
                    _ => {
                        self.published = true;
                        return Err(failure.error.into());
                    }
                },
            }
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free output name").into())
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            let _ = self
                .parent
                .remove_tree_if_identity(&self.name, self.root.identity());
        }
    }
}

#[cfg(test)]
mod tests;
