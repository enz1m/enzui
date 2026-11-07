//! Deterministic per-column overlap layout for timed schedule events.

use super::*;

// Counts interval comparisons per thread so concurrent tests cannot contaminate measurements.
#[cfg(test)]
thread_local! {
    static OVERLAP_COMPARISONS: core::cell::Cell<u64> = const { core::cell::Cell::new(0) };
}

#[cfg(test)]
#[inline]
fn count_comparison() {
    OVERLAP_COMPARISONS.with(|c| c.set(c.get().wrapping_add(1)));
}

#[cfg(not(test))]
#[inline]
fn count_comparison() {}

// Half-open intervals overlap only with positive shared extent; touching or empty intervals do not.
#[inline]
fn intervals_overlap(a: (i32, i32), b: (i32, i32)) -> bool {
    count_comparison();
    // This form treats zero-duration intervals as empty even when they lie inside another interval.
    a.0.max(b.0) < a.1.min(b.1)
}

/// Allocates deterministic lanes for timed events independently in each column.
///
/// All-day events are omitted. Overlap clusters use half-open intervals, and each event receives the lowest available lane in `(start, end, id)` order.
///
/// Each event then expands into the following lanes that hold nothing beside it
/// ([`LaidOutEvent::lane_span`]), so an event with no concurrent neighbour is
/// full width even when its cluster needed several lanes for a busier moment.
#[must_use]
pub fn layout_overlaps<P: Clone>(events: &[ScheduleEvent<P>]) -> Vec<LaidOutEvent<P>> {
    let mut timed: Vec<&ScheduleEvent<P>> = events.iter().filter(|e| !e.all_day).collect();

    timed.sort_by(|a, b| {
        let (as_, ae) = a.interval();
        let (bs, be) = b.interval();
        (as_, ae, a.id).cmp(&(bs, be, b.id))
    });

    let mut by_column: std::collections::BTreeMap<usize, Vec<&ScheduleEvent<P>>> =
        std::collections::BTreeMap::new();
    for e in timed {
        by_column.entry(e.column).or_default().push(e);
    }

    let mut out: Vec<LaidOutEvent<P>> = Vec::with_capacity(events.len());

    for (_column, column_events) in by_column {
        // A new cluster begins when the next start reaches the running maximum end; touching intervals are disjoint.
        let mut i = 0;
        while i < column_events.len() {
            let mut cluster_end = column_events[i].interval().1;
            let mut j = i + 1;
            while j < column_events.len() {
                let (s, e) = column_events[j].interval();
                if s < cluster_end {
                    cluster_end = cluster_end.max(e);
                    j += 1;
                } else {
                    break;
                }
            }
            let cluster = &column_events[i..j];
            place_cluster(cluster, &mut out);
            i = j;
        }
    }

    out
}

fn place_cluster<P: Clone>(cluster: &[&ScheduleEvent<P>], out: &mut Vec<LaidOutEvent<P>>) {
    let mut lanes: Vec<(i32, i32)> = Vec::new();
    // Every interval each lane ever held, in ascending start order. `lanes`
    // keeps only the latest, which is all the allocator needs; the span pass
    // has to test a lane against an arbitrary earlier interval.
    let mut lane_history: Vec<Vec<(i32, i32)>> = Vec::new();
    let start_idx = out.len();

    for e in cluster {
        let iv = e.interval();
        let mut chosen = None;
        for (lane_idx, last) in lanes.iter().enumerate() {
            if !intervals_overlap(*last, iv) {
                chosen = Some(lane_idx);
                break;
            }
        }
        let lane = match chosen {
            Some(l) => {
                lanes[l] = iv;
                l
            }
            None => {
                lanes.push(iv);
                lane_history.push(Vec::new());
                lanes.len() - 1
            }
        };
        lane_history[lane].push(iv);
        out.push(LaidOutEvent {
            id: e.id,
            column: e.column,
            lane,
            lane_span: 1,
            lane_count: 0,
            start_minute: e.start_minute,
            end_minute: iv.1,
            payload: e.payload.clone(),
        });
    }

    let cluster_lane_count = lanes.len().max(1);
    for laid in &mut out[start_idx..] {
        laid.lane_count = cluster_lane_count;
        laid.lane_span = span_from(
            &lane_history,
            laid.lane,
            (laid.start_minute, laid.end_minute),
        );
    }
}

// How many adjacent lanes an event may occupy: its own, plus every following
// lane that holds nothing beside it for its whole interval. A cluster's lane
// count comes from its widest moment, so this is what keeps an event that
// overlaps nothing from being sized by a collision elsewhere in the day.
fn span_from(lane_history: &[Vec<(i32, i32)>], lane: usize, interval: (i32, i32)) -> usize {
    let mut span = 1;
    for next in lane_history.iter().skip(lane + 1) {
        if lane_occupied_beside(next, interval) {
            break;
        }
        span += 1;
    }
    span
}

// Whether `lane` holds any interval overlapping `interval`.
//
// A lane's intervals are pairwise disjoint and ascend by start (a lane accepts
// only an interval that misses the one it currently ends with, and the cluster
// is processed in start order), so ends ascend with starts and the only
// candidate is the last interval starting before `interval` ends. One
// comparison per lane keeps the span pass linear in the cluster.
fn lane_occupied_beside(lane: &[(i32, i32)], interval: (i32, i32)) -> bool {
    let candidates = lane.partition_point(|(start, _)| *start < interval.1);
    match candidates.checked_sub(1) {
        Some(index) => intervals_overlap(lane[index], interval),
        None => false,
    }
}
