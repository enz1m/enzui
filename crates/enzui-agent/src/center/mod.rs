//! Opt-in production connector to the external `enzui-center` daemon.
//!
//! This module speaks the daemon's v2 wire protocol as an enrolled app:
//! `u32` big-endian length-prefixed JSON frames, direction-separated mutual
//! HMAC authentication with fresh client and server nonces, and the typed
//! registration and invocation lifecycle from the daemon's `src/protocol.rs`.
//!
//! # What an app does
//!
//! 1. Declare functions with an [`AppFunctionRegistry`], mapping each to the
//!    app's own `Message` type. Declared consent scopes and capabilities are an
//!    upper bound; the daemon is the authority on what a call may do.
//! 2. Build a [`ConnectorConfig`] from the socket path, credential file, and the
//!    registry's [`AppFunctionRegistry::registration`].
//! 3. [`CentralConnector::spawn`] it. Drain [`CentralConnector::events`] from an
//!    ICED subscription or any loop, dispatch each [`ConnectorEvent::Invoke`]
//!    through the registry, run the work against the [`InvocationTable`]'s
//!    cancellation token, and answer with [`CentralConnector::complete`],
//!    [`CentralConnector::fail`], or [`CentralConnector::cancelled`].
//! 4. [`CentralConnector::shutdown`] joins the background thread with no task
//!    left running.
//!
//! # Consuming events
//!
//! [`CentralConnector::events`] is an ordinary [`std::sync::mpsc::Receiver`] for
//! a polling drain loop; `src/bin/central_connector_headless.rs` is a runnable,
//! GUI-free consumer external conformance tests drive against a real daemon.
//! With the `iced-consumer` feature, `center::iced_consumer::connector_subscription`
//! receives source events on a background pump and maps them while the
//! subscription stream is polled.
//!
//! # Security posture
//!
//! Reachable only with the `central-connector` Cargo feature. The connector
//! never listens on a socket, never exposes MCP, and never mints grants or
//! carries the management credential. Credentials load with the same
//! descriptor-bound checks as every framework secret and are held wiped.

use std::io;

pub mod app_host;
pub mod auth;
#[cfg(feature = "background")]
pub mod background;
pub mod connector;
pub mod credential;
#[cfg(feature = "iced-consumer")]
pub mod iced_consumer;
pub mod installed;
pub mod lifecycle;
pub(crate) mod peer;
pub mod registry;
pub mod review;
#[cfg(feature = "task-host")]
pub mod task_host;
pub(crate) mod unix_connect;

// The wire is shared with the daemon; these keep the connector's paths.
pub use enzui_center_protocol::framing as wire;
pub use enzui_center_protocol::protocol;

pub use app_host::{
    CallRef, CenterCall, CenterEnded, CenterFailure, CenterHost, CenterOutcome, CenterProvision,
    CenterStatus, CenterTurn, CenterUpdate, CommitReported, CompletionDisposition,
    CompletionReceipt, EndReason, HostError, test_invocation,
};
pub use connector::{
    CentralConnector, CommitReportError, CommitReportOutcome, ConnectorConfig, ConnectorEvent,
    ReconnectPolicy, SendError,
};
pub use credential::AppCredential;
#[cfg(feature = "iced-consumer")]
pub use iced_consumer::{
    ConnectorEvents, MessageEvents, MessageReceiveError, connector_subscription,
    message_subscription,
};
pub use lifecycle::{EmitDecision, InvocationTable, LifecycleError};
pub use protocol::{
    AgentCaller, AppMetadata, AppRegistration, CommitReportDisposition, ConsentScope,
    FunctionDescriptor, FunctionRisk, Invocation, InvocationEvent, Uuid, WireRole,
};
pub use registry::{
    AppFunction, AppFunctionRegistry, DispatchError, FunctionSpec, RegistryError, object_schema,
};

/// Why the connector could not be configured or started.
#[derive(Debug)]
pub enum CentralError {
    /// The credential file failed the secure-open checks.
    Credential(crate::secret::SecretError),
    /// The credential file parsed but was not a usable version-1 app
    /// credential. The static reason names the failing field without echoing
    /// any secret.
    CredentialFormat(&'static str),
    /// The registration's app id did not match the credential id, which the
    /// daemon requires.
    AppIdMismatch {
        /// The credential's app id.
        credential: String,
        /// The registration's app id.
        registration: String,
    },
    /// The current process uid could not be established, so the connection
    /// could not fail closed on ownership.
    OwnerUnverifiable,
    /// The background thread could not be started: either the thread itself,
    /// or the interruption channel its setup waits on, which
    /// [`connector::CentralConnector::shutdown`] needs in order to join a
    /// connect that has no socket yet. The error names which one failed.
    Spawn(io::Error),
}

impl core::fmt::Display for CentralError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Credential(error) => write!(f, "credential load failed: {error}"),
            Self::CredentialFormat(reason) => write!(f, "credential format invalid: {reason}"),
            Self::AppIdMismatch {
                credential,
                registration,
            } => write!(
                f,
                "registration app id {registration:?} does not match credential id {credential:?}"
            ),
            Self::OwnerUnverifiable => write!(f, "current uid could not be established"),
            Self::Spawn(error) => write!(f, "connector thread could not start: {error}"),
        }
    }
}

impl std::error::Error for CentralError {}

#[cfg(test)]
pub(crate) mod tests_support {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    /// A uniquely owned 0700 scratch directory removed when the guard drops.
    pub(crate) struct ScratchDir {
        path: PathBuf,
    }

    impl std::ops::Deref for ScratchDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Creates a unique owner-only scratch directory under the test target
    /// tmpdir (or the system temporary directory for lib unit tests).
    pub(crate) fn scratch_dir(label: &str) -> ScratchDir {
        use std::os::unix::fs::DirBuilderExt;

        let base =
            std::env::var_os("CARGO_TARGET_TMPDIR").map_or_else(std::env::temp_dir, PathBuf::from);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let leaf = format!("enzui-center-{label}-{}-{sequence}", std::process::id());
        let path = base.join(leaf);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .expect("create owner-only scratch directory");
        ScratchDir { path }
    }
}
