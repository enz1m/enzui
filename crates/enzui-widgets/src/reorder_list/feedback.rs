//! Live drag feedback for [`EnzReorderList`](super::EnzReorderList).
//!
//! While a drag is past its deadband the list shows it moving:
//!
//! - the dragged rows lift and follow the pointer, along the list's axis
//!   while the pointer is over this list, freely once it has left it;
//! - when this list is the destination, the other rows move apart to open a
//!   gap the size of the dragged rows at the proposed slot. A row from
//!   another collection whose size is unknown gets an insertion bar instead;
//! - when the rows are leaving for another collection, the source list
//!   closes their gap.
//!
//! Rows glide to each new place over the theme's `motion-fast` duration and
//! snap under reduced motion. When the drag ends, every row glides from where
//! it was shown to where it is laid out, so a cancelled row returns to its
//! place and a dropped row settles into its new one. Outside a drag and its
//! settling, rows follow layout at once, so a resize does not animate.

use crate::drag_drop_host::LiveDrag;
use enzui_core::drag::{CollectionId, DropTarget};
use iced::time::Instant;
use iced::{Point, Rectangle, Vector};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

/// The rows of a list as laid out, and how it flows.
#[derive(Debug, Clone, Copy)]
pub(super) struct Geometry<'a> {
    /// Each row's key, in child order.
    pub keys: &'a [String],
    /// Each row's laid-out bounds, in child order.
    pub rows: &'a [Rectangle],
    /// The list's own bounds.
    pub bounds: Rectangle,
    /// Whether rows stack along y.
    pub vertical: bool,
    /// Whether child 0 sits at the far end of the axis (a right-to-left row).
    pub reverse: bool,
    /// The gap between rows.
    pub spacing: f32,
    /// The columns of a wrapped grid, or `None` for a single row or column.
    pub columns: Option<usize>,
}

/// Where each row paints during a drag.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Plan {
    /// Per row in child order: its offset from its laid-out place, and
    /// whether it is lifted (dragged).
    pub rows: Vec<(Vector, bool)>,
    /// An insertion bar, when this list is the destination but the size of
    /// the incoming rows is unknown.
    pub bar: Option<Rectangle>,
    /// Whether this list is the drop destination.
    pub destination: bool,
}

impl Geometry<'_> {
    fn along(&self, rect: &Rectangle) -> (f32, f32) {
        if self.vertical {
            (rect.y, rect.height)
        } else {
            (rect.x, rect.width)
        }
    }

    fn axis(&self, distance: f32) -> Vector {
        if self.vertical {
            Vector::new(0.0, distance)
        } else {
            Vector::new(distance, 0.0)
        }
    }
}

