//! Which widgets actually need rounded clipping?

use crate::corner_probe::{
    assert_probe_contains_child, count_pin_in_rect_cutouts, cutout_capacity, fixed_magenta,
    pin_bbox, placed_at, render,
};

use iced::{Element, Length};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Act,
}

/// Logical render-target size. `PW` is the physical buffer width the probe
/// indexes with.
const W: u32 = 240;
const H: u32 = 200;
const PW: u32 = phys_w();

const fn phys_w() -> u32 {
    const SCALE_FACTOR: u32 = 3;
    W * SCALE_FACTOR
}

/// Where every specimen is placed. Never flush with the render target — a
/// layer whose bounds equal the base layer's is merged into it by
/// `graphics::layer::Stack::merge` (`iced/graphics/src/layer.rs`,
/// which compares bounds and never consults `clip_radius`) and loses its radius
/// on wgpu.
const OX: f32 = 20.0;
const OY: f32 = 16.0;

const PRESSABLE_RECT: (f32, f32, f32, f32) = (OX, OY, 160.0, 120.0);
const PRESSABLE_R: f32 = 24.0;

// 32 -- pill radius at this height

/// Assert the specimen really landed where the probe is looking, that it paints
/// magenta *somewhere*, and that a meaningful fraction of the probe's sampled
/// pixels fall inside the child it measured.
fn calibrate(buf: &[u8], name: &str, rect: (f32, f32, f32, f32), r: f32, min_frac: f32) {
    let bbox = pin_bbox(buf, W, H)
        .unwrap_or_else(|| panic!("{name}: specimen painted no magenta at all — probe is blind"));
    println!("CALIBRATION {name} pin bbox = {bbox:?} (probe rect {rect:?} r={r})");
    let (bx, by, bw, bh) = rect;
    assert!(
        bbox.0 < bx + bw && bbox.1 < by + bh && bbox.0 + bbox.2 > bx && bbox.1 + bbox.3 > by,
        "{name}: magenta bbox {bbox:?} does not overlap the probe rect {rect:?}"
    );
    // The measured child bounds, not an assumed rectangle.
    assert_probe_contains_child(name, rect, r, bbox, min_frac);
}

/// The pressable specimen. `radius` is caller-settable (`.radius(f32)`), so a
/// large one is a legitimate configuration and makes the cutout wide enough to
/// sample thickly.
fn pressable_specimen(radius: f32) -> Element<'static, Msg> {
    use enzui_widgets::pressable::enz_pressable;

    let rect = PRESSABLE_RECT;
    placed_at(
        enz_pressable(fixed_magenta::<Msg>(rect.2, rect.3), Msg::Act)
            .radius(radius)
            .width(Length::Fixed(rect.2))
            .into(),
        OX,
        OY,
    )
}

#[test]
fn pressable_masks_a_full_bleed_child() {
    let rect = PRESSABLE_RECT;
    let cap = cutout_capacity(rect, PRESSABLE_R);
    let buf = render(pressable_specimen(PRESSABLE_R), W, H);
    calibrate(&buf, "pressable", rect, PRESSABLE_R, 0.95);
    let seen = count_pin_in_rect_cutouts(&buf, PW, rect, PRESSABLE_R);
    println!("MEASURED pressable r{PRESSABLE_R} = {seen}/{cap}");

    assert_eq!(
        seen, 0,
        "EnzPressable left {seen}/{cap}px of a full-bleed child outside its \
         {PRESSABLE_R}px corner arc. Its content padding defaults to ZERO, so \
         the child is edge-to-edge and `clip(true)`'s rectangular scissor \
         cannot help — it needs `rounded_clip`."
    );
}

// ScrollCardRadius::Lg
// left half of the r=12 cutout at SCALE=3
