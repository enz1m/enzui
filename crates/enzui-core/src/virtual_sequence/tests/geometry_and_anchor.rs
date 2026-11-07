use super::*;

#[test]
fn scroll_top_past_the_end_is_unclamped_and_still_ordered() {
    let geom = VariableHeightModel::new([50.0_f32; 10]);
    assert_eq!(geom.total_height(), 500.0);

    for idx in [10_usize, 11, 500, usize::MAX] {
        assert_eq!(
            Anchor::new(idx, 0.0).scroll_top(&geom),
            500.0,
            "index {idx} past len collapses onto total_height"
        );
    }

    assert_eq!(Anchor::new(10, 10.0).scroll_top(&geom), 510.0);
    assert_eq!(Anchor::new(11, 25.0).scroll_top(&geom), 525.0);
    assert!(
        Anchor::new(11, 25.0).scroll_top(&geom) > geom.total_height(),
        "past-the-end offsets are deliberately not clamped to the content end"
    );

    for a in [
        Anchor::new(10, 0.0),
        Anchor::new(11, 25.0),
        Anchor::new(usize::MAX, 0.0),
    ] {
        assert_eq!(a.resolve(&geom, 100.0), 10..10);
    }
}
