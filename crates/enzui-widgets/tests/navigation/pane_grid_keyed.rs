use crate::honest_frame_driver::new_renderer;
use enzui_core::semantics::{Role, Semantics};
use enzui_widgets::semantics_container::SemanticsContainer;
use enzui_widgets::{
    EnzPaneContent, EnzPaneGridEvent, EnzPaneGridState, PaneDragEvent, PaneDropTarget, PaneEdge,
    PaneRegion, PaneSplitOrientation, PaneTree, enz_pane_grid_with_content,
};
use iced::advanced::renderer::Headless;
use iced::{Color, Element, Length, Rectangle, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::cell::RefCell;
use std::rc::Rc;

const SIZE: Size = Size::new(640.0, 320.0);
const TITLE: &str = "Full application title with enough text to require elision";
const LEFT: Color = Color::from_rgb(0.8, 0.05, 0.1);
const RIGHT: Color = Color::from_rgb(0.05, 0.15, 0.8);

#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "the test reads some messages' payloads and never sends others"
)]
enum Message {
    Grid(EnzPaneGridEvent),
    Editor(iced::widget::text_editor::Action),
    Activate(&'static str),
    Drag(PaneDragEvent<&'static str>),
    Body(&'static str),
    Close(&'static str),
}

type Allocations = Rc<RefCell<Vec<(&'static str, Rectangle)>>>;
fn state() -> EnzPaneGridState<&'static str> {
    EnzPaneGridState::new(PaneTree::split(
        PaneSplitOrientation::Vertical,
        0.4,
        PaneTree::pane("editor"),
        PaneTree::pane("terminal"),
    ))
}
fn view<'a>(
    state: &'a EnzPaneGridState<&'static str>,
    allocations: Allocations,
) -> Element<'a, Message> {
    enz_pane_grid_with_content(
        state,
        move |key, _, bounds| {
            allocations.borrow_mut().push((*key, bounds));
            let color = if *key == "editor" { LEFT } else { RIGHT };
            let body = SemanticsContainer::new(
                iced::widget::button(
                    iced::widget::Space::new()
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .padding(0)
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(Message::Body(key))
                .style(move |_: &iced::Theme, _| iced::widget::button::Style {
                    background: Some(color.into()),
                    ..Default::default()
                }),
                Semantics::new(Role::Button).label(format!("body:{key}")),
            );
            let close = SemanticsContainer::new(
                iced::widget::button(iced::widget::text("X"))
                    .width(32)
                    .height(28)
                    .on_press(Message::Close(key)),
                Semantics::new(Role::Button).label(format!("close:{key}")),
            );
            EnzPaneContent::new(body).header(TITLE).controls(close)
        },
        Message::Grid,
    )
    .on_activate(Message::Activate)
    .on_drag(Message::Drag)
    .id_base("keyed-grid")
    .into()
}
fn allocations() -> Allocations {
    Rc::new(RefCell::new(Vec::new()))
}
fn paint(state: &EnzPaneGridState<&'static str>) -> Vec<u8> {
    let mut renderer = new_renderer();
    let mut ui = UserInterface::build(
        view(state, allocations()),
        SIZE,
        Cache::default(),
        &mut renderer,
    );
    ui.draw(
        &mut renderer,
        &iced::Theme::Dark,
        &Default::default(),
        mouse::Cursor::Unavailable,
    );
    renderer.screenshot(
        Size::new(SIZE.width as u32, SIZE.height as u32),
        1.0,
        Color::BLACK,
    )
}
fn pixel(pixels: &[u8], x: usize, y: usize) -> &[u8] {
    &pixels[(y * 640 + x) * 4..][..3]
}
fn near(pixel: &[u8], color: Color) -> bool {
    pixel
        .iter()
        .zip([color.r, color.g, color.b])
        .all(|(&p, c)| (f32::from(p) - c * 255.0).abs() <= 2.0)
}

#[test]
fn pane_keyed_swap_drop_snapshot_restore_change_actual_paint() {
    let mut state = state();
    let original = state.snapshot();
    let original_pixels = paint(&state);
    assert!(near(pixel(&original_pixels, 100, 160), LEFT));
    assert!(near(pixel(&original_pixels, 500, 160), RIGHT));
    state.swap_panes(&"editor", &"terminal").unwrap();
    let swapped = paint(&state);
    assert!(near(pixel(&swapped, 100, 160), RIGHT));
    assert!(near(pixel(&swapped, 500, 160), LEFT));
    state
        .drop_pane(
            &"editor",
            &PaneDropTarget::Pane {
                key: "terminal",
                region: PaneRegion::Center,
            },
        )
        .unwrap();
    assert_eq!(paint(&state), original_pixels);
    for edge in [
        PaneEdge::Top,
        PaneEdge::Left,
        PaneEdge::Right,
        PaneEdge::Bottom,
    ] {
        state.restore(original.clone()).unwrap();
        state.update(EnzPaneGridEvent::Focus(0));
        state
            .drop_pane(
                &"editor",
                &PaneDropTarget::Pane {
                    key: "terminal",
                    region: PaneRegion::Edge(edge),
                },
            )
            .unwrap();
        assert_eq!(state.focused_separator(), None);
        assert_eq!(state.ratios(), vec![0.5]);
        let pixels = paint(&state);
        let (source, destination) = match edge {
            PaneEdge::Top => ((320, 80), (320, 250)),
            PaneEdge::Bottom => ((320, 250), (320, 80)),
            PaneEdge::Left => ((100, 160), (500, 160)),
            PaneEdge::Right => ((500, 160), (100, 160)),
        };
        assert!(near(pixel(&pixels, source.0, source.1), LEFT));
        assert!(near(pixel(&pixels, destination.0, destination.1), RIGHT));
        let saved = state.snapshot();
        state.restore(original.clone()).unwrap();
        state.restore(saved.clone()).unwrap();
        assert_eq!(state.snapshot(), saved);
        assert_eq!(paint(&state), pixels);
    }
    state.restore(original).unwrap();
    assert_eq!(paint(&state), original_pixels);
}
