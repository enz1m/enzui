//! An image that plays its animation: GIF, APNG and animated WebP.
//!
//! [`AnimatedFrames::decode`] reads the file once into frames and their
//! delays (do it off the UI thread for large files, for example in a
//! `Task::perform`); [`enz_animated_image`] shows them. Every frame stays
//! decoded in memory, so keep animations small.
//!
//! The animation runs only while it can be seen and wanted: it pauses when
//! the image scrolls out of view or the window stops drawing (minimised,
//! covered), and resumes where it left off; [`EnzAnimatedImage::playing`]
//! pauses it on request; and with reduced motion (the desktop setting, or
//! the theme's motion policy) it shows its first frame and never wakes.
//! A file with a single frame is shown as a still image. Frames shorter than
//! 20 ms play at 100 ms, as browsers do. Showing other frames in the same
//! place (a different [`AnimatedFrames`], not a clone) starts them from their
//! first frame.
//!
//! A named image (`.alt_text("…")`) publishes one
//! [`Role::Image`] node; without alt
//! text it is decorative and publishes nothing.

use crate::image::EnzImageFit;
use enzui_core::collect::SemanticsNode;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::image::{self as iced_image, Renderer as _};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::time::{Duration, Instant};
use iced::{Element, Event, Length, Rectangle, Size, mouse, window};
use std::cell::Cell;
use std::sync::Arc;

/// The shortest frame delay played as written.
const MIN_DELAY: Duration = Duration::from_millis(20);

/// What a too-short frame delay plays as.
const SHORT_DELAY_FALLBACK: Duration = Duration::from_millis(100);

/// Why an animation could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnimatedImageError {
    /// The bytes are not an image this build reads.
    Unreadable(String),
    /// The image has no frames.
    Empty,
    /// The animation is larger than the [`AnimationLimits`] it was read with.
    TooLarge,
}

/// How much an animation may decode to, checked as it is read: a frame past
/// any limit stops the read before that frame is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationLimits {
    /// The most frames kept.
    pub max_frames: usize,
    /// The most decoded RGBA bytes, all frames together.
    pub max_total_bytes: u64,
    /// The longest edge of the canvas, in pixels.
    pub max_edge: u32,
}

impl Default for AnimationLimits {
    /// 1000 frames, 256 MiB of pixels, 8192 px edges: room for any icon,
    /// sticker or loading animation, and a bound for a hostile file.
    fn default() -> Self {
        Self {
            max_frames: 1000,
            max_total_bytes: 256 * 1024 * 1024,
            max_edge: 8192,
        }
    }
}

impl std::fmt::Display for AnimatedImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(reason) => write!(f, "not a readable image: {reason}"),
            Self::Empty => f.write_str("the image has no frames"),
            Self::TooLarge => f.write_str("the animation is larger than its limits"),
        }
    }
}

impl std::error::Error for AnimatedImageError {}

/// Decoded frames of an animation and how long each is shown. Cheap to
/// clone: the frames are shared.
#[derive(Debug, Clone)]
pub struct AnimatedFrames {
    frames: Arc<[(iced_image::Handle, Duration)]>,
    size: Size<u32>,
}

impl AnimatedFrames {
    /// Read a GIF, PNG or APNG, or WebP (still or animated); other formats
    /// this build reads come back as one frame. Read within the default
    /// [`AnimationLimits`].
    ///
    /// # Errors
    /// Returns [`AnimatedImageError`] when the bytes cannot be read or exceed
    /// the limits.
    pub fn decode(bytes: &[u8]) -> Result<Self, AnimatedImageError> {
        Self::decode_bounded(bytes, AnimationLimits::default())
    }

