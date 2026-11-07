//! Who may stop an application: a service the installed OS declares in
//! [`CALLERS_DIRECTORY`](super::CALLERS_DIRECTORY), verified from the bus
//! connection to the running executable.
//!
//! A caller is admitted only when all of these hold, checked within
//! [`ADMISSION_DEADLINE`]:
//!
//! - it runs as this user, and its connection is the current owner of the
//!   service's well-known bus name;
//! - its process runs the installed executable: the same device and inode
//!   as the policy's path, which is a root-owned regular file whose
//!   directories are root-owned and writable by no one else;
//! - it runs in the service's user unit, and is that unit's main process as
//!   the user manager reports it; the unit's file and every drop-in are
//!   root-controlled, so no user override changed what the unit runs;
//! - its process start time is the same before and after the checks, so a
//!   recycled PID cannot stand in for it.
//!
//! The cgroup is one check among these, never sufficient alone: a same-user
//! process can move itself into a user-owned cgroup, but it cannot become
//! the root-owned executable or take over a bus name it does not own.
//!
//! Inside a user namespace "root-owned" can only be shown as "owned outside
//! this user", so there the OS's own witness is required as well (see
//! `witness`).

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long one caller check may take before the call is refused. It
/// covers the OS witness, whose helper has a three-second deadline.
pub const ADMISSION_DEADLINE: Duration = Duration::from_secs(5);

/// One installed service allowed to stop this application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerPolicy {
    /// The service's installed native executable.
    pub executable: PathBuf,
    /// The well-known bus name the service owns while it runs.
    pub bus_name: String,
    /// The user unit the service runs in.
    pub unit: String,
    /// The system service that vouches for it inside a user namespace.
    /// Without one, such a caller is refused there.
    pub witness: Option<WitnessCall>,
}

/// A system-bus method that observes the caller (witness schema 1): asked
/// with `{"schema":1}`, it answers one JSON object describing the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessCall {
    /// Its well-known name on the system bus.
    pub bus: String,
    /// Its object path.
    pub path: String,
    /// Its interface.
    pub interface: String,
    /// The method to call.
    pub method: String,
}

/// Which callers a served lifecycle answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleCallers(pub(crate) Admission);

impl LifecycleCallers {
    /// The services the installed OS declares in
    /// [`CALLERS_DIRECTORY`](super::CALLERS_DIRECTORY), read again on
    /// every call. Without a valid descriptor nobody is admitted.
    #[must_use]
    pub fn installed() -> Self {
        Self(Admission::Installed)
    }
}

impl From<CallerPolicy> for LifecycleCallers {
    fn from(policy: CallerPolicy) -> Self {
        Self(Admission::Policy(policy))
    }
}

/// Which callers a served lifecycle answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Admission {
    /// The services the installed OS declares, verified in full.
    Installed,
    /// One service, verified in full.
    Policy(CallerPolicy),
    /// Only processes running this very executable: tests and harnesses that
    /// play the OS from the app's own binary.
    OwnExecutable,
}

/// A file's identity: device and inode.
pub(crate) type Identity = (u64, u64);

/// How file owners look from this process's user namespace.
///
/// In the initial namespace a root-owned file shows owner 0. A sandboxed app
/// often runs in a user namespace that maps only its own user (`1000 1000 1`
/// in `/proc/self/uid_map`); there the kernel shows every unmapped owner,
/// root included, as the overflow uid (`/proc/sys/kernel/overflowuid`,
/// normally 65534). From inside such a namespace no call tells root apart
/// from another unmapped account.
///
/// What admission needs is that no process of this user can change the
/// file. An owner shown as the overflow uid proves that when the overflow
/// uid is itself unmapped here and this process's own uid is mapped: the
/// file is owned by an account outside the namespace, never by this user.
/// An owner of 65534 in the initial namespace is the real `nobody` account
/// and is refused, as is any other non-root owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OwnerView {
    /// This process's uid as its namespace sees it.
    own_uid: u32,
    /// The uid shown for owners this namespace does not map, when that uid
    /// is itself unmapped; `None` when an overflow owner proves nothing.
    unmapped_owner: Option<u32>,
}

impl OwnerView {
    /// The view of this process.
    pub(crate) fn current() -> Self {
        let overflow = std::fs::read_to_string("/proc/sys/kernel/overflowuid")
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .unwrap_or(65_534);
        let uid_map = std::fs::read_to_string("/proc/self/uid_map").unwrap_or_default();
        Self::from_map(rustix::process::getuid().as_raw(), overflow, &uid_map)
    }

