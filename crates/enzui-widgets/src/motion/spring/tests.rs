use super::*;
use enzui_theme::ExpressiveMotion;

fn ms(start: Instant, ms: u64) -> Instant {
    start + Duration::from_millis(ms)
}

#[test]
fn a_reversal_keeps_position_and_velocity() {
    let start = Instant::now();
    let mut animation = SpringAnimation::new(0.0_f32, ExpressiveMotion::DefaultSpatial.spring());
    animation.set_target(300.0, start);
    let turn = ms(start, 90);
    let (before, speed) = (animation.value(turn), animation.velocity(turn));
    assert!(speed > 500.0, "moving fast towards 300: {speed}");
    animation.set_target(0.0, turn);
    assert!((animation.value(turn) - before).abs() < 1e-3, "no jump");
    assert!(
        (animation.velocity(turn) - speed).abs() < 1e-2,
        "still moving at the same speed the same way"
    );
    // It keeps going a little before it turns back.
    assert!(animation.value(ms(start, 100)) > before);
}

#[test]
fn lane_keyframes_are_the_drawn_values() {
    let start = Instant::now();
    let mut animation = SpringAnimation::new(0.0_f32, ExpressiveMotion::FastSpatial.spring());
    animation.set_target(1.0, start);
    let now = ms(start, 40);
    let lane = animation
        .track_animation(NodeId(1), now, |x| Track {
            node: NodeId(1),
            translation: iced::Vector::new(x * 100.0, 0.0),
            opacity: 1.0,
            scale: 1.0,
            scale_origin: iced::Point::ORIGIN,
            rotation: 0.0,
            clip: None,
        })
        .expect("moving");
    assert_eq!(Some(now + lane.duration), animation.lands_at());
    assert!(lane.keyframes.len() as f32 >= lane.duration.as_secs_f32() * SPRING_TRACK_RATE);
    for (at, track) in &lane.keyframes {
        let drawn = animation.value(now + lane.duration.mul_f32(*at)) * 100.0;
        assert!((track.translation.x - drawn).abs() < 1e-3);
    }
    let (last, track) = lane.keyframes.last().unwrap();
    assert_eq!((*last, track.translation.x), (1.0, 100.0));
}
