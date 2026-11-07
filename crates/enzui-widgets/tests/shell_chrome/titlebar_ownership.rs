//! One titlebar pattern: the shell owns the band's height and the window
//! controls, a bar in its slot fills the band, and hidden floating chrome
//! fades out of the frame and the accessibility tree.

use enzui_core::collect::collect_frame;
use enzui_core::placement::Direction;
use enzui_widgets::app_shell::{ShellLayout, ShellTokens, enz_app_shell};
use enzui_widgets::title_bar::{DEFAULT_TITLEBAR_HEIGHT, enz_title_bar};
use enzui_widgets::title_bar_gesture::WindowCommand;
use enzui_widgets::toolbar_icon::enz_toolbar_icon;
use iced::window::{Decorations, Id};
use iced::{Element, Rectangle, Size};

#[derive(Debug, Clone)]
enum Msg {
    Window,
    Close,
    Back,
    Pin,
}

fn gesture(_: WindowCommand) -> Msg {
    Msg::Window
}

/// What the shell publishes, as labels with their bounds.
fn published(view: Element<'static, Msg>) -> Vec<(String, Rectangle)> {
    collect_frame(view, Size::new(640.0, 400.0))
        .collected()
        .iter()
        .filter_map(|node| Some((node.semantics.label.clone()?, node.bounds)))
        .collect()
}

fn count(nodes: &[(String, Rectangle)], label: &str) -> usize {
    nodes.iter().filter(|(l, _)| l == label).count()
}

fn bounds(nodes: &[(String, Rectangle)], label: &str) -> Rectangle {
    nodes
        .iter()
        .find(|(l, _)| l == label)
        .unwrap_or_else(|| panic!("no {label:?} in {nodes:?}"))
        .1
}

/// A shell window as `desktop` decorates it, its routes wired as `wire` says.
fn window(
    desktop: Decorations,
    wire: impl Fn(
        enzui_widgets::app_shell::EnzAppShell<'static, Msg>,
    ) -> enzui_widgets::app_shell::EnzAppShell<'static, Msg>,
    bar_gesture: bool,
) -> Vec<(String, Rectangle)> {
    let id = Id::unique();
    iced::window::set_decorations(id, desktop);
    published(iced::window::viewing(id, || {
        wire(
            enz_app_shell(|| iced::widget::text("content").into())
                .layout(ShellLayout::TitlebarContent)
                .titlebar(move || {
                    let bar = enz_title_bar().title("Document");
                    if bar_gesture {
                        bar.on_gesture(gesture).into_element()
                    } else {
                        bar.into_element()
                    }
                }),
        )
        .into()
    }))
}

#[test]
fn a_window_has_one_close_button_whoever_wires_it() {
    let own = Decorations::default();
    // The shell's window-gesture route alone gives the window its controls,
    // and the bar in its slot draws none of its own.
    let nodes = window(own, |shell| shell.on_window_gesture(gesture), true);
    assert_eq!(count(&nodes, "Close"), 1, "gesture route: {nodes:?}");
    let nodes = window(own, |shell| shell.on_window_gesture(gesture), false);
    assert_eq!(
        count(&nodes, "Close"),
        1,
        "gesture route, plain bar: {nodes:?}"
    );
    // Explicit messages on the shell: the bar still steps aside.
    let nodes = window(own, |shell| shell.on_close(Msg::Close), true);
    assert_eq!(count(&nodes, "Close"), 1, "explicit close: {nodes:?}");
    // A shell that draws none leaves the controls to the bar.
    let nodes = window(own, |shell| shell, true);
    assert_eq!(count(&nodes, "Close"), 1, "bar-drawn: {nodes:?}");
    // A compositor that decorates the window draws its own: none here.
    let server = Decorations {
        server_side: true,
        ..Decorations::default()
    };
    let nodes = window(server, |shell| shell, true);
    assert_eq!(count(&nodes, "Close"), 0, "server-side: {nodes:?}");
}

#[test]
fn a_titlebar_is_as_tall_inside_a_shell_as_outside() {
    use enzui_core::theme_file::{ThemeDocument, registry::TokenValue};
    use enzui_theme::{EnzThemeMode, text_defaults::TextDefaultsScope};
    for height in [DEFAULT_TITLEBAR_HEIGHT, 58.0] {
        let mut document = ThemeDocument::empty();
        assert!(document.set(
            "sizing.titlebar_height",
            EnzThemeMode::Light,
            TokenValue::Number(height)
        ));
        let theme = document.resolve(EnzThemeMode::Light);
        let _scope = TextDefaultsScope::enter(&theme);
        let alone = published(enz_title_bar().title("Document").into_element());
        let inside = published(
            enz_app_shell(|| iced::widget::text("content").into())
                .layout(ShellLayout::TitlebarContent)
                .titlebar(|| enz_title_bar().title("Document").into_element())
                .on_window_gesture(gesture)
                .into(),
        );
        let group = "\u{2068}Document\u{2069} title bar";
        assert_eq!(bounds(&alone, group).height, height);
        assert_eq!(bounds(&inside, group).height, height);
    }
}

