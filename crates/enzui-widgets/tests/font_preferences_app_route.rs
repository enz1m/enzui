//! The desktop's font preferences, driven through an ordinary application.
//!
//! Everything here goes the way a app application does: `ThemeIntegration`
//! is booted from public configuration, its real `subscription()` recipes are
//! run, the events they produce are folded by the application's own `update`,
//! and the `Task` that comes back is executed — its `Action::Font` payload is
//! applied to renderer settings exactly as `iced_winit` applies it. What is
//! then asserted is glyphs: pixels drawn by the headless renderer and advances
//! measured through the real font database, never a token or a task shape.
//!
//! The families are chosen from the fonts actually installed on the host, so a
//! claim cannot pass by naming a family nothing can resolve.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::file_watch::InMemoryBackend;
use enzui_core::font_preferences::{
    DesktopFontPreferences, FontPreferenceRead, ScriptedFontBackend,
};
use enzui_core::theme_file::ThemePaths;
use enzui_core::theme_file::integration::{ThemeEvent, ThemeIntegration, ThemeIntegrationConfig};
use enzui_theme::{EnzTheme, EnzThemeMode, FontFamily, FontRole};
use enzui_widgets::reading_pane::{
    EnzReadingKind, EnzReadingParagraph, EnzReadingTheme, enz_reading_pane,
};
use enzui_widgets::text::TextSize;
use enzui_widgets::text_measure::{
    MeasureTypography, enz_text_break_offset_with, enz_text_measure_width,
};
use iced::advanced::renderer::{Headless, Settings};
use iced::{Element, Size, Theme};
use iced_test::futures::futures::Stream;
use iced_test::futures::futures::executor::block_on;
use iced_test::runtime::{Action, Task};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// A private configuration directory, never the developer's own
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The fonts this host actually has
// ---------------------------------------------------------------------------

const SAMPLE: &str = "Handgloves quickly wave, 0123456789";

fn installed_faces() -> Vec<(String, bool)> {
    let mut system = iced::advanced::graphics::text::font_system()
        .write()
        .expect("the font system must be readable");
    system
        .raw()
        .db()
        .faces()
        .filter_map(|face| {
            face.families
                .first()
                .map(|(name, _)| (name.clone(), face.monospaced))
        })
        .collect()
}

fn width_of(family: &str, text: &str) -> f32 {
    enz_text_measure_width(
        text,
        MeasureTypography::new(TextSize::Body)
            .font_family(iced::font::Family::name(family))
            .pin_size_px(20.0),
    )
}

const WRAP_WIDTH: f32 = 180.0;

fn prose() -> String {
    SAMPLE.repeat(6)
}

/// Where the first visual line ends, which is what a paginator breaks on.
fn first_break(family: &str) -> usize {
    enz_text_break_offset_with(
        &prose(),
        WRAP_WIDTH,
        1,
        MeasureTypography::new(TextSize::Body)
            .font_family(iced::font::Family::name(family))
            .pin_size_px(20.0),
    )
}

/// Two installed families whose shaping is genuinely different, so a rendering
/// that ignored the preference cannot look like one that honoured it.
fn two_distinguishable_families() -> (String, String) {
    let mut names: Vec<String> = installed_faces()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.sort();
    names.dedup();
    let first = names
        .iter()
        .find(|name| width_of(name, SAMPLE) > 0.0)
        .cloned()
        .expect("this host must have at least one usable installed family");
    let first_width = width_of(&first, SAMPLE);
    let first_break_offset = first_break(&first);
    let second = names
        .iter()
        .find(|name| {
            **name != first
                && (width_of(name, SAMPLE) - first_width).abs() > 4.0
                && first_break(name) != first_break_offset
        })
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "this host has no second installed family that shapes differently \
                 from {first:?}; the fixture cannot distinguish a followed \
                 preference from an ignored one"
            )
        });
    (first, second)
}

// ---------------------------------------------------------------------------
// An ordinary application on the shared theme seam
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Message {
    Theme(ThemeEvent),
}

struct App {
    themes: ThemeIntegration,
    // What the runtime has been told the renderer default is, updated only by
    // executing the tasks the application returns.
    renderer: Settings,
}

impl App {
    fn boot(config: ThemeIntegrationConfig) -> (Self, Task<Message>) {
        let (themes, task) = ThemeIntegration::boot_with(config);
        (
            Self {
                themes,
                renderer: Settings::default(),
            },
            task.map(Message::Theme),
        )
    }

