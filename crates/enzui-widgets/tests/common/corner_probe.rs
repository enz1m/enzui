//! Shared rounded-corner pixel probes for `tests/`: headless
//! render, magenta-pin containment inside corner cutouts, and
//! logical-unit AA slack.
//!
//! Sibling test binaries pull this in with
//! `#[path = "common/corner_probe.rs"] mod corner_probe;` — files
//! under `tests/*/*` are not compiled as their own binaries.

use iced::widget::{Space, container};
use iced::{Color, Element, Length, Theme};

pub const PIN: Color = Color {
    r: 1.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};

/// Corner-arc exclusion slack in logical units. Held logical so
/// the exclusion band stays a constant physical width at any
/// [`SCALE`] (see [`for_each_cutout_pixel`]).
pub const ARC_SLACK_LOGICAL: f32 = 1.5;

/// `SCALE = 3` is the smallest supersample factor at which `r=5`
/// clears with margin under [`ARC_SLACK_LOGICAL`]. Cost grows as
/// `SCALE²`; callers measuring axis-aligned rectangles can pass a
/// smaller `scale` to [`render_scaled`].
///
/// Rounded-clip scale correctness lives in the fork at
/// `iced/tiny_skia/src/lib.rs`.
pub const SCALE: f32 = 3.0;

/// Returned buffer is `SCALE`× larger than `w`/`h` in each axis
/// (see [`phys`]).
pub fn render<Message: 'static>(el: Element<'static, Message>, w: u32, h: u32) -> Vec<u8> {
    render_with_cursor(el, w, h, SCALE, iced::advanced::mouse::Cursor::Unavailable)
}

/// Corner and arc probes must use [`render`] instead: at `scale =
/// 1` a small radius clears to zero usable samples (see [`SCALE`]).
pub fn render_scaled<Message: 'static>(
    el: Element<'static, Message>,
    w: u32,
    h: u32,
    scale: f32,
) -> Vec<u8> {
    render_with_cursor(el, w, h, scale, iced::advanced::mouse::Cursor::Unavailable)
}

/// Parks the pointer at `(x, y)` so hover-only chrome paints.
pub fn render_hovering<Message: 'static>(
    el: Element<'static, Message>,
    w: u32,
    h: u32,
    x: f32,
    y: f32,
) -> Vec<u8> {
    render_with_cursor(
        el,
        w,
        h,
        SCALE,
        iced::advanced::mouse::Cursor::Available(iced::Point::new(x, y)),
    )
}

pub fn phys(logical: u32) -> u32 {
    phys_scaled(logical, SCALE)
}

/// Row stride for a buffer rendered with [`render_scaled`] at the
/// same `scale`.
pub fn phys_scaled(logical: u32, scale: f32) -> u32 {
    (logical as f32 * scale) as u32
}

/// Renders with overlays laid out and painted.
///
/// `UserInterface::draw` paints an overlay only once an `update` or an
/// `operate` pass has laid one out — a plain `build` + `draw` silently skips
/// every tooltip, popover and dropdown in the tree. This runs one inert
/// `find_focused` operation first, which is the cheapest way to make the
/// runtime lay the overlay out, and then draws exactly like
/// [`render_scaled`].
pub fn render_with_overlays<Message: 'static>(
    el: Element<'static, Message>,
    w: u32,
    h: u32,
    scale: f32,
) -> Vec<u8> {
    render_inner(
        el,
        w,
        h,
        scale,
        iced::advanced::mouse::Cursor::Unavailable,
        true,
    )
}

fn render_with_cursor<Message: 'static>(
    el: Element<'static, Message>,
    w: u32,
    h: u32,
    scale: f32,
    cursor: iced::advanced::mouse::Cursor,
) -> Vec<u8> {
    render_inner(el, w, h, scale, cursor, false)
}

fn render_inner<Message: 'static>(
    el: Element<'static, Message>,
    w: u32,
    h: u32,
    scale: f32,
    cursor: iced::advanced::mouse::Cursor,
    overlays: bool,
) -> Vec<u8> {
    use iced::advanced::renderer::Headless;

    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        el,
        iced::Size::new(w as f32, h as f32),
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );

    if overlays {
        let mut operation = iced::advanced::widget::operation::focusable::find_focused();
        let mut boxed: Box<dyn iced::advanced::widget::Operation<()>> =
            Box::new(iced::advanced::widget::operation::black_box(&mut operation));
        ui.operate(&renderer, &mut boxed);
    }

    let theme = Theme::Dark;
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
    ui.draw(
        &mut renderer,
        &theme,
        &iced::advanced::renderer::Style {
            text_color: base.text_color,
        },
        cursor,
    );
    renderer.screenshot(
        iced::Size::new(phys_scaled(w, scale), phys_scaled(h, scale)),
        scale,
        base.background_color,
    )
}

pub fn is_pin(buf: &[u8], w: u32, x: u32, y: u32) -> bool {
    let i = ((y * w + x) * 4) as usize;
    let (r, g, b) = (
        i32::from(buf[i]),
        i32::from(buf[i + 1]),
        i32::from(buf[i + 2]),
    );
    r > 150 && g < 100 && b > 150
}

