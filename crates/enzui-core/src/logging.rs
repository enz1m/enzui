//! Framework diagnostics through the `log` facade, and the default logger.
//!
//! enzUI reports what it notices (an icon name that does not exist, two
//! controls sharing an accessibility id, a layout floor that cannot be met)
//! with the `log` macros under `enzui::<area>` targets, never by printing.
//! An application decides where they go:
//!
//! - **Nothing to do.** `enzui_widgets::app_setup::run` calls
//!   [`init_default`], which installs a small logger unless the application
//!   installed one first. It writes to stderr, or to the systemd journal
//!   with priorities and source locations when stderr already goes there
//!   (a systemd service or a desktop session that logs apps to the journal). It reads `RUST_LOG` (`warn`,
//!   `enzui=debug`, `info,enzui::a11y=off`) and defaults to
//!   [`DEFAULT_FILTER`]: enzUI's warnings and errors, other crates' errors,
//!   and the opt-in traces under `enzui::trace`, which only speak when their
//!   own environment variable turns them on.
//! - **The journal on purpose.** [`init_journal`] sends records to the
//!   journal whatever stderr is; `journalctl --user -t <identifier>` reads
//!   them, `-p warning` filters by priority.
//! - **Your own logger.** Install `env_logger`, `tracing-subscriber` (with
//!   its `log` bridge) or any other `log` implementation before calling
//!   `run`; the default steps aside.

use log::{Level, LevelFilter, Log, Metadata, Record};

/// The filter the default logger uses when `RUST_LOG` is unset.
/// enzUI's warnings, other crates' errors only (a font with no family name
/// or a driver quirk is not the application's concern), and the opt-in traces.
pub const DEFAULT_FILTER: &str = "error,enzui=warn,enzui::trace=info";

/// One `RUST_LOG` directive: an optional target prefix and its level.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Directive {
    target: Option<String>,
    level: LevelFilter,
}

/// A `RUST_LOG`-style filter: comma-separated `level` or `target=level`
/// directives, the longest matching target prefix deciding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    directives: Vec<Directive>,
}

impl Filter {
    /// Parse `spec`; unreadable directives are skipped, and with none the
    /// filter passes warnings and errors.
    #[must_use]
    pub fn parse(spec: &str) -> Self {
        let mut directives = Vec::new();
        for part in spec
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let (target, level) = match part.split_once('=') {
                Some((target, level)) => (Some(target.trim().to_owned()), level.trim()),
                None => match part.parse::<LevelFilter>() {
                    Ok(level) => {
                        directives.push(Directive {
                            target: None,
                            level,
                        });
                        continue;
                    }
                    // A bare target turns everything on for it.
                    Err(_) => (Some(part.to_owned()), "trace"),
                },
            };
            if let Ok(level) = level.parse::<LevelFilter>() {
                directives.push(Directive { target, level });
            }
        }
        if directives.is_empty() {
            directives.push(Directive {
                target: None,
                level: LevelFilter::Warn,
            });
        }
        Self { directives }
    }

    /// The level allowed for `target`.
    #[must_use]
    pub fn level_for(&self, target: &str) -> LevelFilter {
        let matches = |prefix: &str| {
            target == prefix
                || target
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with("::"))
        };
        self.directives
            .iter()
            .filter(|directive| directive.target.as_deref().is_none_or(matches))
            .max_by_key(|directive| directive.target.as_ref().map_or(0, String::len))
            .map_or(LevelFilter::Off, |directive| directive.level)
    }

    /// Whether a record at `level` for `target` passes.
    #[must_use]
    pub fn enabled(&self, target: &str, level: Level) -> bool {
        level <= self.level_for(target)
    }

    /// The most verbose level any directive allows.
    #[must_use]
    pub fn max_level(&self) -> LevelFilter {
        self.directives
            .iter()
            .map(|directive| directive.level)
            .max()
            .unwrap_or(LevelFilter::Off)
    }
}

/// Writes `LEVEL target: message` lines to stderr.
#[derive(Debug)]
struct StderrLogger {
    filter: Filter,
}

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.filter.enabled(metadata.target(), metadata.level())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        use std::io::Write as _;
        // A closed stderr has nowhere to report its own failure.
        let _ = writeln!(
            std::io::stderr().lock(),
            "{:<5} {}: {}",
            record.level(),
            record.target(),
            record.args()
        );
    }

    fn flush(&self) {
        use std::io::Write as _;
        let _ = std::io::stderr().flush();
    }
}

/// The journal's native socket.
pub const JOURNAL_SOCKET: &str = "/run/systemd/journal/socket";

