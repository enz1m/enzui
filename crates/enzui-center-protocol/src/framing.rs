//! `enzui-center` v2 message framing: `u32` big-endian length then one JSON value.
//!
//! Every frame is a 4-byte big-endian byte count followed by exactly that many
//! UTF-8 JSON bytes. A frame must carry [`MIN_FRAME_BYTES`]..=[`MAX_FRAME_BYTES`]
//! bytes; a declared length outside that range is refused before any allocation,
//! so a hostile length header cannot make the reader reserve attacker-chosen
//! memory. The codec performs blocking I/O over [`std::io`] traits and holds no
//! socket of its own.

use std::io::{self, Read, Write};

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The smallest legal frame body. A zero-length frame is refused.
pub const MIN_FRAME_BYTES: usize = 1;

/// The largest legal frame body, matching the daemon's 1 MiB bound.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Why a frame could not be read or written.
#[derive(Debug)]
pub enum WireError {
    /// The peer closed the connection at a frame boundary.
    PeerClosed,
    /// The stream ended or timed out partway through a frame, after `received`
    /// of its header and body bytes. The stream is out of step with the frame
    /// boundaries, so this is never retryable.
    Truncated {
        /// Header and body bytes read before the stream stopped.
        received: usize,
    },
    /// The declared or serialized body length was `0` or above
    /// [`MAX_FRAME_BYTES`]. Carries the offending length; no bytes are read past
    /// the header when the declared length is rejected.
    InvalidLength {
        /// The declared or serialized length in bytes.
        len: usize,
        /// The accepted maximum ([`MAX_FRAME_BYTES`]).
        cap: usize,
    },
    /// The frame body was not the expected UTF-8 JSON value. The underlying
    /// serde error is not surfaced, because it can echo attacker-controlled
    /// field names or enum tags.
    Malformed,
    /// An I/O error occurred. A read timeout before any byte of a frame
    /// surfaces here and is distinguished by [`WireError::is_timeout`].
    Io(io::Error),
}

impl WireError {
    /// Whether this error is a socket read/write timeout before any byte of a
    /// frame, so a polling loop can retry rather than treat the connection as
    /// broken. A timeout partway through a frame is [`WireError::Truncated`].
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        matches!(
            self,
            Self::Io(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                )
        )
    }
}

impl core::fmt::Display for WireError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::PeerClosed => write!(f, "peer closed the connection"),
            Self::Truncated { received } => {
                write!(f, "stream stopped {received} bytes into a frame")
            }
            Self::InvalidLength { len, cap } => {
                write!(f, "frame length {len} outside 1..={cap}")
            }
            Self::Malformed => write!(f, "malformed frame body"),
            Self::Io(error) => write!(f, "frame io failed: {error}"),
        }
    }
}

impl std::error::Error for WireError {}

impl From<io::Error> for WireError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Serializes `value` to JSON and encodes it as one length-prefixed frame.
///
/// # Errors
/// Returns [`WireError::InvalidLength`] when the serialized body is empty or
/// exceeds [`MAX_FRAME_BYTES`], or [`WireError::Malformed`] when serialization
/// fails.
pub fn encode_to_vec<T: Serialize>(value: &T) -> Result<Vec<u8>, WireError> {
    // Serialized straight behind its header; past the cap the bytes are only
    // counted, so an oversized body is measured but never held.
    let mut frame = FrameBuffer {
        bytes: vec![0; 4],
        len: 0,
    };
    serde_json::to_writer(&mut frame, value).map_err(|_| WireError::Malformed)?;
    if frame.len < MIN_FRAME_BYTES || frame.len > MAX_FRAME_BYTES {
        return Err(WireError::InvalidLength {
            len: frame.len,
            cap: MAX_FRAME_BYTES,
        });
    }
    let mut bytes = frame.bytes;
    bytes[..4].copy_from_slice(&(frame.len as u32).to_be_bytes());
    Ok(bytes)
}

/// A frame being serialized: the header slot and the body while it fits
/// [`MAX_FRAME_BYTES`], and the body's full length either way.
struct FrameBuffer {
    bytes: Vec<u8>,
    len: usize,
}