/// Plan the frame for `live` in the list `me` laid out as `geometry`.
///
/// `pointer` is the cursor in the list's coordinate space. `incoming` is the
/// total extent along this list's axis of rows dragged in from another
/// collection, when known. `bar_width` is the insertion bar's thickness.
pub(super) fn plan(
    geometry: Geometry<'_>,
    me: &CollectionId,
    live: &LiveDrag,
    pointer: Option<Point>,
    incoming: Option<f32>,
    bar_width: f32,
) -> Plan {
    if let Some(columns) = geometry.columns {
        return plan_grid(geometry, columns, me, live, pointer, bar_width);
    }
    let count = geometry.rows.len();
    let dragged: Vec<bool> = geometry
        .keys
        .iter()
        .map(|key| &live.source == me && live.items.iter().any(|item| item.as_str() == key))
        .collect();
    let any_dragged = dragged.iter().any(|&d| d);
    let slot = match &live.destination {
        Some((collection, DropTarget::Insertion(slot))) if collection == me => {
            Some(slot.index().min(count))
        }
        _ => None,
    };
    let elsewhere = live
        .destination
        .as_ref()
        .is_some_and(|(collection, _)| collection != me);

    let delta = pointer.map_or(Vector::ZERO, |pointer| pointer - live.origin);
    // Along the list's axis while it targets this list; once the pointer has
    // left it, over another collection or none, the rows follow it freely.
    let left = live.destination.is_none()
        && pointer.is_some_and(|pointer| !geometry.bounds.contains(pointer));
    let lift = if elsewhere || left {
        delta
    } else if geometry.vertical {
        Vector::new(0.0, delta.y)
    } else {
        Vector::new(delta.x, 0.0)
    };

    let gap_size = if any_dragged {
        Some(
            geometry
                .rows
                .iter()
                .zip(&dragged)
                .filter(|(_, d)| **d)
                .map(|(rect, _)| geometry.along(rect).1 + geometry.spacing)
                .sum(),
        )
    } else {
        incoming.map(|extent| extent + geometry.spacing)
    };
    let gap = slot.and_then(|slot| {
        let before = (0..slot).filter(|&i| !dragged[i]).count();
        gap_size.map(|size| (before, size))
    });
    let rearrange = gap.is_some() || (any_dragged && elsewhere);

    let mut rows: Vec<(Vector, bool)> = dragged
        .iter()
        .map(|&d| {
            if d {
                (lift, true)
            } else {
                (Vector::ZERO, false)
            }
        })
        .collect();
    if rearrange && count > 0 {
        let start = geometry
            .rows
            .iter()
            .map(|rect| geometry.along(rect).0)
            .fold(f32::INFINITY, f32::min);
        let end = geometry
            .rows
            .iter()
            .map(|rect| {
                let (at, extent) = geometry.along(rect);
                at + extent
            })
            .fold(f32::NEG_INFINITY, f32::max);
        // Distance from the flow's first edge, laid out without the dragged
        // rows and with the gap at its slot.
        let mut flow = 0.0;
        let mut placed = 0;
        for (index, rect) in geometry.rows.iter().enumerate() {
            if dragged[index] {
                continue;
            }
            if let Some((before, size)) = gap
                && placed == before
            {
                flow += size;
            }
            let (at, extent) = geometry.along(rect);
            let target = if geometry.reverse {
                end - flow - extent
            } else {
                start + flow
            };
            rows[index].0 = geometry.axis(target - at);
            flow += extent + geometry.spacing;
            placed += 1;
        }
    }

    let bar = match (slot, gap) {
        (Some(slot), None) => Some(insertion_bar(geometry, slot, bar_width)),
        _ => None,
    };
    Plan {
        rows,
        bar,
        destination: slot.is_some(),
    }
}

/// [`plan`] for a grid: the rows are cells filled row by row, so the gap
/// opens cell by cell and the items after it move along the cells, wrapping
/// from the end of one row to the start of the next. The dragged items
/// follow the pointer freely.
fn plan_grid(
    geometry: Geometry<'_>,
    columns: usize,
    me: &CollectionId,
    live: &LiveDrag,
    pointer: Option<Point>,
    bar_width: f32,
) -> Plan {
    let count = geometry.rows.len();
    let dragged: Vec<bool> = geometry
        .keys
        .iter()
        .map(|key| &live.source == me && live.items.iter().any(|item| item.as_str() == key))
        .collect();
    let lifted = dragged.iter().filter(|&&d| d).count();
    let slot = match &live.destination {
        Some((collection, DropTarget::Insertion(slot))) if collection == me => {
            Some(slot.index().min(count))
        }
        _ => None,
    };
    let elsewhere = live
        .destination
        .as_ref()
        .is_some_and(|(collection, _)| collection != me);
    let delta = pointer.map_or(Vector::ZERO, |pointer| pointer - live.origin);

    let mut rows: Vec<(Vector, bool)> = dragged
        .iter()
        .map(|&d| {
            if d {
                (delta, true)
            } else {
                (Vector::ZERO, false)
            }
        })
        .collect();
    // From another collection the incoming count is unknown: a bar marks
    // the slot instead of a gap.
    let gap = slot.filter(|_| lifted > 0).map(|slot| {
        let before = (0..slot).filter(|&i| !dragged[i]).count();
        (before, lifted)
    });
    if (gap.is_some() || (lifted > 0 && elsewhere)) && count > 0 {
        let mut cell = 0;
        for (index, rect) in geometry.rows.iter().enumerate() {
            if dragged[index] {
                continue;
            }
            if let Some((before, size)) = gap
                && cell == before
            {
                cell += size;
            }
            rows[index].0 = grid_cell(geometry, columns, cell) - rect.position();
            cell += 1;
        }
    }
    let bar = match (slot, gap) {
        (Some(slot), None) => Some(grid_bar(geometry, columns, slot, bar_width)),
        _ => None,
    };
    Plan {
        rows,
        bar,
        destination: slot.is_some(),
    }
}

