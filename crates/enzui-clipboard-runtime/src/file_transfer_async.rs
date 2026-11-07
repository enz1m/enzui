use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use enzui_clipboard::bridge::{ClipboardTransport, Job};
use enzui_clipboard::{ClipboardError, FileTransfer, MimeType, Pasted, Payload};
use futures::channel::oneshot;
use futures::future::{Either, select};

/// Reads validated local paths and copy/cut intent on the bridge worker.
///
/// Uses the installed bridge and the same MIME preference and URI validation as
/// [`crate::read_file_transfer`]. The budget includes queueing and transport time.
/// Returns `None` if no bridge is installed, admission fails, the budget expires,
/// or the bridge drops the reply. Transport refusals remain in the inner result.
/// Cancellation is checked before the worker starts the request. A transport
/// call already started may continue after cancellation or timeout. Acquiring
/// the bridge registration uses a mutex and is outside the timeout budget.
pub async fn read_file_transfer_async(
    budget: Duration,
) -> Option<Result<FileTransfer, ClipboardError>> {
    read_from(registered_sender(), budget).await
}

/// Offers local files with copy/cut intent on the bridge worker.
///
/// Both GNOME copied-files and plain URI-list formats are published through the
/// installed bridge. Admission, timeout and refusal results match
/// [`read_file_transfer_async`]. Dropped or expired queued offers are skipped.
/// An offer already started may finish after timeout or cancellation; `None`
/// therefore does not prove the clipboard was unchanged.
/// Invalid, duplicated, empty, or over-limit selections are refused rather
/// than silently publishing only their representable subset.
pub async fn write_file_transfer_async(
    transfer: FileTransfer,
    budget: Duration,
) -> Option<Result<(), ClipboardError>> {
    write_to(registered_sender(), transfer, budget).await
}

pub(crate) fn registered_sender() -> Option<SyncSender<Job>> {
    let slot = super::BRIDGE_SLOT.lock().ok()?;
    slot.as_ref()
        .map(|registration| registration.sender.clone())
}

async fn read_from(
    sender: Option<SyncSender<Job>>,
    budget: Duration,
) -> Option<Result<FileTransfer, ClipboardError>> {
    request(sender, budget, |transport| {
        let preferences = super::adapter::preferences_for(iced_core::clipboard::Kind::Files);
        transport
            .paste(&preferences)?
            .file_transfer()
            .ok_or(ClipboardError::NoSelection)
    })
    .await
}

async fn write_to(
    sender: Option<SyncSender<Job>>,
    transfer: FileTransfer,
    budget: Duration,
) -> Option<Result<(), ClipboardError>> {
    request(sender, budget, move |transport| {
        if transfer.paths().len() > enzui_clipboard::payload::MAX_FILE_URI_COUNT {
            return Err(ClipboardError::InvalidFileSelection);
        }
        let payload = Payload::file_transfer(transfer.paths(), transfer.intent());
        let mime = MimeType::new("x-special/gnome-copied-files");
        let normalized = payload
            .bytes_for(&mime)
            .and_then(|bytes| Pasted::new(mime, bytes.to_vec()).file_transfer());
        if normalized.as_ref() != Some(&transfer) {
            return Err(ClipboardError::InvalidFileSelection);
        }
        transport.offer(payload)
    })
    .await
}

pub(crate) async fn request<T: Send + 'static>(
    sender: Option<SyncSender<Job>>,
    budget: Duration,
    work: impl FnOnce(&dyn ClipboardTransport) -> Result<T, ClipboardError> + Send + 'static,
) -> Option<Result<T, ClipboardError>> {
    let sender = sender?;
    let deadline = Instant::now().checked_add(budget)?;
    let (tx, rx) = oneshot::channel();
    let job: Job = Box::new(move |transport| {
        if tx.is_canceled() || Instant::now() >= deadline {
            return;
        }
        let _ = tx.send(transport.and_then(work));
    });
    sender.try_send(job).ok()?;
    match select(
        rx,
        futures_timer::Delay::new(deadline.saturating_duration_since(Instant::now())),
    )
    .await
    {
        Either::Left((result, _)) => result.ok(),
        Either::Right(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_clipboard::bridge::BridgeWorker;
    use enzui_clipboard::{FileIntent, MimeType, Pasted};
    use futures::{FutureExt, executor::block_on};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, mpsc};

    const BUDGET: Duration = Duration::from_secs(2);

    #[derive(Clone)]
    struct Transport {
        offered: Arc<Mutex<Vec<Payload>>>,
        paste: Result<Pasted, ClipboardError>,
    }

    impl ClipboardTransport for Transport {
        fn paste(&self, preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
            assert_eq!(
                preferences[0],
                MimeType::new("x-special/gnome-copied-files")
            );
            self.paste.clone()
        }
        fn offer(&self, payload: Payload) -> Result<(), ClipboardError> {
            self.offered
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(payload);
            Ok(())
        }
        fn has_focus(&self) -> Result<bool, ClipboardError> {
            Ok(true)
        }
    }

    fn fixture() -> (BridgeWorker, Arc<Mutex<Vec<Payload>>>) {
        let offered = Arc::new(Mutex::new(Vec::new()));
        let transport = Transport {
            offered: Arc::clone(&offered),
            paste: Ok(Pasted::new(
                MimeType::new("x-special/gnome-copied-files"),
                b"cut\nfile:///tmp/file%20one\nfile://foreign/tmp/no\nfile:///tmp/folder\n"
                    .to_vec(),
            )),
        };
        (BridgeWorker::start(transport).unwrap(), offered)
    }

    fn transfer() -> FileTransfer {
        FileTransfer::new(
            FileIntent::Cut,
            vec![PathBuf::from("/tmp/file one"), PathBuf::from("/tmp/folder")],
        )
    }

    #[test]
    fn timeout_does_not_claim_to_undo_an_offer_already_started() {
        let (worker, offered) = fixture();
        let (started, wait_started) = mpsc::sync_channel(1);
        let (release, wait_release) = mpsc::sync_channel(1);
        let mut pending = Box::pin(request(
            worker.sender(),
            Duration::from_millis(50),
            move |transport| {
                started.send(()).unwrap();
                let _ = wait_release.recv_timeout(Duration::from_secs(5));
                let transfer = transfer();
                transport.offer(Payload::file_transfer(transfer.paths(), transfer.intent()))
            },
        ));
        assert!(pending.as_mut().now_or_never().is_none());
        wait_started.recv_timeout(BUDGET).unwrap();
        assert_eq!(block_on(pending), None);
        release.send(()).unwrap();
        assert_eq!(
            block_on(write_to(worker.sender(), transfer(), BUDGET)),
            Some(Ok(()))
        );
        assert_eq!(offered.lock().unwrap().len(), 2);
    }
}
