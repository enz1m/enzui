//! The terminal view drawn by the real wgpu renderer, read back as pixels.
//!
//! Skips, saying so, where no GPU adapter exists: the software renderer
//! does not draw custom pipelines.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_terminal::{
    Cell, CellMetrics, Cursor, CursorShape, Rgba, TerminalScreen, TerminalStyle, enz_terminal,
};
use iced::advanced::renderer::Headless as _;
use iced_test::runtime::user_interface::{Cache, UserInterface};

const RED: Rgba = Rgba::rgb(255, 0, 0);
const WHITE: Rgba = Rgba::rgb(255, 255, 255);
const GREEN: Rgba = Rgba::rgb(0, 255, 0);

/// The view's pixels once every glyph has drawn: new glyphs may spread over
/// several frames when a frame runs out of rasterising time.
fn render(
    screen: &TerminalScreen,
    style: &TerminalStyle,
    size: iced::Size<u32>,
) -> Option<Vec<u8>> {
    render_on("wgpu", screen, style, size)
}

/// [`render`] on the named backend: `wgpu` or the `tiny-skia` software one.
fn render_on(
    backend: &str,
    screen: &TerminalScreen,
    style: &TerminalStyle,
    size: iced::Size<u32>,
) -> Option<Vec<u8>> {
    render_view_on(backend, screen, size, || {
        enz_terminal(screen).style(style.clone()).into()
    })
}

/// [`render_on`] for any view of `screen`.
fn render_view_on<'a>(
    backend: &str,
    screen: &TerminalScreen,
    size: iced::Size<u32>,
    view: impl FnOnce() -> iced::Element<'a, ()>,
) -> Option<Vec<u8>> {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some(backend),
        ),
    );
    let Some(renderer) = renderer.as_mut() else {
        eprintln!("SKIPPED: no {backend} renderer, the terminal cannot draw");
        return None;
    };
    let element = view();
    let mut ui = UserInterface::build(
        element,
        iced::Size::new(size.width as f32, size.height as f32),
        Cache::default(),
        renderer,
    );
    let revisions = || {
        (0..screen.read(enzui_terminal::ScreenState::rows))
            .map(|y| screen.read(|s| s.row_revision(y)))
            .collect::<Vec<_>>()
    };
    for _ in 0..50 {
        let before = revisions();
        ui.draw(
            renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::WHITE,
            },
            iced::mouse::Cursor::Unavailable,
        );
        let pixels = renderer.screenshot(size, 1.0, iced::Color::BLACK);
        if revisions() == before {
            return Some(pixels);
        }
    }
    panic!("the terminal never finished drawing");
}

