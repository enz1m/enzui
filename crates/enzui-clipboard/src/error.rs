//! Clipboard setup, protocol, transfer and worker-admission failures.

use std::fmt;
use std::time::Duration;

use crate::mime::MimeType;

/// Why a clipboard operation did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClipboardError {
    /// No Wayland connection could be adopted, so the transport never started.
    ///
    /// The contained reason is the underlying connection or protocol failure,
    /// kept verbatim so a log line can distinguish an absent compositor from a
    /// compositor that closed the connection.
    Unavailable {
        /// Human-readable description of the connection failure.
        reason: String,
    },

    /// Registry discovery or initial compositor synchronization did not finish.
    StartupStalled {
        /// How long construction waited before refusing adoption.
        after: Duration,
    },

    /// No usable `wl_data_device_manager` was discovered on this connection.
    NoDataDeviceManager,

    /// The compositor offers no primary selection
    /// (`zwp_primary_selection_device_manager_v1`), so middle-click paste is
    /// unavailable; the ordinary clipboard is unaffected.
    NoPrimarySelection,

    /// The compositor advertises no `wl_seat`, so no selection can be owned or
    /// read: a selection belongs to a seat, not to a process.
    NoSeat,

    /// A seat was named that the compositor never advertised.
    UnknownSeat {
        /// The seat name the caller asked for.
        requested: String,
    },

    /// No seat holds keyboard focus with a usable input serial.
    ///
    /// `wl_data_device::set_selection` is only honoured for the focused client
    /// and only with a serial the compositor issued to it. Refusing here is the
    /// honest answer; the alternative is a request the compositor discards
    /// while the application believes it copied something.
    NotFocused,

    /// The seat holds no selection at all: nothing has been copied, or the
    /// previous owner exited and the compositor cleared the selection.
    NoSelection,

    /// A file selection cannot be represented exactly within the clipboard's
    /// path rules and entry limit. No partial selection was offered.
    InvalidFileSelection,

    /// The compositor did not make this application the selection owner.
    ///
    /// The selection confirmation did not arrive within the configured claim window.
    ClaimNotHonoured {
        /// How long the confirmation was awaited.
        after: Duration,
    },

    /// The selection exists but not in any acceptable format.
    ///
    /// The offered types are reported so a caller can say what *was* available
    /// instead of pretending the clipboard was empty.
    MimeUnavailable {
        /// Every MIME type the current selection offers.
        offered: Vec<MimeType>,
    },

    /// The peer owning the data stopped writing (or never started) and the
    /// bounded transfer deadline expired.
    TransferStalled {
        /// How long the transfer waited before giving up.
        after: Duration,
    },

    /// The peer sent more bytes than the configured transfer limit allows.
    TransferTooLarge {
        /// The configured byte ceiling that was exceeded.
        limit: usize,
    },

    /// The descriptor transfer failed at the OS level.
    TransferFailed {
        /// The underlying I/O error description.
        reason: String,
    },

    /// The compositor connection ended while the request was in flight.
    Disconnected,

    /// The clipboard bridge is at capacity and did not retain the operation.
    Busy,

    /// The clipboard runtime has been shut down and accepts no further work.
    Stopped,
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { reason } => {
                write!(f, "no Wayland clipboard connection: {reason}")
            }
            Self::StartupStalled { after } => {
                write!(f, "Wayland clipboard startup stalled for {after:?}")
            }
            Self::NoDataDeviceManager => {
                f.write_str("the compositor advertises no wl_data_device_manager")
            }
            Self::NoPrimarySelection => {
                f.write_str("the compositor advertises no primary selection manager")
            }
            Self::NoSeat => f.write_str("the compositor advertises no wl_seat"),
            Self::UnknownSeat { requested } => {
                write!(f, "no seat named {requested}")
            }
            Self::NotFocused => {
                f.write_str("no focused seat with a valid input serial owns this window")
            }
            Self::NoSelection => f.write_str("the seat holds no selection"),
            Self::InvalidFileSelection => {
                f.write_str("the file selection cannot be represented exactly on the clipboard")
            }
            Self::ClaimNotHonoured { after } => {
                write!(
                    f,
                    "the compositor did not grant the selection within {after:?}"
                )
            }
            Self::MimeUnavailable { offered } => {
                write!(f, "the selection offers no acceptable type (offered:")?;
                for mime in offered {
                    write!(f, " {mime}")?;
                }
                f.write_str(")")
            }
            Self::TransferStalled { after } => {
                write!(f, "the clipboard owner stalled for {after:?}")
            }
            Self::TransferTooLarge { limit } => {
                write!(f, "the clipboard payload exceeds {limit} bytes")
            }
            Self::TransferFailed { reason } => {
                write!(f, "clipboard descriptor transfer failed: {reason}")
            }
            Self::Disconnected => f.write_str("the Wayland connection ended"),
            Self::Busy => f.write_str("the clipboard bridge is busy"),
            Self::Stopped => f.write_str("the clipboard runtime has stopped"),
        }
    }
}

impl std::error::Error for ClipboardError {}