    /// The view of a process with `own_uid`, the kernel's `overflow` uid and
    /// the `uid_map` text its namespace reports. An unreadable map line
    /// makes overflow owners prove nothing.
    pub(crate) fn from_map(own_uid: u32, overflow: u32, uid_map: &str) -> Self {
        let mut ranges = Vec::new();
        for line in uid_map.lines().filter(|line| !line.trim().is_empty()) {
            let fields: Option<Vec<u64>> = line
                .split_whitespace()
                .map(|field| field.parse().ok())
                .collect();
            match fields.as_deref() {
                Some(&[inside, _outside, count]) => ranges.push((inside, count)),
                _ => {
                    return Self {
                        own_uid,
                        unmapped_owner: None,
                    };
                }
            }
        }
        let mapped = |uid: u32| {
            ranges
                .iter()
                .any(|&(inside, count)| (inside..inside + count).contains(&u64::from(uid)))
        };
        let proves = overflow != own_uid && mapped(own_uid) && !mapped(overflow);
        Self {
            own_uid,
            unmapped_owner: proves.then_some(overflow),
        }
    }

    /// Whether `meta` is owned by root, or by an account outside this
    /// namespace, and is writable by no group or other user.
    pub(crate) fn protects(self, meta: &std::fs::Metadata) -> bool {
        self.protected_owner(meta.uid()) && meta.mode() & 0o022 == 0
    }

    /// Whether ownership here can only be shown as "outside this user", so
    /// the OS witness must vouch for root ownership.
    pub(crate) fn needs_witness(self) -> bool {
        self.unmapped_owner.is_some()
    }

    fn protected_owner(self, uid: u32) -> bool {
        uid != self.own_uid && (uid == 0 || Some(uid) == self.unmapped_owner)
    }
}

/// The identity of `path` when it is a root-owned regular file reached only
/// through root-owned directories that no one else can write.
pub(crate) fn installed_identity(path: &Path) -> Option<Identity> {
    installed_identity_in(OwnerView::current(), path)
}

fn installed_identity_in(view: OwnerView, path: &Path) -> Option<Identity> {
    if !path.is_absolute() {
        return None;
    }
    let file = std::fs::symlink_metadata(path).ok()?;
    if !file.is_file() || !view.protects(&file) {
        return None;
    }
    for ancestor in path.ancestors().skip(1) {
        let directory = std::fs::symlink_metadata(ancestor).ok()?;
        if !directory.is_dir() || !view.protects(&directory) {
            return None;
        }
    }
    Some((file.dev(), file.ino()))
}

/// The identity of the executable process `pid` is running now.
pub(crate) fn running_identity(pid: u32) -> Option<Identity> {
    let meta = std::fs::metadata(format!("/proc/{pid}/exe")).ok()?;
    Some((meta.dev(), meta.ino()))
}

/// The process's start time in clock ticks since boot (`/proc/PID/stat`
/// field 22), which a recycled PID does not share.
pub(crate) fn start_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces and parentheses; fields resume after
    // the last ')'. Field 3 is the first after it, so 22 is the 20th.
    let (_, rest) = stat.rsplit_once(')')?;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// Whether process `pid` runs in the user unit `unit`.
pub(crate) fn in_unit(pid: u32, unit: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/cgroup")).is_ok_and(|cgroups| {
        cgroups.lines().any(|line| {
            line.strip_prefix("0::")
                .is_some_and(|path| path.trim_end().ends_with(&format!("/{unit}")))
        })
    })
}

/// The unit's main process, unit file and drop-ins, as the systemd user
/// manager on this bus reports them.
async fn unit_facts(
    connection: &zbus::Connection,
    unit: &str,
) -> Option<(u32, String, Vec<String>)> {
    let manager = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
    )
    .await
    .ok()?;
    let path: zbus::zvariant::OwnedObjectPath = manager.call("GetUnit", &(unit,)).await.ok()?;
    let service = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        path.clone(),
        "org.freedesktop.systemd1.Service",
    )
    .await
    .ok()?;
    let main_pid: u32 = service.get_property("MainPID").await.ok()?;
    let unit_proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.systemd1",
        path,
        "org.freedesktop.systemd1.Unit",
    )
    .await
    .ok()?;
    let fragment: String = unit_proxy.get_property("FragmentPath").await.ok()?;
    let drop_ins: Vec<String> = unit_proxy.get_property("DropInPaths").await.ok()?;
    Some((main_pid, fragment, drop_ins))
}