    /// [`Self::decode`] within `limits`, checked before each frame is kept.
    ///
    /// # Errors
    /// Returns [`AnimatedImageError`] when the bytes cannot be read or exceed
    /// `limits`.
    pub fn decode_bounded(
        bytes: &[u8],
        limits: AnimationLimits,
    ) -> Result<Self, AnimatedImageError> {
        use image::{AnimationDecoder as _, ImageDecoder as _};
        let unreadable =
            |error: image::ImageError| AnimatedImageError::Unreadable(error.to_string());
        let format = image::guess_format(bytes).map_err(unreadable)?;
        let cursor = || std::io::Cursor::new(bytes);
        // The canvas is checked before any frame is decoded.
        let canvas = |(width, height): (u32, u32)| {
            if width.max(height) > limits.max_edge {
                Err(AnimatedImageError::TooLarge)
            } else {
                Ok(())
            }
        };
        let frames: Vec<image::Frame> = match format {
            image::ImageFormat::Gif => {
                let decoder = image::codecs::gif::GifDecoder::new(cursor()).map_err(unreadable)?;
                canvas(decoder.dimensions())?;
                bounded_frames(decoder.into_frames(), limits)?
            }
            image::ImageFormat::WebP => {
                let decoder =
                    image::codecs::webp::WebPDecoder::new(cursor()).map_err(unreadable)?;
                canvas(decoder.dimensions())?;
                if decoder.has_animation() {
                    bounded_frames(decoder.into_frames(), limits)?
                } else {
                    still(bytes, limits)?
                }
            }
            image::ImageFormat::Png => {
                let decoder = image::codecs::png::PngDecoder::new(cursor()).map_err(unreadable)?;
                canvas(decoder.dimensions())?;
                if decoder.is_apng().map_err(unreadable)? {
                    bounded_frames(decoder.apng().map_err(unreadable)?.into_frames(), limits)?
                } else {
                    still(bytes, limits)?
                }
            }
            _ => still(bytes, limits)?,
        };
        let first = frames.first().ok_or(AnimatedImageError::Empty)?;
        let size = Size::new(first.buffer().width(), first.buffer().height());
        let frames = frames
            .into_iter()
            .map(|frame| {
                let (numerator, denominator) = frame.delay().numer_denom_ms();
                let delay = Duration::from_secs_f64(
                    f64::from(numerator) / f64::from(denominator.max(1)) / 1000.0,
                );
                let delay = if delay < MIN_DELAY {
                    SHORT_DELAY_FALLBACK
                } else {
                    delay
                };
                let buffer = frame.into_buffer();
                let (width, height) = buffer.dimensions();
                (
                    iced_image::Handle::from_rgba(width, height, buffer.into_raw()),
                    delay,
                )
            })
            .collect();
        Ok(Self { frames, size })
    }

    /// How many frames there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether there are no frames (never true for a decoded image).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The width and height in pixels.
    #[must_use]
    pub fn size(&self) -> Size<u32> {
        self.size
    }

    /// How long frame `index` is shown.
    #[must_use]
    pub fn delay(&self, index: usize) -> Option<Duration> {
        self.frames.get(index).map(|(_, delay)| *delay)
    }
}

/// The frames of `frames` up to `limits`, refused as soon as one passes them,
/// so the decoded total never exceeds them.
fn bounded_frames(
    frames: image::Frames<'_>,
    limits: AnimationLimits,
) -> Result<Vec<image::Frame>, AnimatedImageError> {
    let mut kept = Vec::new();
    let mut total = 0u64;
    for frame in frames {
        let frame = frame.map_err(|error| AnimatedImageError::Unreadable(error.to_string()))?;
        total = total.saturating_add(frame.buffer().as_raw().len() as u64);
        if kept.len() == limits.max_frames || total > limits.max_total_bytes {
            return Err(AnimatedImageError::TooLarge);
        }
        kept.push(frame);
    }
    Ok(kept)
}

/// A still image as a one-frame animation, decoded within `limits`.
fn still(bytes: &[u8], limits: AnimationLimits) -> Result<Vec<image::Frame>, AnimatedImageError> {
    let unreadable = |error: image::ImageError| AnimatedImageError::Unreadable(error.to_string());
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| AnimatedImageError::Unreadable(error.to_string()))?;
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(limits.max_edge);
    decode_limits.max_image_height = Some(limits.max_edge);
    decode_limits.max_alloc = Some(limits.max_total_bytes);
    reader.limits(decode_limits);
    let image = reader
        .decode()
        .map_err(|error| match error {
            image::ImageError::Limits(_) => AnimatedImageError::TooLarge,
            other => unreadable(other),
        })?
        .into_rgba8();
    Ok(vec![image::Frame::new(image)])
}

/// Builder for [`enz_animated_image`].
#[must_use]
pub struct EnzAnimatedImage {
    frames: AnimatedFrames,
    fit: EnzImageFit,
    alt_text: String,
    width: Length,
    height: Length,
    playing: bool,
}

/// An image playing `frames`.
pub fn enz_animated_image(frames: &AnimatedFrames) -> EnzAnimatedImage {
    EnzAnimatedImage {
        frames: frames.clone(),
        fit: EnzImageFit::Contain,
        alt_text: String::new(),
        width: Length::Shrink,
        height: Length::Shrink,
        playing: true,
    }
}

impl EnzAnimatedImage {
    /// How the frames fill the widget; default [`EnzImageFit::Contain`].
    pub fn fit(mut self, fit: EnzImageFit) -> Self {
        self.fit = fit;
        self
    }

    /// The image's accessible name; empty means decorative.
    pub fn alt_text(mut self, text: impl Into<String>) -> Self {
        self.alt_text = text.into();
        self
    }

