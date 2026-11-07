#[test]
fn shell_geometry_matches_pane_visibility_direction_and_material_1() {
    // hidden_floating_chrome_omits_its_semantics
    {
    fn labels(visible: bool) -> Vec<String> {
        let el: Element<'static, Msg> = enz_app_shell(|| {
            SemanticsContainer::leaf(
                Space::new(),
                Semantics::new(Role::Button).label("content-action"),
            )
            .into()
        })
        .layout(ShellLayout::FloatingTitlebar)
        .chrome_visible(visible)
        .titlebar(|| {
            SemanticsContainer::leaf(
                Space::new(),
                Semantics::new(Role::Button).label("chrome-action"),
            )
            .into()
        })
        .into();

        let mut r = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let mut ui =
            UserInterface::build(el, Size::new(W as f32, H as f32), Cache::default(), &mut r);
        ui = ui.relayout(Size::new(W as f32, H as f32), &mut r);
        let mut c = SemanticsCollector::new();
        ui.operate(&r, &mut c as &mut dyn Operation);
        c.into_collected()
            .into_iter()
            .filter(|n| n.semantics.role == Role::Button)
            .filter_map(|n| n.semantics.label)
            .collect()
    }

    let visible = labels(true);
    assert!(
        visible.iter().any(|l| l == "chrome-action"),
        "calibration: visible chrome must publish its control, saw {visible:?}",
    );

    let hidden = labels(false);
    assert!(
        !hidden.iter().any(|l| l == "chrome-action"),
        "hidden chrome must leave the tree entirely, but its control is still \
         announced: {hidden:?}",
    );
    assert!(
        hidden.iter().any(|l| l == "content-action"),
        "hiding the chrome must not take the content with it, saw {hidden:?}",
    );
}

    // the_titlebar_island_sits_at_the_trailing_edge_in_either_direction
    {
    use enzui_core::placement::Direction;
    use enzui_widgets::app_shell::ShellTokens;

    fn bounds(direction: Direction) -> (iced::Rectangle, iced::Rectangle) {
        let el: Element<'static, Msg> = enz_app_shell(|| Space::new().into())
            .layout(ShellLayout::FloatingTitlebar)
            .tokens(ShellTokens::default().direction(direction))
            .titlebar(|| {
                SemanticsContainer::leaf(
                    Space::new().width(iced::Length::Fill).height(30),
                    Semantics::new(Role::Button).label("title"),
                )
                .into()
            })
            .titlebar_island(enzui_theme::MaterialStyle::Regular, || {
                SemanticsContainer::leaf(
                    Space::new().width(30).height(30),
                    Semantics::new(Role::Button).label("island-action"),
                )
                .into()
            })
            .into();
        let mut r = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let mut ui =
            UserInterface::build(el, Size::new(W as f32, H as f32), Cache::default(), &mut r);
        ui = ui.relayout(Size::new(W as f32, H as f32), &mut r);
        let mut c = SemanticsCollector::new();
        ui.operate(&r, &mut c as &mut dyn Operation);
        let nodes = c.into_collected();
        let find = |label: &str| {
            nodes
                .iter()
                .find(|n| n.semantics.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("no {label}"))
                .bounds
        };
        (find("title"), find("island-action"))
    }

    let (title, island) = bounds(Direction::LeftToRight);
    assert!(
        island.x >= title.x + title.width,
        "left-to-right: island right of the title"
    );
    let ltr_gap = W as f32 - (island.x + island.width);
    let (title, island) = bounds(Direction::RightToLeft);
    assert!(
        island.x + island.width <= title.x,
        "right-to-left: island left of the title, island {island:?} title {title:?}"
    );
    assert!(
        (island.x - ltr_gap).abs() < 0.5,
        "the trailing inset mirrors: {} from the left against {ltr_gap} from the right",
        island.x
    );
}

    // the_content_titlebar_stands_off_the_sidebar_and_the_window_edge
    {
    use enzui_core::placement::Direction;
    use enzui_widgets::app_shell::{ShellTokens, TOGGLE_EDGE_LEAD};

    fn bounds(direction: Direction) -> (iced::Rectangle, iced::Rectangle) {
        let el: Element<'static, Msg> = enz_app_shell(|| Space::new().into())
            .layout(ShellLayout::TitlebarSidebarContent)
            .tokens(
                ShellTokens::default()
                    .direction(direction)
                    .permanent_sidebar(),
            )
            .titlebar(|| {
                SemanticsContainer::leaf(
                    Space::new().width(iced::Length::Fill).height(20),
                    Semantics::new(Role::Button).label("title"),
                )
                .into()
            })
            .sidebar(|| {
                SemanticsContainer::leaf(
                    Space::new().width(iced::Length::Fill).height(30),
                    Semantics::new(Role::Button).label("nav"),
                )
                .into()
            })
            .into();
        let mut r = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let size = Size::new(1100.0, 700.0);
        let mut ui = UserInterface::build(el, size, Cache::default(), &mut r);
        ui = ui.relayout(size, &mut r);
        let mut c = SemanticsCollector::new();
        ui.operate(&r, &mut c as &mut dyn Operation);
        let nodes = c.into_collected();
        let find = |label: &str| {
            nodes
                .iter()
                .find(|n| n.semantics.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("no {label}"))
                .bounds
        };
        (find("title"), find("nav"))
    }

    let (title, nav) = bounds(Direction::LeftToRight);
    assert!(
        title.x >= nav.x + nav.width + TOGGLE_EDGE_LEAD - 0.5,
        "left-to-right: the title stands off the sidebar: {title:?} after {nav:?}"
    );
    assert!(
        title.x + title.width <= 1100.0 - TOGGLE_EDGE_LEAD + 0.5,
        "left-to-right: the title stands off the window edge: {title:?}"
    );
    let (title, nav) = bounds(Direction::RightToLeft);
    assert!(
        title.x + title.width <= nav.x - TOGGLE_EDGE_LEAD + 0.5,
        "right-to-left: the title stands off the sidebar: {title:?} before {nav:?}"
    );
    assert!(
        title.x >= TOGGLE_EDGE_LEAD - 0.5,
        "right-to-left: the title stands off the window edge: {title:?}"
    );
}

    // a_collapsing_sidebar_keeps_the_width_it_had_inline
    {
    use enzui_widgets::app_shell::ShellTokens;
    use enzui_widgets::nav_split::{NavSplitState, SidebarPreference};
    let available = 900.0;
    let geometry = |preference| {
        enz_app_shell(|| Space::new().into())
            .layout(ShellLayout::TitlebarSidebarContent)
            .sidebar(|| Space::new().into())
            .on_toggle_sidebar(Msg::Nav)
            .state(NavSplitState {
                preference,
                ..NavSplitState::new()
            })
            .geometry_for(available)
    };
    let shown = geometry(SidebarPreference::Shown);
    let hidden = geometry(SidebarPreference::Hidden);
    assert!(
        shown.sidebar_width < ShellTokens::default().sidebar_width,
        "precondition: the inline sidebar narrows to its content share ({})",
        shown.sidebar_width
    );
    assert_eq!(
        hidden.sidebar_width, shown.sidebar_width,
        "the collapse animates the width the sidebar had, not a wider one"
    );
}
}







