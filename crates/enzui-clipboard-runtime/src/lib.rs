//! Adopts the iced event loop's existing Wayland connection for clipboard access.
//!
//! Call [`adopt_process_clipboard`] before starting the event loop. The default
//! policy installs the ordinary Wayland transport; set
//! [`enzui_clipboard::bootstrap::POLICY_VARIABLE`] to `default` or call
//! [`adopt_process_clipboard_with`] to keep the runtime transport. An adopted
//! outcome means the provider was installed, not that a display was connected
//! or a transfer succeeded.
//!
//! The provider connects once per event loop. Non-Wayland displays retain the
//! runtime's default clipboard. No additional Wayland client is opened.
//! [`native_drag`] exposes native source tasks and destination subscriptions
//! through that same provider after the application window opens.
//!
//! The optional `image` feature supports PNG conversion; text, HTML and local
//! file paths work without it. PNG dimensions and encoded size are checked
//! against [`enzui_core::decode_limits`] before allocating pixels.
//!
//! Transfers can outlive backend teardown. The worker transport and dispatch
//! thread retain the event loop's display lease until they stop using it.

#![cfg_attr(docsrs, feature(doc_cfg))]
mod adapter;
mod file_transfer_async;
pub mod native_drag;

pub use file_transfer_async::{read_file_transfer_async, write_file_transfer_async};

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender as MpscSender};
use std::time::Duration;

use enzui_clipboard::bootstrap::{AdoptionSeal, InstallRefusal, Installer, Outcome, Policy};
use enzui_clipboard::bridge::{BridgeWorker, ClipboardTransport, Job};
use enzui_clipboard::error::ClipboardError;
use enzui_clipboard::lease::DisplayLease;
use enzui_clipboard::mime::MimeType;
use enzui_clipboard::payload::{Pasted, Payload};
use enzui_clipboard::{TransferLimits, WaylandClipboard};
use enzui_core::decode_limits::DecodeLimits;
use iced_core::clipboard::{Content, Kind};
use iced_winit::clipboard::platform::{Backend, Connection, Provider};
use iced_winit::winit::event_loop::OwnedDisplayHandle;
use iced_winit::winit::raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

/// Offers protected text on the adopted Wayland clipboard with a history-exclusion hint and expiry.
///
/// The lifetime is positive and at most 120 seconds; text is limited to 64 KiB.
/// Use from a trusted focused activation. No ordinary text action, keeper,
/// helper process or additional display connection is used. Protected ownership
/// survives queueing and transfer, then wipes when its last holder drops.
/// Revocation stops blocked transfers within a 50 ms poll interval. A compositor
/// or peer that retained its own copy can still serve it after source expiry.
///
/// Returns `None` if the bridge is unavailable, admission fails, the budget
/// expires or its reply is dropped. `Some(Ok(()))` confirms ownership; focus,
/// lifetime and transport refusals remain errors. A request already started
/// can complete after the response budget; the source expires independently.
/// Dropping the future skips a queued request that has not started.
pub async fn write_sensitive_text_async(
    text: enzui_security::secret::SecretText,
    lifetime: Duration,
    budget: Duration,
) -> Option<Result<(), ClipboardError>> {
    let payload = match Payload::sensitive_text(text, lifetime) {
        Ok(payload) => payload,
        Err(error) => return Some(Err(error)),
    };
    file_transfer_async::request(
        file_transfer_async::registered_sender(),
        budget,
        move |transport| transport.offer(payload),
    )
    .await
}

/// Claim an ordinary, focused text selection before asynchronously obtaining its bytes.
///
/// `session` owns pending request generations; invalidating it rejects stale replies.
/// A successful result returns a one-shot fulfiller after native ownership confirmation.
/// Fulfil after the remote request completes, even if focus has since left. Dropping
/// the fulfiller or this future cancels pending work; no placeholder bytes, exit keeper
/// or unfocused selection claim are used. The source lifetime includes preparation
/// and native claim time. Transfer deadlines and byte caps still apply to each reader.
///
/// Returns `None` when the bridge is unavailable, saturated, cancelled or out of time;
/// native claim and policy refusals remain errors. An already started claim may briefly
/// take ownership after cancellation, but its cancelled source cannot serve future text.
pub fn reserve_deferred_text_async(
    session: &enzui_clipboard::deferred::DeferredTextSession,
    limits: enzui_clipboard::deferred::DeferredTextLimits,
    budget: Duration,
) -> impl std::future::Future<
    Output = Option<Result<enzui_clipboard::deferred::DeferredText, ClipboardError>>,
