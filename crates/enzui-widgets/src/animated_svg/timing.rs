//! The SMIL timing model: when each animation element is active.
//!
//! Each animation owns `begin` and `end` condition lists. Offsets resolve at
//! once; syncbases (`other.begin`, `other.end+1s`), repeats
//! (`other.repeat(2)`) and events (`click`, `button.mouseover`) resolve as
//! the intervals and events they name appear. Intervals are generated in time
//! order up to a horizon, so a chain of animations that start each other
//! (`a: begin="0;b.end"`, `b: begin="a.end"`) unrolls as far as it is played.

#![allow(
    clippy::string_slice,
    reason = "condition lists are cut at ASCII signs found by `char_indices`"
)]

use super::dom::NodeId;

/// Intervals generated per animation at most, a bound for a document whose
/// animations restart each other endlessly at zero length.
const MAX_INTERVALS: usize = 20_000;

/// A pointer or document event a `begin` or `end` can wait for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Event {
    Click,
    MouseDown,
    MouseUp,
    MouseOver,
    MouseOut,
    MouseMove,
}

impl Event {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "click" | "activate" | "DOMActivate" => Self::Click,
            "mousedown" | "pointerdown" => Self::MouseDown,
            "mouseup" | "pointerup" => Self::MouseUp,
            "mouseover" | "mouseenter" | "pointerover" | "pointerenter" => Self::MouseOver,
            "mouseout" | "mouseleave" | "pointerout" | "pointerleave" => Self::MouseOut,
            "mousemove" | "pointermove" => Self::MouseMove,
            _ => return None,
        })
    }
}

/// One entry of a `begin` or `end` list.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Condition {
    Offset(f64),
    /// The begin (`end: false`) or end of another animation's intervals.
    Syncbase {
        animation: usize,
        end: bool,
        offset: f64,
    },
    /// The start of another animation's `iteration`th repeat.
    Repeat {
        animation: usize,
        iteration: u32,
        offset: f64,
    },
    Event {
        node: NodeId,
        event: Event,
        offset: f64,
    },
    /// `indefinite`, `accessKey(…)`, `wallclock(…)` and anything unreadable:
    /// never resolved here.
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Restart {
    Always,
    WhenNotActive,
    Never,
}

/// The timing attributes of one animation element.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Timing {
    pub(super) begin: Vec<Condition>,
    pub(super) end: Vec<Condition>,
    /// The simple duration; `None` for indefinite (or unspecified).
    pub(super) dur: Option<f64>,
    /// `repeatCount`: `Some(None)` for indefinite.
    pub(super) repeat_count: Option<Option<f64>>,
    /// `repeatDur`: `Some(None)` for indefinite.
    pub(super) repeat_dur: Option<Option<f64>>,
    pub(super) min: f64,
    pub(super) max: Option<f64>,
    pub(super) restart: Restart,
    pub(super) freeze: bool,
}

/// A clock value (`2s`, `150ms`, `1.5min`, `0.1h`, `01:30`, `00:01:30.5`,
/// or a bare number of seconds), in seconds.
pub(super) fn parse_clock(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if text.contains(':') {
        let parts: Vec<&str> = text.split(':').collect();
        let numbers: Vec<f64> = parts
            .iter()
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        return match numbers.as_slice() {
            [m, s] => Some(m * 60.0 + s),
            [h, m, s] => Some(h * 3600.0 + m * 60.0 + s),
            _ => None,
        };
    }
    let (number, scale) = if let Some(n) = text.strip_suffix("ms") {
        (n, 0.001)
    } else if let Some(n) = text.strip_suffix("min") {
        (n, 60.0)
    } else if let Some(n) = text.strip_suffix('h') {
        (n, 3600.0)
    } else if let Some(n) = text.strip_suffix('s') {
        (n, 1.0)
    } else {
        (text, 1.0)
    };
    let value: f64 = number.trim().parse().ok()?;
    value.is_finite().then_some(value * scale)
}

