//! Box drawing, block elements, braille and Powerline separators, drawn to
//! the cell's exact pixels instead of taken from a font.
//!
//! Font glyphs for these characters rarely span the cell, so borders drawn
//! with them show gaps between rows and columns. Every major terminal draws
//! them itself. The line-join rules follow Ghostty's `linesChar`
//! (src/font/sprite/draw/box.zig, MIT licence).

/// Whether `ch` is drawn here rather than from a font.
#[must_use]
pub(crate) fn is_drawn(ch: char) -> bool {
    matches!(ch as u32, 0x2500..=0x259F | 0x2800..=0x28FF | 0xE0B0..=0xE0B3)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Line {
    None,
    Light,
    Heavy,
    Double,
}

use Line::{Double as D, Heavy as H, Light as L, None as N};

/// The arms of a line character: up, right, down, left.
#[derive(Clone, Copy)]
struct Arms {
    up: Line,
    right: Line,
    down: Line,
    left: Line,
}

const fn arms(up: Line, right: Line, down: Line, left: Line) -> Arms {
    Arms {
        up,
        right,
        down,
        left,
    }
}

fn line_arms(ch: u32) -> Option<Arms> {
    Some(match ch {
        0x2500 => arms(N, L, N, L),
        0x2501 => arms(N, H, N, H),
        0x2502 => arms(L, N, L, N),
        0x2503 => arms(H, N, H, N),
        0x250C => arms(N, L, L, N),
        0x250D => arms(N, H, L, N),
        0x250E => arms(N, L, H, N),
        0x250F => arms(N, H, H, N),
        0x2510 => arms(N, N, L, L),
        0x2511 => arms(N, N, L, H),
        0x2512 => arms(N, N, H, L),
        0x2513 => arms(N, N, H, H),
        0x2514 => arms(L, L, N, N),
        0x2515 => arms(L, H, N, N),
        0x2516 => arms(H, L, N, N),
        0x2517 => arms(H, H, N, N),
        0x2518 => arms(L, N, N, L),
        0x2519 => arms(L, N, N, H),
        0x251A => arms(H, N, N, L),
        0x251B => arms(H, N, N, H),
        0x251C => arms(L, L, L, N),
        0x251D => arms(L, H, L, N),
        0x251E => arms(H, L, L, N),
        0x251F => arms(L, L, H, N),
        0x2520 => arms(H, L, H, N),
        0x2521 => arms(H, H, L, N),
        0x2522 => arms(L, H, H, N),
        0x2523 => arms(H, H, H, N),
        0x2524 => arms(L, N, L, L),
        0x2525 => arms(L, N, L, H),
        0x2526 => arms(H, N, L, L),
        0x2527 => arms(L, N, H, L),
        0x2528 => arms(H, N, H, L),
        0x2529 => arms(H, N, L, H),
        0x252A => arms(L, N, H, H),
        0x252B => arms(H, N, H, H),
        0x252C => arms(N, L, L, L),
        0x252D => arms(N, L, L, H),
        0x252E => arms(N, H, L, L),
        0x252F => arms(N, H, L, H),
        0x2530 => arms(N, L, H, L),
        0x2531 => arms(N, L, H, H),
        0x2532 => arms(N, H, H, L),
        0x2533 => arms(N, H, H, H),
        0x2534 => arms(L, L, N, L),
        0x2535 => arms(L, L, N, H),
        0x2536 => arms(L, H, N, L),
        0x2537 => arms(L, H, N, H),
        0x2538 => arms(H, L, N, L),
        0x2539 => arms(H, L, N, H),
        0x253A => arms(H, H, N, L),
        0x253B => arms(H, H, N, H),
        0x253C => arms(L, L, L, L),
        0x253D => arms(L, L, L, H),
        0x253E => arms(L, H, L, L),
        0x253F => arms(L, H, L, H),
        0x2540 => arms(H, L, L, L),
        0x2541 => arms(L, L, H, L),
        0x2542 => arms(H, L, H, L),
        0x2543 => arms(H, L, L, H),
        0x2544 => arms(H, H, L, L),
        0x2545 => arms(L, L, H, H),
        0x2546 => arms(L, H, H, L),
        0x2547 => arms(H, H, L, H),
        0x2548 => arms(L, H, H, H),
        0x2549 => arms(H, L, H, H),
        0x254A => arms(H, H, H, L),
        0x254B => arms(H, H, H, H),
        0x2550 => arms(N, D, N, D),
        0x2551 => arms(D, N, D, N),
        0x2552 => arms(N, D, L, N),
        0x2553 => arms(N, L, D, N),
        0x2554 => arms(N, D, D, N),
        0x2555 => arms(N, N, L, D),
        0x2556 => arms(N, N, D, L),
        0x2557 => arms(N, N, D, D),
        0x2558 => arms(L, D, N, N),
        0x2559 => arms(D, L, N, N),
        0x255A => arms(D, D, N, N),
        0x255B => arms(L, N, N, D),
        0x255C => arms(D, N, N, L),
        0x255D => arms(D, N, N, D),
        0x255E => arms(L, D, L, N),
        0x255F => arms(D, L, D, N),
        0x2560 => arms(D, D, D, N),
        0x2561 => arms(L, N, L, D),
        0x2562 => arms(D, N, D, L),
        0x2563 => arms(D, N, D, D),
        0x2564 => arms(N, D, L, D),
        0x2565 => arms(N, L, D, L),
        0x2566 => arms(N, D, D, D),
        0x2567 => arms(L, D, N, D),
        0x2568 => arms(D, L, N, L),
        0x2569 => arms(D, D, N, D),
        0x256A => arms(L, D, L, D),
        0x256B => arms(D, L, D, L),
        0x256C => arms(D, D, D, D),
        0x2574 => arms(N, N, N, L),
        0x2575 => arms(L, N, N, N),
        0x2576 => arms(N, L, N, N),
        0x2577 => arms(N, N, L, N),
        0x2578 => arms(N, N, N, H),
        0x2579 => arms(H, N, N, N),
        0x257A => arms(N, H, N, N),
        0x257B => arms(N, N, H, N),
        0x257C => arms(N, H, N, L),
        0x257D => arms(L, N, H, N),
        0x257E => arms(N, L, N, H),
        0x257F => arms(H, N, L, N),
        _ => return None,
    })
}

/// An 8-bit coverage mask the size of one cell.
struct Canvas {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height) as usize],
        }
    }

    /// Fills `x0..x1` by `y0..y1`, clamped to the canvas.
    fn rect(&mut self, x0: u32, y0: u32, x1: u32, y1: u32, alpha: u8) {
        for y in y0.min(self.height)..y1.min(self.height) {
            for x in x0.min(self.width)..x1.min(self.width) {
                let p = &mut self.data[(y * self.width + x) as usize];
                *p = (*p).max(alpha);
            }
        }
    }

    /// Fills by coverage of `inside`, sampled at 4 x 4 points per pixel,
    /// in pixel coordinates.
    fn shape(&mut self, inside: impl Fn(f32, f32) -> bool) {
        const N: u32 = 4;
        for y in 0..self.height {
            for x in 0..self.width {
                let mut hits = 0;
                for sy in 0..N {
                    for sx in 0..N {
                        let px = x as f32 + (sx as f32 + 0.5) / N as f32;
                        let py = y as f32 + (sy as f32 + 0.5) / N as f32;
                        hits += u32::from(inside(px, py));
                    }
                }
                if hits > 0 {
                    let alpha = (hits * 255 / (N * N)) as u8;
                    let p = &mut self.data[(y * self.width + x) as usize];
                    *p = (*p).max(alpha);
                }
            }
        }
    }
}

