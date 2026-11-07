//! Focus normalization contract: six additional widgets use the shared focus-visible gate.

use enzui_core::focus::{InputModality, set_modality};
use iced::{Color, Element, Point, Size, Theme};
use std::sync::Mutex;

// -- widget imports --------------------------------------------------------
use enzui_core::zoom_pan::ViewTransform;
use enzui_theme::Palette;
use enzui_widgets::combo_box::enz_combo_box;
use enzui_widgets::image_viewer::{ImageViewerTokens, enz_image_viewer};
use enzui_widgets::key_capture_chip::{KeyCapture, KeyCaptureChipTokens, enz_key_capture_chip};
use enzui_widgets::radio_group::{RadioTokens, enz_radio_group};
use enzui_widgets::tab_pill::{TabPillTokens, enz_tab_list};
use iced::widget::image;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Usize(usize),
    Str(String),
    Xf(ViewTransform),
    Kc(KeyCapture),
}

/// A vivid, unique ring colour (pure magenta) painted nowhere else by these
/// widgets in their default state, so "ring present" is a clean pixel count.
const RING: Color = Color {
    r: 1.0,
    g: 0.0,
    b: 1.0,
    a: 1.0,
};

const W: u32 = 220;
const H: u32 = 120;

/// Serialises the process-global modality across every case.
pub(crate) static MODALITY_LOCK: Mutex<()> = Mutex::new(());

type Rgb = (u8, u8, u8);

fn qc(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}
fn rgb(c: Color) -> Rgb {
    (qc(c.r), qc(c.g), qc(c.b))
}
fn px(buf: &[u8], w: u32, x: u32, y: u32) -> Rgb {
    let i = ((y * w + x) * 4) as usize;
    (buf[i], buf[i + 1], buf[i + 2])
}

/// Count pixels within `tol` of `target` across the whole frame.
fn count_near(buf: &[u8], w: u32, h: u32, target: Rgb, tol: i32) -> u32 {
    let mut n = 0;
    for y in 0..h {
        for x in 0..w {
            let p = px(buf, w, x, y);
            if (i32::from(p.0) - i32::from(target.0)).abs() <= tol
                && (i32::from(p.1) - i32::from(target.1)).abs() <= tol
                && (i32::from(p.2) - i32::from(target.2)).abs() <= tol
            {
                n += 1;
            }
        }
    }
    n
}

/// Cursor far from the widget: focused-but-not-hovered.
const AWAY: Point = Point::new((W - 2) as f32, (H - 2) as f32);
const OVER_RADIO: Point = Point::new(20.0, 50.0);
const OVER_TAB: Point = Point::new(20.0, 12.0);
const OVER_COMBO: Point = Point::new(30.0, 20.0);
const OVER_IMAGE: Point = Point::new(110.0, 60.0);
const OVER_CHIP: Point = Point::new(20.0, 12.0);

/// Build a fresh single-use element of the widget under test.
type Build = Box<dyn Fn() -> Element<'static, Msg>>;

/// Render pipeline shared by every case, generalised over the `Theme` so a
/// widget whose ring colour comes from the resolved palette (tree_view) can be
/// driven by a theme carrying a magenta focus_ring.
fn render(
    theme: &Theme,
    build: &Build,
    pointer_click: Option<Point>,
    keyboard_focus: bool,
    modality: InputModality,
    cursor: Point,
) -> Vec<u8> {
    use iced::advanced::renderer::Headless;
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(theme);

    let mut cache = Cache::default();
    let mk = |cache: Cache, renderer: &mut iced_test::renderer::Renderer| {
        UserInterface::build(build(), Size::new(W as f32, H as f32), cache, renderer)
    };

    let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();

    // Phase 1: pointer click (sets Pointer modality + pointer focus).
    if let Some(click_at) = pointer_click {
        let mut ui = mk(cache, &mut renderer);
        let cursor_at = iced::advanced::mouse::Cursor::Available(click_at);
        let events = [
            iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: click_at }),
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
        ];
        let _ = ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &events,
            cursor_at,
            &mut renderer,
            &mut messages,
        );
        cache = ui.into_cache();
    }

    // Phase 2: keyboard focus via the framework's own traversal operation.
    if keyboard_focus {
        let mut ui = mk(cache, &mut renderer);
        let mut op: Box<dyn iced::advanced::widget::Operation<()>> =
            enzui_core::focus::traversal_operation::<()>(enzui_core::focus::Traversal::Next);
        loop {
            ui.operate(&renderer, &mut *op);
            match op.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => op = next,
                _ => break,
            }
        }
        cache = ui.into_cache();
    }

    set_modality(modality);

    // Phase 3: draw with the observation cursor.
    let mut ui = mk(cache, &mut renderer);
    let cursor_at = iced::advanced::mouse::Cursor::Available(cursor);
    ui.draw(
        &mut renderer,
        theme,
        &iced::advanced::renderer::Style {
            text_color: base.text_color,
        },
        cursor_at,
    );
    renderer.screenshot(iced::Size::new(W, H), 1.0, base.background_color)
}