>
+ Send
+ 'static
+ use<> {
    let prepared = session.prepare(limits);
    async move {
        let (payload, fulfil) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => return Some(Err(error)),
        };
        match file_transfer_async::request(
            file_transfer_async::registered_sender(),
            budget,
            move |transport| transport.offer(payload),
        )
        .await
        {
            Some(Ok(())) => Some(Ok(fulfil)),
            Some(Err(error)) => Some(Err(error)),
            None => None,
        }
    }
}

/// Hands what this process still has on the clipboard to a keeper, so it can
/// be pasted after the process exits (see `enzui_clipboard::keeper`).
/// `app_setup::run` calls it when the event loop ends; `Ok(false)` when
/// nothing needed keeping.
///
/// # Errors
/// The keeper could not be started.
pub fn keep_clipboard_after_exit() -> std::io::Result<bool> {
    enzui_clipboard::keeper::hand_off()
}

/// Applies the shared startup policy, installing this clipboard by default.
///
/// Safe to call from any application's `main`, including on non-Wayland
/// sessions where the runtime keeps its existing clipboard. The [`Outcome`]
/// is returned rather than logged so the caller decides how to surface it.
pub fn adopt_process_clipboard() -> Outcome {
    adopt_process_clipboard_with(Policy::FromEnvironment)
}

/// Applies an explicit startup policy before the event loop starts.
///
/// The first successful explicit or environment choice remains effective when
/// a later automatic startup call runs. An adopted outcome confirms provider
/// installation; connection and transfer errors are reported later by the
/// runtime clipboard.
pub fn adopt_process_clipboard_with(policy: Policy) -> Outcome {
    enzui_clipboard::bootstrap::install(&RuntimeInstaller::default(), policy)
}

#[derive(Debug, Default)]
struct RuntimeInstaller {
    limits: TransferLimits,
}

impl Installer for RuntimeInstaller {
    fn install(&self, seal: AdoptionSeal) -> Result<(), InstallRefusal> {
        let provider = WaylandProvider {
            limits: self.limits,
            seal,
        };

        if iced_winit::clipboard::platform::set_provider(provider) {
            Ok(())
        } else {
            Err(InstallRefusal::AlreadyInstalled)
        }
    }

    fn describe(&self) -> &str {
        "iced_winit wayland provider"
    }
}

/// Supplies the Wayland clipboard for a Wayland display.
///
/// Installed through [`adopt_process_clipboard`] or
/// [`adopt_process_clipboard_with`]; direct construction is unavailable.
pub struct WaylandProvider {
    limits: TransferLimits,
    seal: AdoptionSeal,
}

impl Provider for WaylandProvider {
    fn connect(&self, display: OwnedDisplayHandle) -> Connection {
        let pointer = {
            let Ok(handle) = display.display_handle() else {
                return Connection::Unsupported;
            };

            let RawDisplayHandle::Wayland(wayland) = handle.as_raw() else {
                return Connection::Unsupported;
            };

            wayland.display
        };

        let guest = guest_of(&display);
        let lease = DisplayLease::new((display, guest.clone()));

        let adopted = match adopt_on_guest(guest.as_ref(), self.limits, &self.seal, &lease) {
            Some(adopted) => adopted,
            // SAFETY: the pointer comes from the event loop's live
            // `wl_display`, and the lease shares the handle that keeps that
            // display open, so every thread dispatching on the adopted
            // connection holds the display alive for its own whole life.
            #[allow(unsafe_code, reason = "adopts the toolkit's Wayland display")]
            None => unsafe {
                WaylandClipboard::adopt_foreign_display(
                    pointer,
                    self.limits,
                    &self.seal,
                    lease.clone(),
                )
            },
        };

        match adopted {
            Ok(clipboard) => match AdoptedConnection::new(clipboard, lease, self.limits) {
                Ok(adopted) => Connection::Adopted(Box::new(WaylandIcedBackend { adopted })),
                Err(error) => Connection::Refused(adapter::translate(&error)),
            },
            Err(error) => Connection::Refused(adapter::translate(&error)),
        }
    }
}

