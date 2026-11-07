//! Decoder queue time and buffering thresholds.

use super::pipeline::refused;
use crate::media::MediaError;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::time::Duration;

/// Decoder buffering limits, separate from unread disk retention.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VideoBuffering {
    /// Network buffering duration, defaulting to five seconds.
    /// Zero disables queue2's time cap; parsed streams use this duration as
    /// their interleave buffering horizon. Decoder queues retain additional data.
    pub duration: Duration,
    /// Fraction below which buffering begins; defaults to 0.01.
    pub low_watermark: f64,
    /// Fraction at which buffering ends; defaults to 0.6.
    pub high_watermark: f64,
}

impl Default for VideoBuffering {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(5),
            low_watermark: 0.01,
            high_watermark: 0.6,
        }
    }
}

impl VideoBuffering {
    pub(super) fn validate(self) -> Result<(), MediaError> {
        if self.duration.as_nanos() > i64::MAX as u128
            || !self.low_watermark.is_finite()
            || !self.high_watermark.is_finite()
            || self.low_watermark < 0.0
            || self.low_watermark >= self.high_watermark
            || self.high_watermark > 1.0
        {
            return Err(refused("invalid video buffering time or watermarks"));
        }
        Ok(())
    }

    pub(super) fn apply(self, playbin: &gst::Element) {
        let duration = i64::try_from(self.duration.as_nanos()).expect("validated buffering time");
        playbin.set_property("buffer-duration", duration);
        playbin.connect("deep-element-added", false, move |values| {
            if let Some(queue) = values
                .get(2)
                .and_then(|value| value.get::<gst::Element>().ok())
                && queue
                    .factory()
                    .is_some_and(|factory| factory.name() == "urisourcebin")
            {
                queue.set_property("buffer-duration", duration);
                queue.set_property("low-watermark", self.low_watermark);
                queue.set_property("high-watermark", self.high_watermark);
            }
            None
        });
    }
}
