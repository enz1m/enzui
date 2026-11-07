//! Startup policy for selecting the application's clipboard transport.
//!
//! [`Policy::FromEnvironment`] uses trimmed, case-insensitive
//! `ENZUI_CLIPBOARD=wayland` or `default`; absent/empty values adopt the
//! ordinary Wayland transport and unknown values are reported. The first
//! successful explicit or environment decision remains effective for later
//! startup calls. A refused installation releases the process-wide slot for
//! retry; installer panic leaves it occupied. Adoption seals authorize
//! transport construction but do not enforce a one-use or one-handle limit.
//!
//! ```
//! use enzui_clipboard::bootstrap::{Decision, Policy, decide};
//!
//! assert!(matches!(decide(Policy::FromEnvironment, None), Decision::Adopt));
//!
//! assert!(matches!(decide(Policy::Adopt, None), Decision::Adopt));
//! ```

use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};

/// The environment variable an application or a session may set.
pub const POLICY_VARIABLE: &str = "ENZUI_CLIPBOARD";

/// The value of [`POLICY_VARIABLE`] that asks for this transport.
pub const ADOPT_VALUE: &str = "wayland";

/// The value of [`POLICY_VARIABLE`] that asks for the runtime's own clipboard.
pub const DEFAULT_VALUE: &str = "default";

/// What an application asks for at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Use this transport.
    Adopt,

    /// Use whatever the runtime builds on its own.
    KeepDefault,

    /// Let the environment decide, defaulting to this transport.
    FromEnvironment,
}

/// Why the runtime's own clipboard was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepReason {
    /// The application did not ask for this transport.
    NotRequested,

    /// The application, or the session, asked for the runtime's clipboard.
    RequestedDefault,

    /// The environment named something that is not a known choice.
    ///
    /// Reported rather than ignored: a misspelt session variable otherwise
    /// looks exactly like a working opt-in.
    Unrecognised(String),
}

/// The resolved startup decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Install this transport.
    Adopt,

    /// Leave the runtime's clipboard in place.
    KeepDefault {
        /// Why it was kept.
        reason: KeepReason,
    },
}

/// Resolves the startup decision from a policy and a requested value.
///
/// `requested` is the value of [`POLICY_VARIABLE`], if any. Explicit `Adopt`
/// and `KeepDefault` policies ignore it. No process environment is read here.
pub fn decide(policy: Policy, requested: Option<&str>) -> Decision {
    match policy {
        Policy::Adopt => Decision::Adopt,
        Policy::KeepDefault => Decision::KeepDefault {
            reason: KeepReason::RequestedDefault,
        },
        Policy::FromEnvironment => match requested.map(str::trim) {
            None | Some("") => Decision::Adopt,
            Some(value) if value.eq_ignore_ascii_case(ADOPT_VALUE) => Decision::Adopt,
            Some(value) if value.eq_ignore_ascii_case(DEFAULT_VALUE) => Decision::KeepDefault {
                reason: KeepReason::RequestedDefault,
            },
            Some(value) => Decision::KeepDefault {
                reason: KeepReason::Unrecognised(value.to_owned()),
            },
        },
    }
}

/// Why a runtime binding could not install the transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallRefusal {
    /// A clipboard was already installed for this process.
    AlreadyInstalled,

    /// The runtime cannot accept a clipboard at this point.
    ///
    /// Installing has to happen before the event loop asks for a clipboard;
    /// afterwards there is nothing to replace.
    TooLate,

    /// The binding declined, with its own description.
    Declined {
        /// What the binding reported.
        reason: String,
    },
}

impl fmt::Display for InstallRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyInstalled => f.write_str("a clipboard was already installed"),
            Self::TooLate => f.write_str("the runtime had already built its clipboard"),
            Self::Declined { reason } => write!(f, "the runtime binding declined: {reason}"),
        }
    }
}

/// What actually happened at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// This transport is installed for the process.
    Adopted,

    /// The runtime's own clipboard is in place.
    KeptDefault {
        /// Why it was kept.
        reason: KeepReason,
    },

    /// This transport was asked for and could not be installed.
    ///
    /// Distinct from [`Outcome::KeptDefault`] on purpose: an application that
    /// asked for a clipboard and did not get one has a problem to report,
    /// while one that never asked has not.
    Refused {
        /// Why it could not be installed.
        refusal: InstallRefusal,
    },
}

impl Outcome {
    /// Whether the installer reported successful adoption.
    pub fn adopted(&self) -> bool {
        matches!(self, Self::Adopted)
    }
}

impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Adopted => f.write_str("the Wayland clipboard is in use"),
            Self::KeptDefault {
                reason: KeepReason::NotRequested,
            } => f.write_str("the runtime clipboard is in use; it was not asked to change"),
            Self::KeptDefault {
                reason: KeepReason::RequestedDefault,
            } => f.write_str("the runtime clipboard is in use, as requested"),
            Self::KeptDefault {
                reason: KeepReason::Unrecognised(value),
            } => write!(
                f,
                "the runtime clipboard is in use; {POLICY_VARIABLE}={value} names no known choice"
            ),
            Self::Refused { refusal } => {
                write!(f, "the Wayland clipboard was asked for but {refusal}")
            }
        }
    }
}

/// Proof that the bootstrap policy granted adoption.
///
/// Successful bootstrap admission gives the installer a seal. Transport
/// constructors borrow it, so one seal can construct several handles. The
/// `test-seal` feature also exposes construction without bootstrap admission.
#[derive(Debug)]
pub struct AdoptionSeal(());

