//! The terminal drawn with the renderer's own quads and text, for renderers
//! that cannot draw the GPU pipeline (the software fallback).
//!
//! It draws one quad per background run and one text per glyph cell, which
//! is far slower than the pipeline, but a terminal must work wherever the
//! window does.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use iced::advanced::text::{self, Renderer as _};
use iced::advanced::{Renderer as _, renderer};
use iced::{Color, Font, Pixels, Point, Rectangle, Size};

use crate::TerminalStyle;
use crate::font::CellMetrics;
use crate::render::Frame;
use crate::screen::{CellFlags, Rgba, TerminalScreen};

fn color(rgba: Rgba) -> Color {
    let [r, g, b, a] = rgba.0;
    Color::from_rgba8(r, g, b, f32::from(a) / 255.0)
}

/// A family name that lives as long as the process, as iced's fonts need;
/// each distinct name is stored once.
fn interned(name: &str) -> &'static str {
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(name) = names.get(name) {
        return name;
    }
    let leaked: &'static str = Box::leak(name.to_owned().into_boxed_str());
    let _ = names.insert(name.to_owned(), leaked);
    leaked
}

/// Where the grid sits, in device pixels, and how its cursor shows.
pub(crate) struct ViewState {
    pub origin: (f32, f32),
    pub scale: f32,
    pub focused: bool,
    pub cursor_on: bool,
    /// The rows drawn above the view's bounds, and their fade's strength.
    pub fade_above: (u16, f32),
}

