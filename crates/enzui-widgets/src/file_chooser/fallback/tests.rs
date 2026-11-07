use std::time::{Duration, Instant};

use enzui_test_support::test_scratch::ScratchDir;

use super::*;

/// Opens a chooser on `dir` and waits for its first listing.
fn open(operation: PortalOperation, options: ChooserOptions) -> FallbackChooser {
    let mut chooser = FallbackChooser::new(operation, &options);
    settle(&mut chooser);
    chooser
}

fn settle(chooser: &mut FallbackChooser) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !chooser.absorb() {
        assert!(Instant::now() < deadline, "the listing never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn names(chooser: &FallbackChooser) -> Vec<String> {
    let mut names: Vec<String> = chooser
        .model()
        .visible()
        .iter()
        .map(|entry| entry.name.clone())
        .collect();
    names.sort();
    names
}

fn uri(path: &Path) -> FileUri {
    FileUri::new(OpenUri::from_file_path(path).unwrap().as_str())
}

fn fixture() -> ScratchDir {
    let scratch = ScratchDir::new("fallback-chooser");
    std::fs::create_dir_all(scratch.path().join("photos")).unwrap();
    std::fs::write(scratch.path().join("notes.txt"), "n").unwrap();
    std::fs::write(scratch.path().join("Holiday.PNG"), "p").unwrap();
    std::fs::write(scratch.path().join(".hidden"), "h").unwrap();
    std::fs::write(scratch.path().join("photos").join("a.png"), "a").unwrap();
    scratch
}

#[test]
fn fallback_chooser_lists_selects_saves_and_rejects_stale_work_1() {
    // glob_and_mime_patterns_match_names
    {
        assert!(glob_matches("*.png", "holiday.png"));
        assert!(glob_matches("img_??.jpg", "img_01.jpg"));
        assert!(!glob_matches("*.png", "holiday.png.txt"));
        assert!(glob_matches("*", ""));
        assert!(mime_matches("image/*", "image/png"));
        assert!(!mime_matches("image/*", "text/plain"));
        let images = FileFilter::new("Images").glob("*.png");
        assert!(
            filter_accepts(&images, "Holiday.PNG", None),
            "globs ignore case"
        );
        assert!(!filter_accepts(&images, "notes.txt", None));
        let table = GlobTable::new(vec![enzui_icons::mime::GlobRule::glob(
            "image/png",
            "*.png",
        )]);
        let by_type = FileFilter::new("Images").mime("image/*");
        assert!(filter_accepts(&by_type, "a.png", Some(&table)));
        assert!(!filter_accepts(&by_type, "a.txt", Some(&table)));
    }

    // lists_the_start_folder_with_the_active_filter
    {
        let scratch = fixture();
        let options = ChooserOptions::new()
            .current_folder(scratch.path())
            .filter(FileFilter::new("Images").glob("*.png"))
            .filter(FileFilter::new("Text").glob("*.txt"));
        let mut chooser = open(PortalOperation::OpenFile, options);
        assert_eq!(chooser.directory(), scratch.path());
        assert_eq!(
            names(&chooser),
            ["Holiday.PNG", "photos"],
            "folders always show"
        );
        chooser.update(FallbackEvent::FilterChanged(1));
        assert_eq!(names(&chooser), ["notes.txt", "photos"]);
        chooser.update(FallbackEvent::ToggleHidden);
        assert_eq!(
            names(&chooser),
            ["notes.txt", "photos"],
            "hidden files still pass the filter"
        );
    }

    // open_file_returns_the_selected_file_and_enters_folders
    {
        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenFile,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        assert_eq!(
            chooser.update(FallbackEvent::Activate(scratch.path().join("photos"))),
            None
        );
        settle(&mut chooser);
        assert_eq!(chooser.directory(), scratch.path().join("photos"));
        let file = scratch.path().join("photos").join("a.png");
        assert_eq!(
            chooser.update(FallbackEvent::Activate(file.clone())),
            Some(PortalOutcome::Completed(FileResult {
                uris: vec![uri(&file)]
            }))
        );
    }

    // open_multiple_toggles_and_returns_every_selected_file
    {
        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenMultiple,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        let (a, b) = (
            scratch.path().join("notes.txt"),
            scratch.path().join("Holiday.PNG"),
        );
        chooser.update(FallbackEvent::Select(a.clone()));
        chooser.update(FallbackEvent::Select(b.clone()));
        chooser.update(FallbackEvent::Select(a.clone()));
        chooser.update(FallbackEvent::Select(a.clone()));
        assert_eq!(
            chooser.update(FallbackEvent::Accept),
            Some(PortalOutcome::Completed(FileResult {
                uris: vec![uri(&b), uri(&a)]
            }))
        );
    }
}

#[test]
fn fallback_chooser_lists_selects_saves_and_rejects_stale_work_2() {
    // folder_mode_lists_folders_only_and_accepts_the_current_folder
    {
        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenDirectory,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        assert_eq!(names(&chooser), ["photos"]);
        assert_eq!(
            chooser.update(FallbackEvent::Accept),
            Some(PortalOutcome::Completed(FileResult {
                uris: vec![uri(scratch.path())]
            }))
        );
    }

    // save_asks_before_replacing_and_refuses_bad_names
    {
        let scratch = fixture();
        let options = ChooserOptions::new()
            .current_folder(scratch.path())
            .current_name("notes.txt");
        let mut chooser = open(PortalOperation::SaveFile, options);
        assert_eq!(
            chooser.update(FallbackEvent::Accept),
            None,
            "an existing name asks first"
        );
        assert_eq!(chooser.accept_label(), text("enzui-file-chooser-replace"));
        let existing = scratch.path().join("notes.txt");
        assert_eq!(
            chooser.update(FallbackEvent::Accept),
            Some(PortalOutcome::Completed(FileResult {
                uris: vec![uri(&existing)]
            }))
        );

        let mut chooser = open(
            PortalOperation::SaveFile,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        chooser.update(FallbackEvent::NameChanged("a/b".into()));
        assert_eq!(chooser.update(FallbackEvent::Accept), None);
        chooser.update(FallbackEvent::NameChanged("new file.txt".into()));
        let target = scratch.path().join("new file.txt");
        assert_eq!(
            chooser.update(FallbackEvent::Accept),
            Some(PortalOutcome::Completed(FileResult {
                uris: vec![uri(&target)]
            }))
        );
    }

    // cancel_reports_a_user_cancellation
    {
        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenFile,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        assert_eq!(
            chooser.update(FallbackEvent::Cancel),
            Some(PortalOutcome::Cancelled(super::super::CancelReason::User))
        );
    }

    // a_stale_listing_is_ignored
    {
        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenFile,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        chooser.update(FallbackEvent::Activate(scratch.path().join("photos")));
        chooser.update(FallbackEvent::Up);
        settle(&mut chooser);
        // Two listings were started; only the newest (the parent) may land.
        let deadline = Instant::now() + Duration::from_millis(200);
        while Instant::now() < deadline {
            chooser.absorb();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(chooser.directory(), scratch.path());
    }
}

#[test]
fn fallback_chooser_lists_selects_saves_and_rejects_stale_work_3() {
    // choosing_a_row_then_open_leaves_one_focused_control
    {
        use enzui_core::collect::SemanticsCollector;
        use iced_test::runtime::user_interface::{Cache, UserInterface};

        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenFile,
            ChooserOptions::new().current_folder(scratch.path()),
        );
        let mut renderer = iced::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .unwrap();
        let size = iced::Size::new(800.0, 700.0);
        let mut cache = Cache::default();
        // Presses `label`, applies the chooser's events, and returns how many
        // nodes claim focus afterwards.
        let mut press = |chooser: &mut FallbackChooser, label: &str| -> usize {
            let view: Element<'_, FallbackEvent> = chooser.host(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
                |event| event,
            );
            let mut ui =
                UserInterface::build(view, size, std::mem::take(&mut cache), &mut renderer);
            let mut messages = iced::advanced::shell::Bus::new();
            let _ = ui.update(
                &iced::advanced::graphics::core::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[],
                iced::mouse::Cursor::Unavailable,
                &mut renderer,
                &mut messages,
            );
            let mut collector = SemanticsCollector::new();
            ui.operate(&renderer, &mut collector);
            let target = collector
                .collected()
                .iter()
                .find(|node| node.semantics.label.as_deref() == Some(label))
                .unwrap_or_else(|| panic!("no {label:?}"))
                .bounds;
            let at = iced::Point::new(target.center_x(), target.center_y());
            let events = [
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: at }),
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                    iced::mouse::Button::Left,
                )),
            ];
            let _ = ui.update(
                &iced::advanced::graphics::core::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &events,
                iced::mouse::Cursor::Available(at),
                &mut renderer,
                &mut messages,
            );
            let mut collector = SemanticsCollector::new();
            ui.operate(&renderer, &mut collector);
            let focused = collector
                .collected()
                .iter()
                .filter(|node| node.semantics.focused)
                .count();
            cache = ui.into_cache();
            for event in messages.messages().to_vec() {
                let _ = chooser.update(event);
            }
            focused
        };
        assert!(
            press(&mut chooser, "notes.txt") <= 1,
            "after choosing the row"
        );
        assert!(press(&mut chooser, "Open") <= 1, "after pressing Open");
    }

    // a_typed_location_opens_folders_and_selects_files
    {
        let scratch = fixture();
        let mut chooser = open(
            PortalOperation::OpenFile,
            ChooserOptions::new().current_folder(scratch.path().join("photos")),
        );
        chooser.update(FallbackEvent::EditLocation);
        assert_eq!(
            chooser.location.as_deref(),
            Some(scratch.path().join("photos").display().to_string().as_str()),
            "editing starts from the current folder"
        );
        chooser.update(FallbackEvent::LocationChanged(
            scratch.path().join("notes.txt").display().to_string(),
        ));
        chooser.update(FallbackEvent::GoToLocation);
        settle(&mut chooser);
        assert_eq!(chooser.directory(), scratch.path());
        assert_eq!(chooser.selected, vec![scratch.path().join("notes.txt")]);
        assert!(
            chooser.location.is_none(),
            "the field closes once it opened something"
        );

        chooser.update(FallbackEvent::EditLocation);
        chooser.update(FallbackEvent::LocationChanged("photos".into()));
        chooser.update(FallbackEvent::GoToLocation);
        settle(&mut chooser);
        assert_eq!(
            chooser.directory(),
            scratch.path().join("photos"),
            "relative to the current folder"
        );

        chooser.update(FallbackEvent::EditLocation);
        chooser.update(FallbackEvent::LocationChanged("nowhere/at/all".into()));
        chooser.update(FallbackEvent::GoToLocation);
        assert!(
            chooser.error.is_some(),
            "the error stays inside the chooser"
        );
        assert_eq!(chooser.location.as_deref(), Some("nowhere/at/all"));
        assert_eq!(chooser.directory(), scratch.path().join("photos"));
    }
}
