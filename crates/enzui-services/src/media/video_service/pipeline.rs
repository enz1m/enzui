//! The pieces of a `playbin3` the worker assembles: the video sink, its
//! output caps, stream descriptions, flags and error wording.

use super::{Shared, Track};
use crate::media::gst_worker::SampleOutcome;
use crate::media::rgba::MAX_EDGE;
use crate::media::{FrameSeq, Generation, MediaError, MediaErrorKind};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer::tags::Tag;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone)]
pub(super) struct Stream {
    pub(super) kind: gst::StreamType,
    pub(super) id: String,
    pub(super) language: Option<String>,
    pub(super) title: Option<String>,
    pub(super) codec: Option<String>,
    pub(super) caps: Option<gst::Caps>,
    // Orientation tags can swap display width and height.
    pub(super) quarter_turn: bool,
}

impl Stream {
    pub(super) fn of(stream: &gst::Stream) -> Option<Self> {
        let id = stream.stream_id()?.to_string();
        let tags = stream.tags();
        let kind = stream.stream_type();
        let codec_tag = if kind.contains(gst::StreamType::AUDIO) {
            gst::tags::AudioCodec::TAG_NAME
        } else if kind.contains(gst::StreamType::VIDEO) {
            gst::tags::VideoCodec::TAG_NAME
        } else {
            gst::tags::SubtitleCodec::TAG_NAME
        };
        let language = tag_string(tags.as_ref(), gst::tags::LanguageCode::TAG_NAME)
            .or_else(|| tag_string(tags.as_ref(), gst::tags::LanguageName::TAG_NAME))
            // "und" is the container's way of saying it does not know.
            .filter(|code| code != "und");
        Some(Self {
            kind,
            language,
            title: tag_string(tags.as_ref(), gst::tags::Title::TAG_NAME),
            codec: tag_string(tags.as_ref(), codec_tag),
            caps: stream.caps(),
            quarter_turn: tag_string(tags.as_ref(), gst::tags::ImageOrientation::TAG_NAME)
                .is_some_and(|orientation| {
                    matches!(
                        orientation.as_str(),
                        "rotate-90" | "rotate-270" | "flip-rotate-90" | "flip-rotate-270"
                    )
                }),
            id,
        })
    }

    pub(super) fn is(&self, kind: gst::StreamType) -> bool {
        self.kind.contains(kind)
    }

    pub(super) fn track(&self, index: usize) -> Track {
        Track {
            index: i32::try_from(index).unwrap_or(i32::MAX),
            language: self.language.clone(),
            title: self.title.clone(),
            codec: self.codec.clone(),
        }
    }

    // Subtitle files use a different stream-id origin from the main source.
    pub(super) fn origin(&self) -> &str {
        self.id.split('/').next().unwrap_or_default()
    }
}

fn tag_string(list: Option<&gst::TagList>, name: &str) -> Option<String> {
    list?
        .generic(name)?
        .get::<String>()
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

// Read dimensions from encoded caps as well as raw video caps.
pub(super) fn video_info(caps: &gst::Caps) -> Option<((u32, u32), Option<f64>)> {
    let structure = caps.structure(0)?;
    let width = u64::try_from(structure.get::<i32>("width").ok()?).ok()?;
    let height = u32::try_from(structure.get::<i32>("height").ok()?).ok()?;
    let par = structure
        .get::<gst::Fraction>("pixel-aspect-ratio")
        .ok()
        .filter(|par| par.numer() > 0 && par.denom() > 0)
        .unwrap_or_else(|| gst::Fraction::new(1, 1));
    let numer = u64::try_from(par.numer()).ok()?;
    let denom = u64::try_from(par.denom()).ok()?;
    let width = u32::try_from(width * numer / denom).ok()?;
    let rate = structure
        .get::<gst::Fraction>("framerate")
        .ok()
        .filter(|fps| fps.numer() > 0 && fps.denom() > 0)
        .map(|fps| f64::from(fps.numer()) / f64::from(fps.denom()));
    (width > 0 && height > 0).then_some(((width, height), rate))
}

pub(super) fn video_sink(
    generation: Generation,
    shared: &Arc<Shared>,
    limit: Option<(u32, u32)>,
) -> Result<(gst::Element, gst::Element), MediaError> {
    let bin = gst::Bin::new();
    // Convert before flipping to support decoder formats videoflip cannot consume.
    let upright_convert = gst::ElementFactory::make("videoconvert")
        .build()
        .map_err(|_| missing_element("videoconvert"))?;
    let flip = gst::ElementFactory::make("videoflip")
        .build()
        .map_err(|_| missing_element("videoflip"))?;
    flip.set_property_from_str("video-direction", "auto");
    let convert = gst::ElementFactory::make("videoconvertscale")
        .build()
        .or_else(|_| gst::ElementFactory::make("videoconvert").build())
        .map_err(|_| missing_element("videoconvert"))?;
    if convert.find_property("n-threads").is_some() {
        convert.set_property("n-threads", 0u32);
    }
    let caps_filter = gst::ElementFactory::make("capsfilter")
        .property("caps", scale_caps(limit))
        .build()
        .map_err(|_| missing_element("capsfilter"))?;
    let sink = gst_app::AppSink::builder()
        .max_buffers(1)
        .drop(true)
        .sync(true)
        .build();
    let sequence = Arc::new(AtomicU64::new(0));
    let deliver = move |sink: &gst_app::AppSink, preroll: bool, shared: &Shared| {
        let sample = if preroll {
            sink.pull_preroll()
        } else {
            sink.pull_sample()
        };
        let Ok(sample) = sample else {
            return Err(gst::FlowError::Eos);
        };
        let seq = FrameSeq(sequence.fetch_add(1, Ordering::Relaxed) + 1);
        if let SampleOutcome::Frame(frame) =
            crate::media::gst_worker::sample_to_frame(&sample, generation, seq)
        {
            shared.offer_frame(frame);
        }
        Ok(gst::FlowSuccess::Ok)
    };
    let deliver = Arc::new(deliver);
    let on_sample = Arc::clone(&deliver);
    let sample_shared = Arc::clone(shared);
    let preroll_shared = Arc::clone(shared);
    sink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |sink| on_sample(sink, false, &sample_shared))
            .new_preroll(move |sink| deliver(sink, true, &preroll_shared))
            .build(),
    );
    let sink: gst::Element = sink.upcast();
    if convert
        .factory()
        .is_some_and(|factory| factory.name() == "videoconvertscale")
    {
        bin.add_many([&upright_convert, &flip, &convert, &caps_filter, &sink])
            .map_err(|_| backend("could not assemble the video sink"))?;
        gst::Element::link_many([&upright_convert, &flip, &convert, &caps_filter, &sink])
            .map_err(|_| backend("could not link the video sink"))?;
    } else {
        let scale = gst::ElementFactory::make("videoscale")
            .build()
            .map_err(|_| missing_element("videoscale"))?;
        bin.add_many([
            &upright_convert,
            &flip,
            &convert,
            &scale,
            &caps_filter,
            &sink,
        ])
        .map_err(|_| backend("could not assemble the video sink"))?;
        gst::Element::link_many([
            &upright_convert,
            &flip,
            &convert,
            &scale,
            &caps_filter,
            &sink,
        ])
        .map_err(|_| backend("could not link the video sink"))?;
    }
    let pad = upright_convert
        .static_pad("sink")
        .ok_or_else(|| backend("the video converter has no input"))?;
    let ghost = gst::GhostPad::with_target(&pad).map_err(|_| backend("no video sink pad"))?;
    bin.add_pad(&ghost)
        .map_err(|_| backend("could not expose the video sink pad"))?;
    Ok((bin.upcast(), caps_filter))
}