/// Whether the unit file and every drop-in are root-controlled installed
/// files: no user-writable file shapes what the unit runs.
pub(crate) fn unit_files_root_controlled(fragment: &str, drop_ins: &[String]) -> bool {
    let view = OwnerView::current();
    !fragment.is_empty()
        && installed_identity_in(view, Path::new(fragment)).is_some()
        && drop_ins
            .iter()
            .all(|path| installed_identity_in(view, Path::new(path)).is_some())
}

/// Which check refused a caller. It names the check, never the values seen,
/// so the OS service can log why it was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// The bus could not say who the caller is.
    UnknownCaller,
    /// The caller runs as another user.
    OtherUser,
    /// The caller's connection does not own the service's bus name.
    NotNameOwner,
    /// The caller does not run in the service's user unit.
    NotInUnit,
    /// The user manager could not describe the service's unit.
    UnitUnknown,
    /// The caller is not the unit's main process.
    NotMainProcess,
    /// The unit file or a drop-in is not a root-controlled installed file.
    UnitOverridden,
    /// The policy's executable is not a root-controlled installed file.
    ExecutableNotInstalled,
    /// The caller runs another executable.
    OtherExecutable,
    /// The caller's PID now names another process.
    ProcessReplaced,
    /// The checks did not finish within [`ADMISSION_DEADLINE`].
    Deadline,
    /// The OS declares no valid lifecycle caller.
    NoInstalledCaller,
    /// Inside a user namespace, the OS witness could not be obtained.
    WitnessUnavailable,
    /// The witness did not come from the root-owned system service.
    WitnessNotSystem,
    /// The witness describes another process, boot or unit.
    WitnessMismatch,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownCaller => "the bus could not identify the caller",
            Self::OtherUser => "the caller runs as another user",
            Self::NotNameOwner => "the caller does not own the service's bus name",
            Self::NotInUnit => "the caller does not run in the service's unit",
            Self::UnitUnknown => "the user manager could not describe the service's unit",
            Self::NotMainProcess => "the caller is not the unit's main process",
            Self::UnitOverridden => "the service's unit is not root-controlled",
            Self::ExecutableNotInstalled => "the service's executable is not installed root-owned",
            Self::OtherExecutable => "the caller runs another executable",
            Self::ProcessReplaced => "the caller's process changed during the check",
            Self::Deadline => "the caller check did not finish in time",
            Self::NoInstalledCaller => "the system declares no lifecycle caller",
            Self::WitnessUnavailable => "the OS could not vouch for the service",
            Self::WitnessNotSystem => "the service's witness did not come from the system",
            Self::WitnessMismatch => "the OS witness describes another process",
        })
    }
}

/// Checks the caller behind `sender` on `connection` against `admission`.
///
/// # Errors
/// The first check the caller fails.
pub(crate) async fn admit(
    admission: &Admission,
    connection: &zbus::Connection,
    sender: &zbus::names::UniqueName<'_>,
) -> Result<(), Refusal> {
    tokio::time::timeout(ADMISSION_DEADLINE, check(admission, connection, sender))
        .await
        .unwrap_or(Err(Refusal::Deadline))
}

