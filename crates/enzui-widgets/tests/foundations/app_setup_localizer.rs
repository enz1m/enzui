//! `AppServices::localizer` runs the whole application in its catalog: the
//! app's own messages read through `localization::text` in boot, update and
//! title, with no app-side translation helper.

use enzui_core::localization::{CatalogBuilder, Locale, Localizer};
use enzui_widgets::app_setup::{AppServices, Traversing, with_services};
use iced::Program;

#[derive(Debug, Clone)]
enum Msg {
    Greet,
}

fn greeting() -> String {
    enzui_core::localization::text("greeting")
}

/// What the application read last.
struct Greeting(String);

fn update(state: &mut Greeting, Msg::Greet: Msg) {
    state.0 = greeting();
}

fn view(state: &Greeting) -> iced::Element<'_, Msg> {
    iced::widget::text(state.0.as_str()).into()
}

fn localizer() -> Localizer {
    let mut catalog = CatalogBuilder::new();
    catalog
        .add_resource("en-US", "greeting = Hello\n")
        .expect("English");
    catalog
        .add_resource("hu", "greeting = Szia\n")
        .expect("Hungarian");
    Localizer::new(
        catalog.build(),
        vec!["hu".parse::<Locale>().expect("hu")],
        "en-US".parse().expect("en-US"),
    )
}

/// What boot, update and the title read for "greeting" under `services`.
fn read(services: AppServices) -> (String, String, String) {
    let program = with_services(
        iced::application(|| Greeting(greeting()), update, view).title(|_: &Greeting| greeting()),
        services,
    );
    let (mut state, _) = program.boot();
    let booted = state.0.clone();
    let _ = program.update(&mut state, Traversing::User(Msg::Greet));
    let title = program.title(&state, iced::window::Id::unique());
    (booted, state.0, title)
}

#[test]
fn the_application_reads_its_own_catalog_everywhere() {
    let localizer = localizer();
    let (booted, updated, title) = read(AppServices::default().localizer(localizer.clone()));
    assert_eq!(booted, "Szia", "boot");
    assert_eq!(updated, "Szia", "update");
    assert_eq!(title, "Szia", "title");

    // A clone switches the language for the running application.
    assert!(localizer.select(vec!["en-US".parse().expect("en-US")]));
    let (_, updated, _) = read(AppServices::default().localizer(localizer));
    assert_eq!(updated, "Hello", "after selecting English");

    // Without it, the app's keys are not in any catalog.
    let (_, updated, _) = read(AppServices::default());
    assert_eq!(updated, "greeting", "no application catalog");
}
