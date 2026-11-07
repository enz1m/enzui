//! The continuously scrolling time window behind [`EnzAreaChart::live`].
//!
//! A live chart's points are ages in seconds relative to the newest sample.
//! The timeline slides left at the window's speed, so the history moves
//! smoothly instead of jumping once per sample. The chart shows the window
//! ending one sample interval ago: a new sample is drawn just beyond the
//! right edge and slides in, rather than appearing at it, and the edge
//! holds the newest value if the next sample is late.
//!
//! The chart around the plot (grid, axes, labels) is painted once; the
//! areas are drawn once per sample into a node the render lane moves, so
//! between samples the UI thread does not draw. The slide moves in
//! whole-pixel steps measured from one origin for every sample (each step
//! repaints the curve), so the last sample's slide and the next agree, and
//! it stops when the data goes stale (three sample intervals without a
//! sample). Under reduced
//! motion the samples are drawn unmoved. The surrounding [`ViewportGate`]
//! stops it off-screen.
//!
//! [`EnzAreaChart::live`]: crate::area_chart::EnzAreaChart::live
//! [`ViewportGate`]: crate::viewport_gate::ViewportGate

use crate::area_chart::{AreaChartKey, AreaChartProgram};
use enzui_core::chart::Series;
use iced::advanced::graphics::geometry::Renderer as _;
use iced::advanced::renderer::{NodeId, Track};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::time::{Duration, Instant};
use iced::widget::canvas;
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Vector};
use std::cell::{Cell, RefCell};

/// The window a live chart scrolls through.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LiveWindow {
    /// The window's length in seconds.
    pub seconds: f64,
    /// When the newest sample was taken.
    pub sampled_at: Instant,
}

/// How far the slide moves between two presented frames, in logical pixels:
/// every presented step repaints the whole curve.
const STEP: f32 = 1.0;

pub(crate) struct LiveAreaChart {
    program: AreaChartProgram,
    window: LiveWindow,
}

/// The painted layers and what the lane was handed.
struct State {
    node: NodeId,
    chrome: canvas::Cache,
    chrome_key: RefCell<Option<AreaChartKey>>,
    areas: canvas::Cache,
    /// What the areas were drawn for: the key, the newest sample's instant
    /// and whether they were drawn to slide.
    areas_for: RefCell<Option<(AreaChartKey, Instant, bool)>>,
    /// The sample whose areas the last draw recorded in the node.
    recorded: Cell<Option<Instant>>,
    /// The sample whose slide the lane was handed.
    published: Option<Instant>,
    generation: u64,
    /// The latest presented frame's instant; before the first frame the
    /// newest sample's instant stands in.
    frame_at: Option<Instant>,
    /// Where the timeline's offset is measured from. One origin for every
    /// sample keeps the last sample's slide, still playing when the next
    /// sample's areas are recorded, placing them right.
    origin: Cell<Option<Instant>>,
    /// The window's lag behind its newest sample, kept while the sample
    /// interval holds, so jitter between samples does not nudge the curve.
    delay: Cell<Option<f64>>,
    /// What the last draw read: reduced motion, and the plot's width.
    reduced: Cell<bool>,
    plot_width: Cell<f32>,
}

impl LiveAreaChart {
    pub(crate) fn new(program: AreaChartProgram, window: LiveWindow) -> Self {
        Self { program, window }
    }

    /// The newest sample interval, if the data has two samples.
    fn interval(&self) -> Option<f64> {
        self.program
            .series()
            .iter()
            .filter_map(|series| {
                let n = series.points.len();
                (n >= 2).then(|| series.points[n - 1].x - series.points[n - 2].x)
            })
            .filter(|interval| interval.is_finite() && *interval > 0.0)
            .fold(None, |shortest: Option<f64>, interval| {
                Some(shortest.map_or(interval, |shortest| shortest.min(interval)))
            })
    }

    /// How long the data stays fresh after its newest sample: three of its
    /// own sample intervals, or the whole window when the spacing is unknown.
    fn fresh_for(&self) -> f64 {
        self.interval()
            .map_or(self.window.seconds, |interval| 3.0 * interval)
    }

