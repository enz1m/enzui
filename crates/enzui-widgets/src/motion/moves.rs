// motion split production module: layout moves of keyed children

//! Children that slide from where they were to where a new layout puts them,
//! for a collection whose children are inserted, removed or reordered.
//!
//! The collection lays out exactly every frame; only the paint travels. When
//! the children change, the next frame compares each child's place in the
//! collection with its last one, and a child that moved starts from where it
//! was shown, on a spring, towards where it now is. Each moving child is
//! recorded under its own scene node and publishes a track, so the render
//! lane carries the slide. A layout that moves children without the children
//! changing (a resize, a reflow) is not animated.

use super::{SpringAnimation, next_scene_node};
use enzui_theme::{MotionScale, Spring};
use iced::advanced::renderer::{NodeId, Track};
use iced::advanced::{Renderer as _, Shell};
use iced::time::Instant;
use iced::{Point, Rectangle, Vector};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

/// Where each keyed child of a collection is shown relative to where layout
/// puts it.
pub(crate) struct Moves {
    spring: Spring,
    /// Each child's move, noted by every draw so a change compares with
    /// where the child was last shown.
    children: RefCell<BTreeMap<String, Move>>,
    /// The children changed since the last frame, so the next frame's moves
    /// are animated.
    changed: bool,
    /// The latest frame instant.
    now: Cell<Instant>,
    /// The theme's motion policy, from the last draw.
    scale: Cell<Option<MotionScale>>,
}

struct Move {
    node: NodeId,
    /// Where layout last put the child, relative to the collection.
    at: Vector,
    /// How far from `at` the child is shown.
    offset: SpringAnimation<Vector>,
    /// The offset the last draw painted the child at under `node`; `None`
    /// when it was painted in place, outside any node.
    recorded: Cell<Option<Vector>>,
    /// The last draw found the child's whole travel out of view, so its
    /// frames show nothing.
    hidden: Cell<bool>,
}

impl Move {
    fn new(at: Vector, spring: Spring) -> Self {
        Self {
            node: next_scene_node(),
            at,
            offset: SpringAnimation::new(Vector::ZERO, spring).precision(1e-2),
            recorded: Cell::new(None),
            hidden: Cell::new(false),
        }
    }

    fn track(&self, offset: Vector) -> Track {
        Track {
            node: self.node,
            translation: offset,
            opacity: 1.0,
            scale: 1.0,
            scale_origin: Point::ORIGIN,
            rotation: 0.0,
            clip: None,
        }
    }
}

impl Moves {
    pub(crate) fn new(spring: Spring) -> Self {
        Self {
            spring,
            children: RefCell::new(BTreeMap::new()),
            changed: false,
            now: Cell::new(Instant::now()),
            scale: Cell::new(None),
        }
    }

    /// The spring later moves travel on.
    pub(crate) fn set_spring(&mut self, spring: Spring) {
        self.spring = spring;
    }

    /// The children are now `keys`: the next frame animates the moves this
    /// causes, and children no longer there are forgotten.
    pub(crate) fn change(&mut self, keys: &[String]) {
        self.changed = true;
        self.children
            .get_mut()
            .retain(|key, _| keys.iter().any(|kept| kept == key));
    }

    /// Whether the next frame has anything to do.
    pub(crate) fn frame_interest(&self) -> bool {
        let now = self.now.get();
        self.changed
            || self.children.borrow().values().any(|child| {
                child.offset.is_animating(now)
                    || child
                        .recorded
                        .get()
                        .is_some_and(|recorded| recorded != child.offset.value(now))
            })
    }

