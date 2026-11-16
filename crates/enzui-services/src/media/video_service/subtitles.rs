//! Plain-text subtitle cleanup; styled ASS streams remain owned by libass.

use gstreamer as gst;
use gstreamer::prelude::*;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(super) struct Layout(Arc<Mutex<Placement>>);

#[derive(Clone, Copy, Default)]
struct Placement {
    viewport: Option<(u32, u32)>,
    bottom: u32,
}

fn is_text_overlay(element: &gst::Element) -> bool {
    element
        .factory()
        .is_some_and(|factory| factory.name() == "textoverlay")
}

impl Layout {
    pub(super) fn update(
        &self,
        viewport: Option<(u32, u32)>,
        bottom: u32,
        playbin: Option<&gst::Element>,
    ) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Placement { viewport, bottom };
        if let Some(bin) = playbin.and_then(|element| element.downcast_ref::<gst::Bin>()) {
            for element in bin
                .iterate_recurse()
                .into_iter()
                .flatten()
                .filter(is_text_overlay)
            {
                if let Some(caps) = element
                    .static_pad("video_sink")
                    .and_then(|pad| pad.current_caps())
                {
                    self.apply(&element, &caps);
                }
            }
        }
    }

    fn apply(&self, overlay: &gst::Element, caps: &gst::CapsRef) {
        let placement = *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some((size, _)) = super::pipeline::video_info(&caps.to_owned()) else {
            return;
        };
        overlay.set_property_from_str(
            "valignment",
            if placement.bottom == 0 {
                "baseline"
            } else {
                "bottom"
            },
        );
        overlay.set_property("ypad", placement.padding(size));
    }
}

impl Placement {
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "padding is clamped to a nonnegative half-height that fits i32"
    )]
    fn padding(self, (width, height): (u32, u32)) -> i32 {
        let Some((viewport_width, viewport_height)) = self.viewport else {
            return 25;
        };
        if self.bottom == 0
            || width == 0
            || height == 0
            || viewport_width == 0
            || viewport_height == 0
        {
            return 25;
        }
        let scale = (f64::from(viewport_width) / f64::from(width))
            .min(f64::from(viewport_height) / f64::from(height));
        let letterbox = (f64::from(viewport_height) - f64::from(height) * scale) / 2.0;
        ((f64::from(self.bottom) - letterbox).max(0.0) / scale)
            .ceil()
            .clamp(
                25.0_f64.min(f64::from(height) / 2.0),
                f64::from(height) / 2.0,
            ) as i32
    }
}

pub(super) fn install(playbin: &gst::Element, layout: Layout) {
    playbin.connect("deep-element-added", false, move |values| {
        if let Some(element) = values
            .get(2)
            .and_then(|value| value.get::<gst::Element>().ok())
            && is_text_overlay(&element)
            && let Some(pad) = element.static_pad("video_sink")
        {
            let overlay = element.downgrade();
            let layout = layout.clone();
            pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, move |_, info| {
                if let Some(gst::PadProbeData::Event(event)) = &info.data
                    && let gst::EventView::Caps(caps) = event.view()
                    && let Some(overlay) = overlay.upgrade()
                {
                    layout.apply(&overlay, caps.caps());
                }
                gst::PadProbeReturn::Ok
            });
        }
        if let Some(element) = values
            .get(2)
            .and_then(|value| value.get::<gst::Element>().ok())
            && element
                .factory()
                .is_some_and(|factory| factory.name() == "subtitleoverlay")
            && let Some(pad) = element.static_pad("subtitle_sink")
        {
            pad.add_probe(gst::PadProbeType::BUFFER, |pad, info| {
                let text_caps = pad.current_caps().is_some_and(|caps| {
                    caps.structure(0)
                        .is_some_and(|structure| structure.name() == "text/x-raw")
                });
                if !text_caps {
                    return gst::PadProbeReturn::Ok;
                }
                if let Some(gst::PadProbeData::Buffer(buffer)) = info.data.as_mut() {
                    let cleaned = buffer.map_readable().ok().and_then(|map| {
                        (map.len() <= 256 * 1024)
                            .then(|| std::str::from_utf8(&map).ok().and_then(cleanup))
                            .flatten()
                    });
                    if let Some(cleaned) = cleaned {
                        let buffer = buffer.make_mut();
                        buffer.remove_all_memory();
                        buffer.append_memory(gst::Memory::from_mut_slice(cleaned.into_bytes()));
                    }
                }
                gst::PadProbeReturn::Ok
            });
        }
        None
    });
}