async fn check(
    admission: &Admission,
    connection: &zbus::Connection,
    sender: &zbus::names::UniqueName<'_>,
) -> Result<(), Refusal> {
    let bus = zbus::fdo::DBusProxy::new(connection)
        .await
        .map_err(|_| Refusal::UnknownCaller)?;
    let who = zbus::names::BusName::from(sender.clone());
    let uid = bus
        .get_connection_unix_user(who.clone())
        .await
        .map_err(|_| Refusal::UnknownCaller)?;
    if uid != rustix::process::getuid().as_raw() {
        return Err(Refusal::OtherUser);
    }
    let pid = bus
        .get_connection_unix_process_id(who)
        .await
        .map_err(|_| Refusal::UnknownCaller)?;
    let caller = Caller {
        connection,
        bus: &bus,
        sender,
        uid,
        pid,
        started: start_ticks(pid).ok_or(Refusal::UnknownCaller)?,
    };
    let expected = match admission {
        Admission::Policy(policy) => caller.installed_service(policy).await?,
        Admission::Installed => {
            let policies = super::callers::installed(
                OwnerView::current(),
                Path::new(super::callers::CALLERS_DIRECTORY),
            )
            .filter(|policies| !policies.is_empty())
            .ok_or(Refusal::NoInstalledCaller)?;
            // A connection owns a well-known name at most once: only the
            // descriptors whose name the caller owns can admit it.
            let mut refusal = Refusal::NotNameOwner;
            let mut admitted = None;
            for policy in &policies {
                match caller.installed_service(policy).await {
                    Ok(identity) => {
                        admitted = Some(identity);
                        break;
                    }
                    Err(Refusal::NotNameOwner) => {}
                    Err(other) => refusal = other,
                }
            }
            admitted.ok_or(refusal)?
        }
        Admission::OwnExecutable => {
            running_identity(std::process::id()).ok_or(Refusal::ExecutableNotInstalled)?
        }
    };
    if running_identity(pid) != Some(expected) {
        return Err(Refusal::OtherExecutable);
    }
    if start_ticks(pid) != Some(caller.started) {
        return Err(Refusal::ProcessReplaced);
    }
    Ok(())
}

/// The caller facts the user bus authenticated.
struct Caller<'a> {
    connection: &'a zbus::Connection,
    bus: &'a zbus::fdo::DBusProxy<'a>,
    sender: &'a zbus::names::UniqueName<'a>,
    uid: u32,
    pid: u32,
    started: u64,
}

impl Caller<'_> {
    /// Checks the caller against one service and returns the identity of the
    /// executable it must be running.
    async fn installed_service(&self, policy: &CallerPolicy) -> Result<Identity, Refusal> {
        let name = zbus::names::BusName::try_from(policy.bus_name.as_str())
            .map_err(|_| Refusal::NotNameOwner)?;
        let owns_name = self
            .bus
            .get_name_owner(name)
            .await
            .is_ok_and(|owner| owner.as_str() == self.sender.as_str());
        if !owns_name {
            return Err(Refusal::NotNameOwner);
        }
        if !in_unit(self.pid, &policy.unit) {
            return Err(Refusal::NotInUnit);
        }
        let (main_pid, fragment, drop_ins) = unit_facts(self.connection, &policy.unit)
            .await
            .ok_or(Refusal::UnitUnknown)?;
        if main_pid != self.pid {
            return Err(Refusal::NotMainProcess);
        }
        if !unit_files_root_controlled(&fragment, &drop_ins) {
            return Err(Refusal::UnitOverridden);
        }
        let installed =
            installed_identity(&policy.executable).ok_or(Refusal::ExecutableNotInstalled)?;
        if OwnerView::current().needs_witness() {
            let call = policy.witness.as_ref().ok_or(Refusal::WitnessUnavailable)?;
            let boot_id = super::witness::boot_id().ok_or(Refusal::WitnessUnavailable)?;
            let observed = super::witness::Observed {
                uid: self.uid,
                bus_name: self.sender.as_str(),
                pid: self.pid,
                start_ticks: self.started,
                boot_id: &boot_id,
                unit: &policy.unit,
            };
            super::witness::verify(call, &observed)
                .await
                .map_err(|refusal| match refusal {
                    super::witness::WitnessRefusal::Unavailable => Refusal::WitnessUnavailable,
                    super::witness::WitnessRefusal::NotSystem => Refusal::WitnessNotSystem,
                    super::witness::WitnessRefusal::Mismatch => Refusal::WitnessMismatch,
                })?;
        }
        Ok(installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_override_of_the_unit_is_refused() {
        let ours = std::env::current_exe().unwrap().display().to_string();
        assert!(
            !unit_files_root_controlled(&ours, &[]),
            "a user-owned unit file"
        );
        assert!(!unit_files_root_controlled("", &[]), "no unit file at all");
        let system = [
            "/usr/bin/env",
            "/usr/bin/bash",
            "/bin/bash",
            "/usr/bin/true",
        ]
        .into_iter()
        .find(|path| installed_identity(Path::new(path)).is_some())
        .expect("a root-owned system binary to stand in for an installed unit file");
        assert!(unit_files_root_controlled(system, &[]));
        assert!(
            !unit_files_root_controlled(system, &[ours]),
            "a user-owned drop-in overrides a root-owned unit"
        );
    }
}