/// The top-left of grid cell `cell`: where the item laid out there sits, or
/// for a cell past the last item, a step on from the cells that exist.
fn grid_cell(geometry: Geometry<'_>, columns: usize, cell: usize) -> Point {
    let rows = geometry.rows;
    if let Some(rect) = rows.get(cell) {
        return rect.position();
    }
    let Some(first) = rows.first() else {
        return geometry.bounds.position();
    };
    let (row, column) = (cell / columns, cell % columns);
    let x = match rows.get(column) {
        Some(rect) => rect.x,
        None => first.x + column as f32 * (first.width + geometry.spacing),
    };
    let last_row = (rows.len() - 1) / columns;
    let last_row_top = rows[last_row * columns].y;
    let last_row_height = rows[last_row * columns..]
        .iter()
        .map(|rect| rect.height)
        .fold(0.0, f32::max);
    let y = match rows.get(row * columns) {
        Some(rect) => rect.y,
        None => last_row_top + (row - last_row) as f32 * (last_row_height + geometry.spacing),
    };
    Point::new(x, y)
}

/// A `thickness` bar down the leading edge of grid cell `slot`, centred in
/// the spacing before it.
fn grid_bar(geometry: Geometry<'_>, columns: usize, slot: usize, thickness: f32) -> Rectangle {
    let rows = geometry.rows;
    let Some(first) = rows.first() else {
        let b = geometry.bounds;
        return Rectangle::new(b.position(), iced::Size::new(thickness, b.height));
    };
    // The slot past a row's last item is drawn after that item, on its row,
    // rather than before the first item of the next row.
    let (anchor, after) = if slot > 0 && (slot.is_multiple_of(columns) || slot >= rows.len()) {
        (rows[slot - 1], true)
    } else {
        (rows.get(slot).copied().unwrap_or(*first), false)
    };
    let edge = if after {
        anchor.x + anchor.width + geometry.spacing / 2.0
    } else {
        anchor.x - geometry.spacing / 2.0
    };
    Rectangle::new(
        Point::new(edge - thickness / 2.0, anchor.y),
        iced::Size::new(thickness, anchor.height),
    )
}

/// A `thickness` bar across the list at `slot`, centred in the spacing
/// there.
fn insertion_bar(geometry: Geometry<'_>, slot: usize, thickness: f32) -> Rectangle {
    let count = geometry.rows.len();
    let edge = if count == 0 {
        let (at, extent) = geometry.along(&geometry.bounds);
        if geometry.reverse { at + extent } else { at }
    } else if slot < count {
        let (at, extent) = geometry.along(&geometry.rows[slot]);
        if geometry.reverse {
            at + extent + geometry.spacing / 2.0
        } else {
            at - geometry.spacing / 2.0
        }
    } else {
        let (at, extent) = geometry.along(&geometry.rows[count - 1]);
        if geometry.reverse {
            at - geometry.spacing / 2.0
        } else {
            at + extent + geometry.spacing / 2.0
        }
    };
    let b = geometry.bounds;
    if geometry.vertical {
        Rectangle::new(
            Point::new(b.x, edge - thickness / 2.0),
            iced::Size::new(b.width, thickness),
        )
    } else {
        Rectangle::new(
            Point::new(edge - thickness / 2.0, b.y),
            iced::Size::new(thickness, b.height),
        )
    }
}

