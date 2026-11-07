//! Exercises public file widgets through the default service, rather than a
//! fixture-owned native runtime. GTK retains the receipt until disk work ends.
use super::super::report;
use super::{Input, assert_no_terminal, compositor, gtk, positions};
use enzui_widgets::file_drag::{
    DragOutcome, Payload, PreparedFiles, enz_data_drag_source, enz_file_drag_source,
    enz_file_drag_source_with,
};
use enzui_widgets::file_drop::{FileDrop, enz_file_drop_target_with_receipt};
use iced::{Element, Length, Task};
use std::path::PathBuf;
use std::time::Duration;

pub(super) fn both_ways() {
    for (outgoing, prepared, data, protected, selection) in [
        (false, false, false, false, 0),
        (true, false, false, false, 0),
        (true, true, false, false, 0),
        (true, false, true, false, 0),
        (true, false, true, true, 0),
        (true, false, false, false, 1),
        (true, false, false, false, 2),
        (true, false, false, false, 3),
    ] {
        let display = compositor();
        let mut input = Input::new(&display);
        let app = display.app(
            if selection == 1 {
                "native-drag-widgets-glyph-send"
            } else if selection == 3 {
                "native-drag-widgets-scope-retired-send"
            } else if selection == 2 {
                "native-drag-widgets-scope-send"
            } else if protected {
                "native-drag-widgets-data-protected-send"
            } else if data {
                "native-drag-widgets-data-send"
            } else if prepared {
                "native-drag-widgets-prepared-send"
            } else if outgoing {
                "native-drag-widgets-send"
            } else {
                "native-drag-widgets-receive"
            },
            None,
        );
        app.expect_line("widget-ready");
        let peer = gtk(
            &display,
            if outgoing { "destination" } else { "source" },
            if selection > 0 {
                "selection-copy"
            } else if data {
                "copy"
            } else if prepared {
                "prepared-copy"
            } else {
                "file-copy"
            },
        );
        peer.expect_line("peer-ready");
        let exported = display.runtime.path().join("prepared-file");
        assert!(!exported.exists(), "view construction must not stage files");
        let (source, destination) = positions(&display, outgoing);
        input.move_to(source);
        if !outgoing {
            peer.expect_line("peer-entered");
        }
        if selection >= 2 {
            input.button(true);
            input.button(false);
            input.button(true);
            input.button(false);
            app.expect_line("widget-selection-ready");
        }
        input.button(true);
        if !outgoing {
            peer.expect_line("peer-pressed");
        }
        if selection == 3 {
            std::fs::write(display.runtime.path().join("retire-selection"), b"retire").unwrap();
            app.expect_line("widget-selection-retired");
        }
        if !outgoing || prepared || data {
            input.move_to((source.0 + 24, source.1));
        }
        if !outgoing {
            peer.expect_line("peer-started");
        }
        if prepared {
            app.expect_line("widget-prepared");
        }
        input.move_to(destination);
        std::thread::sleep(Duration::from_millis(150));
        input.button(false);
        if selection == 3 {
            let deadline = std::time::Instant::now() + Duration::from_millis(400);
            loop {
                match peer
                    .lines
                    .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                {
                    Ok(line) => assert_ne!(
                        line, "peer-received",
                        "a retired document selection must not be exported"
                    ),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                    Err(error) => panic!("GTK receiver stopped during refused selection: {error}"),
                }
            }
        } else if protected {
            app.expect_line("native-refused");
            assert_no_terminal(&peer, Duration::from_millis(300));
        } else if outgoing {
            peer.expect_line("peer-received");
            if prepared {
                assert!(exported.exists(), "export survives destination processing");
            }
            assert_no_terminal(&app, Duration::from_millis(300));
            std::fs::write(display.runtime.path().join("commit-peer-drop"), b"commit").unwrap();
            peer.expect_line("peer-committed");
            if selection == 0 {
                app.expect_line("native-finished copy");
            }
            if selection == 2 {
                input.move_to(source);
                input.button(true);
                input.button(false);
                app.expect_line("widget-selection-cleared");
            }
            assert!(!exported.exists(), "completed export is retired");
        } else {
            app.expect_line("widget-received");
            assert_no_terminal(&peer, Duration::from_millis(300));
            std::fs::write(display.runtime.path().join("commit-native-drop"), b"commit").unwrap();
            app.expect_line("widget-committed");
            peer.expect_line("peer-ended");
        }
    }
}

#[derive(Debug, Clone)]
enum Message {
    Tick,
    Dropped(FileDrop),
    Finished(DragOutcome),
    Click,
    Selection(Option<enzui_widgets::selection_scope::ScopeSelection>),
}

struct App {
    outgoing: bool,
    selection: u8,
    glyphs: enzui_core::glyph_selection::GlyphText,
    prepared: bool,
    data: bool,
    protected: bool,
    ready: bool,
    retired: u8,
    received: Option<FileDrop>,
}

