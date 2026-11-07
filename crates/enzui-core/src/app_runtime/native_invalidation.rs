//! Opt-in control and reporting for native multi-window invalidation checks.
//!
//! The controller polls a bounded command file and emits line-delimited JSON
//! tied to the running executable and a caller-supplied source identity. It is
//! available only with `native-invalidation`; normal application builds carry
//! no instrumentation or file polling.

use iced_core::window::Id;
use iced_winit::invalidation::{self, instrumentation};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::{self, Read, Write as _};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Environment variable naming the command file polled by an acceptance build.
pub const CONTROL_PATH_ENV: &str = "ENZUI_INVALIDATION_CONTROL";
/// Environment variable selecting the total native window count.
pub const WINDOWS_ENV: &str = "ENZUI_INVALIDATION_WINDOWS";

const MAX_COMMAND_BYTES: u64 = 4096;

/// One sequenced command read from the control file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// Monotonically increasing command sequence.
    pub sequence: u64,
    /// Application-defined command name.
    pub verb: String,
    /// Remaining command text, if present.
    pub argument: Option<String>,
}

/// One labelled native window included in a report.
#[derive(Debug, Clone, Copy)]
pub struct Window<'a> {
    /// Stable label used by the external capture runner.
    pub label: &'a str,
    /// Runtime window identity whose counters are read.
    pub id: Id,
}

/// Command-file and report state for one instrumented process.
pub struct Controller {
    app: &'static str,
    source_id: &'static str,
    binary_sha256: String,
    control_path: PathBuf,
    last_sequence: u64,
    stage_wall: Instant,
    stage_cpu_ticks: u64,
}

impl Controller {
    /// Creates a controller when [`CONTROL_PATH_ENV`] is set.
    ///
    /// The window count accepts only 2, 4, or 8. The source identity is
    /// embedded by the application build and is emitted with every record.
    ///
    /// # Errors
    ///
    /// A message when the window count is not 2, 4 or 8, or the executable cannot be hashed.
    pub fn from_env(app: &'static str, source_id: &'static str) -> Result<Option<Self>, String> {
        let Some(control_path) = std::env::var_os(CONTROL_PATH_ENV) else {
            return Ok(None);
        };
        let windows = window_count_from(std::env::var(WINDOWS_ENV).ok().as_deref())?;
        let binary_sha256 = executable_sha256().map_err(|error| error.to_string())?;
        instrumentation::enable();
        instrumentation::reset();

        let controller = Self {
            app,
            source_id,
            binary_sha256,
            control_path: PathBuf::from(control_path),
            last_sequence: 0,
            stage_wall: Instant::now(),
            stage_cpu_ticks: process_cpu_ticks().unwrap_or(0),
        };
        controller.emit_event("started", &format!("windows={windows}"));
        Ok(Some(controller))
    }

    /// Returns the configured total window count.
    pub fn window_count(&self) -> usize {
        window_count_from(std::env::var(WINDOWS_ENV).ok().as_deref()).unwrap_or(2)
    }

