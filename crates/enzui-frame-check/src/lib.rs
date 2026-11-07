//! Frame-by-frame check that an animated view replays what a full redraw
//! draws.
//!
//! An animation that only moves, fades, scales or clips something is usually
//! not drawn again each frame: the runtime replays the frame it recorded and
//! moves the recorded paint by the tracks the widgets publish, on the UI
//! thread or on the render lane. A widget whose recording or tracks are wrong
//! then shows something no draw would: a panel cut off, missing, or painted
//! over a neighbour. Such a frame passes unnoticed in tests that only draw.
//!
//! [`FrameCheck`] runs a view twice in lockstep on the GPU. One copy replays
//! its recording on every frame the runtime would replay, the other draws
//! every frame, and each simulated frame at the whole-view size is compared.
//! This headless GPU harness does not measure native compositor presentation:
//!
//! ```no_run
//! # use iced::{Element, Size};
//! # fn view(open: bool) -> Element<'static, ()> { iced::widget::text("").into() }
//! use enzui_frame_check::FrameCheck;
//!
//! let Some(mut check) = FrameCheck::new(Size::new(800.0, 600.0), iced::Theme::Dark, || view(false))
//! else {
//!     return; // no GPU adapter
//! };
//! check.frames(30);
//! check.view(|| view(true));
//! check.frames(120);
//! check.assert_replays_match();
//! assert!(check.replays() > 60, "the animation reaches replay");
//! ```
//!
//! With `ENZUI_FRAME_CHECK_DUMP=<dir>` both renderings of every differing
//! frame are written there as PNG files.

use iced::advanced::renderer::Headless as _;
use iced::time::{Duration, Instant};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::runtime::user_interface::{self, Cache, State, UserInterface};

/// A colour channel must differ by more than this for a replayed pixel to
/// count as wrong: rasterisation may round differently, never this much.
const TOLERANCE: u8 = 24;

/// A frame counts as mismatched above this many differing pixels: a lone
/// pixel is a rasterisation race, not a widget's fault.
const STRAY_PIXELS: usize = 2;

/// A colour channel must change by more than this between drawn frames for
/// the frame to count as moved: a soft gradient advancing a pixel changes
/// each pixel only a little.
const MOTION: u8 = 1;

type Ui<Message> = UserInterface<'static, Message, iced::Theme, iced::Renderer>;

/// A frame whose replay differed from a full redraw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mismatch {
    /// The frame, counted from the first one this check presented.
    pub frame: usize,
    /// How many pixels differ.
    pub pixels: usize,
}

/// Two copies of one view: one replays like the runtime, one always draws.
pub struct FrameCheck<Message: 'static> {
    renderer: iced::Renderer,
    theme: iced::Theme,
    size: Size,
    /// The replaying copy, then the drawing one.
    uis: [Option<Ui<Message>>; 2],
    scene: Option<iced::advanced::renderer::Recording>,
    now: Instant,
    period: Duration,
    frames: usize,
    replays: usize,
    mismatches: Vec<Mismatch>,
    messages: Vec<Message>,
    /// The last drawn frame and how many frames changed the picture.
    previous: Option<Vec<u8>>,
    changes: usize,
    content_frames: usize,
    frame_requests: usize,
    uncarried_tracks: usize,
}