impl App {
    fn new() -> Self {
        Self {
            outgoing: std::env::var(super::super::MODE).unwrap().ends_with("send"),
            selection: if std::env::var(super::super::MODE)
                .unwrap()
                .contains("-glyph-")
            {
                1
            } else if std::env::var(super::super::MODE)
                .unwrap()
                .contains("-scope-")
            {
                2
            } else {
                0
            },
            glyphs: enzui_core::glyph_selection::GlyphText::new(
                "nativefromenzui"
                    .chars()
                    .enumerate()
                    .map(|(index, ch)| {
                        enzui_core::glyph_selection::Glyph::new(
                            ch,
                            [20.0 + index as f32 * 14.0, 110.0, 14.0, 20.0],
                            0,
                        )
                    })
                    .collect(),
            ),
            protected: std::env::var(super::super::MODE)
                .unwrap()
                .contains("-protected-"),
            data: std::env::var(super::super::MODE)
                .unwrap()
                .contains("-data-"),
            prepared: std::env::var(super::super::MODE)
                .unwrap()
                .contains("prepared"),
            ready: false,
            retired: 0,
            received: None,
        }
    }
    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                if self.selection == 2
                    && std::env::var(super::super::MODE)
                        .unwrap()
                        .contains("-retired-")
                    && PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
                        .join("retire-selection")
                        .exists()
                {
                    if self.retired == 0 {
                        self.retired = 1;
                    } else if self.retired == 1 {
                        self.retired = 2;
                        report("widget-selection-retired");
                    }
                }
                if !self.ready {
                    self.ready = true;
                    report("widget-ready");
                }
                if self.received.is_some()
                    && std::env::var_os("XDG_RUNTIME_DIR")
                        .is_some_and(|dir| PathBuf::from(dir).join("commit-native-drop").exists())
                {
                    self.received.take().unwrap().finish();
                    report("widget-committed");
                }
            }
            Message::Dropped(drop) => {
                assert!(
                    drop.is_temporary(),
                    "native offer metadata must reach the deferred receiver"
                );
                assert_eq!(drop.paths(), [PathBuf::from("/tmp/native-from-gtk")]);
                assert_eq!(
                    drop.action(),
                    Some(enzui_widgets::file_drop::DragAction::Copy)
                );
                self.received = Some(drop);
                report("widget-received");
            }
            Message::Finished(outcome) => {
                if self.protected {
                    assert_eq!(outcome, DragOutcome::Cancelled);
                    report("native-refused");
                    return Task::none();
                }
                assert_eq!(
                    outcome,
                    DragOutcome::Finished(enzui_widgets::file_drop::DragAction::Copy)
                );
                report("native-finished copy");
            }
            Message::Selection(selection) => {
                if selection.is_some_and(|selection| selection.text == "nativefromenzui") {
                    report("widget-selection-ready");
                } else {
                    report("widget-selection-cleared");
                }
            }
            Message::Click => panic!("native handoff must cancel the source button click"),
        }
        Task::none()
    }
    fn view(&self) -> Element<'_, Message> {
        if self.selection > 0 {
            let content = iced::widget::container(
                enzui_widgets::text::enz_text("nativefromenzui").size_px(18.0),
            )
            .center(Length::Fill);
            return if self.selection == 1 {
                enzui_widgets::glyph_selection::enz_glyph_selection(content, &self.glyphs)
                    .selection(Some(0..self.glyphs.glyphs().len()))
                    .on_clear(Message::Click)
                    .into()
            } else {
                enzui_widgets::selection_scope::enz_selection_scope(content)
                    .clear_generation(u64::from(self.retired > 0))
                    .on_selection_change(Message::Selection)
                    .into()
            };
        }
        if self.outgoing {
            let content = iced::widget::button("Drag files")
                .width(Length::Fill)
                .height(Length::Fill)
                .on_press(Message::Click);
            let source = if self.data {
                let payload = if self.protected {
                    use enzui_clipboard::deferred::{DeferredTextLimits, DeferredTextSession};
                    let (mut payload, fulfiller) = DeferredTextSession::default()
                        .prepare(DeferredTextLimits {
                            max_bytes: 128,
                            lifetime: Duration::from_secs(30),
                        })
                        .unwrap();
                    // Replacing the aliases must not strip selection-only custody.
                    for mime in payload.mime_types() {
                        payload = payload.with(mime, b"native-from-enzui".as_slice());
                    }
                    drop(fulfiller);
                    payload
                } else {
                    Payload::text("native-from-enzui")
                };
                enz_data_drag_source(content, "native-data-owner", payload)
            } else if self.prepared {
                enz_file_drag_source_with(content, |cancel| {
                    assert!(!cancel.is_terminal());
                    let path = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
                        .join("prepared-file");
                    std::fs::write(&path, b"owned attachment export").unwrap();
                    report("widget-prepared");
                    Ok(PreparedFiles::owned(vec![path.clone()], Export(path)))
                })
            } else {
                enz_file_drag_source(content, [PathBuf::from("/tmp/native-from-enzui")])
                    .with_data("application/x-enzui-row", b"stable-row".as_slice())
            };
            let source: Element<'_, Message> = source
                .allows_captured_press(true)
                .on_finished(Message::Finished)
                .into();
            iced::widget::scrollable(
                iced::widget::container(source)
                    .height(400)
                    .width(Length::Fill),
            )
            .height(Length::Fill)
            .into()
        } else {
            enz_file_drop_target_with_receipt(
                iced::widget::container("Drop files")
                    .width(Length::Fill)
                    .height(Length::Fill),
                Message::Dropped,
            )
            .copy_only(true)
            .into()
        }
    }
}

struct Export(PathBuf);
impl Drop for Export {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

pub(super) fn application() -> ! {
    enzui_widgets::app_setup::run(
        iced::application(App::new, App::update, App::view)
            .subscription(|_| iced::time::every(Duration::from_millis(20)).map(|_| Message::Tick))
            .title("native-enzui")
            .window_size((240.0, 240.0))
            .decorations(false),
    )
    .unwrap();
    std::process::exit(0)
}
