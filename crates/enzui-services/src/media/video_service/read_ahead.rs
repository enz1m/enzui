//! Owned anonymous storage for unread remote bytes.

use super::pipeline::{backend, refused};
use crate::media::MediaError;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::fs::File;
use std::path::Path;
use std::sync::Mutex;

/// Opt-in disk retention for remote video, independent of decoder buffering.
///
/// Consumed bytes are discarded; demand seeking starts a fresh screened Range
/// request. Each remote pipeline reserves the full capacity before transfer.
/// Reservation or I/O failure stops playback, without a memory fallback.
/// Anonymous Linux backing leaves no media filenames after teardown or a crash.
/// Filesystem metadata, copy-on-write overhead and OS page cache are outside
/// the byte limit; free-space admission cannot constrain other writers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiskReadAhead {
    /// Unread-byte ceiling and fully reserved file size, at least 256 KiB.
    pub capacity: u32,
    /// Free bytes required after reservation, in addition to the capacity.
    pub min_free_bytes: u64,
}

impl DiskReadAhead {
    pub(super) fn validate(self) -> Result<(), MediaError> {
        if u64::from(self.capacity) < super::super::bounded_http::RECEIVE_LIMIT {
            return Err(refused("disk read-ahead capacity must be at least 256 KiB"));
        }
        self.min_free_bytes
            .checked_add(u64::from(self.capacity))
            .ok_or_else(|| refused("disk read-ahead free-space requirement overflowed"))?;
        if !cfg!(target_os = "linux") {
            return Err(refused("anonymous disk read-ahead requires Linux"));
        }
        Ok(())
    }
}

/// Native observations of the current pipeline's unread disk FIFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadAheadStatus {
    /// Configured unread-byte ceiling.
    pub capacity: u32,
    /// Bytes currently queued, excluding bytes already handed downstream.
    pub buffered_bytes: u64,
    /// Largest observed unread-byte population across this pipeline's seeks.
    pub peak_buffered_bytes: u64,
    /// Anonymous file size, fully reserved before transfer.
    pub reserved_bytes: u64,
}

pub(super) struct Backing {
    file: File,
    policy: DiskReadAhead,
    source: Mutex<Option<gst::glib::WeakRef<gst::Element>>>,
}

