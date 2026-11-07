//! The emoji picker: search by name and shortcode, recent emoji, and the
//! events its search field, tabs and cells send.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking"
)]

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_widgets::emoji_picker::{
    EmojiCategory, EmojiPickerEvent, EmojiRecents, MAX_RECENTS, enz_emoji_picker, search,
    search_up_to,
};
use iced::advanced::widget::Operation;
use iced::keyboard::{self, Key, Location, Modifiers, key::Code, key::Named, key::Physical};
use iced::{Element, Event, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[test]
fn search_finds_names_and_shortcodes_best_first() {
    let found: Vec<&str> = search("tada").iter().map(|emoji| emoji.as_str()).collect();
    assert_eq!(found.first(), Some(&"🎉"), "by shortcode: {found:?}");
    let cat: Vec<&str> = search("cat face").iter().map(|e| e.as_str()).collect();
    assert_eq!(cat.first(), Some(&"🐱"), "name first: {cat:?}");
    // Keywords count: a grinning cat is a cat face too.
    let cats: Vec<&str> = search("cat face").iter().map(|e| e.name()).collect();
    assert!(cats.contains(&"grinning cat"), "{cats:?}");
    assert!(search("   ").is_empty());
    // Emoji 15.1's head shake is newer than the default fonts.
    assert!(search("head shaking").is_empty(), "left out by default");
    assert!(
        !search_up_to("head shaking", (15, 1)).is_empty(),
        "offered when asked"
    );
    assert!(search("zzqqxx").is_empty());
}

#[test]
fn recents_keep_the_newest_first_without_repeats() {
    let mut recents = EmojiRecents::new();
    recents.record("👍");
    recents.record("🎉");
    recents.record("👍");
    recents.record("not an emoji");
    assert_eq!(recents.list(), ["👍", "🎉"]);
    for emoji in emojis::iter().take(40) {
        recents.record(emoji.as_str());
    }
    assert_eq!(recents.list().len(), MAX_RECENTS);
    let restored = EmojiRecents::from_list(recents.list().to_vec());
    assert_eq!(restored, recents, "saved and restored in order");
}

/// One frame of the picker: the events it sent and its buttons.
fn frame(
    query: &str,
    category: EmojiCategory,
    recents: &EmojiRecents,
    events: &[Event],
    cursor: mouse::Cursor,
    cache: &mut Cache,
    renderer: &mut Renderer,
) -> (
    Vec<EmojiPickerEvent>,
    Vec<enzui_core::collect::CollectedNode>,
) {
    let element: Element<'_, EmojiPickerEvent> =
        enz_emoji_picker(query, category, recents, |event| event).into();
    let mut ui = UserInterface::build(
        element,
        Size::new(500.0, 500.0),
        std::mem::take(cache),
        renderer,
    );
    let mut messages = iced::advanced::shell::Bus::new();
    let _ = ui.update(
        &window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        events,
        cursor,
        renderer,
        &mut messages,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector as &mut dyn Operation);
    *cache = ui.into_cache();
    let buttons = collector
        .into_collected()
        .into_iter()
        .filter(|node| node.semantics.role == Role::Button)
        .collect();
    (messages.into_iter().collect(), buttons)
}

fn click() -> Vec<Event> {
    vec![
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
    ]
}

#[test]
fn cells_tabs_and_enter_send_their_events() {
    let mut cache = Cache::default();
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut recents = EmojiRecents::new();
    recents.record("🎉");
    let none = mouse::Cursor::Unavailable;

    let (_, buttons) = frame(
        "",
        EmojiCategory::Recent,
        &recents,
        &[],
        none,
        &mut cache,
        &mut renderer,
    );
    let names: Vec<String> = buttons
        .iter()
        .filter_map(|node| node.semantics.label.clone())
        .collect();
    assert!(names.contains(&"party popper".to_owned()), "{names:?}");
    assert!(
        names.contains(&"Smileys & Emotion".to_owned()),
        "a named tab: {names:?}"
    );

    // A cell picks its emoji.
    let cell = buttons
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some("party popper"))
        .unwrap()
        .bounds
        .center();
    let (sent, _) = frame(
        "",
        EmojiCategory::Recent,
        &recents,
        &click(),
        mouse::Cursor::Available(cell),
        &mut cache,
        &mut renderer,
    );
    assert_eq!(sent, [EmojiPickerEvent::Picked("🎉".into())]);

    // A tab chooses its category.
    let tab = buttons
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some("Flags"))
        .unwrap()
        .bounds
        .center();
    let (sent, _) = frame(
        "",
        EmojiCategory::Recent,
        &recents,
        &click(),
        mouse::Cursor::Available(tab),
        &mut cache,
        &mut renderer,
    );
    assert_eq!(sent, [EmojiPickerEvent::Category(EmojiCategory::Flags)]);

    // A query shows its matches; Enter in the search field picks the first.
    let mut cache = Cache::default();
    let (_, buttons) = frame(
        "tada",
        EmojiCategory::Smileys,
        &recents,
        &[],
        none,
        &mut cache,
        &mut renderer,
    );
    assert!(
        buttons
            .iter()
            .any(|node| node.semantics.label.as_deref() == Some("party popper")),
        "the match is shown"
    );
    let (_, _) = frame(
        "tada",
        EmojiCategory::Smileys,
        &recents,
        &click(),
        mouse::Cursor::Available(iced::Point::new(60.0, 20.0)),
        &mut cache,
        &mut renderer,
    );
    let enter = |pressed: bool| {
        let key = Key::Named(Named::Enter);
        Event::Keyboard(if pressed {
            keyboard::Event::KeyPressed {
                key: key.clone(),
                modified_key: key,
                physical_key: Physical::Code(Code::Enter),
                location: Location::Standard,
                modifiers: Modifiers::empty(),
                text: None,
                repeat: false,
            }
        } else {
            keyboard::Event::KeyReleased {
                key: key.clone(),
                modified_key: key,
                physical_key: Physical::Code(Code::Enter),
                location: Location::Standard,
                modifiers: Modifiers::empty(),
            }
        })
    };
    let (sent, _) = frame(
        "tada",
        EmojiCategory::Smileys,
        &recents,
        &[enter(true), enter(false)],
        none,
        &mut cache,
        &mut renderer,
    );
    assert_eq!(sent, [EmojiPickerEvent::Picked("🎉".into())]);
}