#[cfg(feature = "wayland")]
type Guest = std::sync::Arc<iced_winit::guest::GuestConnection>;
#[cfg(not(feature = "wayland"))]
type Guest = ();

#[cfg(feature = "wayland")]
fn guest_of(display: &OwnedDisplayHandle) -> Option<Guest> {
    iced_winit::guest::GuestConnection::of(display)
}

#[cfg(not(feature = "wayland"))]
fn guest_of(_display: &OwnedDisplayHandle) -> Option<Guest> {
    None
}

/// Starts the clipboard on the display's guest connection, whose reader
/// routes the clipboard's events to it, so the clipboard worker wakes for
/// its own events only.
#[cfg(feature = "wayland")]
fn adopt_on_guest(
    guest: Option<&Guest>,
    limits: TransferLimits,
    seal: &AdoptionSeal,
    lease: &DisplayLease,
) -> Option<Result<WaylandClipboard, ClipboardError>> {
    let guest = guest?;
    Some(WaylandClipboard::adopt_read_elsewhere(
        guest.connection().clone(),
        limits,
        seal,
        lease.clone(),
    ))
}

#[cfg(not(feature = "wayland"))]
fn adopt_on_guest(
    _guest: Option<&Guest>,
    _limits: TransferLimits,
    _seal: &AdoptionSeal,
    _lease: &DisplayLease,
) -> Option<Result<WaylandClipboard, ClipboardError>> {
    None
}

// A bridge only ever clears the slot for its own generation, so a replaced
// bridge cannot deregister its successor.
static GENERATION: AtomicU64 = AtomicU64::new(0);

struct Registration {
    sender: MpscSender<Job>,
    generation: u64,
}

// Queries may clone this sender; teardown must release the registered clone
// before waiting for the worker to stop.
static BRIDGE_SLOT: Mutex<Option<Registration>> = Mutex::new(None);

fn register_bridge(sender: MpscSender<Job>) -> u64 {
    let next = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
    let mut slot = BRIDGE_SLOT
        .lock()
        .expect("bridge slot lock is not poisoned");
    *slot = Some(Registration {
        sender,
        generation: next,
    });
    next
}

fn deregister_bridge(generation: u64) {
    let mut slot = BRIDGE_SLOT
        .lock()
        .expect("bridge slot lock is not poisoned");
    if slot.as_ref().is_some_and(|r| r.generation == generation) {
        *slot = None;
    }
}

/// Whether the adopted clipboard currently sees keyboard focus on any seat.
///
/// Returns `None` if the registration is unavailable, queue admission fails,
/// the reply exceeds `budget`, or the transport cannot report focus. The budget
/// bounds reply waiting, not mutex acquisition; timeout does not cancel the job.
pub fn holds_keyboard_focus(budget: Duration) -> Option<bool> {
    let sender = {
        let slot = BRIDGE_SLOT.lock().ok()?;
        slot.as_ref().map(|r| r.sender.clone())?
    };
    let (tx, rx) = mpsc::channel();

    let job: Job = Box::new(move |transport| {
        let focused = transport
            .ok()
            .and_then(|transport| transport.has_focus().ok());
        let _ = tx.send(focused);
    });

    sender.try_send(job).ok()?;
    rx.recv_timeout(budget).ok()?
}