/// A `begin` or `end` list. `resolve_id` maps an element id to its node,
/// `resolve_animation` an element id to an animation index, and `target` is
/// the element an unprefixed event listens on.
pub(super) fn parse_conditions(
    text: &str,
    target: NodeId,
    resolve_id: &dyn Fn(&str) -> Option<NodeId>,
    resolve_animation: &dyn Fn(&str) -> Option<usize>,
) -> Vec<Condition> {
    text.split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| parse_condition(item, target, resolve_id, resolve_animation))
        .collect()
}

fn parse_condition(
    item: &str,
    target: NodeId,
    resolve_id: &dyn Fn(&str) -> Option<NodeId>,
    resolve_animation: &dyn Fn(&str) -> Option<usize>,
) -> Condition {
    if item == "indefinite" {
        return Condition::Never;
    }
    if let Some(offset) = parse_clock(item) {
        return Condition::Offset(offset);
    }
    // Split off a trailing `+clock` / `-clock` offset.
    let (base, offset) = split_offset(item);
    let (element, token) = match base.rsplit_once('.') {
        // `id.token`, where the id itself may contain escaped dots (`a\.b`).
        Some((element, token)) if !element.ends_with('\\') => {
            (Some(element.replace("\\.", ".")), token)
        }
        _ => (None, base),
    };
    if let Some(iteration) = token
        .strip_prefix("repeat(")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|n| n.trim().parse().ok())
    {
        return element.as_deref().and_then(resolve_animation).map_or(
            Condition::Never,
            |animation| Condition::Repeat {
                animation,
                iteration,
                offset,
            },
        );
    }
    match token {
        "begin" | "beginEvent" | "end" | "endEvent" => element
            .as_deref()
            .and_then(resolve_animation)
            .map_or(Condition::Never, |animation| Condition::Syncbase {
                animation,
                end: token.starts_with("end"),
                offset,
            }),
        "load" | "SVGLoad" => Condition::Offset(offset),
        _ => match Event::parse(token) {
            Some(event) => {
                let node = match element.as_deref() {
                    Some(id) => resolve_id(id),
                    None => Some(target),
                };
                node.map_or(Condition::Never, |node| Condition::Event {
                    node,
                    event,
                    offset,
                })
            }
            None => Condition::Never,
        },
    }
}

fn split_offset(item: &str) -> (&str, f64) {
    // The last sign not inside the id part.
    for (index, character) in item.char_indices().rev() {
        if (character == '+' || character == '-')
            && index > 0
            && let Some(offset) = parse_clock(&item[index + 1..])
        {
            let sign = if character == '-' { -1.0 } else { 1.0 };
            return (item[..index].trim(), sign * offset);
        }
    }
    (item.trim(), 0.0)
}

/// One active interval: `[begin, end)`, `end` `None` for indefinite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Interval {
    pub(super) begin: f64,
    pub(super) end: Option<f64>,
}

/// The intervals of every animation, generated up to a horizon.
#[derive(Debug, Clone, Default)]
pub(super) struct Timeline {
    pub(super) intervals: Vec<Vec<Interval>>,
    /// Pointer events received so far: `(time, node, event)`.
    events: Vec<(f64, NodeId, Event)>,
    /// Generated up to here.
    horizon: f64,
    /// Per animation: no further interval can ever start.
    finished: Vec<bool>,
}

/// Where an animation stands at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Phase {
    /// Not active and showing nothing.
    Idle,
    /// Active, `elapsed` seconds into the interval that began at `begin`.
    Active { begin: f64, elapsed: f64 },
    /// After an interval that lasted `active` seconds, holding its end.
    Frozen { begin: f64, active: f64 },
}

impl Timeline {
    pub(super) fn new(count: usize) -> Self {
        Self {
            intervals: vec![Vec::new(); count],
            events: Vec::new(),
            horizon: f64::NEG_INFINITY,
            finished: vec![false; count],
        }
    }

    /// Record a pointer event at `time` and regenerate from scratch: an event
    /// can start an animation that time order had already passed.
    pub(super) fn add_event(&mut self, time: f64, node: NodeId, event: Event) {
        self.events.push((time, node, event));
        let count = self.intervals.len();
        let events = std::mem::take(&mut self.events);
        *self = Self::new(count);
        self.events = events;
    }