#[test]
fn frame_material_preserves_the_reported_content_region() {
    use enzui_theme::{EnzTheme, MaterialStyle, WindowFrame};
    use enzui_widgets::app_shell::{ShellTokens, enz_app_shell_with_geometry};
    use iced::{
        Length,
        widget::{Id, container},
    };
    use std::cell::Cell;

    for (material, themed) in [
        (Some(MaterialStyle::FlatFrosted), false),
        (Some(MaterialStyle::Regular), false),
        (None, true),
    ] {
        let mut theme = EnzTheme::light();
        if themed {
            theme.window_frame = WindowFrame::Glass(MaterialStyle::Regular);
        }
        let _scope = enzui_theme::text_defaults::TextDefaultsScope::enter(&theme);
        let reported = Cell::new(0.0);
        let mut tokens = ShellTokens::inset_panes().gap(4.0);
        tokens.sidebar_width = 232.0;
        tokens.content_minimum = 600.0;
        tokens.frame_material = material;
        let view: Element<'_, ()> = enz_app_shell_with_geometry(|geometry| {
            reported.set(geometry.content_width);
            container(Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .id(Id::new("frame-content-region"))
                .into()
        })
        .layout(ShellLayout::FloatingTitlebarSidebar)
        .tokens(tokens)
        .sidebar(|| Space::new().into())
        .on_sidebar_state(|_| ())
        .titlebar(|| Space::new().into())
        .into();
        let mut ui = iced_test::Simulator::with_size(
            iced::Settings::default(),
            Size::new(1280.0, 800.0),
            view,
        );
        let actual = ui
            .find(Id::new("frame-content-region"))
            .expect("content region")
            .visible_bounds()
            .expect("visible content region");
        assert!(
            (actual.width - reported.get()).abs() < 0.5,
            "frame {material:?}, themed={themed}: reported {}, laid out {actual:?}",
            reported.get()
        );
        assert_eq!(
            actual.y, 4.0,
            "the frame surrounds panes, not unprotected text"
        );
        assert_eq!(
            actual.height, 792.0,
            "the requested edge owns the viewport budget"
        );
    }
}
