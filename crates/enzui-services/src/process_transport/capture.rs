//! Bounded nonblocking pipe capture for process workers.

use super::{POLL_INTERVAL, ReadOutcome, ReadResult, WorkerStop, bounded_diagnostic};
use std::io::{self, Read};
use std::thread;

pub(super) fn read_bounded(
    mut reader: impl Read,
    limit: usize,
    stop: &WorkerStop,
    mut on_bytes: impl FnMut(&[u8]),
    mut on_would_block: impl FnMut(),
) -> ReadResult {
    let retained_limit = limit + 1;
    let mut result = ReadResult {
        bytes: Vec::with_capacity(retained_limit.min(8192)),
        outcome: ReadOutcome::Stopped,
    };
    let mut chunk = WipingChunk([0_u8; 8192]);
    loop {
        if stop.requested() {
            return result;
        }
        let remaining = retained_limit - result.bytes.len();
        if remaining == 0 {
            result.outcome = ReadOutcome::Overflow;
            return result;
        }
        let requested = remaining.min(chunk.len());
        match reader.read(&mut chunk[..requested]) {
            Ok(0) => {
                result.outcome = ReadOutcome::Complete;
                return result;
            }
            Ok(count) => {
                if result.bytes.capacity() - result.bytes.len() < count {
                    let capacity = (result.bytes.capacity().saturating_mul(2))
                        .max(result.bytes.len() + count)
                        .min(retained_limit);
                    let mut grown = Vec::with_capacity(capacity);
                    grown.extend_from_slice(&result.bytes);
                    drop(enzui_security::secret::Secret::new(std::mem::replace(
                        &mut result.bytes,
                        grown,
                    )));
                }
                result.bytes.extend_from_slice(&chunk[..count]);
                on_bytes(&chunk[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // Nonblocking checkpoints let teardown join a reader even when an escaped process retains the writer.
                on_would_block();
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                result.outcome =
                    ReadOutcome::Failed(bounded_diagnostic(format!("read pipe: {error}")));
                return result;
            }
        }
        if result.bytes.len() > limit {
            result.outcome = ReadOutcome::Overflow;
            return result;
        }
    }
}

struct WipingChunk([u8; 8192]);
impl std::ops::Deref for WipingChunk {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        &self.0
    }
}
impl std::ops::DerefMut for WipingChunk {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}
impl Drop for WipingChunk {
    fn drop(&mut self) {
        enzui_security::secret::wipe_bytes(&mut self.0);
    }
}