/// One row gliding from where it was shown to where it belongs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    from: Point,
    to: Point,
    start: Instant,
}

/// The rows' glides, kept in the list's retained state.
#[derive(Debug, Default)]
pub(super) struct RowMotion {
    // Keyed by row so a long list costs O(n) per frame, not O(n²).
    glides: RefCell<HashMap<String, Glide>>,
    frame_at: Cell<Option<Instant>>,
    duration_ms: Cell<f32>,
    easing: Cell<enzui_theme::MotionEasing>,
    was_live: Cell<bool>,
}

impl RowMotion {
    /// Latch the frame's timestamp from `RedrawRequested`.
    pub(super) fn note_frame(&self, at: Instant) {
        self.frame_at.set(Some(at));
    }

    /// Whether any row is still gliding at `at`.
    pub(super) fn in_motion(&self, at: Instant) -> bool {
        let duration = self.duration_ms.get();
        self.glides.borrow().values().any(|glide| {
            glide.from != glide.to
                && at.saturating_duration_since(glide.start).as_secs_f32() * 1000.0 < duration
        })
    }

    /// Whether the last frame was drawn during a drag that is now over:
    /// the next draw starts the rows' settle.
    pub(super) fn ended_since_drawn(&self, live: bool) -> bool {
        self.was_live.get() && !live
    }

    /// Start a frame drawn with `theme`, with or without a live drag. Rows
    /// glide only during a drag, on the frame it ends, and while a glide
    /// from either is still running.
    pub(super) fn begin(&self, theme: &iced::Theme, live: bool) -> bool {
        let motion = crate::theme_lookup::motion(theme);
        self.duration_ms
            .set(motion.ms(enzui_theme::MotionToken::Fast));
        self.easing.set(motion.easing);
        let glide =
            live || self.was_live.get() || self.frame_at.get().is_some_and(|at| self.in_motion(at));
        self.was_live.set(live);
        glide
    }

    /// Where the row `key` paints this frame, given where it is laid out
    /// (`rest`) and where it belongs now (`target`). A row not yet tracked
    /// starts from `rest`; `snap` places it at `target` at once (a lifted
    /// row).
    pub(super) fn place(&self, key: &str, rest: Point, target: Point, snap: bool) -> Point {
        let mut glides = self.glides.borrow_mut();
        let Some(now) = self.frame_at.get() else {
            glides.remove(key);
            return target;
        };
        let snapped = Glide {
            from: target,
            to: target,
            start: now,
        };
        let glide = glides.entry(key.to_owned()).or_insert(Glide {
            from: rest,
            to: rest,
            start: now,
        });
        if snap || self.duration_ms.get() <= 0.0 {
            *glide = snapped;
            return target;
        }
        let shown = self.shown(glide, now);
        if glide.to != target {
            *glide = Glide {
                from: shown,
                to: target,
                start: now,
            };
            return shown;
        }
        shown
    }

    /// Forget every row: the list is at rest.
    pub(super) fn clear(&self) {
        let mut glides = self.glides.borrow_mut();
        if !glides.is_empty() {
            glides.clear();
        }
    }

    /// Forget rows that are no longer in the list.
    pub(super) fn retain(&self, keys: &[String]) {
        let mut glides = self.glides.borrow_mut();
        if glides.is_empty() {
            return;
        }
        let present: HashSet<&str> = keys.iter().map(String::as_str).collect();
        glides.retain(|key, _| present.contains(key.as_str()));
    }

    fn shown(&self, glide: &Glide, now: Instant) -> Point {
        let duration = self.duration_ms.get();
        let elapsed = now.saturating_duration_since(glide.start).as_secs_f32() * 1000.0;
        let t = if duration <= 0.0 {
            1.0
        } else {
            self.easing.get().apply(elapsed / duration)
        };
        glide.from + (glide.to - glide.from) * t
    }
}