    /// Whether any animation waits for `event` on `node`.
    pub(super) fn listens(timings: &[Timing], node: NodeId, event: Event) -> bool {
        timings.iter().any(|timing| {
            timing
                .begin
                .iter()
                .chain(&timing.end)
                .any(|c| matches!(c, Condition::Event { node: n, event: e, .. } if *n == node && *e == event))
        })
    }

    /// Resolved instance times of `conditions`, sorted.
    fn instances(&self, timings: &[Timing], conditions: &[Condition]) -> Vec<f64> {
        let mut times = Vec::new();
        for condition in conditions {
            match *condition {
                Condition::Offset(offset) => times.push(offset),
                Condition::Syncbase {
                    animation,
                    end,
                    offset,
                } => {
                    for interval in &self.intervals[animation] {
                        if end {
                            if let Some(e) = interval.end {
                                times.push(e + offset);
                            }
                        } else {
                            times.push(interval.begin + offset);
                        }
                    }
                }
                Condition::Repeat {
                    animation,
                    iteration,
                    offset,
                } => {
                    let Some(dur) = timings[animation].dur.filter(|d| *d > 0.0) else {
                        continue;
                    };
                    for interval in &self.intervals[animation] {
                        let at = interval.begin + dur * f64::from(iteration);
                        if interval.end.is_none_or(|end| at < end) {
                            times.push(at + offset);
                        }
                    }
                }
                Condition::Event {
                    node,
                    event,
                    offset,
                } => {
                    for &(time, n, e) in &self.events {
                        if n == node && e == event {
                            times.push(time + offset);
                        }
                    }
                }
                Condition::Never => {}
            }
        }
        times.sort_by(f64::total_cmp);
        times
    }

    /// The active duration of an interval beginning at `begin` with the
    /// first end instance at or after it, `None` for indefinite.
    fn active_duration(timing: &Timing, begin: f64, end: Option<f64>) -> Option<f64> {
        let repeat_specified = timing.repeat_count.is_some() || timing.repeat_dur.is_some();
        let intrinsic = if repeat_specified {
            let by_count = match timing.repeat_count {
                Some(Some(count)) => timing.dur.map(|d| d * count),
                Some(None) => None,
                None => Some(f64::INFINITY),
            };
            let by_dur = match timing.repeat_dur {
                Some(Some(d)) => Some(d),
                Some(None) => None,
                None => Some(f64::INFINITY),
            };
            // An unspecified side does not limit; an indefinite one does not either.
            let limits: Vec<f64> = [by_count, by_dur]
                .into_iter()
                .flatten()
                .filter(|v| v.is_finite())
                .collect();
            if limits.is_empty() {
                None
            } else {
                Some(limits.into_iter().fold(f64::INFINITY, f64::min))
            }
        } else if timing.dur.is_none() && !timing.end.is_empty() {
            None
        } else {
            timing.dur
        };
        let mut active = match (intrinsic, end) {
            (Some(i), Some(e)) => Some(i.min(e - begin)),
            (None, Some(e)) => Some(e - begin),
            (i, None) => i,
        };
        if let Some(a) = active.as_mut() {
            *a = a.max(timing.min);
            if let Some(max) = timing.max {
                *a = a.min(max);
            }
            *a = a.max(0.0);
        }
        active
    }

    /// The next interval of `index` after its last one, from the instances
    /// known now; `None` when no begin is resolved.
    fn candidate(&self, timings: &[Timing], index: usize) -> Option<Interval> {
        let timing = &timings[index];
        let previous = self.intervals[index].last();
        if previous.is_some() && timing.restart == Restart::Never {
            return None;
        }
        let threshold = match previous {
            Some(p) => p.end?,
            None => f64::NEG_INFINITY,
        };
        let begins = self.instances(timings, &timing.begin);
        let begin = begins.iter().copied().find(|b| {
            *b >= threshold
                && previous.is_none_or(|p| *b > p.begin || p.end.is_some_and(|e| e > p.begin))
        })?;
        let ends = self.instances(timings, &timing.end);
        let end_instance = if timing.end.is_empty() {
            None
        } else {
            ends.iter()
                .copied()
                .find(|e| *e > begin || (*e == begin && timing.dur == Some(0.0)))
        };
        // An `end` list with nothing resolved yet and no later offset keeps
        // the interval open: an event may end it.
        let mut end = Self::active_duration(timing, begin, end_instance).map(|a| begin + a);
        if timing.restart == Restart::Always
            && let Some(next) = begins
                .iter()
                .copied()
                .find(|b| *b > begin && end.is_none_or(|e| *b < e))
        {
            end = Some(next);
        }
        Some(Interval { begin, end })
    }