    /// How far behind its newest sample the window ends: one interval, the
    /// one `state` keeps unless the interval moved by a quarter.
    fn delay(&self, state: &State) -> f64 {
        let interval = self.interval().unwrap_or(0.0);
        match state.delay.get() {
            Some(kept) if (interval - kept).abs() <= kept * 0.25 => kept,
            _ => {
                state.delay.set(Some(interval));
                interval
            }
        }
    }

    /// Where the timeline's slide is measured from: one origin for every
    /// sample, moved to the newest sample once an hour old.
    fn origin(&self, state: &State) -> Instant {
        let sampled_at = self.window.sampled_at;
        match state.origin.get() {
            Some(origin) if sampled_at.saturating_duration_since(origin).as_secs() < 3600 => origin,
            _ => {
                state.origin.set(Some(sampled_at));
                sampled_at
            }
        }
    }

    /// How far right of the origin the newest sample's areas are drawn, so
    /// the slide's offset is one function of time across samples.
    fn lead(&self, origin: Instant, width: f32) -> f32 {
        (self
            .window
            .sampled_at
            .saturating_duration_since(origin)
            .as_secs_f64()
            * f64::from(self.speed(width))) as f32
    }

    /// Whether the timeline slides at all: motion allowed, and a series to
    /// slide.
    fn slides(&self, reduced: bool) -> bool {
        !reduced
            && self
                .program
                .series()
                .iter()
                .any(|series| series.points.len() > 1)
    }

    /// Whether it is sliding at `now`: it slides and the data is fresh.
    fn moving(&self, reduced: bool, now: Instant) -> bool {
        self.slides(reduced)
            && now
                .saturating_duration_since(self.window.sampled_at)
                .as_secs_f64()
                < self.fresh_for()
    }

    /// Logical pixels a second the timeline moves across a plot `width`.
    fn speed(&self, width: f32) -> f32 {
        (f64::from(width) / self.window.seconds) as f32
    }

    /// The slide's offset at `now`, in whole steps from the origin; it
    /// rests where the data goes stale.
    fn offset(&self, origin: Instant, width: f32, now: Instant) -> Vector {
        let stale = self.window.sampled_at + Duration::from_secs_f64(self.fresh_for());
        self.drift(origin, width).position(now.min(stale))
    }

    /// The timeline's slide across a plot `width`, from `origin`.
    fn drift(&self, origin: Instant, width: f32) -> crate::motion::Drift {
        crate::motion::Drift {
            origin,
            speed: self.speed(width),
            direction: Vector::new(-1.0, 0.0),
            step: STEP,
        }
    }

    /// How far right of the plot the slid areas reach.
    fn overhang(&self, width: f32, delay: f64) -> f32 {
        self.speed(width) * (delay + self.fresh_for()) as f32 + 2.0 * STEP
    }

    /// The series to draw: moved right by the delay when sliding, the newest
    /// value held past the right edge for as long as the data is fresh, and
    /// smoothed for the chart's curve.
    fn areas(&self, slides: bool, delay: f64) -> Vec<Series> {
        let (shift, hold) = if slides {
            (delay, self.fresh_for())
        } else {
            (0.0, 0.0)
        };
        self.program
            .series()
            .iter()
            .map(|series| {
                let mut points: Vec<(f64, f64)> = series
                    .points
                    .iter()
                    .map(|point| (point.x + shift, point.y))
                    .collect();
                if let Some(&(x, y)) = points.last()
                    && hold > 0.0
                {
                    points.push((x + hold, y));
                }
                let points = crate::area_chart::curve_points(&points, self.program.edge_curve());
                Series::from_points(series.id, points)
            })
            .collect()
    }
}

