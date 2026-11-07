//! Controls the framework builds inside its own dialogs carry routable ids,
//! so assistive technology gets their invoke actions: the shortcuts dialog's
//! Close and the in-app file chooser's Cancel, Open, parent-folder and
//! hidden-files controls.

use enzui_core::collect::collect_frame;
use enzui_widgets::file_chooser::{ChooserOptions, FallbackChooser, PortalOperation};
use enzui_widgets::shortcuts_dialog::enz_shortcuts_dialog;
use iced::{Element, Size};

fn routable(element: Element<'_, ()>, labels: &[&str]) {
    let frame = collect_frame(element, Size::new(900.0, 700.0));
    for label in labels {
        let node = frame
            .collected()
            .iter()
            .find(|node| node.semantics.label.as_deref() == Some(*label))
            .unwrap_or_else(|| panic!("{label} is published"));
        assert!(node.id.is_some(), "{label} has a routable id");
    }
}

#[test]
fn chooser_and_dialog_controls_remain_visible_and_routable() {
    // the_shortcuts_dialog_close_is_routable
    {
        routable(
            enz_shortcuts_dialog::<()>(true, Vec::new())
                .close_label("Close")
                .into_surface(Some(Box::new(|| ()))),
            &["Close"],
        );
    }

    // the_fallback_chooser_controls_are_routable
    {
        let chooser = FallbackChooser::new(PortalOperation::OpenFile, &ChooserOptions::new());
        let view = chooser.view().map(|_| ());
        routable(view, &["Cancel", "Open", "Parent folder", "Hidden files"]);
    }

    // a_short_save_chooser_keeps_its_name_and_actions_in_the_window
    {
        let chooser = FallbackChooser::new(PortalOperation::SaveFile, &ChooserOptions::new());
        let size = Size::new(420.0, 360.0);
        let frame = collect_frame(chooser.view().map(|_| ()), size);
        for label in ["Cancel", "Save", "Name"] {
            let node = frame
                .collected()
                .iter()
                .find(|node| node.semantics.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("{label} is published"));
            assert!(
                node.bounds.height > 0.0 && node.bounds.y + node.bounds.height <= size.height + 0.5,
                "{label} at {:?} leaves the {size:?} window",
                node.bounds
            );
        }
    }

    // ctrl_l_edits_the_choosers_location
    {
        use enzui_widgets::file_chooser::FallbackEvent;
        use iced::keyboard::key::{NativeCode, Physical};
        use iced::keyboard::{Event as KeyEvent, Key, Location, Modifiers};
        use iced_test::runtime::user_interface::{Cache, UserInterface};
        let scratch =
            enzui_test_support::test_scratch::ScratchDir::new("chooser-location-selection");
        let first = scratch.path().join("first");
        let second = scratch.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        let mut chooser = FallbackChooser::new(
            PortalOperation::OpenDirectory,
            &ChooserOptions::new().current_folder(scratch.path()),
        );
        routable(chooser.view().map(|_| ()), &["Location"]);
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let mut ui = UserInterface::build(
            chooser.view(),
            Size::new(900.0, 700.0),
            Cache::default(),
            &mut renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let key = Key::Character("l".into());
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[
                iced::Event::Keyboard(KeyEvent::ModifiersChanged(Modifiers::CTRL)),
                iced::Event::Keyboard(KeyEvent::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: Physical::Unidentified(NativeCode::Unidentified),
                    location: Location::Standard,
                    modifiers: Modifiers::CTRL,
                    repeat: false,
                    text: Some("l".into()),
                }),
            ],
            iced::mouse::Cursor::Unavailable,
            &mut renderer,
            &mut messages,
        );
        assert!(
            messages.contains(&FallbackEvent::EditLocation),
            "Ctrl+L asks to edit the location: {messages:?}"
        );
        let mut cache = ui.into_cache();
        for (path, submit) in [(&first, true), (&first, false), (&second, true)] {
            chooser.update(FallbackEvent::EditLocation);
            let mut ui = UserInterface::build(
                chooser.view(),
                Size::new(900.0, 700.0),
                cache,
                &mut renderer,
            );
            let mut stream = iced_test::runtime::task::into_stream(
                enzui_widgets::file_chooser::fallback::focus_task::<FallbackEvent>(
                    &FallbackEvent::EditLocation,
                ),
            )
            .unwrap();
            let waker = iced::futures::task::noop_waker();
            let mut cx = std::task::Context::from_waker(&waker);
            while let std::task::Poll::Ready(Some(action)) =
                iced::futures::StreamExt::poll_next_unpin(&mut stream, &mut cx)
            {
                if let iced_test::runtime::Action::Widget(mut operation) = action {
                    loop {
                        ui.operate(&renderer, operation.as_mut());
                        match operation.finish() {
                            iced::advanced::widget::operation::Outcome::Chain(next) => {
                                operation = next;
                            }
                            _ => break,
                        }
                    }
                }
            }
            let mut entered = iced::advanced::shell::Bus::new();
            let mut events = vec![iced::Event::Keyboard(KeyEvent::ModifiersChanged(
                Modifiers::empty(),
            ))];
            events.extend(iced_test::simulator::typewrite(&path.display().to_string()));
            if submit {
                events.push(iced::Event::Keyboard(KeyEvent::KeyPressed {
                    key: Key::Named(iced::keyboard::key::Named::Enter),
                    modified_key: Key::Named(iced::keyboard::key::Named::Enter),
                    physical_key: Physical::Unidentified(NativeCode::Unidentified),
                    location: Location::Standard,
                    modifiers: Modifiers::empty(),
                    repeat: false,
                    text: None,
                }));
            }
            let _ = ui.update(
                &iced::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &events,
                iced::mouse::Cursor::Unavailable,
                &mut renderer,
                &mut entered,
            );
            cache = ui.into_cache();
            assert!(
                entered.iter().any(|message| matches!(message,
            FallbackEvent::LocationChanged(value) if value == &path.display().to_string())),
                "Ctrl+L replaces the existing location, including a pending edit: {entered:?}"
            );
            for message in entered {
                chooser.update(message);
            }
            if submit {
                assert_eq!(chooser.directory(), path);
            }
        }
    }
}