// Square pixels preserve the display shape of anamorphic sources.
pub(super) fn scale_caps(limit: Option<(u32, u32)>) -> gst::Caps {
    let edge = i32::try_from(MAX_EDGE).unwrap_or(i32::MAX);
    let (width, height) = limit.map_or((edge, edge), |(width, height)| {
        (
            i32::try_from(width).unwrap_or(edge).clamp(1, edge),
            i32::try_from(height).unwrap_or(edge).clamp(1, edge),
        )
    });
    gst_video::VideoCapsBuilder::new()
        .format(gst_video::VideoFormat::Rgba)
        .width_range(1..=width)
        .height_range(1..=height)
        .pixel_aspect_ratio(gst::Fraction::new(1, 1))
        .build()
}

pub(super) fn set_flag(playbin: &gst::Element, nick: &str, on: bool) {
    let value = playbin.property_value("flags");
    let Some(class) = glib_flags(&value) else {
        return;
    };
    let changed = if on {
        class.set_by_nick(value, nick)
    } else {
        class.unset_by_nick(value, nick)
    };
    if let Ok(value) = changed {
        playbin.set_property_from_value("flags", &value);
    }
}

fn glib_flags(value: &gst::glib::Value) -> Option<gst::glib::FlagsClass> {
    gst::glib::FlagsClass::with_type(value.type_())
}

pub(super) fn file_uri(path: &std::path::Path) -> Result<String, MediaError> {
    gst::glib::filename_to_uri(path, None)
        .map(|uri| uri.to_string())
        .map_err(|_| MediaError {
            kind: MediaErrorKind::SourceNotFound,
            message: "The subtitle file could not be opened".into(),
        })
}

// Use generic error wording rather than copying backend paths or URLs.
//
// `received` is the payload a remote source delivered, when observed. A stream
// or core failure after an empty transfer is the transport's, not the media's.
pub(super) fn classify(
    error: gst::glib::Error,
    shared: &Shared,
    received: Option<u64>,
) -> MediaError {
    let missing = shared.lock().status.missing_decoders.first().cloned();
    if let Some(missing) = missing {
        return MediaError {
            kind: MediaErrorKind::UnsupportedFormat,
            message: format!("This video needs a decoder that is not installed: {missing}"),
        };
    }
    if let Some(kind) = error.kind::<gst::ResourceError>() {
        return match kind {
            gst::ResourceError::NotFound | gst::ResourceError::OpenRead => MediaError {
                kind: MediaErrorKind::SourceNotFound,
                message: "The video could not be opened".into(),
            },
            gst::ResourceError::NotAuthorized => MediaError {
                kind: MediaErrorKind::Refused,
                message: "Access to the video was refused".into(),
            },
            _ => MediaError {
                kind: MediaErrorKind::Backend,
                message: "The video could not be read".into(),
            },
        };
    }
    let media_error =
        error.kind::<gst::StreamError>().is_some() || error.kind::<gst::CoreError>().is_some();
    if media_error && received == Some(0) {
        return MediaError {
            kind: MediaErrorKind::SourceNotFound,
            message: "The stream ended before any video arrived".into(),
        };
    }
    if media_error {
        return MediaError {
            kind: MediaErrorKind::UnsupportedFormat,
            message: "This file is not a video that can be played".into(),
        };
    }
    MediaError {
        kind: MediaErrorKind::Backend,
        message: "Playback failed".into(),
    }
}

pub(super) fn refused(message: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Refused,
        message: message.into(),
    }
}

pub(super) fn backend(message: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Backend,
        message: message.into(),
    }
}

pub(super) fn missing_element(name: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Backend,
        message: format!("GStreamer is missing the {name} element"),
    }
}
