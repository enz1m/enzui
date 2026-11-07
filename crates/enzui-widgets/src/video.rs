//! Snapshot-driven video surfaces with composed media transport controls.
//!
//! The declared video capability wins over any supplied frame. A video source
//! with no frame shows a loading skeleton and status region; an audio-only source
//! shows a named no-video notice. Snapshot errors are displayed by the transport.
//!
//! Frames may be borrowed or owned. Converting the builder to an element copies
//! the pixel bytes into a fresh image handle; repeated views do not reuse its
//! renderer identity. [`FramePresenter`] offers allocation-backed handle reuse
//! for streaming hosts that manage presentation separately.
//!
//! Surface width defaults to a 640 logical-pixel cap. Height follows the frame's
//! aspect ratio, or 16:9 without a frame; hosts still own viewport sizing. The
//! transport is shown by default and uses the caller's stable identity prefix.

//! Native fullscreen is window policy: use [`crate::app_shell::ShellWindowState::set_fullscreen`]
//! or `toggle_fullscreen`. [`crate::app_shell`] removes window chrome automatically.
//! Transport and pointer-idle controls remain application content; fullscreen does
//! not replace the media session or imply playback. Use the portal idle inhibitor
//! while playing when appropriate, releasing it on pause or session end.

use crate::media_controls::{MediaControlsMessage, MediaControlsTokens, enz_media_controls};
use enzui_core::live_region::Announcement;
use enzui_core::media::{Rgba8Frame, Snapshot};
use enzui_core::semantics::Semantics;
use iced::widget::{column, container, image as iced_image};
use iced::{Element, Length, Padding, Theme};

/// The standard 16:9 video frame shape, used until a frame reports its own.
pub const DEFAULT_ASPECT_RATIO: f32 = 16.0 / 9.0;

/// Default surface-width cap in logical pixels; aspect ratio determines its height.
pub const DEFAULT_MAX_SURFACE_WIDTH: f32 = 640.0;

/// The capability-selected picture, loading, or absent-track presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoSurface {
    /// The source has a video track and a decoded frame is available: show it.
    Frame,
    /// The source has a video track but no frame has arrived yet: show a
    /// loading placeholder that does not claim to be a picture.
    AwaitingFirstFrame,
    /// The source has no video track. Show a named notice instead of a
    /// surface, so an audio-only file does not look like broken video.
    NoVideoTrack,
}

impl VideoSurface {
    /// Chooses a surface from declared video capability and frame availability.
    /// A source without video always yields `NoVideoTrack`, even if a frame is supplied.
    #[must_use]
    pub fn for_snapshot(snap: &Snapshot, has_frame: bool) -> VideoSurface {
        if !snap.capabilities.has_video {
            VideoSurface::NoVideoTrack
        } else if has_frame {
            VideoSurface::Frame
        } else {
            VideoSurface::AwaitingFirstFrame
        }
    }

    /// Whether this surface presents actual picture content.
    #[must_use]
    pub fn shows_picture(self) -> bool {
        matches!(self, VideoSurface::Frame)
    }
}

/// Copies frame pixel bytes into a fresh renderer image handle with the same dimensions.
/// Each call creates a new identity; this helper does not cache handles or allocate a GPU texture.
#[must_use]
pub fn frame_handle(frame: &Rgba8Frame) -> iced_image::Handle {
    iced_image::Handle::from_rgba(frame.width(), frame.height(), frame.pixels().to_vec())
}

/// How many recent borrowed frames a thread keeps a handle for: one per
/// video on screen, with room for a few.
const HANDLES_KEPT: usize = 4;

/// What identifies a borrowed frame: its source generation and sequence, its
/// size, and the buffer the sink keeps alive while lending it.
type FrameKey = (
    enzui_core::media::Generation,
    enzui_core::media::FrameSeq,
    u32,
    u32,
    usize,
);