/// The coverage mask for `ch` in a `width` by `height` cell, with light
/// lines `stroke` pixels thick; `None` when `ch` is not drawn here.
#[must_use]
pub(crate) fn draw(ch: char, width: u32, height: u32, stroke: u32) -> Option<Vec<u8>> {
    if width == 0 || height == 0 {
        return None;
    }
    let code = ch as u32;
    let mut canvas = Canvas::new(width, height);
    let light = stroke.max(1);
    let heavy = (light * 2).max(light + 1);
    if let Some(lines) = line_arms(code) {
        lines_char(&mut canvas, lines, light, heavy);
    } else {
        match code {
            0x2504..=0x250B | 0x254C..=0x254F => dashes(&mut canvas, code, light, heavy),
            0x256D..=0x2570 => arc(&mut canvas, code, light),
            0x2571..=0x2573 => diagonals(&mut canvas, code, light),
            0x2580..=0x259F => block(&mut canvas, code),
            0x2800..=0x28FF => braille(&mut canvas, code),
            0xE0B0..=0xE0B3 => powerline(&mut canvas, code, light),
            _ => return None,
        }
    }
    Some(canvas.data)
}

fn lines_char(canvas: &mut Canvas, lines: Arms, light: u32, heavy: u32) {
    let (w, h) = (canvas.width, canvas.height);
    let h_light_top = h.saturating_sub(light) / 2;
    let h_light_bottom = h_light_top + light;
    let h_heavy_top = h.saturating_sub(heavy) / 2;
    let h_heavy_bottom = h_heavy_top + heavy;
    let h_double_top = h_light_top.saturating_sub(light);
    let h_double_bottom = h_light_bottom + light;
    let v_light_left = w.saturating_sub(light) / 2;
    let v_light_right = v_light_left + light;
    let v_heavy_left = w.saturating_sub(heavy) / 2;
    let v_heavy_right = v_heavy_left + heavy;
    let v_double_left = v_light_left.saturating_sub(light);
    let v_double_right = v_light_right + light;

    let horizontal_heavy = lines.left == H || lines.right == H;
    let horizontal_double = lines.left == D || lines.right == D;
    let vertical_heavy = lines.up == H || lines.down == H;
    let vertical_double = lines.up == D || lines.down == D;

    let up_bottom = if horizontal_heavy {
        h_heavy_bottom
    } else if lines.left != lines.right || lines.down == lines.up {
        if horizontal_double {
            h_double_bottom
        } else {
            h_light_bottom
        }
    } else if lines.left == N && lines.right == N {
        h_light_bottom
    } else {
        h_light_top
    };
    let down_top = if horizontal_heavy {
        h_heavy_top
    } else if lines.left != lines.right || lines.up == lines.down {
        if horizontal_double {
            h_double_top
        } else {
            h_light_top
        }
    } else if lines.left == N && lines.right == N {
        h_light_top
    } else {
        h_light_bottom
    };
    let left_right = if vertical_heavy {
        v_heavy_right
    } else if lines.up != lines.down || lines.left == lines.right {
        if vertical_double {
            v_double_right
        } else {
            v_light_right
        }
    } else if lines.up == N && lines.down == N {
        v_light_right
    } else {
        v_light_left
    };
    let right_left = if vertical_heavy {
        v_heavy_left
    } else if lines.up != lines.down || lines.right == lines.left {
        if vertical_double {
            v_double_left
        } else {
            v_light_left
        }
    } else if lines.up == N && lines.down == N {
        v_light_left
    } else {
        v_light_right
    };

    match lines.up {
        N => {}
        L => canvas.rect(v_light_left, 0, v_light_right, up_bottom, 255),
        H => canvas.rect(v_heavy_left, 0, v_heavy_right, up_bottom, 255),
        D => {
            let left_bottom = if lines.left == D {
                h_light_top
            } else {
                up_bottom
            };
            let right_bottom = if lines.right == D {
                h_light_top
            } else {
                up_bottom
            };
            canvas.rect(v_double_left, 0, v_light_left, left_bottom, 255);
            canvas.rect(v_light_right, 0, v_double_right, right_bottom, 255);
        }
    }
    match lines.right {
        N => {}
        L => canvas.rect(right_left, h_light_top, w, h_light_bottom, 255),
        H => canvas.rect(right_left, h_heavy_top, w, h_heavy_bottom, 255),
        D => {
            let top_left = if lines.up == D {
                v_light_right
            } else {
                right_left
            };
            let bottom_left = if lines.down == D {
                v_light_right
            } else {
                right_left
            };
            canvas.rect(top_left, h_double_top, w, h_light_top, 255);
            canvas.rect(bottom_left, h_light_bottom, w, h_double_bottom, 255);
        }
    }
    match lines.down {
        N => {}
        L => canvas.rect(v_light_left, down_top, v_light_right, h, 255),
        H => canvas.rect(v_heavy_left, down_top, v_heavy_right, h, 255),
        D => {
            let left_top = if lines.left == D {
                h_light_bottom
            } else {
                down_top
            };
            let right_top = if lines.right == D {
                h_light_bottom
            } else {
                down_top
            };
            canvas.rect(v_double_left, left_top, v_light_left, h, 255);
            canvas.rect(v_light_right, right_top, v_double_right, h, 255);
        }
    }
    match lines.left {
        N => {}
        L => canvas.rect(0, h_light_top, left_right, h_light_bottom, 255),
        H => canvas.rect(0, h_heavy_top, left_right, h_heavy_bottom, 255),
        D => {
            let top_right = if lines.up == D {
                v_light_left
            } else {
                left_right
            };
            let bottom_right = if lines.down == D {
                v_light_left
            } else {
                left_right
            };
            canvas.rect(0, h_double_top, top_right, h_light_top, 255);
            canvas.rect(0, h_light_bottom, bottom_right, h_double_bottom, 255);
        }
    }
}

