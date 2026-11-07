use super::*;
use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use std::fs;
use std::os::unix::process::CommandExt;

#[derive(Debug)]
pub(super) enum LeaderProbe {
    Running,
    ExitedUnreaped,
}

#[derive(Debug, Clone, Copy)]
struct Identity {
    pgid: Pid,
    start_time: u64,
}

#[derive(Debug)]
pub(super) struct OwnedGroup {
    pid: Pid,
    identity: Option<Identity>,
    #[cfg(test)]
    fail_first_group_kill: bool,
    #[cfg(test)]
    fail_every_group_kill: bool,
    #[cfg(test)]
    signal_probe: Option<SignalProbe>,
    #[cfg(test)]
    cleanup_seam: Option<CleanupSeam>,
}

pub(super) fn configure(command: &mut Command) {
    command.process_group(0);
}

impl OwnedGroup {
    pub(super) fn capture(child: &Child, _faults: &Faults) -> Self {
        let pid = Pid::from_child(child);
        let identity = process_snapshot(pid)
            .and_then(|(pgid, start_time)| (pgid == pid).then_some(Identity { pgid, start_time }));
        Self {
            pid,
            identity,
            #[cfg(test)]
            fail_first_group_kill: _faults.fail_first_group_kill,
            #[cfg(test)]
            fail_every_group_kill: _faults.fail_every_group_kill,
            #[cfg(test)]
            signal_probe: _faults.signal_probe.clone(),
            #[cfg(test)]
            cleanup_seam: _faults.cleanup_seam.clone(),
        }
    }

    pub(super) fn setup_error(&self) -> Option<String> {
        self.identity
            .is_none()
            .then(|| "could not establish the child process-group identity".to_owned())
    }

    // Releasing the numeric group identity permanently disables further group signalling.
    pub(super) fn release_identity(&mut self) {
        self.identity = None;
    }

    pub(super) fn probe_leader(&self, _child: &mut Child) -> Result<LeaderProbe, String> {
        let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
        waitid(WaitId::Pid(self.pid), options)
            .map(|status| {
                if status.is_some() {
                    LeaderProbe::ExitedUnreaped
                } else {
                    LeaderProbe::Running
                }
            })
            .map_err(|error| bounded_diagnostic(format!("query child without reaping: {error}")))
    }

    // Retry group kill while the leader remains waitable; on failure, use only the exact child handle.
    pub(super) fn kill_residual(&mut self) -> Result<(), TeardownError> {
        if let Err(first) = self.signal_group(Signal::KILL) {
            // Keep the leader waitable while retrying. Reaping it here
            // would free the numeric PGID before cleanup was complete.
            thread::sleep(POLL_INTERVAL);
            self.signal_group(Signal::KILL).map_err(|second| {
                TeardownError::group_signal(format!(
                    "signal child process group: {first}; retry: {second}; \
                         falling back to the exact child handle"
                ))
            })?;
        }
        Ok(())
    }

    pub(super) fn wait_for_residuals(&self) -> Result<(), TeardownError> {
        #[cfg(test)]
        if let Some(seam) = self.cleanup_seam.clone() {
            let scan_seam = seam.clone();
            let sleep_seam = seam.clone();
            return self.wait_for_residuals_via(
                move || seam.elapsed(),
                move |pgid, leader| {
                    scan_seam.record_poll();
                    if scan_seam.residual_never_clears {
                        Ok(Some(leader))
                    } else {
                        residual_group_member(pgid, leader)
                    }
                },
                move |step| sleep_seam.advance(step),
            );
        }
        let start = Instant::now();
        self.wait_for_residuals_via(
            move || start.elapsed(),
            residual_group_member,
            thread::sleep,
        )
    }

