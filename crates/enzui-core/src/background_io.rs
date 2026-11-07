//! Background reading that stays out of the person's way.
//!
//! A worker that walks a folder on its own initiative (a repository status,
//! a size count) should yield to the window and to the person's own work.
//! [`lower_current_thread`] drops the calling thread to a lower CPU priority
//! and the idle I/O class, so its reads wait for an idle disk. Call it at the
//! start of the job, on the worker thread; it never touches the UI thread.
//!
//! [`is_network_filesystem`] tells a folder on a network or user-space mount
//! (NFS, SMB, GVfs and sshfs through FUSE, AFS, Ceph, 9p) from a local one,
//! where thousands of stat calls are cheap. Treat a network folder as
//! count-on-request.

use std::path::Path;

/// The nice value a background worker thread runs at.
pub const BACKGROUND_NICE: i32 = 10;

/// Lowers the calling thread to [`BACKGROUND_NICE`] and the idle I/O class.
/// Returns whether both took effect; a failure leaves the thread as it was
/// and the work still runs.
pub fn lower_current_thread() -> bool {
    let tid = rustix::thread::gettid();
    // Per-thread on Linux: PRIO_PROCESS with a thread id names that thread.
    let nice = rustix::process::setpriority_process(Some(tid), BACKGROUND_NICE).is_ok();
    nice && idle_io(tid.as_raw_nonzero().get())
}

/// `ioprio_set(IOPRIO_WHO_PROCESS, tid, IOPRIO_CLASS_IDLE)`: the thread's
/// reads and writes wait for an otherwise idle disk.
#[allow(unsafe_code, reason = "`ioprio_set` has no safe wrapper")]
fn idle_io(tid: i32) -> bool {
    const IOPRIO_WHO_PROCESS: libc::c_long = 1;
    const IOPRIO_CLASS_IDLE: libc::c_long = 3;
    const IOPRIO_CLASS_SHIFT: libc::c_long = 13;
    // SAFETY: ioprio_set takes three integers and dereferences nothing.
    let result = unsafe {
        libc::syscall(
            libc::SYS_ioprio_set,
            IOPRIO_WHO_PROCESS,
            libc::c_long::from(tid),
            IOPRIO_CLASS_IDLE << IOPRIO_CLASS_SHIFT,
        )
    };
    result == 0
}

/// Whether `path` is on a network or user-space filesystem. An unreadable
/// path counts as network, the conservative answer for a costly walk.
#[must_use]
pub fn is_network_filesystem(path: &Path) -> bool {
    const NFS: u64 = 0x6969;
    const SMB: u64 = 0x517b;
    const SMB2: u64 = 0xfe53_4d42;
    const CIFS: u64 = 0xff53_4d42;
    const FUSE: u64 = 0x6573_5546;
    const AFS: u64 = 0x5346_414f;
    const CEPH: u64 = 0x00c3_6400;
    const V9FS: u64 = 0x0102_1997;
    const CODA: u64 = 0x7375_7245;
    match rustix::fs::statfs(path) {
        Ok(stats) => matches!(
            stats.f_type as u64,
            NFS | SMB | SMB2 | CIFS | FUSE | AFS | CEPH | V9FS | CODA
        ),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowered_thread_runs_nice_and_the_others_do_not() {
        let nice = std::thread::spawn(|| {
            assert!(lower_current_thread(), "nice and idle I/O both applied");
            rustix::process::getpriority_process(Some(rustix::thread::gettid())).unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(nice, BACKGROUND_NICE);
        let own = rustix::process::getpriority_process(Some(rustix::thread::gettid())).unwrap();
        assert_ne!(own, BACKGROUND_NICE, "only the worker thread changed");
    }
}