    /// Advances to the frame at `now`, where layout puts each keyed child at
    /// its place in `places`, relative to the collection: starts the moves
    /// a change caused and publishes the tracks of every moving child.
    pub(crate) fn frame<'a, Message>(
        &mut self,
        now: Instant,
        places: impl Iterator<Item = (&'a String, Vector)>,
        shell: &mut Shell<'_, Message>,
    ) {
        self.now.set(now);
        let changed = std::mem::take(&mut self.changed);
        let scale = self.scale.get();
        let spring = self.spring;
        let children = self.children.get_mut();
        for (key, at) in places {
            let Some(child) = children.get_mut(key) else {
                let _ = children.insert(key.clone(), Move::new(at, spring));
                continue;
            };
            if child.at == at {
                continue;
            }
            // Where the child was shown, from where it is laid out now.
            let shown = child.at - at + child.offset.value(now);
            let velocity = child.offset.velocity(now);
            child.at = at;
            if !changed {
                // Layout moved it without the children changing: it is
                // shown in its new place, a move under way included.
                continue;
            }
            if child.offset.spring() != spring {
                child.offset.set_spring(spring, now);
            }
            // The theme's speed, and under reduced motion a move that lands
            // at once.
            if let Some(scale) = scale {
                child.offset.sync_motion(&scale);
            }
            child.offset.release(shown, velocity, Vector::ZERO, now);
        }
        for child in children.values() {
            let animating = child.offset.is_animating(now);
            let offset = child.offset.value(now);
            let recorded = child.recorded.get();
            if animating && !child.hidden.get() {
                crate::motion::request_motion_frame(shell, recorded.is_some());
                // The lane may carry the rest of the move without waking
                // this thread: wake where it lands to record it in place.
                if let Some(lands) = child.offset.lands_at() {
                    shell.schedule_redraw_at(lands);
                }
            } else if !animating && recorded.is_some_and(|recorded| recorded != offset) {
                // Settled away from where it was recorded: one drawn frame
                // records it in place.
                shell.request_redraw();
            }
            // A recording made mid-travel replays where it was painted unless
            // a track says where the child is now.
            if recorded.is_some() && (animating || recorded != Some(offset)) {
                shell.publish_track(child.track(offset));
                child
                    .offset
                    .publish(shell, child.node, now, |offset| child.track(offset));
            }
        }
    }

    /// How far from its laid-out place the child with `key` is shown.
    pub(crate) fn offset(&self, key: &str) -> Vector {
        self.children
            .borrow()
            .get(key)
            .map_or(Vector::ZERO, |child| child.offset.value(self.now.get()))
    }

    /// Notes, as the collection is drawn, the theme's motion policy and
    /// where layout puts each keyed child, relative to the collection: where
    /// the next change moves them from.
    pub(crate) fn note<'a>(
        &self,
        scale: MotionScale,
        places: impl Iterator<Item = (&'a String, Vector)>,
    ) {
        self.scale.set(Some(scale));
        let mut children = self.children.borrow_mut();
        for (key, at) in places {
            match children.get_mut(key) {
                Some(child) => child.at = at,
                None => {
                    let _ = children.insert(key.clone(), Move::new(at, self.spring));
                }
            }
        }
    }

    /// Notes that the child with `key` was painted in place, outside its
    /// node.
    pub(crate) fn drawn_in_place(&self, key: &str) {
        if let Some(child) = self.children.borrow().get(key) {
            child.recorded.set(None);
        }
    }

    /// Paints the child with `key`, laid out at `bounds`: in place when it is
    /// still, otherwise shown at its offset under its own node, kept whole so
    /// a replay can move in what `viewport` cut off. A moving child whose
    /// travel stays out of `viewport` is not painted.
    pub(crate) fn draw(
        &self,
        key: &str,
        renderer: &mut iced::Renderer,
        bounds: Rectangle,
        viewport: &Rectangle,
        paint: impl FnOnce(&mut iced::Renderer, Vector),
    ) {
        let children = self.children.borrow();
        let Some(child) = children.get(key) else {
            paint(renderer, Vector::ZERO);
            return;
        };
        let now = self.now.get();
        let offset = child.offset.value(now);
        child.hidden.set(false);
        if !child.offset.is_animating(now) && offset == Vector::ZERO {
            child.recorded.set(None);
            if bounds.intersects(viewport) {
                paint(renderer, Vector::ZERO);
            }
            return;
        }
        // Where the child is shown on the rest of its way, with room for the
        // spring to overshoot.
        let reach = (offset.x.abs() + offset.y.abs()) / 4.0;
        if !(bounds + offset)
            .union(&bounds)
            .expand(reach)
            .intersects(viewport)
        {
            child.recorded.set(None);
            child.hidden.set(true);
            return;
        }
        let retained = (bounds + offset).union(viewport);
        renderer.with_layer_retained(*viewport, retained, |renderer| {
            renderer.with_motion(child.node, offset, |renderer| paint(renderer, offset));
        });
        child.recorded.set(Some(offset));
    }
}
