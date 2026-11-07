use super::*;
use iced::widget::{container, text};

#[derive(Debug, Clone, PartialEq)]
enum Msg {}

fn content<'a>() -> Element<'a, Msg> {
    container(text("anchor")).into()
}

fn popup<'a>() -> Element<'a, Msg> {
    container(text("popup")).into()
}
fn tree_for(w: &mut AnchoredOverlay<'_, Msg>) -> widget::Tree {
    let mut tree = widget::Tree {
        tag: Widget::<Msg, iced::Theme, iced::Renderer>::tag(w),
        state: Widget::<Msg, iced::Theme, iced::Renderer>::state(w),
        children: Vec::new(),
        ..widget::Tree::empty()
    };
    Widget::<Msg, iced::Theme, iced::Renderer>::diff(w, &mut tree);
    tree
}

#[test]
fn closing_the_popup_drops_its_subtree() {
    let mut open = anchored_overlay(content(), Some(popup()));
    let mut tree = tree_for(&mut open);
    assert_eq!(tree.children.len(), 2);

    let mut closed = anchored_overlay(content(), None::<Element<'_, Msg>>);
    Widget::<Msg, iced::Theme, iced::Renderer>::diff(&mut closed, &mut tree);
    assert_eq!(tree.children.len(), 1);
}

#[test]
fn a_popup_leaves_the_window_only_when_it_does_not_fit_inside_or_may() {
    use iced::advanced::renderer::Headless as _;
    let renderer = iced::futures::executor::block_on(iced::Renderer::new(
        iced::advanced::renderer::Settings::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer");
    let window = Size::new(300.0, 200.0);
    let screen = Some(Size::new(1920.0, 1080.0));
    // A button near the bottom edge, with room above it but not below.
    let anchor = Rectangle::new(Point::new(10.0, 170.0), Size::new(60.0, 24.0));
    let placement = Placement::new(Side::Bottom);
    let place = |height: f32, reach: Option<Size>, overflow: PopupOverflow| {
        let mut popup: Element<'_, Msg> = container(text("popup"))
            .width(Length::Fixed(100.0))
            .height(Length::Fixed(height))
            .into();
        let mut tree = widget::Tree::new(popup.as_widget());
        resolve_popup_layout_within(
            &mut popup, &mut tree, &renderer, window, anchor, &placement, None, reach, overflow,
        )
        .bounds()
    };
    let inside = |rect: Rectangle| fits_within(Rectangle::with_size(window), rect);

    // It fits above the button: it stays in the window unless allowed out.
    let flipped = place(150.0, screen, PopupOverflow::WhenNeeded);
    assert!(inside(flipped), "flipped above, inside: {flipped:?}");
    assert_eq!(flipped.height, 150.0);
    assert_eq!(place(150.0, screen, PopupOverflow::Never), flipped);
    let below = place(150.0, screen, PopupOverflow::Allowed);
    assert_eq!(
        below.y,
        anchor.y + anchor.height,
        "allowed out, below: {below:?}"
    );
    assert_eq!(
        place(150.0, None, PopupOverflow::Allowed),
        flipped,
        "no popups, no way out"
    );

    // Taller than the window: only the room around it holds it whole.
    let tall = place(260.0, screen, PopupOverflow::WhenNeeded);
    assert!(!inside(tall), "out of the window: {tall:?}");
    assert_eq!(tall.height, 260.0, "at full height: {tall:?}");
    let kept = place(260.0, screen, PopupOverflow::Never);
    assert!(inside(kept), "kept inside: {kept:?}");
}