impl AdoptionSeal {
    /// Constructs a seal without the bootstrap policy.
    ///
    /// Available only in test builds or when the `test-seal` feature is active.
    /// Protocol-level tests and the verification peer use this because they
    /// exercise the transport directly, without a toolkit runtime.
    #[cfg(any(test, feature = "test-seal"))]
    pub fn for_test() -> Self {
        Self(())
    }
}

/// A runtime binding that can hand this transport to a toolkit.
///
/// The binding lives with the toolkit it patches; this crate only decides
/// whether to call it, so an application's startup policy is testable without
/// a windowing system.
pub trait Installer {
    /// Installs the transport for this process.
    ///
    /// The seal proves the bootstrap policy granted adoption; pass it to
    /// [`crate::WaylandClipboard::adopt`] or [`crate::WaylandClipboard::adopt_foreign_display`].
    ///
    /// # Errors
    ///
    /// Return [`InstallRefusal`] when runtime timing or transport setup prevents installation.
    fn install(&self, seal: AdoptionSeal) -> Result<(), InstallRefusal>;

    /// A short name for the binding, for whatever the application logs.
    fn describe(&self) -> &str;
}

const UNDECIDED: u8 = 0;
const INSTALLING: u8 = 1;
const ADOPTED: u8 = 2;
const KEPT_DEFAULT: u8 = 3;

static PROCESS_DECISION: AtomicU8 = AtomicU8::new(UNDECIDED);

/// Applies the startup policy, installing the transport if it is wanted.
///
/// Reads [`POLICY_VARIABLE`] only when the policy defers to the environment.
/// Non-Unicode environment values behave as absent. The first successful
/// transport decision remains in force for later startup calls. A refused
/// installation frees the slot for retry; default decisions do not invoke the
/// installer.
///
/// # Panics
///
/// Propagates installer panics, leaving the installation slot occupied.
pub fn install(installer: &dyn Installer, policy: Policy) -> Outcome {
    let requested = match policy {
        Policy::FromEnvironment => std::env::var(POLICY_VARIABLE).ok(),
        _ => None,
    };

    install_with(installer, policy, requested.as_deref())
}

/// Apply the startup policy against an explicit value, without reading the environment.
///
/// Uses the same process-wide installation slot as [`install`].
///
/// # Panics
///
/// Propagates installer panics, leaving the slot occupied.
pub fn install_with(installer: &dyn Installer, policy: Policy, requested: Option<&str>) -> Outcome {
    install_into(&PROCESS_DECISION, installer, policy, requested)
}

fn install_into(
    state: &AtomicU8,
    installer: &dyn Installer,
    policy: Policy,
    requested: Option<&str>,
) -> Outcome {
    match decide(policy, requested) {
        Decision::KeepDefault { reason } => {
            match state.compare_exchange(
                UNDECIDED,
                KEPT_DEFAULT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) | Err(KEPT_DEFAULT) => Outcome::KeptDefault { reason },
                Err(_) => Outcome::Refused {
                    refusal: InstallRefusal::AlreadyInstalled,
                },
            }
        }
        Decision::Adopt => {
            match state.compare_exchange(UNDECIDED, INSTALLING, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => {}
                Err(KEPT_DEFAULT) => {
                    return Outcome::KeptDefault {
                        reason: KeepReason::RequestedDefault,
                    };
                }
                Err(_) => {
                    return Outcome::Refused {
                        refusal: InstallRefusal::AlreadyInstalled,
                    };
                }
            }

            match installer.install(AdoptionSeal(())) {
                Ok(()) => {
                    state.store(ADOPTED, Ordering::Release);
                    Outcome::Adopted
                }
                Err(refusal) => {
                    state.store(UNDECIDED, Ordering::Release);
                    Outcome::Refused { refusal }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct AcceptingInstaller;

    impl Installer for AcceptingInstaller {
        fn install(&self, _seal: AdoptionSeal) -> Result<(), InstallRefusal> {
            Ok(())
        }

        fn describe(&self) -> &str {
            "accepting test installer"
        }
    }

    #[test]
    fn the_environment_defaults_to_wayland_and_can_override_it() {
        assert_eq!(decide(Policy::FromEnvironment, None), Decision::Adopt);
        assert_eq!(decide(Policy::FromEnvironment, Some("")), Decision::Adopt);
        assert_eq!(
            decide(Policy::FromEnvironment, Some("wayland")),
            Decision::Adopt
        );
        assert_eq!(
            decide(Policy::FromEnvironment, Some("WAYLAND")),
            Decision::Adopt
        );
        assert_eq!(
            decide(Policy::FromEnvironment, Some("default")),
            Decision::KeepDefault {
                reason: KeepReason::RequestedDefault
            }
        );
        assert_eq!(
            decide(Policy::FromEnvironment, Some("other")),
            Decision::KeepDefault {
                reason: KeepReason::Unrecognised(String::from("other"))
            }
        );
        assert_eq!(
            decide(Policy::KeepDefault, Some("wayland")),
            Decision::KeepDefault {
                reason: KeepReason::RequestedDefault
            }
        );
        assert_eq!(decide(Policy::Adopt, Some("default")), Decision::Adopt);
    }

    #[test]
    fn an_adopted_or_foreign_provider_is_not_replaced_by_keep_default() {
        let state = AtomicU8::new(UNDECIDED);
        assert_eq!(
            install_into(&state, &AcceptingInstaller, Policy::Adopt, None),
            Outcome::Adopted
        );
        assert_eq!(
            install_into(&state, &AcceptingInstaller, Policy::KeepDefault, None),
            Outcome::Refused {
                refusal: InstallRefusal::AlreadyInstalled
            }
        );
    }
}