fn dashes(canvas: &mut Canvas, code: u32, light: u32, heavy: u32) {
    let (count, thick, vertical) = match code {
        0x2504 => (3, light, false),
        0x2505 => (3, heavy, false),
        0x2506 => (3, light, true),
        0x2507 => (3, heavy, true),
        0x2508 => (4, light, false),
        0x2509 => (4, heavy, false),
        0x250A => (4, light, true),
        0x250B => (4, heavy, true),
        0x254C => (2, light, false),
        0x254D => (2, heavy, false),
        0x254E => (2, light, true),
        _ => (2, heavy, true),
    };
    let (w, h) = (canvas.width, canvas.height);
    let length = if vertical { h } else { w };
    // Each dash leaves a gap after it, half on each side, so dashes in
    // neighbouring cells are spaced evenly.
    let gap = (length / (count * 4)).max(1);
    for i in 0..count {
        let start = i * length / count + gap;
        let end = (i + 1) * length / count - gap;
        if vertical {
            let x = w.saturating_sub(thick) / 2;
            canvas.rect(x, start, x + thick, end.max(start + 1), 255);
        } else {
            let y = h.saturating_sub(thick) / 2;
            canvas.rect(start, y, end.max(start + 1), y + thick, 255);
        }
    }
}

fn arc(canvas: &mut Canvas, code: u32, light: u32) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let t = light as f32;
    // The centre lines the straight box characters use.
    let cx = (canvas.width.saturating_sub(light) / 2) as f32 + t / 2.0;
    let cy = (canvas.height.saturating_sub(light) / 2) as f32 + t / 2.0;
    let r = cx.min(cy).min(w - cx).min(h - cy).max(t);
    // Which corner of the cell the arc's straight ends reach.
    let (right, down) = match code {
        0x256D => (true, true),
        0x256E => (false, true),
        0x256F => (false, false),
        _ => (true, false),
    };
    // Circle centre, offset from the cell centre towards the reached edges.
    let ox = if right { cx + r } else { cx - r };
    let oy = if down { cy + r } else { cy - r };
    let half = t / 2.0;
    canvas.shape(|x, y| {
        let in_quadrant =
            (if right { x <= ox } else { x >= ox }) && (if down { y <= oy } else { y >= oy });
        if in_quadrant {
            let d = ((x - ox).powi(2) + (y - oy).powi(2)).sqrt();
            return (d - r).abs() <= half;
        }
        // Straight runs from the arc's ends to the cell edges.
        let horizontal = (y - cy).abs() <= half && if right { x >= ox } else { x <= ox };
        let vertical = (x - cx).abs() <= half && if down { y >= oy } else { y <= oy };
        horizontal || vertical
    });
}