impl Backing {
    #[cfg(target_os = "linux")]
    pub(super) fn reserve(directory: &Path, policy: DiskReadAhead) -> Result<Self, MediaError> {
        policy.validate()?;
        let root = enzui_security::secure_dir::SecureRoot::open(directory)
            .map_err(|_| refused("the read-ahead directory is not private"))?;
        let file = root
            .create_anonymous_file()
            .map_err(|_| backend("anonymous read-ahead storage is unavailable"))?;
        let required = policy.min_free_bytes + u64::from(policy.capacity);
        if available_bytes(&file)? < required {
            return Err(refused("insufficient free space for disk read-ahead"));
        }
        rustix::fs::fallocate(
            &file,
            rustix::fs::FallocateFlags::empty(),
            0,
            u64::from(policy.capacity),
        )
        .map_err(|_| backend("disk read-ahead capacity could not be reserved"))?;
        if available_bytes(&file)? < policy.min_free_bytes {
            return Err(refused("the disk read-ahead free-space floor was crossed"));
        }
        Ok(Self {
            file,
            policy,
            source: Mutex::new(None),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub(super) fn reserve(_directory: &Path, _policy: DiskReadAhead) -> Result<Self, MediaError> {
        Err(refused("anonymous disk read-ahead requires Linux"))
    }

    pub(super) fn configure(&self, source: &gst::Element) -> Result<(), MediaError> {
        use std::os::fd::AsRawFd;
        if source
            .factory()
            .is_none_or(|factory| factory.name() != super::super::bounded_http::FACTORY)
            || source.find_property("read-ahead-capacity").is_none()
            || source.find_property("read-ahead-fd").is_none()
        {
            return Err(refused("the source does not support owned disk read-ahead"));
        }
        source.set_property("read-ahead-capacity", self.policy.capacity);
        source.set_property("read-ahead-fd", self.file.as_raw_fd());
        if !source.property::<bool>("read-ahead-ready")
            || source.property::<u32>("read-ahead-capacity") != self.policy.capacity
            || source.property::<u64>("read-ahead-file-bytes") != u64::from(self.policy.capacity)
        {
            return Err(refused("the disk read-ahead reservation was not adopted"));
        }
        *self
            .source
            .lock()
            .map_err(|_| backend("the read-ahead observation was unavailable"))? =
            Some(source.downgrade());
        Ok(())
    }

    pub(super) fn status(&self) -> Option<ReadAheadStatus> {
        let source = self.source.lock().ok()?.as_ref()?.upgrade()?;
        source
            .property::<bool>("read-ahead-ready")
            .then(|| ReadAheadStatus {
                capacity: self.policy.capacity,
                buffered_bytes: source.property("read-ahead-buffered-bytes"),
                peak_buffered_bytes: source.property("read-ahead-peak-bytes"),
                reserved_bytes: source.property("read-ahead-file-bytes"),
            })
    }
}

#[cfg(target_os = "linux")]
fn available_bytes(file: &File) -> Result<u64, MediaError> {
    let stat = rustix::fs::fstatvfs(file)
        .map_err(|_| backend("read-ahead free space could not be inspected"))?;
    stat.f_bavail
        .checked_mul(stat.f_frsize)
        .ok_or_else(|| backend("read-ahead free-space count overflowed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::video_service::{VideoBuffering, VideoService, VideoServiceConfig};
    use enzui_test_support::test_scratch::ScratchDir;
    use std::time::Duration;

    #[test]
    fn disk_read_ahead_configuration_refuses_invalid_limits_before_loading() {
        let scratch = ScratchDir::new_restricted("disk-read-ahead-config");
        let directory = scratch.join("storage");
        let defaults = VideoServiceConfig::default();
        assert!(defaults.disk_read_ahead.is_none());
        assert_eq!(defaults.buffering.duration, Duration::from_secs(5));
        assert_eq!(defaults.buffering.low_watermark, 0.01);
        assert_eq!(defaults.buffering.high_watermark, 0.6);
        for disk in [
            DiskReadAhead {
                capacity: 0,
                min_free_bytes: 0,
            },
            DiskReadAhead {
                capacity: 262_143,
                min_free_bytes: 0,
            },
            DiskReadAhead {
                capacity: 262_144,
                min_free_bytes: u64::MAX,
            },
        ] {
            assert!(
                VideoService::new_with_read_ahead_directory(
                    VideoServiceConfig {
                        disk_read_ahead: Some(disk),
                        ..defaults
                    },
                    directory.clone()
                )
                .is_err()
            );
        }
        for buffering in [
            VideoBuffering {
                duration: Duration::MAX,
                ..defaults.buffering
            },
            VideoBuffering {
                low_watermark: f64::NAN,
                ..defaults.buffering
            },
            VideoBuffering {
                high_watermark: f64::INFINITY,
                ..defaults.buffering
            },
            VideoBuffering {
                low_watermark: -0.1,
                ..defaults.buffering
            },
            VideoBuffering {
                low_watermark: 0.6,
                high_watermark: 0.6,
                ..defaults.buffering
            },
            VideoBuffering {
                high_watermark: 1.1,
                ..defaults.buffering
            },
        ] {
            assert!(
                VideoService::new_with_read_ahead_directory(
                    VideoServiceConfig {
                        buffering,
                        ..defaults
                    },
                    directory.clone()
                )
                .is_err()
            );
        }
        assert!(
            VideoService::new_with_read_ahead_directory(
                VideoServiceConfig {
                    position_interval: Duration::ZERO,
                    ..defaults
                },
                directory.clone()
            )
            .is_err()
        );
        assert!(VideoService::new_with_read_ahead_directory(defaults, "relative".into()).is_err());
        let accepted = VideoService::new_with_read_ahead_directory(
            VideoServiceConfig {
                disk_read_ahead: Some(DiskReadAhead {
                    capacity: 262_144,
                    min_free_bytes: 0,
                }),
                ..defaults
            },
            directory.clone(),
        )
        .unwrap();
        assert!(
            !directory.exists(),
            "storage is reserved only by approved remote loads"
        );
        drop(accepted);
    }
}
