//! Launch a native helper with selected descriptors, inherited by it alone.
//!
//! A helper with a descriptor ABI (a sealed password on fd 3, a private output
//! memfd on fd 4) needs those descriptors inherited across `exec`. Clearing
//! close-on-exec in the parent would expose them to every process the app
//! launches meanwhile, and `Command::pre_exec` needs unsafe code and can
//! overwrite the descriptor std itself uses to report a failed `exec`.
//! [`NativeCommand`] launches through `posix_spawn` instead.
//!
//! Each source is cloned close-on-exec at a number above every target, so the
//! parent never holds an inheritable copy and no clone collides with a target.
//! The spawn's file actions `dup2` each clone onto its target, which is
//! inheritable in that child alone; the clones close at `exec`. The caller's
//! descriptors keep their flags and seals (a clone shares the open file
//! description). Every clone is closed when [`NativeCommand::spawn`] returns,
//! whether the launch succeeded or failed. Standard input, output and error
//! are configured explicitly ([`Stdio`]); targets 0–2 are refused. Dropping a
//! [`NativeChild`] that was not waited for kills and reaps it. Other platforms
//! refuse with [`SpawnError::Unsupported`].

use std::ffi::{CString, OsStr, OsString};
use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt as _;

/// The highest descriptor target accepted.
pub const MAX_TARGET: i32 = 255;

/// Why a helper could not be launched.
#[derive(Debug)]
pub enum SpawnError {
    /// A target below 3 (standard I/O) or above [`MAX_TARGET`].
    InvalidTarget(i32),
    /// Two descriptors name the same target.
    DuplicateTarget(i32),
    /// A program, argument or environment entry contains a NUL byte.
    Nul,
    /// Cloning a descriptor, creating a pipe or the launch itself failed.
    Io(std::io::Error),
    /// This platform has no supported launch.
    Unsupported,
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTarget(target) => {
                write!(f, "descriptor target {target} is outside 3..={MAX_TARGET}")
            }
            Self::DuplicateTarget(target) => {
                write!(f, "descriptor target {target} is mapped twice")
            }
            Self::Nul => f.write_str("a launch string contains a NUL byte"),
            Self::Io(error) => write!(f, "native helper launch: {error}"),
            Self::Unsupported => f.write_str("native helper launch is unsupported here"),
        }
    }
}

impl std::error::Error for SpawnError {}

/// How one standard stream of the helper is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stdio {
    /// The app's own stream.
    #[default]
    Inherit,
    /// `/dev/null`.
    Null,
    /// A pipe the parent reads or writes through [`NativeChild`].
    Piped,
}

/// A helper launch: program, arguments, environment, standard streams and the
/// descriptors it inherits.
#[derive(Debug)]
pub struct NativeCommand {
    program: OsString,
    args: Vec<OsString>,
    env: Option<Vec<(OsString, OsString)>>,
    stdio: [Stdio; 3],
    descriptors: Vec<(OwnedFd, i32)>,
}