fn diagonals(canvas: &mut Canvas, code: u32, light: u32) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let half = light as f32 / 2.0 * (1.0 + (h / w).powi(2)).sqrt().min(2.0);
    let rising = |x: f32, y: f32| (y - (h - x * h / w)).abs() <= half * w.max(h) / w;
    let falling = |x: f32, y: f32| (y - x * h / w).abs() <= half * w.max(h) / w;
    match code {
        0x2571 => canvas.shape(rising),
        0x2572 => canvas.shape(falling),
        _ => canvas.shape(|x, y| rising(x, y) || falling(x, y)),
    }
}

fn block(canvas: &mut Canvas, code: u32) {
    let (w, h) = (canvas.width, canvas.height);
    let eighth_h = |n: u32| (h * n + 4) / 8;
    let eighth_w = |n: u32| (w * n + 4) / 8;
    match code {
        // The upper half ends where the lower half (an eighth count from the
        // bottom) begins, so the two are exact complements at any height.
        0x2580 => canvas.rect(0, 0, w, h - eighth_h(4), 255),
        0x2581..=0x2588 => {
            let n = code - 0x2580;
            canvas.rect(0, h - eighth_h(n), w, h, 255);
        }
        0x2589..=0x258F => {
            let n = 8 - (code - 0x2588);
            canvas.rect(0, 0, eighth_w(n), h, 255);
        }
        0x2590 => canvas.rect(eighth_w(4), 0, w, h, 255),
        0x2591 => canvas.rect(0, 0, w, h, 64),
        0x2592 => canvas.rect(0, 0, w, h, 128),
        0x2593 => canvas.rect(0, 0, w, h, 192),
        0x2594 => canvas.rect(0, 0, w, eighth_h(1), 255),
        0x2595 => canvas.rect(w - eighth_w(1), 0, w, h, 255),
        _ => {
            // Quadrants: upper-left, upper-right, lower-left, lower-right.
            let quads: [bool; 4] = match code {
                0x2596 => [false, false, true, false],
                0x2597 => [false, false, false, true],
                0x2598 => [true, false, false, false],
                0x2599 => [true, false, true, true],
                0x259A => [true, false, false, true],
                0x259B => [true, true, true, false],
                0x259C => [true, true, false, true],
                0x259D => [false, true, false, false],
                0x259E => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let (mx, my) = (eighth_w(4), h - eighth_h(4));
            let rects = [
                (0, 0, mx, my),
                (mx, 0, w, my),
                (0, my, mx, h),
                (mx, my, w, h),
            ];
            for (on, (x0, y0, x1, y1)) in quads.into_iter().zip(rects) {
                if on {
                    canvas.rect(x0, y0, x1, y1, 255);
                }
            }
        }
    }
}

fn braille(canvas: &mut Canvas, code: u32) {
    let bits = code - 0x2800;
    let (w, h) = (canvas.width, canvas.height);
    // Dots 1-3 and 7 run down the left column, 4-6 and 8 down the right.
    let dots = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let dot = (w / 4).max(1).min(h / 8).max(1);
    let col_x = |c: u32| (w * (1 + 2 * c)) / 4 - dot / 2;
    let row_y = |r: u32| (h * (1 + 2 * r)) / 8 - dot / 2;
    for (bit, (c, r)) in dots.into_iter().enumerate() {
        if bits & (1 << bit) != 0 {
            let (x, y) = (col_x(c), row_y(r));
            canvas.rect(x, y, x + dot, y + dot, 255);
        }
    }
}

fn powerline(canvas: &mut Canvas, code: u32, light: u32) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    let half = light as f32 / 2.0 + 0.25;
    match code {
        // Solid triangles pointing right and left.
        0xE0B0 => canvas.shape(|x, y| x <= w * (1.0 - (2.0 * y / h - 1.0).abs())),
        0xE0B2 => canvas.shape(|x, y| w - x <= w * (1.0 - (2.0 * y / h - 1.0).abs())),
        // Outlined arrows: the two edges of the triangle.
        0xE0B1 => canvas.shape(|x, y| {
            let edge = w * (1.0 - (2.0 * y / h - 1.0).abs());
            (x - edge).abs() <= half * 1.5
        }),
        _ => canvas.shape(|x, y| {
            let edge = w - w * (1.0 - (2.0 * y / h - 1.0).abs());
            (x - edge).abs() <= half * 1.5
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Coverage along the middle half of each edge: up, right, down, left.
    // A double line leaves the very centre empty, so a single pixel would not do.
    fn edges(mask: &[u8], w: u32, h: u32) -> [bool; 4] {
        let at = |x: u32, y: u32| mask[(y * w + x) as usize] > 0;
        let across = |len: u32, f: &dyn Fn(u32) -> bool| (len / 4..len - len / 4).any(f);
        [
            across(w, &|x| at(x, 0)),
            across(h, &|y| at(w - 1, y)),
            across(w, &|x| at(x, h - 1)),
            across(h, &|y| at(0, y)),
        ]
    }

    #[test]
    fn every_line_reaches_exactly_the_edges_it_names() {
        // Odd and even cell sizes, light strokes of 1 and 2 pixels.
        for (w, h, stroke) in [(9, 19, 1), (10, 20, 1), (17, 36, 2)] {
            for code in 0x2500..=0x257F {
                let Some(arms) = line_arms(code) else {
                    continue;
                };
                let ch = char::from_u32(code).unwrap();
                let mask = draw(ch, w, h, stroke).unwrap();
                let named = [arms.up, arms.right, arms.down, arms.left].map(|a| a != Line::None);
                assert_eq!(edges(&mask, w, h), named, "{ch} ({code:X}) in {w}x{h}");
            }
        }
    }
}