/// Sends records to the systemd journal over its native protocol: the
/// message with `PRIORITY` (error 3, warning 4, info 6, debug and trace 7),
/// `SYSLOG_IDENTIFIER`, `CODE_FILE`, `CODE_LINE`, `CODE_MODULE` and the
/// record's target as `LOG_TARGET`.
#[derive(Debug)]
pub struct JournalLogger {
    filter: Filter,
    identifier: String,
    socket: std::os::unix::net::UnixDatagram,
    path: std::path::PathBuf,
}

impl JournalLogger {
    /// A logger for the journal at [`JOURNAL_SOCKET`], naming the
    /// application `identifier`.
    ///
    /// # Errors
    /// The socket could not be created.
    pub fn new(identifier: impl Into<String>, filter: Filter) -> std::io::Result<Self> {
        Self::with_socket(identifier, filter, JOURNAL_SOCKET)
    }

    /// A logger for the journal socket at `path`.
    ///
    /// # Errors
    /// The socket could not be created.
    pub fn with_socket(
        identifier: impl Into<String>,
        filter: Filter,
        path: impl Into<std::path::PathBuf>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            filter,
            identifier: identifier.into(),
            socket: std::os::unix::net::UnixDatagram::unbound()?,
            path: path.into(),
        })
    }

    /// The journal entry for `record`, in the native protocol.
    fn entry(&self, record: &Record<'_>) -> Vec<u8> {
        let priority = match record.level() {
            Level::Error => "3",
            Level::Warn => "4",
            Level::Info => "6",
            Level::Debug | Level::Trace => "7",
        };
        let mut entry = Vec::new();
        field(&mut entry, "MESSAGE", &record.args().to_string());
        field(&mut entry, "PRIORITY", priority);
        field(&mut entry, "SYSLOG_IDENTIFIER", &self.identifier);
        field(&mut entry, "LOG_TARGET", record.target());
        if let Some(file) = record.file() {
            field(&mut entry, "CODE_FILE", file);
        }
        if let Some(line) = record.line() {
            field(&mut entry, "CODE_LINE", &line.to_string());
        }
        if let Some(module) = record.module_path() {
            field(&mut entry, "CODE_MODULE", module);
        }
        entry
    }
}

/// Appends one field: `KEY=value`, or the length-prefixed form for a value
/// with a line break.
fn field(entry: &mut Vec<u8>, key: &str, value: &str) {
    entry.extend_from_slice(key.as_bytes());
    if value.contains('\n') {
        entry.push(b'\n');
        entry.extend_from_slice(&(value.len() as u64).to_le_bytes());
    } else {
        entry.push(b'=');
    }
    entry.extend_from_slice(value.as_bytes());
    entry.push(b'\n');
}

impl Log for JournalLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        self.filter.enabled(metadata.target(), metadata.level())
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            // A journal that is gone has nowhere to report its own failure.
            let _ = self.socket.send_to(&self.entry(record), &self.path);
        }
    }

    fn flush(&self) {}
}

/// Whether stderr is the journal: systemd sets `JOURNAL_STREAM` to the
/// stream's `device:inode` when it connects stderr there.
fn stderr_is_journal() -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(stream) = std::env::var("JOURNAL_STREAM") else {
        return false;
    };
    let Ok(stderr) = std::fs::metadata("/proc/self/fd/2") else {
        return false;
    };
    stream == format!("{}:{}", stderr.dev(), stderr.ino())
}

/// The application's name for the journal: the executable's file name.
fn identifier() -> String {
    std::env::args_os()
        .next()
        .and_then(|program| {
            std::path::Path::new(&program)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "enzui".to_owned())
}

fn filter_from_env() -> Filter {
    Filter::parse(&std::env::var("RUST_LOG").unwrap_or_else(|_| DEFAULT_FILTER.to_owned()))
}

fn install(logger: Box<dyn Log>, max: LevelFilter) -> bool {
    if log::set_boxed_logger(logger).is_err() {
        return false;
    }
    log::set_max_level(max);
    true
}

/// Install the default logger, filtered by `RUST_LOG` or [`DEFAULT_FILTER`]:
/// the journal when stderr goes there, stderr otherwise. Returns `false`,
/// changing nothing, when the application already installed a logger.
pub fn init_default() -> bool {
    let filter = filter_from_env();
    let max = filter.max_level();
    if stderr_is_journal()
        && let Ok(journal) = JournalLogger::new(identifier(), filter.clone())
    {
        return install(Box::new(journal), max);
    }
    install(Box::new(StderrLogger { filter }), max)
}

/// Install a [`JournalLogger`] at [`JOURNAL_SOCKET`], filtered by `RUST_LOG`
/// or [`DEFAULT_FILTER`]. Returns `false`, changing nothing, when a logger is
/// already installed or the socket could not be created.
pub fn init_journal(identifier: impl Into<String>) -> bool {
    let filter = filter_from_env();
    let max = filter.max_level();
    match JournalLogger::new(identifier, filter) {
        Ok(journal) => install(Box::new(journal), max),
        Err(_) => false,
    }
}
