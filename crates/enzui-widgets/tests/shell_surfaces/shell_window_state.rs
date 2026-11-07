//! Window identity isolation through emitted runtime actions.

use enzui_widgets::app_shell::{ShellWindowEvent, ShellWindowIdentity, ShellWindowState};
use enzui_widgets::title_bar_gesture::WindowCommand;
use iced::window;
use iced_test::futures::futures::FutureExt;
use iced_test::futures::futures::StreamExt;
use iced_test::runtime::Action;
use iced_test::runtime::task::into_stream;
use iced_test::runtime::window::Action as WA;

// Poll only ready actions: compositor queries await replies this fixture does not send.
fn window_actions(task: iced::Task<()>) -> Vec<WA> {
    let mut actions = Vec::new();
    if let Some(mut stream) = into_stream(task) {
        for _ in 0..16 {
            match StreamExt::next(&mut stream).now_or_never() {
                Some(Some(Action::Window(w))) => actions.push(w),
                Some(Some(_)) => {}
                Some(None) | None => break,
            }
        }
    }
    actions
}

#[test]
fn a_closed_shell_does_not_adopt_the_next_window() {
    let main = window::Id::unique();
    let preview = window::Id::unique();

    let mut state = ShellWindowState::new();
    let _ = state.update(ShellWindowEvent::Opened(main));
    let _ = state.update(ShellWindowEvent::Closed(main));
    assert_eq!(state.identity(), ShellWindowIdentity::Unbound);

    let _ = state.update(ShellWindowEvent::Opened(preview));
    assert_eq!(
        state.identity(),
        ShellWindowIdentity::Unbound,
        "a closed shell must not re-bind to a later window",
    );
    assert!(
        window_actions(state.run_command(WindowCommand::Drag)).is_empty(),
        "a closed shell commands nothing",
    );
}

#[test]
fn fullscreen_requests_target_the_bound_window_and_toggle_the_native_answer() {
    let main = window::Id::unique();
    let other = window::Id::unique();
    let mut state = ShellWindowState::for_window(main);
    for (enabled, expected) in [
        (true, window::Mode::Fullscreen),
        (false, window::Mode::Windowed),
    ] {
        let actions = window_actions(state.set_fullscreen(enabled));
        assert!(
            matches!(actions.as_slice(), [WA::SetMode(id, mode)] if *id == main && *mode == expected)
        );
    }
    for (native, expected) in [
        (window::Mode::Fullscreen, window::Mode::Windowed),
        (window::Mode::Windowed, window::Mode::Fullscreen),
    ] {
        let task: iced::Task<()> = state.toggle_fullscreen();
        let mut stream = into_stream(task).expect("query then native transition");
        let query =
            iced_test::futures::futures::executor::block_on(stream.next()).expect("mode query");
        let Action::Window(WA::GetMode(id, reply)) = query else {
            panic!("query native mode before deciding")
        };
        assert_eq!(id, main);
        reply.send(native).expect("mode response");
        let action = iced_test::futures::futures::executor::block_on(stream.next())
            .expect("mode transition");
        assert!(
            matches!(action, Action::Window(WA::SetMode(id, mode)) if id == main && mode == expected)
        );
    }
    let changed = window::Event::DecorationsChanged(window::Decorations {
        fullscreen: true,
        ..Default::default()
    });
    let event = ShellWindowEvent::from_window_event(&changed, main)
        .expect("same-size state changes must be observed");
    assert!(matches!(event, ShellWindowEvent::Observed(id) if id == main));
    let actions = window_actions(state.update(event).map(|_| ()));
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, WA::GetMode(id, _) if *id == main))
    );
    let _ = state.update(ShellWindowEvent::ModeChanged {
        window: other,
        mode: window::Mode::Fullscreen,
    });
    assert!(!state.fullscreen());
    let _ = state.update(ShellWindowEvent::Closed(main));
    assert!(window_actions(state.toggle_fullscreen()).is_empty());
    assert!(window_actions(state.set_fullscreen(true)).is_empty());
}