impl Write for FrameBuffer {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.len = self.len.saturating_add(buf.len());
        if self.len <= MAX_FRAME_BYTES {
            self.bytes.extend_from_slice(buf);
        } else if !self.bytes.is_empty() {
            self.bytes = Vec::new();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes one length-prefixed JSON frame and flushes it.
///
/// # Errors
/// Returns [`WireError::InvalidLength`] for an empty or oversized body,
/// [`WireError::Malformed`] for a serialization failure, or [`WireError::Io`].
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<(), WireError> {
    let frame = encode_to_vec(value)?;
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// Reads one length-prefixed JSON frame and decodes it as `T`.
///
/// The declared length is validated against [`MAX_FRAME_BYTES`] before the body
/// buffer is allocated. A clean end of stream at the frame boundary is reported
/// as [`WireError::PeerClosed`] rather than an I/O error; one partway through a
/// frame, or a timeout there, is [`WireError::Truncated`].
///
/// # Errors
/// Returns [`WireError::PeerClosed`], [`WireError::Truncated`], [`WireError::InvalidLength`],
/// [`WireError::Malformed`], or [`WireError::Io`] (including a read timeout,
/// which [`WireError::is_timeout`] identifies).
pub fn read_message<R: Read, T: DeserializeOwned>(reader: &mut R) -> Result<T, WireError> {
    let mut header = [0u8; 4];
    fill(reader, &mut header, 0)?;
    let len = u32::from_be_bytes(header) as usize;
    if !(MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&len) {
        return Err(WireError::InvalidLength {
            len,
            cap: MAX_FRAME_BYTES,
        });
    }
    let mut body = vec![0u8; len];
    fill(reader, &mut body, header.len())?;
    serde_json::from_slice(&body).map_err(|_| WireError::Malformed)
}

/// Fills `buf`, `before` bytes into the current frame. End of stream or a
/// timeout with nothing of the frame read is [`WireError::PeerClosed`] or the
/// retryable I/O error; past that it is [`WireError::Truncated`].
fn fill<R: Read>(reader: &mut R, buf: &mut [u8], before: usize) -> Result<(), WireError> {
    let mut read = 0;
    while read < buf.len() {
        match reader.read(&mut buf[read..]) {
            Ok(0) if before + read == 0 => return Err(WireError::PeerClosed),
            Ok(0) => {
                return Err(WireError::Truncated {
                    received: before + read,
                });
            }
            Ok(n) => read += n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error)
                if before + read > 0
                    && matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
            {
                return Err(WireError::Truncated {
                    received: before + read,
                });
            }
            Err(error) => return Err(WireError::Io(error)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out `steps` in order: bytes, or an error kind; then end of stream.
    struct Script(std::collections::VecDeque<Result<Vec<u8>, io::ErrorKind>>);

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(Err(kind)) => Err(io::Error::from(kind)),
                Some(Ok(mut bytes)) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.0.push_front(Ok(bytes.split_off(n)));
                    }
                    Ok(n)
                }
            }
        }
    }

    fn read(steps: Vec<Result<Vec<u8>, io::ErrorKind>>) -> Result<serde_json::Value, WireError> {
        read_message(&mut Script(steps.into()))
    }

    #[test]
    fn a_frame_is_its_length_then_its_json_and_an_oversized_one_reports_its_size() {
        let value = serde_json::json!({"text": "héllo", "n": [1, 2, 3]});
        let frame = encode_to_vec(&value).unwrap();
        let body = serde_json::to_vec(&value).unwrap();
        assert_eq!(&frame[..4], &(body.len() as u32).to_be_bytes());
        assert_eq!(&frame[4..], &body[..]);

        // A string of the cap serializes two quotes over it.
        let oversized = "x".repeat(MAX_FRAME_BYTES);
        match encode_to_vec(&oversized) {
            Err(WireError::InvalidLength { len, cap }) => {
                assert_eq!((len, cap), (MAX_FRAME_BYTES + 2, MAX_FRAME_BYTES));
            }
            other => panic!("expected InvalidLength, got {other:?}"),
        }
    }

    #[test]
    fn a_frame_cut_short_is_truncated_and_never_a_retryable_timeout() {
        let frame = encode_to_vec(&serde_json::json!({"a": 1})).unwrap();
        let (head, body) = frame.split_at(4);

        assert!(matches!(read(vec![]), Err(WireError::PeerClosed)));
        let idle = read(vec![Err(io::ErrorKind::WouldBlock)]).unwrap_err();
        assert!(idle.is_timeout(), "nothing read yet: retry");

        let cases = [
            (vec![Ok(head[..1].to_vec())], 1),
            (vec![Ok(head.to_vec()), Ok(body[..2].to_vec())], 6),
            (
                vec![Ok(head[..3].to_vec()), Err(io::ErrorKind::TimedOut)],
                3,
            ),
            (vec![Ok(head.to_vec()), Err(io::ErrorKind::WouldBlock)], 4),
        ];
        for (steps, expected) in cases {
            match read(steps) {
                Err(error @ WireError::Truncated { received }) => {
                    assert_eq!(received, expected);
                    assert!(!error.is_timeout());
                }
                other => panic!("expected Truncated after {expected}, got {other:?}"),
            }
        }

        // Short reads and interruptions inside a frame still assemble it.
        let value = read(vec![
            Ok(head[..2].to_vec()),
            Err(io::ErrorKind::Interrupted),
            Ok(head[2..].to_vec()),
            Ok(body.to_vec()),
        ])
        .unwrap();
        assert_eq!(value, serde_json::json!({"a": 1}));
    }

    #[test]
    fn declared_length_above_the_cap_is_refused_before_allocation() {
        let mut framed = Vec::new();
        framed.extend_from_slice(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes());
        let mut cursor = io::Cursor::new(framed);
        match read_message::<_, serde_json::Value>(&mut cursor) {
            Err(WireError::InvalidLength { len, cap }) => {
                assert_eq!(len, MAX_FRAME_BYTES + 1);
                assert_eq!(cap, MAX_FRAME_BYTES);
            }
            other => panic!("expected InvalidLength, got {other:?}"),
        }
    }

    #[test]
    fn zero_length_frame_is_refused_in_both_directions() {
        let mut framed = Vec::new();
        framed.extend_from_slice(&0u32.to_be_bytes());
        let mut cursor = io::Cursor::new(framed);
        assert!(matches!(
            read_message::<_, serde_json::Value>(&mut cursor),
            Err(WireError::InvalidLength { len: 0, .. })
        ));
    }

    #[test]
    fn exactly_the_cap_is_accepted_and_one_over_is_refused_on_encode() {
        let fits = "a".repeat(MAX_FRAME_BYTES - 2);
        let value = serde_json::Value::String(fits);
        assert!(encode_to_vec(&value).is_ok(), "cap-sized body must encode");

        let over = "a".repeat(MAX_FRAME_BYTES);
        let value = serde_json::Value::String(over);
        assert!(matches!(
            encode_to_vec(&value),
            Err(WireError::InvalidLength { .. })
        ));
    }
}