thread_local! {
    static HANDLES: std::cell::RefCell<Vec<(FrameKey, iced_image::Handle)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The handle for a frame: an owned one hands over its pixels without a
/// copy; a borrowed one is copied once and its handle (and so its renderer
/// identity) reused while the same frame is shown, as when paused.
fn shown_handle(frame: FrameRef<'_>) -> iced_image::Handle {
    let frame = match frame {
        FrameRef::Owned(frame) => {
            let (width, height) = (frame.width(), frame.height());
            return iced_image::Handle::from_rgba(width, height, frame.into_pixels());
        }
        FrameRef::Borrowed(frame) => frame,
    };
    let key = (
        frame.generation(),
        frame.seq(),
        frame.width(),
        frame.height(),
        frame.pixels().as_ptr() as usize,
    );
    HANDLES.with(|handles| {
        let mut handles = handles.borrow_mut();
        if let Some(at) = handles.iter().position(|(kept, _)| *kept == key) {
            let entry = handles.remove(at);
            let handle = entry.1.clone();
            handles.insert(0, entry);
            return handle;
        }
        let handle = frame_handle(frame);
        handles.insert(0, (key, handle.clone()));
        handles.truncate(HANDLES_KEPT);
        handle
    })
}

/// Returns the validated frame width divided by height, or 16:9 when no frame is supplied.
#[must_use]
pub fn aspect_ratio_for(frame: Option<&Rgba8Frame>) -> f32 {
    match frame {
        Some(f) => f.width() as f32 / f.height() as f32,
        None => DEFAULT_ASPECT_RATIO,
    }
}

/// Returns a role/state name, appending the supplied title when present.
/// The title does not replace prefixes such as `Video:` or `Loading video:`.
#[must_use]
pub fn surface_name(surface: VideoSurface, title: Option<&str>) -> String {
    match (surface, title) {
        (VideoSurface::Frame, Some(t)) => {
            enzui_core::localized!("enzui-video-video-named", "t" => t)
        }
        (VideoSurface::Frame, None) => enzui_core::localization::text("enzui-video-video"),
        (VideoSurface::AwaitingFirstFrame, Some(t)) => {
            enzui_core::localized!("enzui-video-loading-video-named", "t" => t)
        }
        (VideoSurface::AwaitingFirstFrame, None) => {
            enzui_core::localization::text("enzui-video-loading-video")
        }
        (VideoSurface::NoVideoTrack, Some(t)) => {
            enzui_core::localized!("enzui-video-no-video-track-named", "t" => t)
        }
        (VideoSurface::NoVideoTrack, None) => {
            enzui_core::localization::text("enzui-video-no-video-track")
        }
    }
}

/// A borrowed or owned decoded frame supplied to the video builder.
/// Borrow when another component retains the buffer; own frames created locally in a view.
/// Both forms copy pixel bytes when converted to an element.
pub enum FrameRef<'a> {
    /// A borrow of a frame someone else owns (the sink).
    Borrowed(&'a Rgba8Frame),
    /// A frame this widget owns for the duration of the view.
    Owned(Rgba8Frame),
}

impl<'a> FrameRef<'a> {
    /// The frame, however it is held.
    #[must_use]
    pub fn get(&self) -> &Rgba8Frame {
        match self {
            FrameRef::Borrowed(f) => f,
            FrameRef::Owned(f) => f,
        }
    }
}

impl<'a> From<&'a Rgba8Frame> for FrameRef<'a> {
    fn from(f: &'a Rgba8Frame) -> Self {
        FrameRef::Borrowed(f)
    }
}

impl From<Rgba8Frame> for FrameRef<'_> {
    fn from(f: Rgba8Frame) -> Self {
        FrameRef::Owned(f)
    }
}

/// A video player: a capability-gated video surface with a transport bar.
///
/// Construct with [`enz_video`].
pub struct EnzVideo<'a> {
    id_prefix: String,
    snapshot: Snapshot,
    frame: Option<FrameRef<'a>>,
    title: Option<String>,
    tokens: Option<MediaControlsTokens>,
    show_controls: bool,
    max_surface_width: f32,
}

/// Creates a snapshot-driven player from an optional borrowed or owned decoded frame.
/// Use a stable identity prefix for the transport controls. No frame yields a loading
/// surface only when the snapshot declares video capability.
#[must_use]
pub fn enz_video<'a>(
    id_prefix: impl Into<String>,
    snapshot: Snapshot,
    frame: Option<impl Into<FrameRef<'a>>>,
) -> EnzVideo<'a> {
    EnzVideo {
        id_prefix: id_prefix.into(),
        snapshot,
        frame: frame.map(Into::into),
        title: None,
        tokens: None,
        show_controls: true,
        max_surface_width: DEFAULT_MAX_SURFACE_WIDTH,
    }
}

/// Creates a player without a decoded frame, avoiding type inference for a bare `None`.
/// Video-capable sources show loading; other sources show a no-video notice.
#[must_use]
pub fn enz_video_awaiting<'a>(id_prefix: impl Into<String>, snapshot: Snapshot) -> EnzVideo<'a> {
    EnzVideo {
        id_prefix: id_prefix.into(),
        snapshot,
        frame: None,
        title: None,
        tokens: None,
        show_controls: true,
        max_surface_width: DEFAULT_MAX_SURFACE_WIDTH,
    }
}