/// Reads the clipboard file selection with copy/cut intent.
///
/// When the current selection carries `x-special/gnome-copied-files` the
/// returned [`enzui_clipboard::FileTransfer`] preserves copy/cut intent.
/// A plain `text/uri-list` selection has
/// [`enzui_clipboard::FileIntent::Copy`]; cut is never inferred from a
/// format that cannot express it.
///
/// Individual URIs are validated: remote hosts, non-file schemes, malformed
/// percent encoding, NUL/control characters, and relative paths are excluded.
/// Duplicates are removed.
///
/// Returns `None` if registration or queue admission fails, or no reply arrives
/// within `budget`. The budget bounds reply waiting, not mutex acquisition;
/// timeout does not cancel the job. The inner result reports transport errors
/// and selections without usable local files.
pub fn read_file_transfer(
    budget: Duration,
) -> Option<Result<enzui_clipboard::FileTransfer, ClipboardError>> {
    let sender = {
        let slot = BRIDGE_SLOT.lock().ok()?;
        slot.as_ref().map(|r| r.sender.clone())?
    };
    let (tx, rx) = mpsc::channel();

    let preferences = adapter::preferences_for(Kind::Files);
    let job: Job = Box::new(move |transport| {
        let result = transport.and_then(|transport| {
            let pasted = transport.paste(&preferences)?;
            pasted.file_transfer().ok_or(ClipboardError::NoSelection)
        });
        let _ = tx.send(result);
    });

    sender.try_send(job).ok()?;
    rx.recv_timeout(budget).ok()
}

/// Writes a file transfer with copy/cut intent to the clipboard.
///
/// The payload advertises both `x-special/gnome-copied-files` (with the
/// intent header) and `text/uri-list` (without). Unrepresentable paths and
/// duplicates are omitted; use [`write_file_transfer_async`] to require the
/// entire selection to be representable.
///
/// Returns `None` if registration or queue admission fails, or no reply arrives
/// within `budget`. The budget bounds reply waiting, not mutex acquisition or
/// payload encoding. Timeout does not cancel the offer, so `None` does not
/// prove that the clipboard was unchanged.
pub fn write_file_transfer(
    transfer: &enzui_clipboard::FileTransfer,
    budget: Duration,
) -> Option<Result<(), ClipboardError>> {
    let sender = {
        let slot = BRIDGE_SLOT.lock().ok()?;
        slot.as_ref().map(|r| r.sender.clone())?
    };
    let (tx, rx) = mpsc::channel();

    let payload = Payload::file_transfer(transfer.paths(), transfer.intent());
    let job: Job = Box::new(move |transport| {
        let result = transport.and_then(|transport| transport.offer(payload));
        let _ = tx.send(result);
    });

    sender.try_send(job).ok()?;
    rx.recv_timeout(budget).ok()
}

struct RegisteredBridge {
    generation: u64,
    worker: BridgeWorker,
}

impl RegisteredBridge {
    fn new(transport: impl ClipboardTransport) -> Result<Self, ClipboardError> {
        let worker = BridgeWorker::start(transport)?;

        let generation = match worker.sender() {
            Some(sender) => register_bridge(sender),
            None => 0,
        };

        Ok(Self { generation, worker })
    }
}

impl Drop for RegisteredBridge {
    fn drop(&mut self) {
        // Release the registered sender before the worker waits for shutdown.
        deregister_bridge(self.generation);
    }
}

struct LeasedTransport<T> {
    transport: T,
    // Declared after `transport`, so a borrowed display outlives the transfers
    // and worker teardown that dispatch on it.
    _lease: DisplayLease,
}

impl<T: ClipboardTransport> ClipboardTransport for LeasedTransport<T> {
    fn paste(&self, preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
        self.transport.paste(preferences)
    }

    fn offer(&self, payload: Payload) -> Result<(), ClipboardError> {
        self.transport.offer(payload)
    }

    fn has_focus(&self) -> Result<bool, ClipboardError> {
        self.transport.has_focus()
    }

    fn native_drag(&self) -> Option<enzui_clipboard::drag::NativeDragClient> {
        self.transport.native_drag()
    }

    fn paste_primary(&self, preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
        self.transport.paste_primary(preferences)
    }

    fn offer_primary(&self, payload: Payload) -> Result<(), ClipboardError> {
        self.transport.offer_primary(payload)
    }
}

// The worker owns the lease because its 500 ms join budget may expire
// while a transfer still uses the display.
struct AdoptedConnection {
    bridge: RegisteredBridge,
    paste_limits: DecodeLimits,
}

