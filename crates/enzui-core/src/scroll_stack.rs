//! Tracks nested scroll translations and viewport intersections during widget-operation traversal.
//!
//! Shared by core's semantic collection and the widgets' selection scope so
//! both normalize scroll offsets identically; not part of the app-facing API.

use iced_core::widget::Id;
use iced_core::{Rectangle, Vector};

#[derive(Debug, Clone, Copy)]
struct Frame {
    bounds: Rectangle,
    translation: Vector,
    // The frame is popped when traversal returns above this depth.
    pushed_at_depth: u32,
}

// Tracks active ancestor scrollables so bounds can be normalized and clipped during an operation walk.
#[derive(Debug, Default)]
pub struct ScrollStack {
    stack: Vec<Frame>,
    // Frame awaiting the next traversal entry for promotion onto the active stack.
    pending: Option<Frame>,
    depth: u32,
}

impl ScrollStack {
    pub fn new() -> Self {
        Self::default()
    }

    // Records a scrollable whose parent-content bounds must be normalized before the next traversal descent.
    pub fn note_scrollable(&mut self, _id: Option<&Id>, bounds: Rectangle, translation: Vector) {
        // Record the next depth so the matching traversal exit pops this frame.
        self.pending = Some(Frame {
            // Normalize through ancestors before pushing, excluding this frame’s own translation.
            bounds: self.translate(bounds),
            translation,
            pushed_at_depth: self.depth + 1,
        });
    }

    pub fn traverse_enter(&mut self) {
        self.depth += 1;
        if let Some(mut frame) = self.pending.take() {
            frame.pushed_at_depth = self.depth;
            self.stack.push(frame);
        }
    }

    pub fn traverse_exit(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        while let Some(top) = self.stack.last() {
            if top.pushed_at_depth > self.depth {
                self.stack.pop();
            } else {
                break;
            }
        }
    }

    pub fn translate(&self, rect: Rectangle) -> Rectangle {
        let mut r = rect;
        for frame in &self.stack {
            r.x -= frame.translation.x;
            r.y -= frame.translation.y;
        }
        r
    }

    // Returns the screen-space intersection of active ancestor viewports, or `None` outside a scrollable.
    pub fn viewport(&self) -> Option<Rectangle> {
        let mut it = self.stack.iter();
        let first = it.next()?.bounds;
        let mut clip = first;
        for frame in it {
            clip = intersect(clip, frame.bounds);
        }
        Some(clip)
    }

    pub fn intersects_viewport(&self, rect: Rectangle) -> bool {
        match self.viewport() {
            None => true,
            Some(v) => {
                let i = intersect(v, rect);
                i.width > 0.0 && i.height > 0.0
            }
        }
    }
}

fn intersect(a: Rectangle, b: Rectangle) -> Rectangle {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    Rectangle {
        x,
        y,
        width: (right - x).max(0.0),
        height: (bottom - y).max(0.0),
    }
}