impl<Message> Widget<Message, iced::Theme, Renderer> for LiveAreaChart {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State {
            node: crate::motion::next_scene_node(),
            chrome: canvas::Cache::new(),
            chrome_key: RefCell::new(None),
            areas: canvas::Cache::new(),
            areas_for: RefCell::new(None),
            recorded: Cell::new(None),
            published: None,
            generation: 0,
            frame_at: None,
            origin: Cell::new(None),
            delay: Cell::new(None),
            reduced: Cell::new(false),
            plot_width: Cell::new(0.0),
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) {
        tree.size = layout::atomic(limits, Length::Fill, Length::Fill);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let state = tree.state.downcast_ref::<State>();
        self.moving(state.reduced.get(), Instant::now())
            && (state.recorded.get() != Some(self.window.sampled_at)
                || state.published != Some(self.window.sampled_at))
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let state = tree.state.downcast_mut::<State>();
        state.frame_at = Some(*now);
        let sampled_at = self.window.sampled_at;
        if !self.moving(state.reduced.get(), *now) {
            return;
        }
        if state.recorded.get() != Some(sampled_at) {
            // This frame's draw records the sample's areas in the node; the
            // slide below reaches the lane with them.
            shell.request_redraw();
        }
        // Published on every frame the UI thread draws while the timeline
        // slides: the lane plays only what the latest frame handed it, and
        // a frame drawn for something else (a sparkline easing in) must not
        // leave the slide behind, nor a new sample's areas under the last
        // sample's position. The same generation continues it.
        let first = state.published != Some(sampled_at);
        if first {
            state.generation += 1;
        }
        let width = state.plot_width.get();
        let node = state.node;
        let origin = self.origin(state);
        shell.publish_track(Track {
            translation: self.offset(origin, width, *now),
            ..Track::identity(node)
        });
        let fresh = Duration::from_secs_f64(self.fresh_for());
        if let Some(animation) =
            self.drift(origin, width)
                .animation(node, sampled_at, fresh, state.generation)
        {
            shell.publish_track_animation(animation);
        }
        if first {
            state.published = Some(sampled_at);
            // The UI thread draws the timeline again where it stops.
            shell.schedule_redraw_at(sampled_at + fresh);
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;

        let state = tree.state.downcast_ref::<State>();
        let bounds = layout.bounds();
        let size = bounds.size();
        let reduced = crate::theme_lookup::motion(theme).reduced_motion;
        state.reduced.set(reduced);
        let key = self.program.key(theme, size);
        let plot = crate::area_chart::plot_area(size, key);
        state.plot_width.set(plot.width);

        if state.chrome_key.borrow().as_ref() != Some(&key) {
            state.chrome.clear();
            *state.chrome_key.borrow_mut() = Some(key);
        }
        let chrome = state.chrome.draw(renderer, size, |frame| {
            let _ = crate::area_chart::draw_static(frame, size, &[], key);
        });

        // Once the data is stale the timeline rests where its slide ended.
        let slides = self.slides(reduced);
        let drawn_for = (key, self.window.sampled_at, slides);
        if state.areas_for.borrow().as_ref() != Some(&drawn_for) {
            state.areas.clear();
            *state.areas_for.borrow_mut() = Some(drawn_for);
        }
        // Drawn wider than the chart by what slides in from the right: a
        // frame clips its geometry to its size.
        let delay = self.delay(state);
        let overhang = if slides {
            self.overhang(plot.width, delay)
        } else {
            0.0
        };
        let areas = state.areas.draw(
            renderer,
            Size::new(size.width + overhang, size.height),
            |frame| {
                crate::area_chart::draw_marks(frame, size, &self.areas(slides, delay), key);
            },
        );
        let (lead, offset) = if slides {
            let origin = self.origin(state);
            (
                self.lead(origin, plot.width),
                self.offset(
                    origin,
                    plot.width,
                    state.frame_at.unwrap_or(self.window.sampled_at),
                ),
            )
        } else {
            (0.0, Vector::ZERO)
        };

        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            renderer.draw_geometry(chrome);
        });
        // The areas slide inside the plot; the edge's stroke may reach a
        // pixel past its top and bottom. The lane moves in what lay past the
        // right edge when they were recorded: the retained copy keeps it.
        let clip = Rectangle::new(
            iced::Point::new(bounds.x + plot.x, bounds.y - 2.0),
            Size::new(plot.width, plot.height + 4.0),
        );
        let retained = Rectangle::new(
            clip.position(),
            Size::new(clip.width + overhang, clip.height),
        );
        renderer.with_layer_retained(clip, retained, |renderer| {
            renderer.with_motion(state.node, offset, |renderer| {
                renderer.with_translation(Vector::new(bounds.x + lead, bounds.y), |renderer| {
                    renderer.draw_geometry(areas);
                });
            });
        });
        state.recorded.set(Some(self.window.sampled_at));
    }
}

impl<'a, Message: 'a> From<LiveAreaChart> for Element<'a, Message> {
    fn from(chart: LiveAreaChart) -> Self {
        Element::new(chart)
    }
}
