//! Adaptive glass drawn by the real wgpu renderer, read back as pixels.
//!
//! Each test skips, saying so, where no GPU adapter exists.

use std::sync::Arc;

use iced::Rectangle;
use iced::advanced::Renderer as _;
use iced::advanced::image::{self, Renderer as _};
use iced::advanced::renderer::Headless;
use iced_wgpu::primitive::Renderer as _;

use super::{AdaptiveGlass, GlassPrimitive, GlassSilhouette, Material, MaterialPolicy};

fn renderer() -> Option<iced_wgpu::Renderer> {
    let renderer = iced_test::futures::futures::executor::block_on(
        <iced_wgpu::Renderer as Headless>::new(Default::default(), Some("wgpu")),
    );
    if renderer.is_none() {
        println!("SKIPPED: no wgpu adapter, adaptive glass cannot draw");
    }
    renderer
}

/// `rgba` as the backdrop, then each `(silhouette, material, bounds)` as
/// glass over it, in one frame.
fn draw(
    renderer: &mut iced_wgpu::Renderer,
    size: (u32, u32),
    rgba: Vec<u8>,
    glasses: Vec<(GlassSilhouette, Material, Rectangle)>,
) -> Vec<u8> {
    let full = Rectangle::with_size(iced::Size::new(size.0 as f32, size.1 as f32));
    renderer.draw_image(
        image::Image {
            filter_method: image::FilterMethod::Nearest,
            ..image::Image::new(image::Handle::from_rgba(size.0, size.1, rgba))
        },
        full,
        full,
    );
    for (silhouette, material, bounds) in glasses {
        let silhouette = Arc::new(silhouette);
        let mut boundary = material;
        boundary.measure_only = true;
        boundary.blur_sigma = 0.0;
        renderer.with_layer_material(bounds, boundary, |renderer| {
            renderer.draw_primitive(
                bounds,
                GlassPrimitive {
                    key: GlassPrimitive::key_for(&silhouette, bounds, &material, 1.0),
                    silhouette,
                    material,
                    alpha: 1.0,
                },
            );
        });
    }
    <iced_wgpu::Renderer as Headless>::screenshot(
        renderer,
        iced::Size::new(size.0, size.1),
        1.0,
        iced::Color::BLACK,
    )
}

/// Optics only: no highlight, tint or colour separation.
fn optics(preset: AdaptiveGlass, width: f32) -> Material {
    let mut material = preset.material(width, &MaterialPolicy::default());
    material.highlight = 0.0;
    material.tint_opacity = 0.0;
    material.dispersion = 0.0;
    material
}

fn stripes(size: u32) -> Vec<u8> {
    (0..size * size)
        .flat_map(|i| {
            if (i % size / 4).is_multiple_of(2) {
                [240, 30, 80, 255]
            } else {
                [20, 180, 220, 255]
            }
        })
        .collect()
}

/// A square ring, like a zero: shape with a counter.
fn ring(side: usize) -> GlassSilhouette {
    let mut alpha = vec![0; side * side];
    let (edge, hole) = (side / 8, side * 5 / 16);
    for y in edge..side - edge {
        for x in edge..side - edge {
            if x < hole || x >= side - hole || y < hole || y >= side - hole {
                alpha[y * side + x] = 255;
            }
        }
    }
    GlassSilhouette::from_alpha(side as u32, side as u32, &alpha)
}

fn px(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    pixels[i..i + 4].try_into().unwrap()
}

#[test]
fn frost_diffuses_the_backdrop_and_clear_keeps_it_sharp_outside_counters() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = 96;
    let backdrop = stripes(size);
    let full = Rectangle::with_size(iced::Size::new(size as f32, size as f32));
    let mut frosted = optics(AdaptiveGlass::Frosted, size as f32);
    frosted.refraction = 0.0;
    frosted.bevel_width = 0.0;
    let out = draw(
        &mut renderer,
        (size, size),
        backdrop.clone(),
        vec![(ring(32), frosted, full)],
    );
    assert_eq!(
        px(&out, size, 3, 3),
        px(&backdrop, size, 3, 3),
        "outside the shape"
    );
    assert_eq!(
        px(&out, size, 48, 48),
        px(&backdrop, size, 48, 48),
        "the counter stays clear"
    );
    let (before, after) = (px(&backdrop, size, 20, 48), px(&out, size, 20, 48));
    assert!(
        (60..200).contains(&after[0]) && before[0].abs_diff(after[0]) > 30,
        "frost mixes neighbouring stripes: {before:?} -> {after:?}"
    );

    let mut clear = frosted;
    clear.blur_sigma = 0.0;
    let mut renderer = renderer;
    let sharp = draw(
        &mut renderer,
        (size, size),
        backdrop.clone(),
        vec![(ring(32), clear, full)],
    );
    let worst = sharp
        .iter()
        .zip(&backdrop)
        .map(|(a, b)| a.abs_diff(*b))
        .max()
        .unwrap();
    assert!(
        worst <= 1,
        "clear glass with no optics changes nothing ({worst})"
    );
}