impl NativeCommand {
    /// Launches `program`, an absolute path or a name found on `PATH`.
    #[must_use]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            args: Vec::new(),
            env: None,
            stdio: [Stdio::Inherit; 3],
            descriptors: Vec::new(),
        }
    }

    /// Appends arguments, passed as structured strings, never through a shell.
    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Self {
        self.args
            .extend(args.into_iter().map(|arg| arg.as_ref().to_owned()));
        self
    }

    /// Replaces the inherited environment with exactly `env`.
    #[must_use]
    pub fn env_clear_and_set(
        mut self,
        env: impl IntoIterator<Item = (impl AsRef<OsStr>, impl AsRef<OsStr>)>,
    ) -> Self {
        self.env = Some(
            env.into_iter()
                .map(|(key, value)| (key.as_ref().to_owned(), value.as_ref().to_owned()))
                .collect(),
        );
        self
    }

    /// Connects standard input.
    #[must_use]
    pub fn stdin(mut self, stdio: Stdio) -> Self {
        self.stdio[0] = stdio;
        self
    }

    /// Connects standard output.
    #[must_use]
    pub fn stdout(mut self, stdio: Stdio) -> Self {
        self.stdio[1] = stdio;
        self
    }

    /// Connects standard error.
    #[must_use]
    pub fn stderr(mut self, stdio: Stdio) -> Self {
        self.stdio[2] = stdio;
        self
    }

    /// Installs `source` at `target` (3..=[`MAX_TARGET`]) in the helper only.
    ///
    /// # Errors
    ///
    /// An invalid or repeated target, or a source that cannot be cloned.
    pub fn descriptor(mut self, source: BorrowedFd<'_>, target: i32) -> Result<Self, SpawnError> {
        if !(3..=MAX_TARGET).contains(&target) {
            return Err(SpawnError::InvalidTarget(target));
        }
        if self
            .descriptors
            .iter()
            .any(|(_, existing)| *existing == target)
        {
            return Err(SpawnError::DuplicateTarget(target));
        }
        let clone = source.try_clone_to_owned().map_err(SpawnError::Io)?;
        self.descriptors.push((clone, target));
        Ok(self)
    }

    /// Launches the helper.
    ///
    /// # Errors
    ///
    /// [`SpawnError::Nul`], [`SpawnError::Io`] when a clone, a pipe or the
    /// launch fails, and [`SpawnError::Unsupported`] off Linux. Every
    /// descriptor clone is closed before this returns either way.
    #[cfg(target_os = "linux")]
    pub fn spawn(self) -> Result<NativeChild, SpawnError> {
        sys::spawn(self)
    }

    /// Launching a native helper is unsupported off Linux.
    ///
    /// # Errors
    ///
    /// Always [`SpawnError::Unsupported`].
    #[cfg(not(target_os = "linux"))]
    pub fn spawn(self) -> Result<NativeChild, SpawnError> {
        Err(SpawnError::Unsupported)
    }
}

/// A launched helper. Dropping it before [`NativeChild::wait`] kills and reaps
/// it, so a cancelled launch leaves no process and no descriptor behind.
#[derive(Debug)]
pub struct NativeChild {
    pid: i32,
    reaped: bool,
    /// The parent's end of a piped standard input.
    pub stdin: Option<std::fs::File>,
    /// The parent's end of a piped standard output.
    pub stdout: Option<std::fs::File>,
    /// The parent's end of a piped standard error.
    pub stderr: Option<std::fs::File>,
}

impl NativeChild {
    /// The helper's process ID.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.pid.unsigned_abs()
    }

    /// Waits for the helper to exit, closing a piped standard input first.
    ///
    /// # Errors
    ///
    /// The `waitpid` failure.
    pub fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        drop(self.stdin.take());
        let status = sys::wait(self.pid)?;
        self.reaped = true;
        Ok(status)
    }

    /// Kills the helper. It still has to be waited for, or dropped.
    ///
    /// # Errors
    ///
    /// The `kill` failure.
    pub fn kill(&mut self) -> std::io::Result<()> {
        if self.reaped {
            return Ok(());
        }
        sys::kill(self.pid)
    }
}

impl Drop for NativeChild {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = sys::kill(self.pid);
            let _ = sys::wait(self.pid);
        }
    }
}

fn cstring(value: &OsStr) -> Result<CString, SpawnError> {
    CString::new(value.as_bytes()).map_err(|_| SpawnError::Nul)
}

/// A close-on-exec duplicate of `fd` numbered at least `floor`.
#[cfg(target_os = "linux")]
fn lift(fd: RawFd, floor: RawFd) -> std::io::Result<OwnedFd> {
    sys::dup_above(fd, floor)
}

#[cfg(not(target_os = "linux"))]
fn lift(_fd: RawFd, _floor: RawFd) -> std::io::Result<OwnedFd> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

#[cfg(not(target_os = "linux"))]
mod sys {
    pub(super) fn wait(_pid: i32) -> std::io::Result<std::process::ExitStatus> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }

    pub(super) fn kill(_pid: i32) -> std::io::Result<()> {
        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code, reason = "`posix_spawn` has no safe wrapper")]
