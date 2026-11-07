use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Gen(u64);

fn g(n: u64) -> Gen {
    Gen(n)
}
fn s(n: u64) -> FrameSeq {
    FrameSeq(n)
}

#[test]
fn recreate_keeping_displayed_holds_the_displayed_frame_but_drops_the_rest() {
    let mut p = FramePlan::new(g(0));
    p.offer(s(1), g(0));
    p.offer(s(2), g(0));
    p.present();
    p.offer(s(3), g(0));
    assert_eq!(p.retained(), 3, "all three stages full before the swap");

    p.recreate_keeping_displayed(g(1));

    assert_eq!(
        p.displayed_seq(),
        Some(s(1)),
        "the displayed frame must survive a keep-displayed recreate"
    );
    assert_eq!(
        p.in_flight_seq(),
        None,
        "the old in-flight frame is dropped"
    );
    assert_eq!(p.queued_seq(), None, "the old queued frame is dropped");
    assert_eq!(p.retained(), 1, "only the displayed frame remains");
    assert_eq!(p.generation(), g(1));

    assert_eq!(
        p.offer(s(9), g(0)),
        FrameVerdict::DiscardedStaleGeneration { generation: g(0) }
    );
    assert_eq!(
        p.offer(s(1), g(1)),
        FrameVerdict::AcceptedInFlight { seq: s(1) },
        "a new-generation frame must not be judged out-of-order against a \
         held old-generation displayed frame"
    );
    assert_eq!(p.displayed_seq(), Some(s(1)));
    assert_eq!(p.in_flight_seq(), Some(s(1)), "new frame is in-flight");
}