fn cleanup(text: &str) -> Option<String> {
    if !text.contains('\\') {
        return None;
    }
    let mut remaining = text;
    let mut output = String::with_capacity(text.len());
    while !remaining.is_empty() {
        if remaining.starts_with("{\\")
            && let Some(end) = remaining.find('}')
        {
            remaining = remaining.get(end + 1..)?;
            continue;
        }
        if let Some(tail) = remaining.strip_prefix("\\an")
            && tail
                .as_bytes()
                .first()
                .is_some_and(|byte| (b'1'..=b'9').contains(byte))
            && !tail.as_bytes().get(1).is_some_and(u8::is_ascii_digit)
        {
            remaining = tail.get(1..)?;
            continue;
        }
        let (replacement, consumed) =
            if remaining.starts_with("\\N") || remaining.starts_with("\\n") {
                ('\n', 2)
            } else if remaining.starts_with("\\h") {
                ('\u{a0}', 2)
            } else {
                let character = remaining.chars().next()?;
                (character, character.len_utf8())
            };
        output.push(replacement);
        remaining = remaining.get(consumed..)?;
    }
    (output != text).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_cues_hide_ass_commands_without_losing_markup_or_unicode() {
        for (source, expected) in [
            (r"{\an8}<i>Héllo</i>\Nworld", "<i>Héllo</i>\nworld"),
            (r"\an8Hello", "Hello"),
            (r"{\pos(40,80)\b1}Title\htext", "Title\u{a0}text"),
        ] {
            assert_eq!(cleanup(source).as_deref(), Some(expected));
        }
        for unchanged in [
            r"A {normal} cue",
            r"C:\Video\film.mkv",
            r"\an80 remains literal",
            "plain",
        ] {
            assert_eq!(cleanup(unchanged), None);
        }
    }
    #[test]
    fn overlay_probe_cleans_only_plain_text_and_preserves_buffer_timing() {
        gst::init().expect("gstreamer");
        for (installed, caps, expected) in [
            (false, "text/x-raw,format=utf8", r"{\an8}Héllo"),
            (true, "text/x-raw,format=utf8", "Héllo"),
            (true, "application/x-ass", r"{\an8}Héllo"),
        ] {
            let pipeline = gst::Pipeline::new();
            if installed {
                install(pipeline.upcast_ref(), Layout::default());
            }
            let overlay = gst::ElementFactory::make("subtitleoverlay")
                .build()
                .expect("subtitle overlay");
            pipeline.add(&overlay).expect("add overlay");
            let sink = overlay.static_pad("subtitle_sink").expect("subtitle pad");
            let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
            let captured = std::sync::Arc::clone(&observed);
            sink.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data {
                    let map = buffer.map_readable().expect("text cue");
                    *captured.lock().expect("capture") = Some((
                        String::from_utf8_lossy(&map).into_owned(),
                        buffer.pts(),
                        buffer.duration(),
                    ));
                }
                gst::PadProbeReturn::Drop
            });
            let source = gstreamer_app::AppSrc::builder()
                .caps(&caps.parse::<gst::Caps>().expect("caps"))
                .format(gst::Format::Time)
                .build();
            let video = gst::ElementFactory::make("videotestsrc")
                .property("num-buffers", 3i32)
                .build()
                .expect("video source");
            let output = gst::ElementFactory::make("fakesink")
                .property("sync", false)
                .build()
                .expect("video output");
            pipeline
                .add_many([source.upcast_ref(), &video, &output])
                .expect("test elements");
            video
                .link_pads(Some("src"), &overlay, Some("video_sink"))
                .expect("video link");
            source
                .link_pads(Some("src"), &overlay, Some("subtitle_sink"))
                .expect("subtitle link");
            overlay.link(&output).expect("output link");
            pipeline.set_state(gst::State::Playing).expect("play");
            let mut buffer = gst::Buffer::from_mut_slice(r"{\an8}Héllo".as_bytes().to_vec());
            buffer.make_mut().set_pts(gst::ClockTime::from_seconds(3));
            buffer
                .make_mut()
                .set_duration(gst::ClockTime::from_seconds(2));
            source.push_buffer(buffer).expect("push cue");
            source.end_of_stream().expect("end subtitle");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while observed.lock().expect("capture").is_none()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            pipeline.set_state(gst::State::Null).expect("stop");
            assert_eq!(
                *observed.lock().expect("captured cue"),
                Some((
                    expected.into(),
                    Some(gst::ClockTime::from_seconds(3)),
                    Some(gst::ClockTime::from_seconds(2))
                ))
            );
        }
    }
    fn caption_bottom(bottom: u32) -> usize {
        gst::init().expect("gstreamer");
        let pipeline = gst::Pipeline::new();
        let layout = Layout::default();
        layout.update(Some((640, 480)), bottom, None);
        install(pipeline.upcast_ref(), layout);
        let video = gst::ElementFactory::make("videotestsrc")
            .property_from_str("pattern", "black")
            .property("num-buffers", 1i32)
            .build()
            .expect("video");
        let filter = gst::ElementFactory::make("capsfilter")
            .property(
                "caps",
                "video/x-raw,format=RGBA,width=640,height=360"
                    .parse::<gst::Caps>()
                    .expect("caps"),
            )
            .build()
            .expect("capsfilter");
        let text = gst::ElementFactory::make("textoverlay")
            .property("text", "READABLE")
            .property("font-desc", "Sans Bold 8")
            .build()
            .expect("text overlay");
        let sink = gstreamer_app::AppSink::builder().sync(false).build();
        pipeline
            .add_many([&video, &filter, &text, sink.upcast_ref()])
            .expect("elements");
        gst::Element::link_many([&video, &filter, &text, sink.upcast_ref()]).expect("links");
        pipeline.set_state(gst::State::Playing).expect("play");
        let sample = sink
            .try_pull_sample(gst::ClockTime::from_seconds(3))
            .expect("caption frame");
        let outcome = crate::media::gst_worker::sample_to_frame(
            &sample,
            crate::media::Generation::FIRST,
            crate::media::FrameSeq(1),
        );
        pipeline.set_state(gst::State::Null).expect("stop");
        let crate::media::gst_worker::SampleOutcome::Frame(frame) = outcome else {
            panic!("RGBA caption frame");
        };
        frame
            .pixels()
            .as_chunks::<{ 640 * 4 }>()
            .0
            .iter()
            .enumerate()
            .filter_map(|(row, bytes)| {
                bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| pixel[..3].iter().all(|value| *value > 160))
                    .then_some(row)
            })
            .max()
            .expect("visible text")
    }

    #[test]
    fn letterboxed_tiny_captions_reserve_space_above_the_viewport_controls() {
        let baseline = caption_bottom(0);
        let safe = caption_bottom(128);
        assert!(
            baseline > 320,
            "positive control must overlap bottom controls: {baseline}"
        );
        assert!(safe < 300, "caption must clear the safe area: {safe}");
        for viewport in [(640, 480), (640, 400), (600, 450)] {
            let padding = Placement {
                viewport: Some(viewport),
                bottom: 128,
            }
            .padding((1920, 804));
            let scale = (f64::from(viewport.0) / 1920.0).min(f64::from(viewport.1) / 804.0);
            let letterbox = (f64::from(viewport.1) - 804.0 * scale) / 2.0;
            assert!(letterbox + f64::from(padding) * scale >= 127.0);
        }
    }
}