#[test]
fn each_component_bends_towards_its_own_centre() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let (width, height) = (256u32, 96u32);
    let gradient: Vec<u8> = (0..width * height)
        .flat_map(|i| [(i % width) as u8, 60, 70, 255])
        .collect();
    // Two numerals side by side.
    let mut alpha = vec![0; 512 * 192];
    for y in 16..176 {
        for x in (16..224).chain(288..496) {
            alpha[y * 512 + x] = 255;
        }
    }
    let bounds = Rectangle::with_size(iced::Size::new(width as f32, height as f32));
    for preset in [AdaptiveGlass::Clear, AdaptiveGlass::Frosted] {
        let silhouette = GlassSilhouette::from_alpha(512, 192, &alpha);
        let out = draw(
            &mut renderer,
            (width, height),
            gradient.clone(),
            vec![(silhouette, optics(preset, width as f32), bounds)],
        );
        let red = |x| i32::from(px(&out, width, x, 48)[0]);
        assert!(
            red(110) < 105 && red(145) > 150,
            "{preset:?}: inner edges bend towards their own numeral, not the pair: {} {}",
            red(110),
            red(145)
        );
        assert!(
            (red(60) - 60).abs() <= 1,
            "{preset:?}: a numeral's centre stays put"
        );
    }
}

#[test]
fn two_shapes_in_one_frame_keep_their_own_material() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = 96;
    let backdrop = stripes(size);
    let left = Rectangle::new([0.0, 0.0].into(), iced::Size::new(48.0, 48.0));
    let right = Rectangle::new([48.0, 48.0].into(), iced::Size::new(48.0, 48.0));
    // The clear shape is tinted, so it visibly draws.
    let mut clear = optics(AdaptiveGlass::Clear, 48.0);
    clear.refraction = 0.0;
    clear.bevel_width = 0.0;
    clear.tint_opacity = 0.4;
    let mut frosted = optics(AdaptiveGlass::Frosted, 48.0);
    frosted.refraction = 0.0;
    frosted.bevel_width = 0.0;
    let out = draw(
        &mut renderer,
        (size, size),
        backdrop.clone(),
        vec![(ring(32), clear, left), (ring(32), frosted, right)],
    );
    // Neighbouring stripes within each shape's left bar.
    let red = |x, y| i32::from(px(&out, size, x, y)[0]);
    assert_ne!(
        px(&out, size, 8, 24),
        px(&backdrop, size, 8, 24),
        "the clear shape draws"
    );
    assert!((red(8, 24) - red(12, 24)).abs() > 60, "and stays sharp");
    assert!(
        (red(56, 72) - red(60, 72)).abs() < 40,
        "the frosted shape diffuses"
    );
}

#[test]
fn the_tone_lifts_over_dark_backdrops_and_stays_clear_over_bright_ones() {
    let Some(mut renderer) = renderer() else {
        return;
    };
    let size = 64;
    let full = Rectangle::with_size(iced::Size::new(size as f32, size as f32));
    let solid = |v: u8| -> Vec<u8> { (0..size * size).flat_map(|_| [v, v, v, 255]).collect() };
    let mut material = AdaptiveGlass::Clear.material(size as f32, &MaterialPolicy::default());
    material.highlight = 0.0;
    let mut tone = |v: u8| {
        let out = draw(
            &mut renderer,
            (size, size),
            solid(v),
            vec![(ring(32), material, full)],
        );
        i32::from(px(&out, size, 10, 32)[0]) - i32::from(v)
    };
    let (over_dark, over_bright) = (tone(10), tone(240));
    assert!(
        over_dark > 40,
        "dark backdrop lifts towards white ({over_dark})"
    );
    assert!(
        over_bright.abs() <= 6,
        "bright backdrop stays translucent ({over_bright})"
    );
}

/// Equal shapes stacked at equal bounds keep separate slots whenever anything
/// their preparation writes differs, and share one only when it is all equal.
#[test]
fn a_slot_is_shared_only_by_identical_draws() {
    let silhouette = ring(32);
    let bounds = Rectangle::new([4.0, 4.0].into(), iced::Size::new(48.0, 48.0));
    let clear = optics(AdaptiveGlass::Clear, 48.0);
    let mut tinted = clear;
    tinted.tint_opacity = 0.9;
    let key =
        |material: &Material, alpha| GlassPrimitive::key_for(&silhouette, bounds, material, alpha);
    assert_eq!(key(&clear, 1.0), key(&clear, 1.0));
    assert_ne!(key(&clear, 1.0), key(&clear, 0.5), "alpha");
    assert_ne!(key(&clear, 1.0), key(&tinted, 1.0), "material");
    let moved = Rectangle { x: 5.0, ..bounds };
    assert_ne!(
        key(&clear, 1.0),
        GlassPrimitive::key_for(&silhouette, moved, &clear, 1.0),
        "bounds"
    );
}
