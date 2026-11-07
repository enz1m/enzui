use super::*;

fn gallery() -> TileGrid {
    TileGrid::new(96.0, 96.0).with_spacing(8.0, 8.0)
}

use crate::virtual_sequence::{ItemProvider, KeyedProvider, SequenceKey};

struct Photos {
    ids: Vec<u64>,
}

impl Photos {
    fn new(n: u64) -> Self {
        Photos {
            ids: (0..n).collect(),
        }
    }
    fn prepend(&mut self, n: u64) {
        let fresh: Vec<u64> = (0..n).map(|i| 1_000_000 + i).collect();
        self.ids.splice(0..0, fresh);
    }
}

impl ItemProvider<String> for Photos {
    fn len(&self) -> u64 {
        self.ids.len() as u64
    }
    fn item(&self, index: u64) -> Option<String> {
        self.ids.get(index as usize).map(|id| format!("photo-{id}"))
    }
}

impl KeyedProvider<String> for Photos {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.ids
            .get(index as usize)
            .map(|id| SequenceKey::new(format!("photo-{id}")))
    }
}

#[test]
fn a_keyed_anchor_follows_its_content_across_an_index_shifting_mutation() {
    let g = gallery();
    let mut p = Photos::new(1_000);
    let captured = g.capture(&p, 5, 4_160.0 + 30.0).expect("anchor");
    assert_eq!(captured.key().as_str(), "photo-200");

    p.prepend(7);
    assert_eq!(
        p.index_of(captured.key()),
        Some(207),
        "the prepend must have shifted the item's index"
    );

    let r = g.reflow(&p, &captured, 5).expect("the key still resolves");
    assert_eq!(
        (r.row, r.column),
        (207 / 5, 207 % 5),
        "the anchor must follow the content to its new index, got {r:?}"
    );
    assert!(
        (r.scroll_top - (41.0 * 104.0 - 30.0)).abs() < 0.01,
        "and the offset must be its new row's top minus the sub-offset, got {}",
        r.scroll_top
    );
}