impl<'a> EnzVideo<'a> {
    /// Supply a human title for this source, used in the surface's accessible
    /// name. Without it the name states role and state only — the widget does
    /// not invent a description of the picture.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Overrides the composed transport's visual tokens.
    #[must_use]
    pub fn tokens(mut self, tokens: MediaControlsTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Sets the surface-width cap in logical pixels, defaulting to `DEFAULT_MAX_SURFACE_WIDTH`.
    /// Non-finite or non-positive values leave the current cap unchanged. Height follows
    /// aspect ratio; this cap alone does not guarantee the transport fits the viewport.
    #[must_use]
    pub fn max_width(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.max_surface_width = px;
        }
        self
    }

    /// The width cap currently in force.
    #[must_use]
    pub fn max_surface_width(&self) -> f32 {
        self.max_surface_width
    }

    /// Hide the transport bar (for a decorative or externally-controlled
    /// surface). Defaults to shown.
    #[must_use]
    pub fn show_controls(mut self, show: bool) -> Self {
        self.show_controls = show;
        self
    }

    /// Which surface this widget will render, without rendering it.
    #[must_use]
    pub fn surface(&self) -> VideoSurface {
        VideoSurface::for_snapshot(&self.snapshot, self.frame.is_some())
    }

    /// The accessible name this widget will publish for its surface.
    #[must_use]
    pub fn surface_name(&self) -> String {
        surface_name(self.surface(), self.title.as_deref())
    }
}

impl<'a> EnzVideo<'a> {
    /// Converts the builder into a video surface and optional transport controls.
    #[must_use]
    pub fn into_element(self) -> Element<'a, MediaControlsMessage, Theme> {
        Element::from(self)
    }
}

impl<'a> From<EnzVideo<'a>> for Element<'a, MediaControlsMessage, Theme> {
    fn from(v: EnzVideo<'a>) -> Self {
        let surface = VideoSurface::for_snapshot(&v.snapshot, v.frame.is_some());
        let name = surface_name(surface, v.title.as_deref());
        let ratio = aspect_ratio_for(v.frame.as_ref().map(FrameRef::get));
        let mut frame = v.frame;

        let surface_element: Element<'a, MediaControlsMessage, Theme> = match surface {
            VideoSurface::Frame => {
                let held = frame.take().expect("VideoSurface::Frame implies a frame");
                let element = crate::image_widget::enz_image_handle(shown_handle(held))
                    .fit(crate::image_widget::EnzImageFit::Contain)
                    .alt_text(name)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into_element();
                crate::aspect_ratio::enz_aspect_ratio(element, ratio).into()
            }
            VideoSurface::AwaitingFirstFrame => {
                let skeleton: Element<'a, MediaControlsMessage, Theme> =
                    crate::skeleton::enz_skeleton(crate::skeleton::SkeletonKind::Card)
                        .label(name.clone())
                        .into_element();
                let shaped: Element<'a, MediaControlsMessage, Theme> =
                    crate::aspect_ratio::enz_aspect_ratio(skeleton, ratio).into();
                // A group preserves the skeleton's progress node alongside the status region.
                crate::semantics_container::SemanticsContainer::new(
                    shaped,
                    Announcement::Status.region(name),
                )
                .into()
            }
            VideoSurface::NoVideoTrack => {
                let notice: Element<'a, MediaControlsMessage, Theme> =
                    crate::enz_text::enz_text(name.clone())
                        .size_px(enzui_theme::text_scale::ui_px(12.0))
                        // The containing status region carries the same name.
                        .announce(false)
                        .into_element();
                let boxed: Element<'a, MediaControlsMessage, Theme> = container(notice)
                    .padding(Padding::new(12.0))
                    .width(Length::Fill)
                    .into();
                crate::semantics_container::SemanticsContainer::leaf(
                    boxed,
                    Announcement::Status.region(name),
                )
                .into()
            }
        };

        let capped = crate::max_width::max_width(surface_element, v.max_surface_width);
        let mut col = column![capped].spacing(4).width(Length::Fill);

        if v.show_controls {
            let mut controls = enz_media_controls(v.id_prefix.clone(), v.snapshot.clone());
            if let Some(tokens) = v.tokens {
                controls = controls.tokens(tokens);
            }
            let controls_element: Element<'a, MediaControlsMessage, Theme> = controls.into();
            col = col.push(controls_element);
        }

        col.into()
    }
}

/// The semantics a caller can attach to an externally-composed surface, exposed
/// so a host that lays out its own container still publishes the same node.
pub fn surface_semantics(surface: VideoSurface, title: Option<&str>) -> Semantics {
    Announcement::Status.region(surface_name(surface, title))
}

pub mod presenter;

pub use presenter::{FrameHandover, FramePresenter, LatestFramePresenter, ReadyFrame};

#[cfg(test)]
mod tests;