/// A `Theme::Dark` for widgets that carry their own magenta ring token.
fn dark() -> Theme {
    Theme::Dark
}

fn radio_build() -> Build {
    Box::new(|| {
        let tokens = RadioTokens {
            focus_ring: RING,
            ..RadioTokens::dark()
        };
        let opts: &'static [&'static str] = &["A", "B"];
        enz_radio_group("g", opts, Some(0), Msg::Usize)
            .tokens(tokens)
            .into()
    })
}

fn tab_list_build() -> Build {
    Box::new(|| {
        let tokens = TabPillTokens {
            focus_ring: RING,
            ..TabPillTokens::dark()
        };
        enz_tab_list(["One", "Two"], 0, Msg::Usize)
            .tokens(tokens)
            .into()
    })
}

fn combo_build() -> Build {
    Box::new(|| {
        let mut p = Palette::dark();
        p.focus_ring = RING;
        enz_combo_box(
            "lbl",
            vec!["one".to_owned(), "two".to_owned()],
            None,
            Msg::Str,
        )
        .palette(p)
        .into()
    })
}

fn image_build() -> Build {
    Box::new(|| {
        let tokens = ImageViewerTokens {
            focus_ring: RING,
            ..ImageViewerTokens::dark()
        };
        let handle = image::Handle::from_rgba(4, 4, vec![255u8; 4 * 4 * 4]);
        enz_image_viewer(handle, ViewTransform::IDENTITY)
            .on_transform(Msg::Xf)
            .tokens(tokens)
            .into()
    })
}

fn chip_build() -> Build {
    Box::new(|| {
        let dark = KeyCaptureChipTokens::dark();
        let tokens = KeyCaptureChipTokens {
            focus_border: RING,
            ..dark
        };
        enz_key_capture_chip("Save shortcut", Msg::Kc)
            .tokens(tokens)
            .into()
    })
}

macro_rules! ring_pair {
    ($kb:ident, $click:ident, $theme:expr, $build:expr, $over:expr, $name:literal) => {
        #[test]
        fn $kb() {
            let _g = MODALITY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let frame = render(&$theme, &$build, None, true, InputModality::Keyboard, AWAY);
            let ring = count_near(&frame, W, H, rgb(RING), 16);
            assert!(
                ring > 20,
                concat!(
                    "keyboard focus must paint a ",
                    $name,
                    " ring (magenta px = {})"
                ),
                ring
            );
        }

        #[test]
        fn $click() {
            let _g = MODALITY_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let frame = render(
                &$theme,
                &$build,
                Some($over),
                false,
                InputModality::Pointer,
                AWAY,
            );
            let ring = count_near(&frame, W, H, rgb(RING), 16);
            assert_eq!(
                ring, 0,
                concat!(
                    "a pointer click must not paint a ",
                    $name,
                    " focus ring (magenta px = {})"
                ),
                ring
            );
        }
    };
}

ring_pair!(
    radio_group_keyboard_focus_paints_ring,
    radio_group_pointer_click_paints_no_ring,
    dark(),
    radio_build(),
    OVER_RADIO,
    "radio_group"
);

ring_pair!(
    tab_list_keyboard_focus_paints_ring,
    tab_list_pointer_click_paints_no_ring,
    dark(),
    tab_list_build(),
    OVER_TAB,
    "tab_list"
);

ring_pair!(
    combo_box_keyboard_focus_paints_ring,
    combo_box_pointer_click_paints_no_ring,
    dark(),
    combo_build(),
    OVER_COMBO,
    "combo_box"
);

ring_pair!(
    image_viewer_keyboard_focus_paints_ring,
    image_viewer_pointer_click_paints_no_ring,
    dark(),
    image_build(),
    OVER_IMAGE,
    "image_viewer"
);

ring_pair!(
    key_capture_chip_keyboard_focus_paints_ring,
    key_capture_chip_pointer_click_paints_no_ring,
    dark(),
    chip_build(),
    OVER_CHIP,
    "key_capture_chip"
);