/// Visits every physical pixel inside the bounding box of `rect`
/// but strictly outside the rounded path of radius `r`. The
/// exclusion slack past the arc is [`ARC_SLACK_LOGICAL`] scaled
/// with `SCALE`, so the AA ring of the rounded fill does not read
/// as a leak at any [`SCALE`].
pub fn for_each_cutout_pixel(rect: (f32, f32, f32, f32), r: f32, mut f: impl FnMut(u32, u32)) {
    let s = SCALE;
    let (bx, by, bw, bh) = (rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s);
    let r = r * s;
    let slack = ARC_SLACK_LOGICAL * s;
    // (corner-square origin, arc centre) for tl, tr, bl, br.
    let centres = [
        (bx, by, bx + r, by + r),
        (bx + bw - r, by, bx + bw - r, by + r),
        (bx, by + bh - r, bx + r, by + bh - r),
        (bx + bw - r, by + bh - r, bx + bw - r, by + bh - r),
    ];
    for (ox, oy, cx, cy) in centres {
        for y in (oy as u32)..((oy + r) as u32) {
            for x in (ox as u32)..((ox + r) as u32) {
                let dx = x as f32 + 0.5 - cx;
                let dy = y as f32 + 0.5 - cy;
                if (dx * dx + dy * dy).sqrt() > r + slack {
                    f(x, y);
                }
            }
        }
    }
}

/// Sample count from [`for_each_cutout_pixel`]. A leak count is
/// only meaningful reported against this; a capacity of 0 means
/// the probe is blind.
pub fn cutout_capacity(rect: (f32, f32, f32, f32), r: f32) -> u32 {
    let mut n = 0;
    for_each_cutout_pixel(rect, r, |_, _| n += 1);
    n
}

/// `w` is the physical buffer width (see [`phys`]).
pub fn count_pin_in_rect_cutouts(buf: &[u8], w: u32, rect: (f32, f32, f32, f32), r: f32) -> u32 {
    let mut n = 0;
    for_each_cutout_pixel(rect, r, |x, y| {
        if is_pin(buf, w, x, y) {
            n += 1;
        }
    });
    n
}

/// A leak count is computed from the pixels the probe visits, so
/// overlap of `rect` with `child` is not sufficient — the visited
/// pixels must land inside `child`. [`assert_probe_contains_child`]
/// gates on this count.
pub fn cutout_pixels_inside(
    rect: (f32, f32, f32, f32),
    r: f32,
    child: (f32, f32, f32, f32),
) -> u32 {
    let s = SCALE;
    let (cx0, cy0) = (child.0 * s, child.1 * s);
    let (cx1, cy1) = (cx0 + child.2 * s, cy0 + child.3 * s);
    let mut n = 0;
    for_each_cutout_pixel(rect, r, |x, y| {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        if px >= cx0 && px < cx1 && py >= cy0 && py < cy1 {
            n += 1;
        }
    });
    n
}

/// Asserts that at least `min_frac` of [`cutout_capacity`] lies
/// inside `child`. Returns the contained count.
pub fn assert_probe_contains_child(
    name: &str,
    rect: (f32, f32, f32, f32),
    r: f32,
    child: (f32, f32, f32, f32),
    min_frac: f32,
) -> u32 {
    let cap = cutout_capacity(rect, r);
    let inside = cutout_pixels_inside(rect, r, child);
    let frac = if cap == 0 {
        0.0
    } else {
        inside as f32 / cap as f32
    };
    println!(
        "CONTAINMENT {name}: {inside}/{cap} probe px inside child {child:?} \
         ({:.1}%, floor {:.0}%)",
        frac * 100.0,
        min_frac * 100.0
    );
    assert!(
        frac >= min_frac,
        "{name}: only {inside}/{cap} ({:.1}%) of the probe's sampled pixels \
         fall inside the child's measured bounds {child:?} against probe rect \
         {rect:?} r={r} -- a zero leak here would measure empty space, not the \
         mask (this is the titlebar_icon_group empty-space false positive)",
        frac * 100.0
    );
    inside
}

