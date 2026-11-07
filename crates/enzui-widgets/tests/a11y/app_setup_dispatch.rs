//! `app_setup::run` applies assistive-technology actions with no
//! application code: its program subscribes to the accessibility wake and
//! drains each batch in its own update.

use enzui_core::app_runtime::{drain_accessibility_wake, notify_accessibility_wake};
use enzui_widgets::app_setup::{AppServices, Traversing, with_services};
use iced::Program;
use iced::futures::StreamExt;

#[derive(Debug, Clone)]
enum Msg {}

fn update(_state: &mut (), message: Msg) {
    match message {}
}

fn view(_: &()) -> iced::Element<'_, Msg> {
    iced::widget::text("").into()
}

/// The first wake receipt `services`' program publishes after a notify.
fn wake(services: AppServices) -> Option<Traversing<Msg>> {
    let program = with_services(iced::application(|| (), update, view), services);
    let (state, _) = program.boot();
    let recipes = iced::advanced::subscription::into_recipes(program.subscription(&state));
    let mut streams: Vec<_> = recipes
        .into_iter()
        .map(|recipe| recipe.stream(Box::pin(iced::futures::stream::pending())))
        .collect();
    notify_accessibility_wake();
    let waker = iced::futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    streams
        .iter_mut()
        .find_map(|stream| match stream.poll_next_unpin(&mut context) {
            std::task::Poll::Ready(Some(message)) => Some(message),
            _ => None,
        })
}

#[test]
fn run_drains_accessibility_actions_itself() {
    let Some(Traversing::Accessibility(receipt)) = wake(AppServices::default()) else {
        panic!("the program subscribes to the accessibility wake");
    };
    let program = with_services(
        iced::application(|| (), update, view),
        AppServices::default(),
    );
    let (mut state, _) = program.boot();
    let _ = program.update(&mut state, Traversing::Accessibility(receipt.clone()));
    assert!(
        drain_accessibility_wake(receipt).is_none(),
        "the program's update already drained the batch"
    );

    assert!(
        wake(AppServices::default().accessibility_actions(false)).is_none(),
        "an application that applies actions itself gets no wake from the wrapper"
    );
}