    fn wait_for_residuals_via(
        &self,
        mut elapsed: impl FnMut() -> Duration,
        mut scan: impl FnMut(Pid, Pid) -> Result<Option<Pid>, String>,
        mut wait: impl FnMut(Duration),
    ) -> Result<(), TeardownError> {
        let Some(identity) = self.identity else {
            return Err(TeardownError::group_signal(
                "child process-group identity is unavailable".to_owned(),
            ));
        };
        loop {
            self.verify_identity(identity)
                .map_err(TeardownError::group_signal)?;
            if scan(identity.pgid, self.pid)
                .map_err(TeardownError::cleanup)?
                .is_none()
            {
                return Ok(());
            }
            // A successfully delivered SIGKILL cannot be caught. Do not
            // return ownership while a killed descendant is still visible
            // (including as a zombie waiting for its new parent to reap it).
            // A member that cannot be reaped promptly must still not hang
            // the caller, so give up at the explicit cleanup ceiling.
            if elapsed() >= CLEANUP_CEILING {
                return Err(TeardownError::cleanup(format!(
                    "owned process group still had a member after {}ms of \
                         post-KILL observation; the group is no longer signalled \
                         and the direct leader is addressed separately (see any \
                         additional cleanup outcome reported with this one)",
                    CLEANUP_CEILING.as_millis()
                )));
            }
            wait(POLL_INTERVAL);
        }
    }

    pub(super) fn terminate(
        &mut self,
        child: &mut Child,
        grace: Duration,
    ) -> Result<(), TeardownError> {
        if self.identity.is_none() {
            // Fail closed for numeric group signalling when procfs identity
            // capture failed: the caller releases the exact `Child` handle,
            // which cannot be aliased by PID reuse.
            let _ = child;
            return Ok(());
        }

        if self.signal_group(Signal::TERM).is_ok() {
            thread::sleep(grace);
        }
        self.kill_residual()?;
        self.wait_for_residuals()
    }

    fn verify_identity(&self, expected: Identity) -> Result<(), String> {
        if identity_matches(
            Some((expected.pgid.as_raw_pid(), expected.start_time)),
            process_snapshot(self.pid).map(|(pgid, start_time)| (pgid.as_raw_pid(), start_time)),
        ) {
            Ok(())
        } else {
            Err("child process-group identity changed before signalling".to_owned())
        }
    }

    fn signal_group(&mut self, signal: Signal) -> Result<(), String> {
        #[cfg(test)]
        if let Some(probe) = &self.signal_probe {
            probe
                .attempts
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(test)]
        let inject_failure = signal == Signal::KILL
            && (std::mem::take(&mut self.fail_first_group_kill) || self.fail_every_group_kill);
        #[cfg(test)]
        if inject_failure && let Some(probe) = &self.signal_probe {
            probe
                .injected_failures
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        #[cfg(not(test))]
        let inject_failure = false;

        self.signal_group_via(signal, process_snapshot, move |pgid, signal| {
            if inject_failure {
                return Err("injected group-signal failure".to_owned());
            }
            match kill_process_group(pgid, signal) {
                Ok(()) | Err(Errno::SRCH) => Ok(()),
                Err(error) => Err(bounded_diagnostic(format!(
                    "signal child process group: {error}"
                ))),
            }
        })
    }

    fn signal_group_via(
        &self,
        signal: Signal,
        snapshot: impl FnOnce(Pid) -> Option<(Pid, u64)>,
        send: impl FnOnce(Pid, Signal) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(identity) = self.identity else {
            return Err("child process-group identity is unavailable".to_owned());
        };
        if !identity_matches(
            Some((identity.pgid.as_raw_pid(), identity.start_time)),
            snapshot(self.pid).map(|(pgid, start_time)| (pgid.as_raw_pid(), start_time)),
        ) {
            return Err("child process-group identity changed before signalling".to_owned());
        }
        send(identity.pgid, signal)
    }
}

fn process_snapshot(pid: Pid) -> Option<(Pid, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_name = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    let pgid = fields.get(2)?.parse().ok().and_then(Pid::from_raw)?;
    let start_time = fields.get(19)?.parse().ok()?;
    Some((pgid, start_time))
}

fn residual_group_member(pgid: Pid, leader: Pid) -> Result<Option<Pid>, String> {
    let entries = fs::read_dir("/proc")
        .map_err(|error| bounded_diagnostic(format!("scan process group: {error}")))?;
    for entry in entries.flatten() {
        let Some(raw_pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        let Some(pid) = Pid::from_raw(raw_pid) else {
            continue;
        };
        if pid != leader && process_snapshot(pid).is_some_and(|snapshot| snapshot.0 == pgid) {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

fn identity_matches(expected: Option<(i32, u64)>, current: Option<(i32, u64)>) -> bool {
    matches!((expected, current), (Some(expected), Some(current)) if expected == current)
}