impl AdoptedConnection {
    fn new<T: ClipboardTransport>(
        transport: T,
        lease: DisplayLease,
        limits: TransferLimits,
    ) -> Result<Self, ClipboardError> {
        Ok(Self {
            bridge: RegisteredBridge::new(LeasedTransport {
                transport,
                _lease: lease,
            })?,
            paste_limits: adapter::paste_decode_limits(&limits),
        })
    }

    fn worker(&self) -> &BridgeWorker {
        &self.bridge.worker
    }

    fn read(
        &self,
        primary: bool,
        kind: Kind,
        callback: Box<dyn FnOnce(Result<Content, iced_core::clipboard::Error>) + Send>,
    ) {
        let preferences = adapter::preferences_for(kind);
        let limits = self.paste_limits;
        let callback = Box::new(move |result: Result<Pasted, ClipboardError>| {
            callback(match result {
                Ok(pasted) => adapter::map_pasted(pasted, kind, &limits),
                Err(error) => Err(adapter::translate(&error)),
            });
        });

        if primary {
            self.worker().read_primary(preferences, callback);
        } else {
            self.worker().read(preferences, callback);
        }
    }

    fn write(
        &self,
        primary: bool,
        content: Content,
        callback: Box<dyn FnOnce(Result<(), iced_core::clipboard::Error>) + Send>,
    ) {
        // Admitted first, converted on the worker: encoding an image or
        // flattening HTML never runs on the caller's (event loop's) thread,
        // nor for a copy the full queue refuses.
        self.worker().submit(Box::new(move |transport| {
            let result = transport
                .map_err(|error| adapter::translate(&error))
                .and_then(|transport| {
                    let payload = adapter::map_content(content)?;
                    let offered = if primary {
                        transport.offer_primary(payload)
                    } else {
                        transport.offer(payload)
                    };
                    offered.map_err(|error| adapter::translate(&error))
                });
            callback(result);
        }));
    }
}

struct WaylandIcedBackend {
    adopted: AdoptedConnection,
}

impl Backend for WaylandIcedBackend {
    fn read(
        &self,
        kind: Kind,
        callback: Box<dyn FnOnce(Result<Content, iced_core::clipboard::Error>) + Send>,
    ) {
        self.adopted.read(false, kind, callback);
    }

    fn write(
        &mut self,
        content: Content,
        callback: Box<dyn FnOnce(Result<(), iced_core::clipboard::Error>) + Send>,
    ) {
        self.adopted.write(false, content, callback);
    }

    fn read_primary(
        &self,
        kind: Kind,
        callback: Box<dyn FnOnce(Result<Content, iced_core::clipboard::Error>) + Send>,
    ) {
        self.adopted.read(true, kind, callback);
    }

    fn write_primary(
        &mut self,
        content: Content,
        callback: Box<dyn FnOnce(Result<(), iced_core::clipboard::Error>) + Send>,
    ) {
        self.adopted.write(true, content, callback);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    const FIXTURE_BUDGET: Duration = Duration::from_secs(5);

    #[test]
    fn sensitive_tasks_redact_and_report_native_transport_refusal() {
        use enzui_security::secret::{SecretText, observer};
        use futures::{StreamExt, executor::block_on};
        const TEXT: &str = "task-fixture-not-a-real-password";
        let _guard = lock_and_clear_slot();
        observer::reset();
        let task = iced_runtime::Task::perform(
            write_sensitive_text_async(
                SecretText::new(TEXT.to_owned()),
                Duration::from_secs(2),
                FIXTURE_BUDGET,
            ),
            |_| (),
        );
        assert!(!format!("{task:?}").contains(TEXT));
        assert_eq!(observer::snapshot().wiped_bytes, 0);
        drop(task);
        assert!(observer::snapshot().wiped_bytes >= TEXT.len());
        assert_eq!(
            observer::snapshot().wiped_bytes,
            observer::snapshot().zeroed_bytes
        );

        struct Refusing;
        impl ClipboardTransport for Refusing {
            fn paste(&self, _: &[MimeType]) -> Result<Pasted, ClipboardError> {
                Err(ClipboardError::NotFocused)
            }
            fn has_focus(&self) -> Result<bool, ClipboardError> {
                Ok(false)
            }
            fn offer(&self, payload: Payload) -> Result<(), ClipboardError> {
                assert!(payload.is_sensitive());
                assert!(!format!("{payload:?}").contains(TEXT));
                assert!(
                    payload
                        .bytes_for(&MimeType::new("text/plain;charset=utf-8"))
                        .is_none()
                );
                Err(ClipboardError::NotFocused)
            }
        }
        let connection = AdoptedConnection::new(Refusing, plain_lease(), TransferLimits::default())
            .expect("bridge");
        let task = iced_runtime::Task::perform(
            write_sensitive_text_async(
                SecretText::new(TEXT.to_owned()),
                Duration::from_secs(2),
                FIXTURE_BUDGET,
            ),
            |result| result,
        );
        let actions = block_on(
            iced_runtime::task::into_stream(task)
                .expect("task stream")
                .collect::<Vec<_>>(),
        );
        assert!(matches!(
            actions.as_slice(),
            [iced_runtime::Action::Output(Some(Err(
                ClipboardError::NotFocused
            )))]
        ));
        drop(connection);
    }

    // Serialises the tests that share the process-global `BRIDGE_SLOT`.
    static SLOT_GUARD: Mutex<()> = Mutex::new(());

    fn lock_and_clear_slot() -> std::sync::MutexGuard<'static, ()> {
        let guard = SLOT_GUARD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *BRIDGE_SLOT.lock().expect("bridge slot") = None;
        guard
    }

