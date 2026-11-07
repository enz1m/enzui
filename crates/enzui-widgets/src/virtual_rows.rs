//! Shared keyed row realisation for virtual lists and chat transcripts.
//!
//! Visible rows plus overscan occupy clipped, fixed-height slots; unrealised
//! content retains its height through spacers. Missing provider rows also keep
//! empty slots. Row keys use stable logical identities, and edge-spacer keys
//! name their neighbouring realised rows to preserve keyed reconciliation.

use crate::keyed_column::{KeyedColumn, keyed_column};
use crate::row_measure::{RowMeasurement, enz_row_measure};
use enzui_core::virtual_sequence::{Anchor, RealizationWindow, SequenceGeometry};
use iced::widget::{Space, container};
use iced::{Element, Length, Padding};
use std::rc::Rc;

fn row_key(key: &str) -> String {
    format!("row::{key}")
}

fn leading_spacer_key(first_row: Option<&str>) -> String {
    match first_row {
        Some(key) => format!("lead::{key}"),
        None => "enz-virtual-spacer::lead".to_owned(),
    }
}

fn trailing_spacer_key(last_row: Option<&str>) -> String {
    match last_row {
        Some(key) => format!("trail::{key}"),
        None => "enz-virtual-spacer::trail".to_owned(),
    }
}

pub(crate) struct RowChrome<'a, Message> {
    // Insets must stay horizontal: measured content excludes the slot's padding.
    pub(crate) padding: Padding,
    pub(crate) on_measure: Option<Rc<dyn Fn(RowMeasurement) -> Message + 'a>>,
}

impl<Message> Default for RowChrome<'_, Message> {
    fn default() -> Self {
        RowChrome {
            padding: Padding::ZERO,
            on_measure: None,
        }
    }
}

pub(crate) struct RealisedRow<'a, Message> {
    pub(crate) key: String,
    pub(crate) content: Element<'a, Message>,
}

pub(crate) fn realise_keyed_rows<'a, Message, F>(
    geom: &(impl SequenceGeometry + ?Sized),
    viewport_height: f32,
    scroll_offset: f32,
    overscan: usize,
    chrome: RowChrome<'a, Message>,
    mut build: F,
) -> KeyedColumn<'a, Message>
where
    Message: 'a,
    F: FnMut(u64, f32) -> Option<RealisedRow<'a, Message>>,
{
    let anchor = Anchor::from_scroll_top(geom, scroll_offset);
    let placements: Vec<_> = RealizationWindow::new(overscan)
        .place(geom, &anchor, viewport_height)
        .collect();

    let total_height = geom.total_height();
    let leading = placements
        .first()
        .map_or(0.0, |first| geom.offset_of(first.index));

    let mut keyed: Vec<(String, Element<'a, Message>)> =
        Vec::with_capacity(placements.len().saturating_add(2));

    // Reserve the leading slot until its neighbour key is known, avoiding a second fetch.
    let leading_slot = if leading > 0.0 {
        keyed.push((
            leading_spacer_key(None),
            Space::new()
                .unrealized()
                .width(Length::Fill)
                .height(leading)
                .into(),
        ));
        Some(keyed.len() - 1)
    } else {
        None
    };

    let mut first_key: Option<String> = None;
    let mut last_key: Option<String> = None;
    for placement in &placements {
        let Some(row) = build(placement.index as u64, placement.height) else {
            keyed.push((
                format!("\0unrealized:{}", placement.index),
                Space::new()
                    .unrealized()
                    .width(Length::Fill)
                    .height(placement.height)
                    .into(),
            ));
            continue;
        };
        if first_key.is_none() {
            first_key = Some(row.key.clone());
        }
        last_key = Some(row.key.clone());
        // Measure inside the fixed slot; measuring the slot would reproduce its estimate.
        let content = match &chrome.on_measure {
            Some(f) => {
                let f = Rc::clone(f);
                enz_row_measure(placement.index, row.content)
                    .on_measure(move |m| f(m))
                    .into_element()
            }
            None => row.content,
        };
        // Uncorrected estimates must not let taller rows paint over their neighbours.
        keyed.push((
            row_key(&row.key),
            container(content)
                .width(Length::Fill)
                .height(Length::Fixed(placement.height))
                .padding(chrome.padding)
                .clip(true)
                .into(),
        ));
    }

    if let Some(slot) = leading_slot {
        keyed[slot].0 = leading_spacer_key(first_key.as_deref());
    }

    let used = leading + placements.iter().map(|p| p.height).sum::<f32>();
    let trailing = (total_height - used).max(0.0);
    if trailing > 0.0 {
        keyed.push((
            trailing_spacer_key(last_key.as_deref()),
            Space::new()
                .unrealized()
                .width(Length::Fill)
                .height(trailing)
                .into(),
        ));
    }

    keyed_column(keyed).width(Length::Fill)
}