    // The reducer every app application already has: forward the event and
    // return the task.
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Theme(event) => self.themes.update(event).map(Message::Theme),
        }
    }

    fn theme(&self) -> &EnzTheme {
        self.themes.theme()
    }

    // Applies the runtime side of a returned task, the way `iced_winit` does:
    // `Action::Font(SetDefaults)` moves the renderer settings, and every
    // renderer built afterwards is built from them.
    fn run(&mut self, task: Task<Message>) -> Vec<Message> {
        let Some(stream) = iced_test::runtime::task::into_stream(task) else {
            return Vec::new();
        };
        let mut messages = Vec::new();
        for action in block_on(collect(stream)) {
            match action {
                Action::Output(message) => messages.push(message),
                Action::Font(iced_test::runtime::font::Action::SetDefaults { font, text_size }) => {
                    self.renderer.font = font;
                    self.renderer.text_size = text_size;
                }
                _ => {}
            }
        }
        messages
    }
}

async fn collect<T>(stream: iced_test::futures::BoxStream<T>) -> Vec<T> {
    use iced_test::futures::futures::StreamExt;
    stream.collect().await
}

// ---------------------------------------------------------------------------
// Running the real subscription
// ---------------------------------------------------------------------------

type EventStream = Pin<Box<dyn Stream<Item = ThemeEvent> + Send>>;

/// The application's own `subscription()`, run the way the runtime runs it.
fn run_subscription(integration: &ThemeIntegration) -> EventStream {
    use iced_test::futures::futures::StreamExt;
    use iced_test::futures::subscription;

    let recipes = subscription::into_recipes(integration.subscription());
    assert!(
        recipes.len() >= 2,
        "the shared theme seam must contribute a font-preference recipe of its own"
    );
    let streams: Vec<_> = recipes
        .into_iter()
        .map(|recipe| recipe.stream(iced_test::futures::futures::stream::empty().boxed()))
        .collect();
    Box::pin(iced_test::futures::futures::stream::select_all(streams))
}

struct Ping(std::sync::mpsc::SyncSender<()>);

impl Wake for Ping {
    fn wake(self: Arc<Self>) {
        let _ = self.0.try_send(());
    }
    fn wake_by_ref(self: &Arc<Self>) {
        let _ = self.0.try_send(());
    }
}

/// The next event, or a bounded failure. Never an unbounded wait.
fn next_event(stream: &mut EventStream, what: &str) -> ThemeEvent {
    let (sender, receiver) = std::sync::mpsc::sync_channel(8);
    let waker = Waker::from(Arc::new(Ping(sender)));
    let mut context = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match stream.as_mut().poll_next(&mut context) {
            Poll::Ready(Some(event)) => return event,
            Poll::Ready(None) => panic!("the subscription ended before {what}"),
            Poll::Pending => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "{what} did not arrive within 10s");
                let _ = receiver.recv_timeout(remaining.min(Duration::from_millis(25)));
            }
        }
    }
}

/// Drives the application until a font event has moved the UI family to
/// `expected`, or fails with what it saw instead.
fn settle_ui_family(app: &mut App, stream: &mut EventStream, expected: &FontFamily, what: &str) {
    for _ in 0..12 {
        if app.theme().font_catalog.family(FontRole::Ui) == expected {
            return;
        }
        let event = next_event(stream, what);
        let task = app.update(Message::Theme(event));
        let _ = app.run(task);
    }
    panic!(
        "{what}: the application's theme still resolves {:?}, not {expected:?}",
        app.theme().font_catalog.family(FontRole::Ui)
    );
}

// ---------------------------------------------------------------------------
// Pixels
// ---------------------------------------------------------------------------

const W: u32 = 420;
const H: u32 = 64;

