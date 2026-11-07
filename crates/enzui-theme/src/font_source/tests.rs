use super::*;
use crate::EnzTheme;

fn named(name: &str) -> FontFamily {
    FontFamily::Named(name.to_owned())
}

#[test]
fn an_initial_read_reaches_the_theme_catalog() {
    let mut feed =
        DesktopFontFeed::with_backend(Arc::new(ScriptedFontBackend::new("initial", Vec::new())));
    assert_eq!(feed.status(), &FontFeedStatus::AwaitingFirstRead);

    let changed = feed.update(FontPreferenceEvent::Initial(FontPreferenceRead::Delivered(
        DesktopFontPreferences::new()
            .with(FontRole::Ui, "Adwaita Sans 11")
            .with(FontRole::Document, "Adwaita Sans 12")
            .with(FontRole::Monospace, "Adwaita Mono 11"),
    )));
    assert!(
        changed,
        "the first delivery changes the effective precedence"
    );
    assert_eq!(
        feed.status(),
        &FontFeedStatus::Delivered {
            malformed: Vec::new()
        }
    );

    let mut theme = EnzTheme::dark();
    let _task: Task<()> = feed.sync_theme(&mut theme);
    assert_eq!(
        theme.font_catalog.family(FontRole::Ui),
        &named("Adwaita Sans")
    );
    assert_eq!(
        theme.font_catalog.family(FontRole::Monospace),
        &named("Adwaita Mono")
    );
}