#[test]
fn cells_glyphs_lines_and_cursor_land_on_their_pixels() {
    let style = TerminalStyle {
        cursor: GREEN,
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let screen = TerminalScreen::new(20, 4, WHITE);
    screen.update(|w| {
        let mut a = Cell::new('A', WHITE);
        a.bg = RED;
        w.set_cell(0, 0, a);
        for (x, ch) in "─┼─".chars().enumerate() {
            w.set_cell(x as u16, 1, Cell::new(ch, WHITE));
        }
        w.set_cursor(Cursor {
            col: 10,
            row: 0,
            shape: CursorShape::Block,
            visible: true,
        });
    });

    let size = iced::Size::new(400, 160);
    let Some(pixels) = render(&screen, &style, size) else {
        return;
    };
    let metrics = CellMetrics::measure(&style.font.resolved(), 1.0);
    let (cw, ch) = (metrics.width, metrics.height);
    let origin = style.padding.round() as u32;
    let px = |x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        Rgba([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
    };

    // The cell's own background fills its corner; the glyph inks its middle.
    assert_eq!(
        px(origin, origin),
        RED,
        "cell background at its top-left pixel"
    );
    let inked = (origin..origin + cw)
        .flat_map(|x| (origin..origin + ch).map(move |y| (x, y)))
        .filter(|&(x, y)| px(x, y).0[1] > 128)
        .count();
    assert!(
        inked > 5,
        "the A glyph draws over its background ({inked} light pixels)"
    );

    // Three line cells make one unbroken band of light pixels.
    let row1 = origin + ch;
    let band = (row1..row1 + ch).find(|&y| (origin..origin + 3 * cw).all(|x| px(x, y).0[0] > 200));
    assert!(band.is_some(), "─┼─ joins across the three cells");

    // The unfocused view outlines the cursor cell in the cursor colour.
    let cursor_x = origin + 10 * cw;
    assert_eq!(
        px(cursor_x + cw / 2, origin),
        GREEN,
        "hollow cursor's top edge"
    );
    assert_ne!(
        px(cursor_x + cw / 2, origin + ch / 2),
        GREEN,
        "and not its middle"
    );

    // Beside the grid the terminal background shows.
    assert_eq!(
        px(origin + 20 * cw + 2, origin + 2),
        style.background,
        "default background"
    );
}

/// Past the grid's last row and column the edge cells' own backgrounds reach
/// the view's edge, but a selection (or the focused block cursor, drawn the
/// same way) on an edge cell stays inside its cell.
///
/// Sabotage: read the drawn colour past the edge in `bg_fs` (wgpu), or
/// extend the drawn runs in `software::draw` (tiny-skia). The strip under the
/// selected cell turns the selection colour.
/// Rows above the bounds draw there, continuing the grid upwards, and the
/// bounds start with the row after them.
///
/// Sabotage: draw the primitive (and the software grid) over
/// `layout.bounds()` in `draw`. The red row lands inside the bounds and the
/// green one below them.
#[test]
fn rows_above_draw_above_the_bounds() {
    for backend in ["wgpu", "tiny-skia"] {
        rows_above_draw_above(backend);
    }
}

fn rows_above_draw_above(backend: &str) {
    let style = TerminalStyle {
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let screen = TerminalScreen::new(4, 4, WHITE);
    screen.update(|w| {
        let mut cell = Cell::blank(WHITE);
        cell.bg = RED;
        w.set_cell(0, 0, cell.clone());
        cell.bg = GREEN;
        w.set_cell(0, 2, cell);
        w.set_cursor(Cursor {
            visible: false,
            ..Cursor::default()
        });
    });
    let metrics = CellMetrics::measure(&style.font.resolved(), 1.0);
    let (cw, ch) = (metrics.width, metrics.height);
    let top = 3 * ch;
    let size = iced::Size::new(200, top + 4 * ch);
    let Some(pixels) = render_view_on(backend, &screen, size, || {
        iced::widget::column![
            iced::widget::space().height(top as f32),
            enz_terminal(&screen).style(style.clone()).rows_above(2),
        ]
        .into()
    }) else {
        return;
    };
    let px = |x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        Rgba([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
    };
    let origin = style.padding.round() as u32;
    let x = origin + cw / 2;
    assert_eq!(
        px(x, top + origin - 2 * ch + ch / 2),
        RED,
        "{backend}: row 0, two rows above the bounds"
    );
    assert_eq!(
        px(x, top + origin + ch / 2),
        GREEN,
        "{backend}: row 2, the bounds' first row"
    );
}

/// Faded rows above ease from the background at their top to their own
/// colours at the bounds, and the rows in the bounds are untouched.
///
/// Sabotage: return 0.0 from `fade_at` in the shader (and skip the software
/// gradient). The top row stays red.
#[test]
fn faded_rows_above_ease_into_the_background() {
    for backend in ["wgpu", "tiny-skia"] {
        faded_rows_above(backend);
    }
}

fn faded_rows_above(backend: &str) {
    let style = TerminalStyle {
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let screen = TerminalScreen::new(4, 4, WHITE);
    screen.update(|w| {
        let mut cell = Cell::blank(WHITE);
        cell.bg = RED;
        w.set_cell(0, 0, cell.clone());
        w.set_cell(0, 1, cell.clone());
        cell.bg = GREEN;
        w.set_cell(0, 2, cell);
        w.set_cursor(Cursor {
            visible: false,
            ..Cursor::default()
        });
    });
    let metrics = CellMetrics::measure(&style.font.resolved(), 1.0);
    let (cw, ch) = (metrics.width, metrics.height);
    let top = 3 * ch;
    let size = iced::Size::new(200, top + 4 * ch);
    let Some(pixels) = render_view_on(backend, &screen, size, || {
        iced::widget::column![
            iced::widget::space().height(top as f32),
            enz_terminal(&screen)
                .style(style.clone())
                .rows_above(2)
                .fade_rows_above(1.0),
        ]
        .into()
    }) else {
        return;
    };
    let px = |x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        Rgba([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
    };
    let origin = style.padding.round() as u32;
    let x = origin + cw / 2;
    let red = |rgba: Rgba| i32::from(rgba.0[0]) - i32::from(style.background.0[0]);
    let full = red(RED);
    let high = px(x, top + origin - 2 * ch + ch / 2);
    let low = px(x, top + origin - 1);
    assert!(
        red(high) < full / 5,
        "{backend}: the top row is nearly the background: {high:?}"
    );
    assert!(
        red(low) > full * 3 / 4,
        "{backend}: just above the bounds, nearly its own red: {low:?}"
    );
    assert_eq!(
        px(x, top + origin + ch / 2),
        GREEN,
        "{backend}: the first row in the bounds is untouched"
    );
}

#[test]
fn only_a_cells_own_background_reaches_past_the_grid() {
    for backend in ["wgpu", "tiny-skia"] {
        own_background_reaches_past_the_grid(backend);
    }
}

fn own_background_reaches_past_the_grid(backend: &str) {
    let style = TerminalStyle {
        selection_background: GREEN,
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let screen = TerminalScreen::new(4, 2, WHITE);
    screen.update(|w| {
        let mut red = Cell::blank(WHITE);
        red.bg = RED;
        w.set_cell(0, 1, red.clone());
        w.set_cell(1, 1, red);
        w.set_selection(Some(enzui_terminal::Selection {
            start: (1, 1),
            end: (3, 1),
            block: false,
        }));
    });

    let size = iced::Size::new(200, 120);
    let Some(pixels) = render_on(backend, &screen, &style, size) else {
        return;
    };
    let metrics = CellMetrics::measure(&style.font.resolved(), 1.0);
    let (cw, ch) = (metrics.width, metrics.height);
    let origin = style.padding.round() as u32;
    let px = |x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        Rgba([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
    };
    let row = origin + ch + ch / 2;
    let below = size.height - 2;
    assert!(below > origin + 2 * ch + 2, "a strip below the grid");

    assert_eq!(
        px(origin + cw / 2, row),
        RED,
        "{backend}: an unselected red cell"
    );
    assert_eq!(
        px(origin + cw + cw / 2, row),
        GREEN,
        "{backend}: a selected red cell"
    );
    assert_eq!(
        px(origin + cw / 2, below),
        RED,
        "{backend}: red reaches the bottom"
    );
    assert_eq!(
        px(origin + cw + cw / 2, below),
        RED,
        "{backend}: under the selection, the cell's own red"
    );
    assert_eq!(
        px(origin + 3 * cw + cw / 2, below),
        style.background,
        "{backend}: under a selected blank cell, the background"
    );
    assert_eq!(
        px(size.width - 2, row),
        style.background,
        "{backend}: right of a selected blank last cell, the background"
    );
}

#[test]
fn a_page_of_new_glyphs_spreads_over_frames_and_all_of_it_draws() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("wgpu"),
        ),
    );
    let Some(renderer) = renderer.as_mut() else {
        eprintln!("SKIPPED: no wgpu adapter, the terminal pipeline cannot draw");
        return;
    };
    let style = TerminalStyle {
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    // 1200 distinct ideographs, far more than one frame's rasterising budget.
    let (cols, rows) = (60u16, 20u16);
    let screen = TerminalScreen::new(cols, rows, WHITE);
    screen.update(|w| {
        for y in 0..rows {
            let start = 0x4e00 + u32::from(y) * u32::from(cols);
            w.set_row(
                y,
                (0..u32::from(cols) / 2).flat_map(|x| {
                    let mut wide = Cell::new(char::from_u32(start + x).unwrap(), WHITE);
                    wide.flags |= enzui_terminal::CellFlags::WIDE;
                    let mut spacer = Cell::new(' ', WHITE);
                    spacer.flags |= enzui_terminal::CellFlags::WIDE_SPACER;
                    [wide, spacer]
                }),
            );
        }
    });
    let metrics = CellMetrics::measure(&style.font.resolved(), 1.0);
    let size = iced::Size::new(
        u32::from(cols) * metrics.width + 20,
        u32::from(rows) * metrics.height + 20,
    );
    let revisions = || {
        (0..rows)
            .map(|y| screen.read(|s| s.row_revision(y)))
            .collect::<Vec<_>>()
    };
    let mut frames = 0;
    let pixels = loop {
        let before = revisions();
        let element: iced::Element<'_, ()> = enz_terminal(&screen).style(style.clone()).into();
        let mut ui = UserInterface::build(
            element,
            iced::Size::new(size.width as f32, size.height as f32),
            Cache::default(),
            renderer,
        );
        ui.draw(
            renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::WHITE,
            },
            iced::mouse::Cursor::Unavailable,
        );
        drop(ui);
        let pixels = renderer.screenshot(size, 1.0, iced::Color::BLACK);
        frames += 1;
        if revisions() == before {
            break pixels;
        }
        assert!(frames < 200, "the page never finished drawing");
    };
    // The last frame only confirms that nothing was left over.
    assert!(frames > 2, "one frame rasterised the whole page by itself");

    let origin = style.padding.round() as u32;
    let blank: Vec<(u16, u16)> = (0..rows)
        .flat_map(|y| (0..cols / 2).map(move |x| (x * 2, y)))
        .filter(|&(x, y)| {
            let (x0, y0) = (
                origin + u32::from(x) * metrics.width,
                origin + u32::from(y) * metrics.height,
            );
            !(x0..x0 + 2 * metrics.width)
                .flat_map(|px| (y0..y0 + metrics.height).map(move |py| (px, py)))
                .any(|(px, py)| pixels[((py * size.width + px) * 4) as usize] > 128)
        })
        .collect();
    assert!(
        blank.is_empty(),
        "cells left without their glyph: {blank:?}"
    );
}

#[test]
fn the_software_renderer_draws_backgrounds_glyphs_and_the_cursor() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("tiny-skia"),
        ),
    )
    .expect("the software renderer is always available");
    let style = TerminalStyle {
        cursor: GREEN,
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let screen = TerminalScreen::new(20, 2, WHITE);
    screen.update(|w| {
        w.set_row(
            0,
            "MM".chars().map(|ch| {
                let mut cell = Cell::new(ch, WHITE);
                cell.bg = RED;
                cell
            }),
        );
        w.set_cursor(Cursor {
            col: 5,
            row: 1,
            shape: CursorShape::Block,
            visible: true,
        });
    });
    let size = iced::Size::new(400, 120);
    let element: iced::Element<'_, ()> = enz_terminal(&screen)
        .style(style.clone())
        .focused(true)
        .into();
    let mut ui = UserInterface::build(
        element,
        iced::Size::new(size.width as f32, size.height as f32),
        Cache::default(),
        &mut renderer,
    );
    ui.draw(
        &mut renderer,
        &iced::Theme::Dark,
        &iced::advanced::renderer::Style {
            text_color: iced::Color::WHITE,
        },
        iced::mouse::Cursor::Unavailable,
    );
    let pixels = renderer.screenshot(size, 1.0, iced::Color::BLACK);
    let metrics = CellMetrics::measure(&style.font.resolved(), 1.0);
    let (cw, ch) = (metrics.width, metrics.height);
    let origin = style.padding.round() as u32;
    let px = |x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        Rgba([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
    };

    // The two red cells are one background run; the next cell is not red.
    assert_eq!(px(origin, origin), RED, "first cell's background");
    assert_eq!(
        px(origin + 2 * cw - 1, origin + ch - 1),
        RED,
        "second cell's"
    );
    assert_ne!(px(origin + 2 * cw + 1, origin + 1), RED, "third cell's");
    let inked = (origin..origin + cw)
        .flat_map(|x| (origin..origin + ch).map(move |y| (x, y)))
        .filter(|&(x, y)| px(x, y).0[1] > 128)
        .count();
    assert!(
        inked > 5,
        "the M glyph draws over red ({inked} light pixels)"
    );
    let cursor = (origin + 5 * cw + cw / 2, origin + ch + ch / 2);
    assert_eq!(px(cursor.0, cursor.1), GREEN, "the focused block cursor");
}

#[test]
fn a_reused_frame_redraws_the_terminal_and_keeps_the_rest_of_the_window() {
    use iced::widget::{column, container, row, svg, text};
    const BLUE: Rgba = Rgba::rgb(0, 0, 255);

    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("wgpu"),
        ),
    );
    let Some(renderer) = renderer.as_mut() else {
        eprintln!("SKIPPED: no wgpu adapter, the terminal pipeline cannot draw");
        return;
    };
    let style = TerminalStyle {
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let screen = TerminalScreen::new(10, 2, WHITE);
    let size = iced::Size::new(300, 120);
    let square = svg::Handle::from_memory(
        b"<svg xmlns='http://www.w3.org/2000/svg' width='20' height='20'>\
          <rect width='20' height='20' fill='#0000ff'/></svg>"
            .as_slice(),
    );
    // Chrome above the terminal: a filled bar with text and an image.
    let element: iced::Element<'_, ()> = column![
        container(row![
            text("chrome").size(20).color(iced::Color::WHITE),
            svg(square).width(20).height(20)
        ])
        .height(30)
        .width(iced::Length::Fill)
        .style(|_| container::Style::default().background(iced::Color::from_rgb8(0, 255, 0))),
        enz_terminal(&screen).style(style.clone()),
    ]
    .into();
    let mut ui = UserInterface::build(
        element,
        iced::Size::new(size.width as f32, size.height as f32),
        Cache::default(),
        renderer,
    );
    ui.draw(
        renderer,
        &iced::Theme::Dark,
        &iced::advanced::renderer::Style {
            text_color: iced::Color::WHITE,
        },
        iced::mouse::Cursor::Unavailable,
    );
    let first = renderer.screenshot(size, 1.0, iced::Color::BLACK);
    let full_frames = match &*renderer {
        iced::Renderer::Primary(wgpu) => wgpu.quad_statistics().frame,
        iced::Renderer::Secondary(_) => unreachable!("asked for wgpu"),
    };

    // Only the terminal's content changes; no widget draws again.
    screen.update(|w| {
        let mut cell = Cell::new('A', WHITE);
        cell.bg = RED;
        w.set_cell(3, 0, cell);
    });
    assert!(
        iced::advanced::Renderer::reuse_frame(renderer),
        "wgpu presents a frame again"
    );
    let second = renderer.screenshot(size, 1.0, iced::Color::BLACK);
    let px = |pixels: &[u8], x: u32, y: u32| {
        let i = ((y * size.width + x) * 4) as usize;
        Rgba([pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]])
    };
    let cell_width = CellMetrics::measure(&style.font.resolved(), 1.0).width;
    // Cell (3, 0); the unfocused cursor outlines cell (0, 0).
    let origin = (
        style.padding.round() as u32 + 3 * cell_width,
        30 + style.padding.round() as u32,
    );

    assert_ne!(px(&first, origin.0, origin.1), RED, "before the write");
    assert_eq!(
        px(&second, origin.0, origin.1),
        RED,
        "the new cell is drawn"
    );
    let differs = |x0: u32, x1: u32| {
        (x0..x1)
            .flat_map(|x| (0..30).map(move |y| (x, y)))
            .filter(|&(x, y)| px(&first, x, y) != px(&second, x, y))
            .count()
    };
    assert_eq!(differs(0, size.width), 0, "the chrome is pixel-identical");
    let text_ink = (0..80)
        .flat_map(|x| (0..30).map(move |y| (x, y)))
        .filter(|&(x, y)| px(&second, x, y).0[0] > 200)
        .count();
    assert!(
        text_ink > 10,
        "the chrome's text is still there ({text_ink})"
    );
    assert!(
        (0..size.width)
            .flat_map(|x| (0..30).map(move |y| (x, y)))
            .any(|(x, y)| px(&second, x, y) == BLUE),
        "the chrome's image is still there"
    );
    let reused_frames = match &*renderer {
        iced::Renderer::Primary(wgpu) => wgpu.quad_statistics().frame,
        iced::Renderer::Secondary(_) => unreachable!(),
    };
    assert_eq!(
        reused_frames, full_frames,
        "nothing but the terminal was prepared"
    );
}

/// A screen has one view: two views of it in one frame would share GPU state
/// and damage, so debug builds refuse it rather than draw both wrong.
#[cfg(debug_assertions)]
#[test]
fn a_screen_shown_twice_in_one_frame_is_refused() {
    let screen = TerminalScreen::new(10, 2, WHITE);
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("wgpu"),
        ),
    );
    let Some(renderer) = renderer.as_mut() else {
        eprintln!("SKIPPED: no wgpu adapter, the terminal pipeline cannot draw");
        return;
    };
    let element: iced::Element<'_, ()> =
        iced::widget::row![enz_terminal(&screen), enz_terminal(&screen)].into();
    let mut ui = UserInterface::build(
        element,
        iced::Size::new(400.0, 100.0),
        Cache::default(),
        renderer,
    );
    let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ui.draw(
            renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::WHITE,
            },
            iced::mouse::Cursor::Unavailable,
        );
        let _ = renderer.screenshot(iced::Size::new(400, 100), 1.0, iced::Color::BLACK);
    }));
    let payload = drawn.expect_err("two views of one screen are refused");
    let message = payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    assert!(message.contains("one view"), "{message}");
}

