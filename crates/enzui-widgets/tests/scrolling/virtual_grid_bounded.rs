//! Motion contract: count and bound every realised resource
//! at 1k/10k/100k items and several viewport widths, with a positive control in
//! each direction.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_core::virtual_grid::{TileGrid, TileProvider};
use enzui_core::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};
use enzui_widgets::virtual_grid::enz_virtual_grid;
use iced::advanced::widget::Operation;
use iced::widget::image;
use iced::{Element, Length, Size};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Tile edge in pixels. 32x32 RGBA is 4,096 bytes, so the eager control's
/// 4,000-tile arm costs ~16 MB rather than the 147 MB a 96x96 tile would —
/// large enough that a leak is a leak, small enough to run on a shared host.
const TILE_PX: u32 = 32;

#[derive(Debug, Default)]
struct Stats {
    /// Cumulative `image::Handle`s ever created. Never decremented — a
    /// plateauing live count cannot distinguish "evicted and regenerated" from
    /// "silently retained elsewhere", and that distinction is the memory claim.
    decodes: AtomicUsize,
    /// Handles alive right now, by `Drop`.
    live: AtomicUsize,
}

/// A decoded tile: a real `image::Handle` plus the counters that observe its
/// lifetime.
#[derive(Debug)]
#[allow(dead_code, reason = "fixture fields are written but never read")]
struct Decoded {
    stats: Arc<Stats>,
    handle: image::Handle,
    index: u64,
}

impl Decoded {
    fn new(stats: &Arc<Stats>, index: u64) -> Self {
        stats.decodes.fetch_add(1, Ordering::Relaxed);
        stats.live.fetch_add(1, Ordering::Relaxed);
        let mut px = vec![0u8; (TILE_PX * TILE_PX * 4) as usize];
        for (i, chunk) in px.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            chunk[0] = (index.wrapping_mul(37).wrapping_add(i as u64) % 256) as u8;
            chunk[1] = (index.wrapping_mul(83) % 256) as u8;
            chunk[2] = (i as u64 % 256) as u8;
            chunk[3] = 255;
        }
        Decoded {
            stats: Arc::clone(stats),
            handle: image::Handle::from_rgba(TILE_PX, TILE_PX, px),
            index,
        }
    }
}

impl Drop for Decoded {
    fn drop(&mut self) {
        self.stats.live.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The provider's `Item`: a shared handle to a decoded tile.
#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "the test checks which message arrived, never its payload"
)]
struct Photo(Arc<Decoded>);

/// A procedural [`TileProvider`] over `len` tiles that creates each tile's
/// image handle on demand and retains nothing.
struct Library {
    len: u64,
    stats: Arc<Stats>,
}

impl Library {
    fn new(len: u64) -> Self {
        Library {
            len,
            stats: Arc::new(Stats::default()),
        }
    }
}

impl ItemProvider<Photo> for Library {
    fn len(&self) -> u64 {
        self.len
    }
    fn item(&self, index: u64) -> Option<Photo> {
        // Total: every `u64` legal, out of range `None`, never a clamp.
        if index >= self.len {
            return None;
        }
        Some(Photo(Arc::new(Decoded::new(&self.stats, index))))
    }
}

impl KeyedProvider<Photo> for Library {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        (index < self.len).then(|| SequenceKey::new(format!("photo-{index}")))
    }
    fn index_of(&self, key: &SequenceKey) -> Option<u64> {
        key.as_str()
            .strip_prefix("photo-")
            .and_then(|n| n.parse::<u64>().ok())
            .filter(|n| *n < self.len)
    }
}

impl TileProvider<Photo> for Library {}

/// The issue's gallery geometry, scaled to the 32 px tile: 8 px gaps, so the
/// pitch is 40 px on both axes.
fn gallery() -> TileGrid {
    TileGrid::new(TILE_PX as f32, TILE_PX as f32).with_spacing(8.0, 8.0)
}

