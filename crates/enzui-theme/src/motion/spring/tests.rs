use super::*;

/// The same motion integrated numerically (RK4 at 0.05 ms), the reference
/// the closed forms must match.
fn integrate(spring: Spring, x0: f64, v0: f64, t: f64) -> (f64, f64) {
    let k = f64::from(spring.stiffness());
    let c = 2.0 * f64::from(spring.damping_ratio()) * k.sqrt();
    let accel = |x: f64, v: f64| -k * x - c * v;
    let steps = (t / 5e-5).ceil().max(1.0) as usize;
    let h = t / steps as f64;
    let (mut x, mut v) = (x0, v0);
    for _ in 0..steps {
        let (k1x, k1v) = (v, accel(x, v));
        let (k2x, k2v) = (
            v + 0.5 * h * k1v,
            accel(x + 0.5 * h * k1x, v + 0.5 * h * k1v),
        );
        let (k3x, k3v) = (
            v + 0.5 * h * k2v,
            accel(x + 0.5 * h * k2x, v + 0.5 * h * k2v),
        );
        let (k4x, k4v) = (v + h * k3v, accel(x + h * k3x, v + h * k3v));
        x += h / 6.0 * (k1x + 2.0 * k2x + 2.0 * k3x + k4x);
        v += h / 6.0 * (k1v + 2.0 * k2v + 2.0 * k3v + k4v);
    }
    (x, v)
}

const CASES: [(f32, f32); 7] = [
    (800.0, 0.6),
    (380.0, 0.8),
    (1600.0, 1.0),
    (1600.0, 1.000_05),
    (300.0, 1.6),
    (157.9, 0.15),
    (50.0, 4.0),
];

#[test]
fn every_regime_matches_the_integrated_motion() {
    for (stiffness, ratio) in CASES {
        let spring = Spring::new(stiffness, ratio);
        for (x0, v0) in [(300.0, 0.0), (-40.0, 900.0), (0.0, -2500.0), (1.0, 0.0)] {
            for t in [0.004, 0.016, 0.05, 0.12, 0.3, 0.7, 1.5] {
                let (x, v) = spring.evaluate(x0, v0, t);
                let (rx, rv) = integrate(spring, f64::from(x0), f64::from(v0), f64::from(t));
                let scale = f64::from(x0.abs().max(1.0)) + f64::from(v0.abs()) / 10.0;
                assert!(
                    (f64::from(x) - rx).abs() <= 1e-4 * scale,
                    "x at {t}s for k {stiffness} ζ {ratio} from ({x0}, {v0}): {x} vs {rx}"
                );
                assert!(
                    (f64::from(v) - rv).abs() <= 1e-3 * scale * f64::from(stiffness).sqrt(),
                    "v at {t}s for k {stiffness} ζ {ratio} from ({x0}, {v0}): {v} vs {rv}"
                );
            }
        }
    }
}

#[test]
fn effects_springs_never_overshoot_and_spatial_ones_do() {
    for token in ExpressiveMotion::ALL {
        let spring = token.spring();
        let lowest = (0..2000)
            .map(|step| spring.evaluate(1.0, 0.0, step as f32 * 0.001).0)
            .fold(f32::INFINITY, f32::min);
        if token.is_spatial() {
            assert!(lowest < -0.01, "{token:?} overshoots: {lowest}");
            assert!(spring.overshoots());
        } else {
            assert!(
                lowest >= -1e-6,
                "{token:?} never passes its target: {lowest}"
            );
            assert!(!spring.overshoots());
        }
    }
}