    /// Whether every syncbase `index` waits on is known up to `begin`.
    fn dependencies_known(&self, timings: &[Timing], index: usize, begin: f64) -> bool {
        timings[index]
            .begin
            .iter()
            .chain(&timings[index].end)
            .all(|condition| match *condition {
                Condition::Syncbase {
                    animation, offset, ..
                }
                | Condition::Repeat {
                    animation, offset, ..
                } => {
                    self.finished[animation]
                        || self.intervals[animation].last().is_some_and(|last| {
                            last.end.is_some_and(|e| e + offset >= begin) || last.end.is_none()
                        })
                }
                _ => true,
            })
    }

    /// Generate intervals until every animation's next one begins after
    /// `horizon`.
    pub(super) fn extend(&mut self, timings: &[Timing], horizon: f64) {
        if horizon <= self.horizon {
            return;
        }
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > MAX_INTERVALS * 4 {
                break;
            }
            let candidates: Vec<(usize, Interval)> = (0..timings.len())
                .filter(|i| !self.finished[*i] && self.intervals[*i].len() < MAX_INTERVALS)
                .filter_map(|i| self.candidate(timings, i).map(|c| (i, c)))
                .filter(|(_, c)| c.begin <= horizon)
                .collect();
            if candidates.is_empty() {
                break;
            }
            let ready = candidates
                .iter()
                .filter(|(i, c)| self.dependencies_known(timings, *i, c.begin))
                .min_by(|a, b| a.1.begin.total_cmp(&b.1.begin));
            // A cycle with nothing ready: the earliest goes first.
            let (index, interval) = *ready.unwrap_or_else(|| {
                candidates
                    .iter()
                    .min_by(|a, b| a.1.begin.total_cmp(&b.1.begin))
                    .expect("not empty")
            });
            // A zero-length interval at the same instant as the last one
            // would repeat forever.
            if self.intervals[index]
                .last()
                .is_some_and(|last| last.begin == interval.begin && last.end == interval.end)
            {
                self.finished[index] = true;
                continue;
            }
            self.intervals[index].push(interval);
        }
        self.horizon = horizon;
    }

    /// The earliest interval begin after `time` known so far (generate up to
    /// a horizon first), or `None`.
    pub(super) fn next_begin(&self, timings: &[Timing], time: f64) -> Option<f64> {
        let known = self
            .intervals
            .iter()
            .flatten()
            .map(|i| i.begin)
            .filter(|b| *b > time);
        let pending = (0..timings.len())
            .filter(|i| !self.finished[*i])
            .filter_map(|i| self.candidate(timings, i))
            .map(|c| c.begin)
            .filter(|b| *b > time);
        known.chain(pending).fold(None, |best: Option<f64>, b| {
            Some(best.map_or(b, |x| x.min(b)))
        })
    }

    /// Where animation `index` stands at `time`.
    pub(super) fn phase(&self, timings: &[Timing], index: usize, time: f64) -> Phase {
        let intervals = &self.intervals[index];
        let current = intervals.iter().rev().find(|i| i.begin <= time);
        let Some(interval) = current else {
            return Phase::Idle;
        };
        match interval.end {
            Some(end) if time >= end => {
                if timings[index].freeze {
                    Phase::Frozen {
                        begin: interval.begin,
                        active: end - interval.begin,
                    }
                } else {
                    Phase::Idle
                }
            }
            _ => Phase::Active {
                begin: interval.begin,
                elapsed: time - interval.begin,
            },
        }
    }

    /// When the interval holding `time` ends, if it is active then.
    pub(super) fn active_end(&self, index: usize, time: f64) -> Option<f64> {
        self.intervals[index]
            .iter()
            .rev()
            .find(|i| i.begin <= time)
            .and_then(|i| i.end)
            .filter(|end| *end > time)
    }
}