fn renderer() -> iced_test::renderer::Renderer {
    use iced::advanced::renderer::{Headless, Settings};
    iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

/// What one realised cell reports: its logical item index, whether it is
/// published as selected, and where it is on screen.
#[derive(Debug, Clone, Copy)]
struct CellNode {
    index: u64,
    selected: bool,
    bounds: iced::Rectangle,
}

/// The application's selection state, owned by the app exactly as
/// [`EnzVirtualGrid::selection`] requires.
#[derive(Debug, Default)]
struct SelectionModel {
    keys: RefCell<std::collections::BTreeSet<SequenceKey>>,
}

impl SelectionModel {
    /// Select `key`, clearing every sibling — single-selection semantics.
    fn select(&self, key: SequenceKey) {
        let mut keys = self.keys.borrow_mut();
        keys.clear();
        keys.insert(key);
    }
    fn contains(&self, key: &SequenceKey) -> bool {
        self.keys.borrow().contains(key)
    }
    fn len(&self) -> usize {
        self.keys.borrow().len()
    }
}

/// Lay a selection-enabled grid out at `(w, h)` scrolled to `offset`, optionally
/// pressing at `press`, and report every realised cell plus the item indices the
/// press produced messages for.
fn selection_frame(
    library: &Library,
    model: &SelectionModel,
    offset: f32,
    w: f32,
    h: f32,
    press: Option<iced::Point>,
) -> (Vec<CellNode>, Vec<u64>) {
    use iced::mouse;

    let is_selected = |key: &SequenceKey| model.contains(key);
    let build = || -> Element<'_, u64> {
        enz_virtual_grid(library, gallery(), |index, _tile: &Photo| {
            iced::widget::button(iced::widget::text("x").size(8))
                .on_press(index)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        })
        .overscan_px(0.0)
        .scroll_offset(offset)
        .semantics("Photos")
        .selection(&is_selected)
        .into()
    };

    let mut r = renderer();
    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        build(),
        Size::new(w, h),
        iced_test::runtime::user_interface::Cache::default(),
        &mut r,
    );
    ui = ui.relayout(Size::new(w, h), &mut r);

    let mut messages: iced::advanced::shell::Bus<u64> = iced::advanced::shell::Bus::new();
    if let Some(point) = press {
        let events = [
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ];
        let _ = ui.update(
            &iced_test::core::window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &events,
            mouse::Cursor::Available(point),
            &mut r,
            &mut messages,
        );
    }

    let mut collector = SemanticsCollector::new();
    ui.operate(&r, &mut collector as &mut dyn Operation);
    let cells = collector
        .into_collected()
        .iter()
        .filter(|n| n.semantics.role == Role::GridCell)
        .filter_map(|n| {
            n.semantics.set_position.map(|sp| CellNode {
                index: sp.index as u64 - 1,
                selected: n.semantics.selected.unwrap_or(false),
                bounds: n.bounds,
            })
        })
        .collect();
    (cells, messages.into_iter().collect())
}

/// Selection is set on press, keyed, and clears its siblings.
#[test]
fn pressing_a_tile_selects_it_and_deselects_its_siblings() {
    let library = Library::new(100_000);
    let model = SelectionModel::default();

    // 152px is 4 columns, 200px is 5 rows at a 40px pitch: a 20-tile band.
    let (cells, no_msgs) = selection_frame(&library, &model, 0.0, 152.0, 200.0, None);
    assert_eq!(
        cells.len(),
        20,
        "calibration: 4 columns x 5 rows realised, got {}",
        cells.len()
    );
    assert!(
        no_msgs.is_empty(),
        "calibration: no press, no messages, got {no_msgs:?}"
    );
    assert_eq!(
        cells.iter().filter(|c| c.selected).count(),
        0,
        "calibration: nothing is selected before the first press"
    );

    // ---- press tile 6 ---------------------------------------------------
    let target = cells
        .iter()
        .find(|c| c.index == 6)
        .expect("tile 6 realised");
    let (_, msgs) = selection_frame(
        &library,
        &model,
        0.0,
        152.0,
        200.0,
        Some(target.bounds.center()),
    );
    assert_eq!(
        msgs,
        vec![6],
        "pressing tile 6's centre {:?} must emit exactly its own index — an \
         empty list means the press missed and every count below would be \
         vacuously zero",
        target.bounds.center()
    );
    // The application applies its own single-selection policy.
    model.select(SequenceKey::new("photo-6"));

    let (after_first, _) = selection_frame(&library, &model, 0.0, 152.0, 200.0, None);
    let selected: Vec<u64> = after_first
        .iter()
        .filter(|c| c.selected)
        .map(|c| c.index)
        .collect();
    assert_eq!(
        selected,
        vec![6],
        "after pressing tile 6, exactly tile 6 must publish selected"
    );

    // ---- press tile 11: the sibling-clearing case ------------------------
    let second = after_first
        .iter()
        .find(|c| c.index == 11)
        .expect("tile 11 realised");
    let (_, msgs2) = selection_frame(
        &library,
        &model,
        0.0,
        152.0,
        200.0,
        Some(second.bounds.center()),
    );
    assert_eq!(msgs2, vec![11], "pressing tile 11 must emit index 11");
    model.select(SequenceKey::new("photo-11"));

    let (after_second, _) = selection_frame(&library, &model, 0.0, 152.0, 200.0, None);
    let selected2: Vec<u64> = after_second
        .iter()
        .filter(|c| c.selected)
        .map(|c| c.index)
        .collect();
    // The assertion: exactly one, and it is the new one. Asserting only
    // `contains(&11)` would pass with tile 6 still selected.
    assert_eq!(
        selected2,
        vec![11],
        "pressing a second tile must select it AND deselect the first: \
         exactly one cell may publish selected, got {selected2:?}"
    );
    assert_eq!(
        model.len(),
        1,
        "and the application's own selection set must hold exactly one key"
    );
}
