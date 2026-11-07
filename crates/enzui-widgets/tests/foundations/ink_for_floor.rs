//! Arbitrary-fill ink has one floor-aware resolver.

use enzui_theme::{ContrastFloor, Palette, SurfaceRole};
use enzui_widgets::tokens::ink_for_with_floor;
use enzui_widgets::{composite_over, contrast_ratio};

#[test]
fn floor_aware_ink_clears_every_role_derived_painted_backdrop() {
    for (preset, p) in [("dark", Palette::dark()), ("light", Palette::light())] {
        assert_eq!(SurfaceRole::ALL.len(), 5, "SurfaceRole corpus changed");
        for role in SurfaceRole::ALL {
            let layer = role.resolve(&p).expect("every SurfaceRole resolves");
            // Overlay roles are layers, not painted colours. Resolve them over
            // the schedule's surface before making a contrast claim.
            let painted = if layer.a < 1.0 {
                composite_over(layer, p.surface)
            } else {
                layer
            };
            let ink = ink_for_with_floor(painted, &p, ContrastFloor::Text)
                .unwrap_or_else(|failure| panic!("{preset}/{role:?}: {failure:?}"));
            assert!(
                contrast_ratio(ink, painted) >= ContrastFloor::Text.minimum_ratio(),
                "{preset}/{role:?}: floor-aware ink did not clear its advertised floor"
            );
        }
    }
}