/// Frames that rewrite only part of the glyph buffer draw what a fresh view
/// of the same screen draws: a row that keeps its glyph count, then one that
/// loses glyphs and moves every row below it.
#[test]
fn partial_glyph_uploads_draw_what_a_fresh_view_draws() {
    let style = TerminalStyle {
        cursor_blink: None,
        ..TerminalStyle::default()
    };
    let write = |screen: &TerminalScreen, rows: [&str; 3]| {
        screen.update(|w| {
            for (y, text) in rows.iter().enumerate() {
                for x in 0..12u16 {
                    let ch = text.chars().nth(usize::from(x)).unwrap_or(' ');
                    w.set_cell(x, y as u16, Cell::new(ch, WHITE));
                }
            }
        });
    };
    let size = iced::Size::new(300, 120);
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("wgpu"),
        ),
    );
    let Some(renderer) = renderer.as_mut() else {
        eprintln!("SKIPPED: no wgpu adapter, the terminal pipeline cannot draw");
        return;
    };
    let screen = TerminalScreen::new(12, 3, WHITE);
    write(&screen, ["abcdef", "ghijkl", "mnopqr"]);
    let frame = |renderer: &mut iced::Renderer| {
        let element: iced::Element<'_, ()> = enz_terminal(&screen).style(style.clone()).into();
        let mut ui = UserInterface::build(
            element,
            iced::Size::new(size.width as f32, size.height as f32),
            Cache::default(),
            renderer,
        );
        let revisions = || {
            (0..3)
                .map(|y| screen.read(|s| s.row_revision(y)))
                .collect::<Vec<_>>()
        };
        for _ in 0..50 {
            let before = revisions();
            ui.draw(
                renderer,
                &iced::Theme::Dark,
                &iced::advanced::renderer::Style {
                    text_color: iced::Color::WHITE,
                },
                iced::mouse::Cursor::Unavailable,
            );
            let pixels = renderer.screenshot(size, 1.0, iced::Color::BLACK);
            if revisions() == before {
                return pixels;
            }
        }
        panic!("the terminal never finished drawing")
    };
    let _ = frame(renderer);
    // Same count on row 1; row 0 drops four glyphs, moving rows 1 and 2.
    write(&screen, ["abcdef", "stuvwx", "mnopqr"]);
    let _ = frame(renderer);
    write(&screen, ["ab", "stuvwx", "mnopqr"]);
    let incremental = frame(renderer);

    let fresh = TerminalScreen::new(12, 3, WHITE);
    write(&fresh, ["ab", "stuvwx", "mnopqr"]);
    let expected = render(&fresh, &style, size).expect("adapter found above");
    assert!(
        incremental == expected,
        "the incrementally uploaded frame differs from a fresh one"
    );
}