impl<Message: 'static> FrameCheck<Message> {
    /// Builds both copies of `view` at `size`, or `None` when no GPU adapter
    /// is available.
    pub fn new(
        size: Size,
        theme: iced::Theme,
        view: impl Fn() -> Element<'static, Message>,
    ) -> Option<Self> {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        )?;
        let uis = [(); 2].map(|()| {
            Some(UserInterface::build(
                view(),
                size,
                Cache::default(),
                &mut renderer,
            ))
        });
        Some(Self {
            renderer,
            theme,
            size,
            uis,
            scene: None,
            now: Instant::now(),
            period: Duration::from_micros(6944),
            frames: 0,
            replays: 0,
            mismatches: Vec::new(),
            messages: Vec::new(),
            previous: None,
            changes: 0,
            content_frames: 0,
            frame_requests: 0,
            uncarried_tracks: 0,
        })
    }

    /// Sets the time between presented frames; 144 Hz by default.
    #[must_use]
    pub fn period(mut self, period: Duration) -> Self {
        self.period = period;
        self
    }

    /// Replaces the view, as an application does after a message, keeping
    /// every widget's state.
    pub fn view(&mut self, view: impl Fn() -> Element<'static, Message>) {
        for ui in &mut self.uis {
            let cache = ui.take().map(UserInterface::into_cache).unwrap_or_default();
            *ui = Some(UserInterface::build(
                view(),
                self.size,
                cache,
                &mut self.renderer,
            ));
        }
    }

    /// Delivers `event` to both copies at `cursor`.
    pub fn event(&mut self, event: Event, cursor: mouse::Cursor) {
        let _ = self.update(&[event], cursor);
    }

    /// Scrolls by `pixels` (positive scrolls down) with the pointer at `at`.
    pub fn scroll(&mut self, at: Point, pixels: f32) {
        self.event(
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -pixels },
            }),
            mouse::Cursor::Available(at),
        );
    }

    /// Presents `count` frames, one period apart.
    pub fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.now += self.period;
            self.frame();
        }
    }

    /// The frames presented so far.
    pub fn presented(&self) -> usize {
        self.frames
    }

    /// The frames the replaying copy replayed instead of drawing.
    pub fn replays(&self) -> usize {
        self.replays
    }

    /// The frames whose full redraw differed from the frame before: how many
    /// frames an animation actually moved on.
    pub fn changed(&self) -> usize {
        self.changes
    }

    /// The frames that asked for another frame without being animation-only:
    /// each such request costs a full redraw, so a page that has settled
    /// should make none (a spinner or a playing image is the exception).
    pub fn content_frames(&self) -> usize {
        self.content_frames
    }

    /// The frames after which the UI thread asked to run again on the next
    /// frame, animation-only or not: what a widget costs the UI thread. A
    /// motion the render lane plays on its own asks for none.
    pub fn frame_requests(&self) -> usize {
        self.frame_requests
    }

    /// The tracks published with no live travel for their node: motion the
    /// render lane cannot play on its own, so each one needs the UI thread
    /// on that frame.
    pub fn uncarried_tracks(&self) -> usize {
        self.uncarried_tracks
    }

    /// Every frame whose replay differed from the full redraw.
    pub fn mismatches(&self) -> &[Mismatch] {
        &self.mismatches
    }

    /// The messages the view published, once per delivery.
    pub fn take_messages(&mut self) -> Vec<Message> {
        std::mem::take(&mut self.messages)
    }

    /// Asserts that every replayed frame drew what the full redraw drew.
    ///
    /// # Panics
    ///
    /// When any did, listing each frame and how many pixels differed.
    #[track_caller]
    pub fn assert_replays_match(&self) {
        assert!(
            self.mismatches.is_empty(),
            "{} of {} frames replayed differently from a full redraw \
             (frame, pixels): {:?}; set ENZUI_FRAME_CHECK_DUMP=<dir> to see them",
            self.mismatches.len(),
            self.frames,
            self.mismatches
                .iter()
                .map(|mismatch| (mismatch.frame, mismatch.pixels))
                .collect::<Vec<_>>(),
        );
    }

    fn update(&mut self, events: &[Event], cursor: mouse::Cursor) -> [Option<State>; 2] {
        let renderer = &mut self.renderer;
        let mut published = Vec::new();
        let states = self.uis.each_mut().map(|ui| {
            let ui = ui.as_mut()?;
            let mut bus = iced::advanced::shell::Bus::new();
            let (state, _) = ui.update(
                &window::Headless,
                &iced_test::core::shell::Waker::noop(),
                events,
                cursor,
                renderer,
                &mut bus,
            );
            published = bus.drain_messages().collect();
            Some(state)
        });
        // Both copies publish the same messages; keep one set.
        self.messages.extend(published);
        states
    }

    fn frame(&mut self) {
        let states = self.update(
            &[Event::Window(window::Event::RedrawRequested(self.now))],
            mouse::Cursor::Unavailable,
        );
        let style = iced::advanced::renderer::Style {
            text_color: iced::theme::Base::base(&self.theme).text_color,
        };
        let cursor = mouse::Cursor::Unavailable;
        let width = self.size.width as u32;
        let height = self.size.height as u32;
        let mut shots = Vec::with_capacity(2);
        for (index, state) in states.into_iter().enumerate() {
            let (tracks, animation_only) = match state {
                Some(State::Updated {
                    tracks,
                    animations,
                    is_animation_only,
                    redraw_request,
                    ..
                }) => {
                    if index == 1 {
                        let now = self.now;
                        self.uncarried_tracks += tracks
                            .iter()
                            .filter(|track| {
                                !animations.iter().any(|animation| {
                                    animation.node == track.node
                                        && animation.is_usable()
                                        && !animation.is_finished(now)
                                })
                            })
                            .count();
                    }
                    let next = matches!(redraw_request, window::RedrawRequest::NextFrame);
                    if index == 1 && next {
                        self.frame_requests += 1;
                        if !is_animation_only {
                            self.content_frames += 1;
                        }
                    }
                    (tracks, is_animation_only)
                }
                _ => (Vec::new(), false),
            };
            let Some(ui) = self.uis[index].as_mut() else {
                continue;
            };
            if index == 0 {
                let replayed = animation_only
                    && !tracks.is_empty()
                    && self.scene.as_ref().is_some_and(|scene| {
                        user_interface::replay_retained(
                            &mut self.renderer,
                            self.size,
                            scene,
                            &tracks,
                        )
                    });
                if replayed {
                    self.replays += 1;
                } else {
                    self.scene = ui.draw_retained(&mut self.renderer, &self.theme, &style, cursor);
                }
            } else {
                ui.draw(&mut self.renderer, &self.theme, &style, cursor);
            }
            shots.push(
                self.renderer
                    .screenshot(Size::new(width, height), 1.0, iced::Color::BLACK),
            );
        }
        if let [replayed, drawn] = shots.as_slice() {
            let pixels = differing(replayed, drawn, TOLERANCE);
            if self
                .previous
                .as_deref()
                .is_some_and(|previous| differing(previous, drawn, MOTION) > 0)
            {
                self.changes += 1;
            }
            self.previous = Some(drawn.clone());
            if pixels > STRAY_PIXELS {
                self.mismatches.push(Mismatch {
                    frame: self.frames,
                    pixels,
                });
                dump(self.frames, width, height, &shots);
            }
        }
        self.frames += 1;
    }
}

fn differing(a: &[u8], b: &[u8], tolerance: u8) -> usize {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a.iter().zip(*b).any(|(x, y)| x.abs_diff(*y) > tolerance))
        .count()
}

fn dump(frame: usize, width: u32, height: u32, shots: &[Vec<u8>]) {
    let Some(directory) = std::env::var_os("ENZUI_FRAME_CHECK_DUMP") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    if std::fs::create_dir_all(&directory).is_err() {
        return;
    }
    for (name, shot) in ["replayed", "drawn"].iter().zip(shots) {
        if let Some(image) = image::RgbaImage::from_raw(width, height, shot.clone()) {
            let _ = image.save(directory.join(format!("{frame:04}_{name}.png")));
        }
    }
}