/// Draws `screen` into `bounds` with quads and text.
pub(crate) fn draw(
    renderer: &mut iced::Renderer,
    bounds: Rectangle,
    screen: &TerminalScreen,
    style: &TerminalStyle,
    metrics: &CellMetrics,
    view: &ViewState,
) {
    let ViewState {
        origin,
        scale,
        focused,
        cursor_on,
        fade_above,
    } = *view;
    let (cw, ch) = (metrics.width as f32, metrics.height as f32);
    // A device-pixel rectangle relative to cell (x, y), in logical pixels.
    let rect = |x: u16, y: u16, left: f32, top: f32, width: f32, height: f32| Rectangle {
        x: (origin.0 + f32::from(x) * cw + left) / scale,
        y: (origin.1 + f32::from(y) * ch + top) / scale,
        width: width / scale,
        height: height / scale,
    };
    let fill = |renderer: &mut iced::Renderer, bounds: Rectangle, rgba: Rgba| {
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                ..renderer::Quad::default()
            },
            color(rgba),
        );
    };

    let family = style.font.family.as_deref().map_or_else(
        || {
            enzui_theme::text_defaults::font(
                enzui_theme::FontRole::Monospace,
                enzui_theme::WeightRung::Regular,
            )
            .family
        },
        |name| iced::font::Family::Name(interned(name)),
    );

    // The frame consumes the screen's damage and its pending wake, as the
    // pipeline's prepare does.
    let mut state = screen.lock_for_frame();
    let _ = state.take_damage();
    let rows = state.rows();
    let frame = Frame {
        style,
        metrics,
        cursor: state.cursor(),
        selection: state.selection(),
        focused,
        cursor_on,
    };

    renderer.with_layer(bounds, |renderer| {
        fill(renderer, bounds, style.background);
        let right = bounds.x + bounds.width;
        let bottom = bounds.y + bounds.height;
        for y in 0..rows {
            let cells = state.row(y);
            let looks: Vec<_> = cells
                .iter()
                .enumerate()
                .map(|(x, cell)| frame.look(cell, x as u16, y))
                .collect();

            // Backgrounds, one quad per run of equal colour.
            let mut x = 0;
            while x < looks.len() {
                let background = looks[x].background;
                let start = x;
                while x < looks.len() && looks[x].background == background {
                    x += 1;
                }
                if background.is_none() {
                    continue;
                }
                let run = rect(start as u16, y, 0.0, 0.0, (x - start) as f32 * cw, ch);
                fill(renderer, run, background);
            }
            // Past the grid's edge, the edge cells' own colours: not the
            // cursor or a selection drawn on them.
            if style.extend_background {
                let edge = |x: usize| cells[x].bg;
                if let Some(last) = cells.len().checked_sub(1) {
                    let cell = rect(last as u16, y, 0.0, 0.0, cw, ch);
                    let x = cell.x + cell.width;
                    let height = if y + 1 == rows {
                        bottom - cell.y
                    } else {
                        cell.height
                    };
                    let strip = Rectangle::new(Point::new(x, cell.y), Size::new(right - x, height));
                    if !edge(last).is_none() && strip.width > 0.0 {
                        fill(renderer, strip, edge(last));
                    }
                }
                if y + 1 == rows {
                    let mut x = 0;
                    while x < cells.len() {
                        let background = edge(x);
                        let start = x;
                        while x < cells.len() && edge(x) == background {
                            x += 1;
                        }
                        let run = rect(start as u16, y, 0.0, 0.0, (x - start) as f32 * cw, ch);
                        let top = run.y + run.height;
                        let strip = Rectangle::new(
                            Point::new(run.x, top),
                            Size::new(run.width, bottom - top),
                        );
                        if !background.is_none() && strip.height > 0.0 {
                            fill(renderer, strip, background);
                        }
                    }
                }
            }

            for (x, (cell, look)) in cells.iter().zip(&looks).enumerate() {
                let x = x as u16;
                if cell.has_glyph() {
                    let mut content = String::from(cell.ch);
                    if let Some(combining) = &cell.combining {
                        content.push_str(combining);
                    }
                    let span = look.span as f32;
                    renderer.fill_text(
                        text::Text {
                            content,
                            bounds: Size::new(cw * span / scale, ch / scale),
                            size: Pixels(metrics.font_px / scale),
                            line_height: text::LineHeight::Absolute(Pixels(ch / scale)),
                            font: Font {
                                family,
                                weight: if cell.flags.contains(CellFlags::BOLD) {
                                    iced::font::Weight::Bold
                                } else {
                                    iced::font::Weight::Normal
                                },
                                style: if cell.flags.contains(CellFlags::ITALIC) {
                                    iced::font::Style::Italic
                                } else {
                                    iced::font::Style::Normal
                                },
                                ..Font::MONOSPACE
                            },
                            align_x: text::Alignment::Left,
                            align_y: iced::alignment::Vertical::Top,
                            shaping: text::Shaping::Advanced,
                            wrapping: text::Wrapping::None,
                            ellipsis: text::Ellipsis::None,
                            hint_factor: None,
                        },
                        Point::new(
                            (origin.0 + f32::from(x) * cw) / scale,
                            (origin.1 + f32::from(y) * ch) / scale,
                        ),
                        color(look.foreground),
                        bounds,
                    );
                }
                let curly = frame.decorations(cell, look, |left, top, width, height, rgba| {
                    let quad = rect(x, y, left as f32, top as f32, width as f32, height as f32);
                    fill(renderer, quad, rgba);
                });
                // Drawn straight: the curl needs the pipeline's atlas.
                if let Some(line) = curly {
                    let width = cw * look.span as f32;
                    let quad = rect(
                        x,
                        y,
                        0.0,
                        metrics.underline_top as f32,
                        width,
                        metrics.stroke as f32,
                    );
                    fill(renderer, quad, line);
                }
            }
        }
    });
    // The fade the pipeline's shader draws, as one gradient over the rows
    // above, in a layer of its own so it covers their text too.
    let (rows, strength) = fade_above;
    if rows > 0 && strength > 0.0 {
        let bottom = (origin.1 + f32::from(rows) * metrics.height as f32) / scale;
        let area = Rectangle {
            height: (bottom - bounds.y).max(0.0),
            ..bounds
        };
        let background = color(style.background);
        let at = |a: f32| Color {
            a: a * strength,
            ..background
        };
        renderer.with_layer(area, |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: area,
                    ..renderer::Quad::default()
                },
                iced::Background::Gradient(iced::Gradient::Linear(
                    iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                        .add_stop(0.0, at(1.0))
                        .add_stop(0.7, at(0.84))
                        .add_stop(1.0, at(0.0)),
                )),
            );
        });
    }
}