mod sys {
    use super::*;

    /// A close-on-exec duplicate of `fd` numbered at least `floor`.
    pub(super) fn dup_above(fd: RawFd, floor: RawFd) -> std::io::Result<OwnedFd> {
        // SAFETY: `fcntl(F_DUPFD_CLOEXEC)` on a live descriptor returns a new
        // descriptor or -1; the new one is owned exactly once here.
        let raw = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, floor) };
        if raw < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `raw` is a fresh descriptor nothing else owns.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn pipe() -> std::io::Result<(OwnedFd, OwnedFd)> {
        let mut fds = [0; 2];
        // SAFETY: `pipe2` writes two descriptors into the two-element array.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both descriptors are fresh and owned exactly once.
        Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
    }

    /// Owns posix_spawn file actions and attributes, destroyed on drop.
    struct Actions(libc::posix_spawn_file_actions_t);

    impl Drop for Actions {
        fn drop(&mut self) {
            // SAFETY: initialized in `spawn` before any use.
            unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
        }
    }

    struct Attributes(libc::posix_spawnattr_t);

    impl Drop for Attributes {
        fn drop(&mut self) {
            // SAFETY: initialized in `spawn` before any use.
            unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
        }
    }

    fn check(rc: libc::c_int) -> std::io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            Err(std::io::Error::from_raw_os_error(rc))
        }
    }

    pub(super) fn spawn(command: NativeCommand) -> Result<NativeChild, SpawnError> {
        let program = cstring(&command.program)?;
        let mut argv = vec![program.clone()];
        for arg in &command.args {
            argv.push(cstring(arg)?);
        }
        let env: Vec<CString> = match &command.env {
            Some(env) => env
                .iter()
                .map(|(key, value)| {
                    let mut entry = key.clone();
                    entry.push("=");
                    entry.push(value);
                    cstring(&entry)
                })
                .collect::<Result<_, _>>()?,
            None => std::env::vars_os()
                .map(|(key, value)| {
                    let mut entry = key;
                    entry.push("=");
                    entry.push(value);
                    cstring(&entry)
                })
                .collect::<Result<_, _>>()?,
        };
        let mut argv_ptrs: Vec<*mut libc::c_char> =
            argv.iter().map(|arg| arg.as_ptr().cast_mut()).collect();
        argv_ptrs.push(std::ptr::null_mut());
        let mut env_ptrs: Vec<*mut libc::c_char> =
            env.iter().map(|entry| entry.as_ptr().cast_mut()).collect();
        env_ptrs.push(std::ptr::null_mut());

        // Above every target, so no clone is a target and no dup2 is a
        // same-number no-op that would keep close-on-exec.
        let floor = command
            .descriptors
            .iter()
            .map(|(_, target)| *target)
            .max()
            .unwrap_or(2)
            + 1;
        let mut lifted = Vec::with_capacity(command.descriptors.len());
        for (source, target) in &command.descriptors {
            lifted.push((
                lift(source.as_raw_fd(), floor).map_err(SpawnError::Io)?,
                *target,
            ));
        }
        drop(command.descriptors);

        let null = c"/dev/null";
        let mut parent_ends: [Option<OwnedFd>; 3] = [None, None, None];
        let mut child_ends: Vec<(OwnedFd, i32)> = Vec::new();
        for (stream, stdio) in command.stdio.iter().enumerate() {
            if *stdio == Stdio::Piped {
                let (read, write) = pipe().map_err(SpawnError::Io)?;
                let (parent, child) = if stream == 0 {
                    (write, read)
                } else {
                    (read, write)
                };
                // Lifted above the targets too, so no pipe end is overwritten.
                let child = dup_above(child.as_raw_fd(), floor).map_err(SpawnError::Io)?;
                parent_ends[stream] = Some(parent);
                child_ends.push((child, stream as i32));
            }
        }

        // SAFETY: zeroed storage is initialized by the matching `_init` call
        // before use and destroyed by the owning guard.
        let mut actions = Actions(unsafe { std::mem::zeroed() });
        // SAFETY: `actions.0` is writable storage the guard owns.
        check(unsafe { libc::posix_spawn_file_actions_init(&mut actions.0) })
            .map_err(SpawnError::Io)?;
        // SAFETY: as for `actions`, initialized by `posix_spawnattr_init` below.
        let mut attributes = Attributes(unsafe { std::mem::zeroed() });
        // SAFETY: `attributes.0` is writable storage the guard owns.
        check(unsafe { libc::posix_spawnattr_init(&mut attributes.0) }).map_err(SpawnError::Io)?;

        for (stream, stdio) in command.stdio.iter().enumerate() {
            if *stdio == Stdio::Null {
                let flags = if stream == 0 {
                    libc::O_RDONLY
                } else {
                    libc::O_WRONLY
                };
                // SAFETY: valid actions, a NUL-terminated path that outlives the spawn.
                check(unsafe {
                    libc::posix_spawn_file_actions_addopen(
                        &mut actions.0,
                        stream as i32,
                        null.as_ptr(),
                        flags,
                        0,
                    )
                })
                .map_err(SpawnError::Io)?;
            }
        }
        for (fd, target) in child_ends.iter().chain(&lifted) {
            // SAFETY: valid actions; both numbers are live or targets.
            check(unsafe {
                libc::posix_spawn_file_actions_adddup2(&mut actions.0, fd.as_raw_fd(), *target)
            })
            .map_err(SpawnError::Io)?;
        }

        // The helper starts with default signal handling and an empty mask,
        // as `std::process::Command` gives it.
        // SAFETY: the sets are initialized by sigemptyset/sigfillset before use.
        unsafe {
            let mut empty: libc::sigset_t = std::mem::zeroed();
            libc::sigemptyset(&mut empty);
            let mut all: libc::sigset_t = std::mem::zeroed();
            libc::sigfillset(&mut all);
            check(libc::posix_spawnattr_setsigmask(&mut attributes.0, &empty))
                .map_err(SpawnError::Io)?;
            check(libc::posix_spawnattr_setsigdefault(&mut attributes.0, &all))
                .map_err(SpawnError::Io)?;
            check(libc::posix_spawnattr_setflags(
                &mut attributes.0,
                (libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF) as libc::c_short,
            ))
            .map_err(SpawnError::Io)?;
        }

        let mut pid: libc::pid_t = 0;
        let searched = !command.program.as_bytes().contains(&b'/');
        // SAFETY: every pointer is valid and NUL-terminated for the duration
        // of the call; the argument and environment arrays end in null.
        let rc = unsafe {
            if searched {
                libc::posix_spawnp(
                    &mut pid,
                    program.as_ptr(),
                    &actions.0,
                    &attributes.0,
                    argv_ptrs.as_ptr(),
                    env_ptrs.as_ptr(),
                )
            } else {
                libc::posix_spawn(
                    &mut pid,
                    program.as_ptr(),
                    &actions.0,
                    &attributes.0,
                    argv_ptrs.as_ptr(),
                    env_ptrs.as_ptr(),
                )
            }
        };
        drop(lifted);
        drop(child_ends);
        check(rc).map_err(SpawnError::Io)?;
        let [stdin, stdout, stderr] = parent_ends.map(|end| end.map(std::fs::File::from));
        Ok(NativeChild {
            pid,
            reaped: false,
            stdin,
            stdout,
            stderr,
        })
    }

    pub(super) fn wait(pid: libc::pid_t) -> std::io::Result<std::process::ExitStatus> {
        use std::os::unix::process::ExitStatusExt as _;
        let mut status = 0;
        loop {
            // SAFETY: waits for our own child and writes its status.
            let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
            if rc == pid {
                return Ok(std::process::ExitStatus::from_raw(status));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }

    pub(super) fn kill(pid: libc::pid_t) -> std::io::Result<()> {
        // SAFETY: signals our own, not yet reaped child.
        if unsafe { libc::kill(pid, libc::SIGKILL) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
