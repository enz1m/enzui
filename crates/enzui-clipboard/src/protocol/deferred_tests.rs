use super::*;
use crate::deferred::{DeferredTextLimits, DeferredTextSession};
use std::io::Read;

fn wait_for_reader(payload: &Payload) {
    let until = Instant::now() + Duration::from_secs(1);
    while !payload
        .deferred_source()
        .unwrap()
        .waiting
        .load(Ordering::Acquire)
    {
        assert!(
            Instant::now() < until,
            "transfer must await real deferred bytes"
        );
        std::thread::yield_now();
    }
}

#[test]
fn deferred_source_serves_waiting_readers_and_bounds_concurrent_transfers() {
    let _selection = crate::keeper::TEST_SELECTION.lock().unwrap();
    let session = DeferredTextSession::default();
    let (payload, fulfil) = session
        .prepare(DeferredTextLimits {
            max_bytes: 128,
            lifetime: Duration::from_secs(5),
        })
        .unwrap();
    payload.confirm_deferred().unwrap();
    crate::keeper::remember(&payload);
    assert!(
        crate::keeper::owned_selection().is_none(),
        "deferred sources must remain owner-bound"
    );
    let count = Arc::new(AtomicUsize::new(0));
    let data = SourceData::new(
        payload.clone(),
        TransferLimits {
            max_concurrent_transfers: 1,
            ..TransferLimits::default()
        },
        Arc::clone(&count),
        false,
    );
    let mime = MimeType::new("text/plain;charset=utf-8");
    let transfer = data.transfer(&mime).expect("first transfer");
    assert!(
        data.transfer(&mime).is_none(),
        "pending readers must occupy the transfer quota"
    );
    let (read, write) = transfer_pipe().unwrap();
    let writer = std::thread::spawn(move || transfer.write(write));
    wait_for_reader(&payload);
    fulfil.fulfil("available later".into()).unwrap();
    let mut received = Vec::new();
    std::fs::File::from(read)
        .read_to_end(&mut received)
        .unwrap();
    writer.join().unwrap().unwrap();
    assert_eq!(received, b"available later");
    assert_eq!(
        count.load(Ordering::Acquire),
        0,
        "completed reader releases its slot"
    );
    assert!(
        data.transfer(&mime).is_some(),
        "same fulfilled selection serves later readers"
    );
}

#[test]
fn deferred_waits_stop_on_generation_cancel_and_peer_deadline() {
    for cancel in [false, true] {
        let session = DeferredTextSession::default();
        let (payload, fulfil) = session
            .prepare(DeferredTextLimits {
                max_bytes: 128,
                lifetime: Duration::from_secs(5),
            })
            .unwrap();
        payload.confirm_deferred().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let data = SourceData::new(
            payload.clone(),
            TransferLimits {
                send_timeout: Duration::from_millis(100),
                ..TransferLimits::default()
            },
            Arc::clone(&count),
            false,
        );
        let transfer = data
            .transfer(&MimeType::new("text/plain;charset=utf-8"))
            .unwrap();
        let (read, write) = transfer_pipe().unwrap();
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let writer = std::thread::spawn(move || {
            send.send(transfer.write(write)).unwrap();
        });
        wait_for_reader(&payload);
        if cancel {
            session.invalidate_pending();
        }
        let result = receive
            .recv_timeout(Duration::from_secs(1))
            .expect("waiting transfer must finish at its own deadline");
        if cancel {
            assert!(matches!(result, Err(ClipboardError::Stopped)));
        } else {
            assert!(matches!(
                result,
                Err(ClipboardError::TransferStalled { .. })
            ));
        }
        writer.join().unwrap();
        assert_eq!(count.load(Ordering::Acquire), 0);
        let mut received = Vec::new();
        std::fs::File::from(read)
            .read_to_end(&mut received)
            .unwrap();
        assert!(
            received.is_empty(),
            "failure closes the descriptor without placeholder bytes"
        );
        if cancel {
            assert!(fulfil.fulfil("stale".into()).is_err());
        } else {
            fulfil.fulfil("a later paste may retry".into()).unwrap();
        }
    }
}