    /// Reads a new command, if the file contains a sequence not seen before.
    ///
    /// # Errors
    ///
    /// A message when the control file cannot be inspected or read.
    pub fn poll(&mut self) -> Result<Option<Command>, String> {
        let metadata = match std::fs::metadata(&self.control_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("cannot stat control file: {error}")),
        };
        if metadata.len() > MAX_COMMAND_BYTES {
            return Err(format!(
                "control file is {} bytes; limit is {MAX_COMMAND_BYTES}",
                metadata.len()
            ));
        }
        let raw = std::fs::read_to_string(&self.control_path)
            .map_err(|error| format!("cannot read control file: {error}"))?;
        let Some(command) = parse_command(&raw)? else {
            return Ok(None);
        };
        if command.sequence <= self.last_sequence {
            return Ok(None);
        }
        self.last_sequence = command.sequence;
        Ok(Some(command))
    }

    /// Clears runtime counters and starts a new CPU/wall-clock interval.
    pub fn reset_stage(&mut self, stage: &str) {
        self.stage_wall = Instant::now();
        self.stage_cpu_ticks = process_cpu_ticks().unwrap_or(0);
        // Reset after the command's own runtime turn; otherwise that control
        // update is counted as application work in the measured interval.
        let _ = std::thread::Builder::new()
            .name("invalidation-counter-reset".to_owned())
            .spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(150));
                instrumentation::reset();
            });
        self.emit_event("reset", stage);
    }

    /// Emits a lifecycle event on the line-delimited JSON channel.
    #[expect(
        clippy::print_stdout,
        reason = "the channel is stdout, read by the probe"
    )]
    pub fn emit_event(&self, event: &str, detail: &str) {
        println!(
            "ENZUI_INVALIDATION {{\"kind\":\"event\",\"app\":\"{}\",\"event\":\"{}\",\"detail\":\"{}\",\"source\":\"{}\",\"binary_sha256\":\"{}\",\"pid\":{},\"verify\":{}}}",
            json(self.app),
            json(event),
            json(detail),
            json(self.source_id),
            self.binary_sha256,
            std::process::id(),
            invalidation::is_verifying(),
        );
        let _ = io::stdout().flush();
    }

    /// Emits exact per-window counters and a hash of caller-selected state.
    #[expect(
        clippy::print_stdout,
        reason = "the channel is stdout, read by the probe"
    )]
    pub fn report(&self, stage: &str, state: &str, windows: &[Window<'_>]) {
        let now_cpu = process_cpu_ticks().unwrap_or(self.stage_cpu_ticks);
        let elapsed = self.stage_wall.elapsed();
        let state_sha256 = bytes_sha256(state.as_bytes());
        let mut encoded = String::new();
        for (index, window) in windows.iter().enumerate() {
            if index != 0 {
                encoded.push(',');
            }
            let counts = instrumentation::counts(window.id);
            let _ = write!(
                encoded,
                "{{\"label\":\"{}\",\"view\":{},\"build\":{},\"layout\":{},\"draw\":{},\"requested_frame\":{}}}",
                json(window.label),
                counts.view,
                counts.build,
                counts.layout,
                counts.draw,
                counts.redraw_requests,
            );
        }
        println!(
            "ENZUI_INVALIDATION {{\"kind\":\"report\",\"app\":\"{}\",\"stage\":\"{}\",\"source\":\"{}\",\"binary_sha256\":\"{}\",\"pid\":{},\"verify\":{},\"elapsed_ns\":{},\"cpu_ticks\":{},\"state\":\"{}\",\"state_sha256\":\"{}\",\"windows\":[{}]}}",
            json(self.app),
            json(stage),
            json(self.source_id),
            self.binary_sha256,
            std::process::id(),
            invalidation::is_verifying(),
            elapsed.as_nanos(),
            now_cpu.saturating_sub(self.stage_cpu_ticks),
            json(state),
            state_sha256,
            encoded,
        );
        let _ = io::stdout().flush();
    }
}

fn window_count_from(value: Option<&str>) -> Result<usize, String> {
    let windows = value
        .unwrap_or("2")
        .parse::<usize>()
        .map_err(|_| "window count must be 2, 4, or 8".to_owned())?;
    match windows {
        2 | 4 | 8 => Ok(windows),
        _ => Err("window count must be 2, 4, or 8".to_owned()),
    }
}

fn parse_command(raw: &str) -> Result<Option<Command>, String> {
    let line = raw.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let mut fields = line.splitn(3, char::is_whitespace);
    let sequence = fields
        .next()
        .and_then(|field| field.parse::<u64>().ok())
        .ok_or_else(|| "command must start with a numeric sequence".to_owned())?;
    let verb = fields
        .next()
        .filter(|field| !field.is_empty())
        .ok_or_else(|| "command is missing its verb".to_owned())?
        .to_owned();
    let argument = fields
        .next()
        .map(str::trim)
        .filter(|field| !field.is_empty())
        .map(str::to_owned);
    Ok(Some(Command {
        sequence,
        verb,
        argument,
    }))
}

fn executable_sha256() -> io::Result<String> {
    let mut input = std::fs::File::open(std::env::current_exe()?)?;
    let mut digest = Sha256::new();
    let mut chunk = vec![0_u8; 64 * 1024];
    loop {
        let read = input.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        digest.update(&chunk[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn bytes_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn process_cpu_ticks() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let mut fields = fields.split_whitespace();
    let user = fields.nth(11)?.parse::<u64>().ok()?;
    let system = fields.next()?.parse::<u64>().ok()?;
    user.checked_add(system)
}

fn json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                let _ = write!(escaped, "\\u{:04x}", u32::from(character));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// Returns whether every counter is zero.
pub fn is_idle(id: Id) -> bool {
    instrumentation::counts(id).is_idle()
}

/// Returns `true` once for each changed command-file payload.
pub fn control_changed() -> bool {
    static LAST: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    let Some(path) = control_path() else {
        return false;
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.len() > MAX_COMMAND_BYTES {
        return false;
    }
    let Ok(current) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(mut last) = LAST.lock() else {
        return false;
    };
    if last.as_deref() == Some(&current) {
        return false;
    }
    *last = Some(current);
    true
}

/// Returns the command-file path, when native acceptance is configured.
pub fn control_path() -> Option<&'static Path> {
    static PATH: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| std::env::var_os(CONTROL_PATH_ENV).map(PathBuf::from))
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_count_accepts_only_native_matrix() {
        assert_eq!(window_count_from(None), Ok(2));
        for count in ["2", "4", "8"] {
            assert_eq!(
                window_count_from(Some(count)),
                Ok(count.parse::<usize>().unwrap())
            );
        }
        for count in ["0", "1", "3", "16", "many"] {
            assert!(window_count_from(Some(count)).is_err(), "accepted {count}");
        }
    }
}
