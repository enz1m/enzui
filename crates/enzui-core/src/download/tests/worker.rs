use super::*;

use std::sync::Mutex;

#[derive(Debug)]
struct ScriptedTransport {
    meta: TransportResponseMeta,
    chunks: Mutex<Vec<Result<BodyChunk, TransportError>>>,
    read_count: std::sync::atomic::AtomicU64,
}

impl ScriptedTransport {
    fn new(meta: TransportResponseMeta, chunks: Vec<Result<BodyChunk, TransportError>>) -> Self {
        Self {
            meta,
            chunks: Mutex::new(chunks),
            read_count: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn chunked(meta: TransportResponseMeta, n: usize, chunk_size: usize) -> Self {
        let mut chunks: Vec<Result<BodyChunk, TransportError>> = (0..n)
            .map(|_| Ok(BodyChunk::Bytes(vec![0u8; chunk_size])))
            .collect();
        chunks.push(Ok(BodyChunk::Done));
        Self::new(meta, chunks)
    }
}

impl DownloadTransport for ScriptedTransport {
    fn start(
        &self,
        _url: &str,
        _plan: Option<&RangeRequestPlan>,
        cancel: &CancelSignal,
    ) -> Result<TransportResponseMeta, TransportError> {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        Ok(self.meta.clone())
    }

    fn read_chunk(&self, cancel: &CancelSignal) -> Result<BodyChunk, TransportError> {
        if cancel.is_cancelled() {
            return Err(TransportError::Cancelled);
        }
        self.read_count
            .fetch_add(1, std::sync::atomic::Ordering::Release);
        let mut chunks = self
            .chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if chunks.is_empty() {
            Ok(BodyChunk::Done)
        } else {
            chunks.remove(0)
        }
    }
}

fn ok_200_meta() -> TransportResponseMeta {
    TransportResponseMeta {
        status: 200,
        content_range: None,
        response_validator: None,
        content_length: Some(1000),
        content_encoding: None,
    }
}

#[test]
fn worker_completes_a_normal_download_and_emits_progress_then_finished() {
    let transport = ScriptedTransport::chunked(ok_200_meta(), 4, 250);
    let cancel = CancelSignal::new();
    let mut events = Vec::new();

    run_download(
        &transport,
        "http://example.com/file",
        DownloadId(1),
        Attempt::FIRST,
        None,
        &cancel,
        |e| events.push(e),
    );

    assert!(
        !events.is_empty(),
        "the worker must emit at least one event"
    );

    assert!(
        matches!(&events[0], DownloadEvent::ResponseMeta { id, .. } if *id == DownloadId(1)),
        "first event must be ResponseMeta"
    );

    let progress_count = events
        .iter()
        .filter(|e| matches!(e, DownloadEvent::Progress { .. }))
        .count();
    assert!(
        progress_count > 0,
        "the worker must emit progress events; got 0"
    );

    let terminal_count = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                DownloadEvent::Finished { .. } | DownloadEvent::Failed { .. }
            )
        })
        .count();
    assert_eq!(
        terminal_count, 1,
        "exactly one terminal event; got {terminal_count}"
    );
    assert!(
        matches!(events.last(), Some(DownloadEvent::Finished { id, attempt })
            if *id == DownloadId(1) && *attempt == Attempt::FIRST),
        "last event must be Finished with the right id and attempt"
    );
}
