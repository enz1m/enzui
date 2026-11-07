#![allow(
    clippy::string_slice,
    reason = "the tests slice ASCII SVG markup at found delimiters"
)]
use super::*;

/// The frame at `time` and its text.
struct Written {
    text: String,
    next: Next,
}

fn frame_at(svg: &AnimatedSvg, timeline: &mut Timeline, time: f64) -> Written {
    let frame = svg.inner.scene.frame(timeline, time);
    Written {
        text: svg.inner.scene.text(&frame),
        next: frame.next,
    }
}

fn timeline(svg: &AnimatedSvg) -> Timeline {
    Timeline::new(svg.inner.scene.animations.len())
}

#[test]
fn a_finished_animation_asks_for_nothing_and_steps_wake_only_per_step() {
    let svg = AnimatedSvg::from_text(
        r#"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10"><animate attributeName="x" from="0" to="5" dur="1s" fill="freeze"/></rect></svg>"#,
    )
    .unwrap();
    let mut timeline = timeline(&svg);
    assert_eq!(frame_at(&svg, &mut timeline, 0.5).next, Next::Continuous);
    let done = frame_at(&svg, &mut timeline, 1.5);
    assert_eq!(done.next, Next::Never);
    assert!(done.text.contains("x=\"5\""), "{}", done.text);

    let steps = AnimatedSvg::from_text(
        r#"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><style>rect{animation:blink 1s steps(4) infinite}@keyframes blink{to{opacity:0}}</style><rect width="10" height="10"/></svg>"#,
    )
    .unwrap();
    let mut timeline = Timeline::new(0);
    let frame = steps.inner.scene.frame(&mut timeline, 0.1);
    assert_eq!(frame.next, Next::At(0.25));
}

/// The attribute or style value `name` written on the element whose markup
/// starts with `tag`.
fn value_in(text: &str, tag: &str, name: &str) -> Option<String> {
    let start = text.find(tag)?;
    let element = &text[start..start + text[start..].find('>')?];
    // A style declaration wins over a presentation attribute.
    let styled = element.find("style=\"").and_then(|style| {
        let style = style + 7;
        let style = &element[style..style + element[style..].find('"')?];
        style
            .split(';')
            .find_map(|d| d.strip_prefix(&format!("{name}:")).map(str::to_owned))
    });
    styled.or_else(|| {
        let attribute = format!(" {name}=\"");
        let at = element.find(&attribute)? + attribute.len();
        Some(element[at..at + element[at..].find('"')?].to_owned())
    })
}

#[test]
fn common_animation_patterns_play() {
    // Line drawing: a CSS dash offset running down to zero.
    let draw = AnimatedSvg::from_text(
        r#"<svg width="100" height="10" xmlns="http://www.w3.org/2000/svg"><style>.l{stroke-dasharray:100;stroke-dashoffset:100;animation:draw 2s ease-out forwards}@keyframes draw{to{stroke-dashoffset:0}}</style><path class="l" d="M0 5 H100" stroke="black"/></svg>"#,
    )
    .unwrap();
    let mut t = timeline(&draw);
    let start = frame_at(&draw, &mut t, 0.0);
    assert_eq!(
        value_in(&start.text, "<path", "stroke-dashoffset").as_deref(),
        Some("100")
    );
    let done = frame_at(&draw, &mut t, 3.0);
    assert_eq!(
        value_in(&done.text, "<path", "stroke-dashoffset").as_deref(),
        Some("0")
    );
    assert_eq!(done.next, Next::Never);

    // A dot along a path, turned with it.
    let motion = AnimatedSvg::from_text(
        r##"<svg width="100" height="100" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><path id="track" d="M0 0 L100 0 L100 100" fill="none"/><circle r="4"><animateMotion dur="2s" rotate="auto" repeatCount="indefinite"><mpath xlink:href="#track"/></animateMotion></circle></svg>"##,
    )
    .unwrap();
    let mut t = timeline(&motion);
    let quarter = frame_at(&motion, &mut t, 0.5);
    assert_eq!(
        value_in(&quarter.text, "<circle", "transform").as_deref(),
        Some("matrix(1 0 0 1 50 0)")
    );
    let three_quarters = frame_at(&motion, &mut t, 1.5);
    assert_eq!(
        value_in(&three_quarters.text, "<circle", "transform").as_deref(),
        Some("matrix(0 1 -1 0 100 50)"),
        "turned down the second leg"
    );

    // A gradient stop, a used symbol and the root view box.
    let defs = AnimatedSvg::from_text(
        r##"<svg width="20" height="20" viewBox="0 0 20 20" xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><defs><linearGradient id="g"><stop offset="0" stop-color="red"><animate attributeName="stop-color" to="blue" dur="1s" fill="freeze"/></stop></linearGradient><rect id="r" width="10" height="10" fill="url(#g)"/></defs><use xlink:href="#r"/><animate attributeName="viewBox" to="0 0 10 10" dur="1s" fill="freeze"/></svg>"##,
    )
    .unwrap();
    let mut t = timeline(&defs);
    let done = frame_at(&defs, &mut t, 2.0);
    assert_eq!(
        value_in(&done.text, "<stop", "stop-color").as_deref(),
        Some("#0000ff")
    );
    assert_eq!(
        value_in(&done.text, "<svg", "viewBox").as_deref(),
        Some("0 0 10 10")
    );
    assert!(render::parse(&done.text).is_some());

    // A morph between paths of the same commands, from CSS.
    let morph = AnimatedSvg::from_text(
        r#"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><style>path{animation:m 1s linear both}@keyframes m{from{d:path("M0 0 L10 0")}to{d:path("M0 10 L10 10")}}</style><path stroke="black"/></svg>"#,
    )
    .unwrap();
    let mut t = timeline(&morph);
    let half = frame_at(&morph, &mut t, 0.5);
    assert_eq!(
        value_in(&half.text, "<path", "d").as_deref(),
        Some("M 0 5 L 10 5")
    );

    // An additive rotation on top of the element's own transform.
    let additive = AnimatedSvg::from_text(
        r#"<svg width="10" height="10" xmlns="http://www.w3.org/2000/svg"><rect width="2" height="2" transform="translate(5 0)"><animateTransform attributeName="transform" type="rotate" from="0" to="90" dur="1s" additive="sum" fill="freeze"/></rect></svg>"#,
    )
    .unwrap();
    let mut t = timeline(&additive);
    let done = frame_at(&additive, &mut t, 2.0);
    assert_eq!(
        value_in(&done.text, "<rect", "transform").as_deref(),
        Some("matrix(0 1 -1 0 5 0)")
    );
}