    /// The width; by default the image's own.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// The height; by default the image's own.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Play (the default) or hold the current frame.
    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    /// Build the widget.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        Element::new(AnimatedImageWidget { image: self })
    }
}

impl<'a, Message: 'a> From<EnzAnimatedImage> for Element<'a, Message> {
    fn from(image: EnzAnimatedImage) -> Self {
        image.into_element()
    }
}

#[derive(Debug, Default)]
struct State {
    /// The frames being played. A different set (not a clone of these)
    /// starts again from its first frame; held weakly, so the allocation and
    /// its address stay unique while this is held.
    playing: Option<std::sync::Weak<[(iced_image::Handle, Duration)]>>,
    /// The frame on screen.
    frame: usize,
    /// When the frame on screen began; `None` while paused.
    shown_at: Option<Instant>,
    /// The theme's reduced-motion policy, read when drawing.
    theme_reduced: Cell<bool>,
}

struct AnimatedImageWidget {
    image: EnzAnimatedImage,
}

impl AnimatedImageWidget {
    fn animates(&self) -> bool {
        self.image.playing && self.image.frames.len() > 1
    }

    fn reduced(state: &State) -> bool {
        state.theme_reduced.get() || iced_core::preferences::get().reduced_motion
    }

    fn image_size(&self) -> Size {
        let size = self.image.frames.size();
        Size::new(size.width as f32, size.height as f32)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for AnimatedImageWidget {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.image.width, self.image.height)
    }

    fn frame_interest(&self, _tree: &Tree) -> bool {
        self.animates()
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let image = self.image_size();
        let (width, height) = (self.image.width, self.image.height);
        let size = limits
            .width(width)
            .height(height)
            .resolve(width, height, image);
        // A shrinking side takes the image's fitted size, keeping its ratio.
        let fitted = self.image.fit.to_iced().fit(image, size);
        tree.size = Size::new(
            if width == Length::Shrink {
                fitted.width
            } else {
                size.width
            },
            if height == Length::Shrink {
                fitted.height
            } else {
                size.height
            },
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let state = tree.state.downcast_mut::<State>();
        let frames = &self.image.frames;
        let same = state.playing.as_ref().is_some_and(|playing| {
            std::ptr::addr_eq(playing.as_ptr(), Arc::as_ptr(&frames.frames))
        });
        if !same {
            state.playing = Some(Arc::downgrade(&frames.frames));
            state.frame = 0;
            state.shown_at = None;
        }
        if state.frame >= frames.len() {
            state.frame = 0;
        }
        let visible = layout.bounds().intersects(viewport);
        if Self::reduced(state) {
            state.frame = 0;
            state.shown_at = None;
            return;
        }
        if !self.animates() || !visible {
            // Paused: resume from this frame when it plays again.
            state.shown_at = None;
            return;
        }
        let shown_at = *state.shown_at.get_or_insert(*now);
        let mut shown_at = shown_at;
        let mut advanced = false;
        // Catch up on frames the compositor did not draw, at most one loop.
        for _ in 0..frames.len() {
            let delay = frames.delay(state.frame).unwrap_or(SHORT_DELAY_FALLBACK);
            if *now < shown_at + delay {
                break;
            }
            shown_at += delay;
            state.frame = (state.frame + 1) % frames.len();
            advanced = true;
        }
        if *now >= shown_at + frames.delay(state.frame).unwrap_or(SHORT_DELAY_FALLBACK) {
            // Far behind (a long pause): restart the frame's clock.
            shown_at = *now;
        }
        state.shown_at = Some(shown_at);
        if advanced {
            shell.request_redraw();
        }
        let next = shown_at + frames.delay(state.frame).unwrap_or(SHORT_DELAY_FALLBACK);
        shell.schedule_redraw_at(next);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        state
            .theme_reduced
            .set(crate::theme_lookup::reduced_motion(theme));
        let index = if Self::reduced(state) { 0 } else { state.frame };
        let Some((handle, _)) = self.image.frames.frames.get(index) else {
            return;
        };
        let bounds = layout.bounds();
        let drawn = self
            .image
            .fit
            .to_iced()
            .fit(self.image_size(), bounds.size());
        let drawn = Rectangle::new(
            iced::Point::new(
                bounds.center_x() - drawn.width / 2.0,
                bounds.center_y() - drawn.height / 2.0,
            ),
            drawn,
        );
        renderer.draw_image(iced_image::Image::new(handle.clone()), drawn, bounds);
    }

    fn operate(
        &mut self,
        _tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.image.alt_text.trim().is_empty() {
            return;
        }
        let mut node =
            SemanticsNode::new(Semantics::new(Role::Image).label(self.image.alt_text.clone()));
        operation.custom(None, layout.bounds(), &mut node);
    }
}