fn render(settings: Settings, element: Element<'static, Message>) -> Vec<u8> {
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    let mut renderer = block_on(<iced_test::renderer::Renderer as Headless>::new(
        settings,
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("headless renderer");
    let mut ui = UserInterface::build(
        element,
        Size::new(W as f32, H as f32),
        Cache::default(),
        &mut renderer,
    );
    let theme = Theme::Dark;
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
    ui.draw(
        &mut renderer,
        &theme,
        &iced::advanced::renderer::Style {
            text_color: base.text_color,
        },
        iced::advanced::mouse::Cursor::Unavailable,
    );
    renderer.screenshot(iced::Size::new(W, H), 1.0, base.background_color)
}

fn ink(buffer: &[u8]) -> u32 {
    let background = {
        let last = (H * W * 4) as usize - 4;
        (buffer[last], buffer[last + 1], buffer[last + 2])
    };
    let mut painted = 0;
    for index in (0..(W * H * 4) as usize).step_by(4) {
        let pixel = (buffer[index], buffer[index + 1], buffer[index + 2]);
        let distance = (i32::from(pixel.0) - i32::from(background.0)).abs()
            + (i32::from(pixel.1) - i32::from(background.1)).abs()
            + (i32::from(pixel.2) - i32::from(background.2)).abs();
        if distance > 60 {
            painted += 1;
        }
    }
    painted
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn base_config() -> ThemeIntegrationConfig {
    ThemeIntegrationConfig::injected(
        ThemePaths::new(None::<PathBuf>, Vec::<PathBuf>::new(), None::<PathBuf>),
        Arc::new(InMemoryBackend::default()),
        enzui_core::theme_file::ProbeOverride::Dark,
        EnzThemeMode::Dark,
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The prose role reaches the reading surface, not just the chrome.
///
/// No settings *file* publishes a document font (GTK has one interface key,
/// KDE adds a fixed-width one), so the source here is a backend delivering a
/// document description — the same seam the portal backend fills.
#[test]
fn the_document_family_reaches_the_reading_pane() {
    {
        use iced::advanced::graphics::text::cosmic_text::{
            self, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Weight,
        };
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_font_data(include_bytes!("fixtures/fonts/FiraSans-Regular.ttf").to_vec());
        db.load_font_data(include_bytes!("fixtures/fonts/Inter-Regular.ttf").to_vec());
        let mut fallback = db
            .faces()
            .find(|face| face.families.iter().any(|(name, _)| name == "Inter"))
            .unwrap()
            .clone();
        db.remove_face(fallback.id);
        // An installed fallback with the exact weight must not displace the
        // requested family merely because it offers Regular instead of Medium.
        fallback.families[0].0 = "Noto Sans".into();
        fallback.weight = Weight(500);
        db.push_face_info(fallback);
        let mut fonts = FontSystem::new_with_locale_and_db("en-US".into(), db);
        for (family, weight) in [("Fira Sans", 400), ("Noto Sans", 500), ("Fira Sans", 500)] {
            let mut buffer = Buffer::new(&mut fonts, Metrics::new(16.0, 22.0));
            buffer.set_text(
                "Handgloves",
                &Attrs::new()
                    .family(Family::Name(family))
                    .weight(Weight(weight)),
                Shaping::Advanced,
                None,
            );
            buffer.shape_until_scroll(&mut fonts, false);
            let glyphs: Vec<_> = buffer
                .layout_runs()
                .flat_map(|run| run.glyphs.to_vec())
                .collect();
            assert!(!glyphs.is_empty());
            for glyph in glyphs {
                assert!(
                    fonts
                        .db()
                        .face(glyph.font_id)
                        .unwrap()
                        .families
                        .iter()
                        .any(|(name, _)| name == family),
                    "missing weight changed the requested {family} family at {weight}"
                );
            }
        }
    }
    let (family, _) = two_distinguishable_families();
    let backend = Arc::new(ScriptedFontBackend::new(
        "document-2796",
        vec![FontPreferenceRead::Delivered(
            DesktopFontPreferences::new()
                .with(FontRole::Document, format!("{family} 11"))
                .with(FontRole::Ui, format!("{family} 11")),
        )],
    ));
    let (mut app, boot) = App::boot(base_config().desktop_fonts(backend));
    let _ = app.run(boot);
    let default_theme = EnzTheme::dark();
    let mut stream = run_subscription(&app.themes);
    settle_ui_family(
        &mut app,
        &mut stream,
        &FontFamily::Named(family.clone()),
        "the desktop's family",
    );
    assert_eq!(
        app.theme().font_catalog.family(FontRole::Document),
        &FontFamily::Named(family),
        "premise: the document role must have moved"
    );

    let label = || enzui_widgets::text::enz_text(SAMPLE).into();
    let current = render(app.renderer, label());
    assert_eq!(
        current,
        render(app.renderer, label()),
        "same-input pixel control"
    );
    let width = enz_text_measure_width(SAMPLE, MeasureTypography::new(TextSize::Body));
    let mut larger = app.theme().clone();
    larger.typography = larger.typography.with_base_body_size(22.0);
    let scope = enzui_theme::text_defaults::TextDefaultsScope::enter(&larger);
    let changed = render(app.renderer, label());
    assert!(ink(&changed) > 0);
    assert_ne!(
        current, changed,
        "default widget text must follow the UI base size"
    );
    assert!(
        enz_text_measure_width(SAMPLE, MeasureTypography::new(TextSize::Body)) > width * 1.3,
        "measurement must follow the same size as paint"
    );
    drop(scope);

    let pane = |theme: &EnzTheme| {
        let element: Element<'static, Message> = enz_reading_pane(vec![EnzReadingParagraph::new(
            EnzReadingKind::Paragraph,
            SAMPLE.to_owned(),
            0,
        )])
        .theme(EnzReadingTheme::Dark)
        .font_size(20.0)
        .font_from_theme(theme)
        .into();
        render(Settings::default(), element)
    };

    let followed = pane(app.theme());
    let stock = pane(&default_theme);
    assert!(ink(&followed) > 0, "control: the pane must draw ink");
    assert_ne!(
        followed, stock,
        "the reading surface must shape with the desktop's document family"
    );
}

// ---------------------------------------------------------------------------
// The theme file's own `fonts.*` keys
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Text scale × font family independence
// ---------------------------------------------------------------------------
