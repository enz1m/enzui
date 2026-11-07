//! Clipboard transport on the application's existing Wayland connection and seats.
//!
//! [`WaylandClipboard`] uses ordinary `wl_data_device` objects. [`Payload`]
//! supplies offered bytes and [`Kind`] supplies receiving preferences. Offers
//! need focus and an input serial, and wait for selection confirmation. The
//! clipboard owner must stay alive to serve transfers.
//!
//! [`bootstrap`] controls installation; its reusable seal authorizes adoption
//! but does not limit constructed handles. Foreign-display adoption requires a
//! [`DisplayLease`] containing the actual display owner. Detached workers can
//! retain that lease after teardown. [`bridge`] provides bounded queue admission;
//! direct runtime commands use an unbounded queue and synchronous result waits.
//! See [`transfer`] for the scope of byte and blocked-I/O timeout limits.
//! [`drag`] shares the connection for native cross-application copy/move with
//! active-grab authority, bounded command admission and explicit completion.
//!
//! ```no_run
//! use enzui_clipboard::{Kind, Payload, TransferLimits, WaylandClipboard};
//! use enzui_clipboard::bootstrap::AdoptionSeal;
//!
//! # fn example(connection: &wayland_client::Connection, seal: &AdoptionSeal) -> Result<(), Box<dyn std::error::Error>> {
//! let clipboard = WaylandClipboard::adopt(connection, TransferLimits::default(), seal)?;
//!
//! clipboard.offer(Payload::html("<b>bold</b>", "bold"))?;
//! let pasted = clipboard.paste(Kind::Html)?;
//!
//! assert_eq!(pasted.mime().as_str(), "text/html;charset=utf-8");
//! # Ok(())
//! # }
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]
pub mod bootstrap;
pub mod bridge;
pub mod deferred;
pub mod drag;
pub mod error;
pub mod keeper;
pub mod lease;
pub mod mime;
pub mod payload;
pub mod protocol;
pub mod runtime;
pub mod transfer;

pub use error::ClipboardError;
pub use lease::DisplayLease;
pub use mime::{Kind, MimeType};
pub use payload::{
    FileIntent, FileTransfer, FileUriRejection, MAX_FILE_URI_BYTES, MAX_FILE_URI_COUNT, Pasted,
    Payload, validate_file_uri,
};
pub use protocol::{SeatSelector, SeatStatus};
pub use runtime::WaylandClipboard;
pub use transfer::TransferLimits;
