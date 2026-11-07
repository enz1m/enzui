use super::*;

/// The evaluator is the CSS curve: checked against values computed in f64
/// with the same parametric solve, and monotonic in time for a monotonic y.
#[test]
fn the_curve_matches_a_double_precision_reference() {
    fn reference(c: CubicBezier, t: f64) -> f64 {
        let (x1, y1, x2, y2) = c.controls();
        let b = |s: f64, a: f64, b: f64| {
            let r = 1.0 - s;
            3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
        };
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if b(mid, f64::from(x1), f64::from(x2)) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        b(0.5 * (lo + hi), f64::from(y1), f64::from(y2))
    }
    for motion in ExpressiveMotion::ALL {
        let curve = motion.curve();
        for i in 1..100 {
            let t = i as f32 / 100.0;
            let got = f64::from(curve.evaluate(t));
            let want = reference(curve, f64::from(t));
            assert!(
                (got - want).abs() < 1e-4,
                "{motion:?} at {t}: {got} vs {want}"
            );
        }
    }
    let effects = ExpressiveMotion::DefaultEffects.curve();
    let mut last = 0.0;
    for i in 0..=100 {
        let value = effects.evaluate(i as f32 / 100.0);
        assert!(value >= last, "effects curve is monotonic");
        last = value;
    }
}