#[test]
fn a_leading_control_comes_before_the_title_in_either_direction() {
    let bar = |direction| {
        published(
            enz_title_bar()
                .direction(direction)
                .leading(
                    enz_toolbar_icon("navigation.back", Msg::Back)
                        .label("Back")
                        .into_element(),
                )
                .title("Document")
                .push(
                    enz_toolbar_icon("pin", Msg::Pin)
                        .label("Pin")
                        .into_element(),
                )
                .into_element(),
        )
    };
    let ltr = bar(Direction::LeftToRight);
    assert!(
        bounds(&ltr, "Back").x < bounds(&ltr, "Document").x,
        "left-to-right: {ltr:?}"
    );
    assert!(
        bounds(&ltr, "Document").x < bounds(&ltr, "Pin").x,
        "the title before pushed content: {ltr:?}"
    );
    let rtl = bar(Direction::RightToLeft);
    assert!(
        bounds(&rtl, "Back").x > bounds(&rtl, "Document").x,
        "right-to-left: {rtl:?}"
    );
}

#[test]
fn hidden_floating_chrome_publishes_nothing() {
    let floating = |visible: bool| -> Element<'static, Msg> {
        enz_app_shell(|| iced::widget::text("photo").into())
            .layout(ShellLayout::FloatingTitlebar)
            .titlebar(|| enz_title_bar().title("Photo").into_element())
            .titlebar_scrim(true)
            .chrome_visible(visible)
            .on_window_gesture(gesture)
            .into()
    };

    let shown = published(floating(true));
    assert_eq!(count(&shown, "Photo"), 1, "{shown:?}");
    let hidden = published(floating(false));
    assert_eq!(count(&hidden, "Photo"), 0, "{hidden:?}");
}

#[test]
fn the_window_controls_sit_at_the_window_edge_in_either_direction() {
    let close = |direction: Direction| {
        let nodes = published(
            enz_app_shell(|| iced::widget::text("content").into())
                .layout(ShellLayout::TitlebarSidebarContent)
                .tokens(
                    ShellTokens::default()
                        .direction(direction)
                        .permanent_sidebar(),
                )
                .sidebar(|| iced::widget::text("nav").into())
                .titlebar(|| enz_title_bar().title("Document").into_element())
                .on_window_gesture(gesture)
                .into(),
        );
        bounds(&nodes, "Close")
    };
    // The window is 640 px wide; the sidebar sits on the leading side.
    let ltr = close(Direction::LeftToRight);
    assert!(ltr.x + ltr.width > 600.0, "left to right: {ltr:?}");
    let rtl = close(Direction::RightToLeft);
    assert!(rtl.x < 40.0, "right to left: {rtl:?}");
}

/// The ids of what the view publishes, by label.
fn published_ids(view: Element<'static, Msg>) -> Vec<(String, Option<iced::advanced::widget::Id>)> {
    collect_frame(view, Size::new(640.0, 400.0))
        .collected()
        .iter()
        .filter_map(|node| Some((node.semantics.label.clone()?, node.id.clone())))
        .collect()
}

#[test]
fn titlebar_commands_and_window_controls_are_routable() {
    use enzui_widgets::action_bar::ActionItem;
    let bar = |prefix: &'static str| {
        enz_title_bar()
            .title("Doc")
            .actions(
                vec![ActionItem::icon("new", "square-pen", "New", Msg::Pin)],
                Msg::Back,
            )
            .action_menu_id_prefix(prefix)
            .into_element()
    };
    let nodes = published_ids(
        enz_app_shell(|| iced::widget::text("content").into())
            .layout(ShellLayout::TitlebarContent)
            .titlebar(move || bar("main"))
            .on_window_gesture(gesture)
            .into(),
    );
    let id = |label: &str| {
        nodes
            .iter()
            .find(|(l, _)| l == label)
            .unwrap_or_else(|| panic!("no {label:?} in {nodes:?}"))
            .1
            .clone()
    };
    assert!(id("New").is_some(), "{nodes:?}");
    assert!(id("Close").is_some(), "{nodes:?}");

    // The same command in two bars of one window: distinct prefixes, distinct ids.
    let two: Element<'static, Msg> = iced::widget::column![bar("main"), bar("inspector")].into();
    let ids: Vec<_> = published_ids(two)
        .into_iter()
        .filter(|(label, _)| label == "New")
        .map(|(_, id)| id.expect("routable"))
        .collect();
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert_ne!(ids[0], ids[1]);
}