/// Diagnostic bbox of every magenta pixel in `buf` (logical units).
pub fn pin_bbox(buf: &[u8], w: u32, h: u32) -> Option<(f32, f32, f32, f32)> {
    let (pw, ph) = (phys(w), phys(h));
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..ph {
        for x in 0..pw {
            if is_pin(buf, pw, x, y) {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then(|| {
        (
            x0 as f32 / SCALE,
            y0 as f32 / SCALE,
            (x1 - x0 + 1) as f32 / SCALE,
            (y1 - y0 + 1) as f32 / SCALE,
        )
    })
}

pub fn full_bleed_child<Message: 'static>() -> Element<'static, Message> {
    container(Space::new().width(Length::Fill).height(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_: &Theme| container::Style {
            background: Some(iced::Background::Color(PIN)),
            ..Default::default()
        })
        .into()
}

/// For `Length::Shrink` widgets, where a `Fill` child collapses to
/// nothing.
pub fn fixed_magenta<Message: 'static>(w: f32, h: f32) -> Element<'static, Message> {
    container(
        Space::new()
            .width(Length::Fixed(w))
            .height(Length::Fixed(h)),
    )
    .width(Length::Fixed(w))
    .height(Length::Fixed(h))
    .style(|_: &Theme| container::Style {
        background: Some(iced::Background::Color(PIN)),
        ..Default::default()
    })
    .into()
}

/// Always inset from the target: a layer whose bounds equal the
/// base layer's is folded into it by `graphics::layer::Stack::merge`
/// (`iced/graphics/src/layer.rs`, whose merge
/// condition compares bounds without consulting `clip_radius`),
/// losing the radius on wgpu.
pub fn placed_at<Message: 'static>(
    el: Element<'static, Message>,
    x: f32,
    y: f32,
) -> Element<'static, Message> {
    container(el)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(iced::Padding {
            top: y,
            left: x,
            right: 0.0,
            bottom: 0.0,
        })
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Top)
        .into()
}

/// Counterpart of [`pin_bbox`] for widgets that compose their own
/// children and cannot be handed a magenta slot: bbox of every
/// non-background pixel (logical units).
pub fn ink_bbox(buf: &[u8], w: u32, h: u32) -> Option<(f32, f32, f32, f32)> {
    let (pw, ph) = (phys(w), phys(h));
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(&Theme::Dark);
    let bg = base.background_color;
    let (br, bgc, bb) = (
        (bg.r * 255.0) as i32,
        (bg.g * 255.0) as i32,
        (bg.b * 255.0) as i32,
    );
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    let mut any = false;
    for y in 0..ph {
        for x in 0..pw {
            let i = ((y * pw + x) * 4) as usize;
            let d = (i32::from(buf[i]) - br).abs()
                + (i32::from(buf[i + 1]) - bgc).abs()
                + (i32::from(buf[i + 2]) - bb).abs();
            if d > 24 {
                any = true;
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    any.then(|| {
        (
            x0 as f32 / SCALE,
            y0 as f32 / SCALE,
            (x1 - x0 + 1) as f32 / SCALE,
            (y1 - y0 + 1) as f32 / SCALE,
        )
    })
}

#[derive(Clone, Copy)]
pub struct InkTarget {
    pub rgb: (i32, i32, i32),
    pub tol: i32,
}

impl InkTarget {
    pub fn new(ink: Color, tol: i32) -> Self {
        InkTarget {
            rgb: (
                (ink.r * 255.0) as i32,
                (ink.g * 255.0) as i32,
                (ink.b * 255.0) as i32,
            ),
            tol,
        }
    }
}

/// One pass over `buf`, dispatching each pixel to whichever
/// `inks[i]` it matches; result at index `i` is `None` when that
/// ink is absent. `scale` must match the value passed to
/// [`render_scaled`].
pub fn ink_bboxes_scaled(
    buf: &[u8],
    w: u32,
    h: u32,
    scale: f32,
    inks: &[InkTarget],
) -> Vec<Option<(f32, f32, f32, f32)>> {
    let (pw, ph) = (phys_scaled(w, scale), phys_scaled(h, scale));
    // (x0, y0, x1, y1, any) per ink.
    let mut acc: Vec<(u32, u32, u32, u32, bool)> =
        vec![(u32::MAX, u32::MAX, 0, 0, false); inks.len()];
    for y in 0..ph {
        let row = (y * pw) as usize * 4;
        for x in 0..pw {
            let i = row + x as usize * 4;
            let (r, g, b) = (
                i32::from(buf[i]),
                i32::from(buf[i + 1]),
                i32::from(buf[i + 2]),
            );
            for (k, ink) in inks.iter().enumerate() {
                let (tr, tg, tb) = ink.rgb;
                if (r - tr).abs() + (g - tg).abs() + (b - tb).abs() < ink.tol {
                    let a = &mut acc[k];
                    a.4 = true;
                    a.0 = a.0.min(x);
                    a.1 = a.1.min(y);
                    a.2 = a.2.max(x);
                    a.3 = a.3.max(y);
                    // A physical pixel belongs to at most one saturated ink, so
                    // stop at the first match.
                    break;
                }
            }
        }
    }
    acc.into_iter()
        .map(|(x0, y0, x1, y1, any)| {
            any.then(|| {
                (
                    x0 as f32 / scale,
                    y0 as f32 / scale,
                    (x1 - x0 + 1) as f32 / scale,
                    (y1 - y0 + 1) as f32 / scale,
                )
            })
        })
        .collect()
}

/// Single-ink convenience over [`ink_bboxes_scaled`].
pub fn ink_bbox_scaled(
    buf: &[u8],
    w: u32,
    h: u32,
    scale: f32,
    ink: Color,
    tol: i32,
) -> Option<(f32, f32, f32, f32)> {
    ink_bboxes_scaled(buf, w, h, scale, &[InkTarget::new(ink, tol)])
        .into_iter()
        .next()
        .flatten()
}