    fn plain_lease() -> DisplayLease {
        DisplayLease::new(String::from("display handle"))
    }

    // Exercises the constructor `WaylandProvider::connect` uses, so the drop
    // path under test is the production one.

    struct GnomeCopyTransport;

    impl ClipboardTransport for GnomeCopyTransport {
        fn paste(&self, _preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
            Ok(Pasted::new(
                MimeType::new("x-special/gnome-copied-files"),
                b"copy\nfile:///tmp/file.txt\nfile:///home/user/docs\n".to_vec(),
            ))
        }
        fn offer(&self, _payload: Payload) -> Result<(), ClipboardError> {
            Ok(())
        }
        fn has_focus(&self) -> Result<bool, ClipboardError> {
            Ok(true)
        }
    }

    /// Converting a copy (here, encoding an image) happens on the bridge
    /// worker after admission, not on the thread that asked to write: even a
    /// conversion failure is reported from the worker.
    #[cfg(feature = "image")]
    #[test]
    fn a_write_converts_on_the_worker_not_the_caller() {
        let _guard = lock_and_clear_slot();
        let connection =
            AdoptedConnection::new(GnomeCopyTransport, plain_lease(), TransferLimits::default())
                .expect("bridge starts");
        let (sender, receiver) = mpsc::channel();
        connection.write(
            false,
            Content::Image(iced_core::clipboard::Image {
                rgba: iced_core::Bytes::from_static(&[0, 0, 0]),
                size: iced_core::Size::new(4, 4),
            }),
            Box::new(move |result| {
                let _ = sender.send((std::thread::current().id(), result.is_ok()));
            }),
        );
        let (thread, ok) = receiver
            .recv_timeout(FIXTURE_BUDGET)
            .expect("the write completes");
        assert!(!ok, "pixels that do not fit the size fail to convert");
        assert_ne!(
            thread,
            std::thread::current().id(),
            "converted on the caller"
        );
    }

    #[test]
    fn read_file_transfer_returns_copy_intent() {
        let _guard = lock_and_clear_slot();
        let _adopted =
            AdoptedConnection::new(GnomeCopyTransport, plain_lease(), TransferLimits::default())
                .expect("bridge starts");

        let ft = crate::read_file_transfer(FIXTURE_BUDGET)
            .expect("bridge exists")
            .expect("paste succeeds");

        assert_eq!(
            ft.intent(),
            enzui_clipboard::FileIntent::Copy,
            "a GNOME copy selection must report Copy intent"
        );
        assert_eq!(
            ft.paths(),
            &[
                std::path::PathBuf::from("/tmp/file.txt"),
                std::path::PathBuf::from("/home/user/docs")
            ]
        );
    }
}
