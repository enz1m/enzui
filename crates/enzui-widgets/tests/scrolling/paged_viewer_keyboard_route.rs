//! Routed keyboard navigation, document pixels and model source ranges.

use enzui_core::paged_text::{
    MemoryPagedTextSource, NavigateOutcome, PagedTextNavigation, PagedTextPolicy, PagedTextState,
    PagedTextViewport,
};
use enzui_widgets::focus_button::focus_button;
use enzui_widgets::paged_text_viewer::{
    PagedTextViewModel, PagedTextViewportState, enz_paged_text_viewer,
};
use iced::keyboard::key::Named;
use iced::widget::{Space, column, text};
use iced::{Element, Event, Length, Point, Size, keyboard, mouse};
use iced_test::core::{shell, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::Arc;
const SURFACE: Size = Size::new(600.0, 400.0);
const CENTER: Point = Point::new(300.0, 200.0);
const SIBLING_H: f32 = 24.0;
const SIBLING: Point = Point::new(300.0, 12.0);
fn policy() -> PagedTextPolicy {
    PagedTextPolicy {
        eager_cutoff: 32,
        read_chunk_size: 1024,
        max_display_row_bytes: 64,
        cache_budget: 4096,
        overscan_rows: 2,
    }
}
fn dense_fixture(lines: usize) -> Vec<u8> {
    (0..lines)
        .map(|i| format!("line {i:06}\n"))
        .collect::<String>()
        .into_bytes()
}
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum HostShape {
    Stable,
    // Mounting the tail spacer shifts the viewer's ancestor index and loses focus.
    ConditionalTailSpacer,
}
struct Doc {
    state: PagedTextState,
    viewport: PagedTextViewport,
    viewport_state: PagedTextViewportState,
    shape: HostShape,
    scrollbar_tokens: Option<enzui_theme::ScrollbarTokens>,
    backdrop: iced::Color,
    pad_history: std::cell::RefCell<Vec<usize>>,
}

impl Doc {
    fn new(bytes: Vec<u8>, shape: HostShape) -> Self {
        let source = Arc::new(MemoryPagedTextSource::new(Arc::<[u8]>::from(bytes)));
        let mut state = PagedTextState::new(policy());
        let open = state.open(source).expect("fixture source opens");
        assert!(state.apply(open.execute()), "first page applies");
        Self {
            state,
            viewport: PagedTextViewport::new(8),
            viewport_state: PagedTextViewportState::new(),
            shape,
            scrollbar_tokens: None,
            backdrop: iced::Color::BLACK,
            pad_history: std::cell::RefCell::new(Vec::new()),
        }
    }
    fn view(&self) -> Element<'_, PagedTextNavigation> {
        let mut viewer = enz_paged_text_viewer(&self.state, &self.viewport, |nav| nav)
            .label("paged fixture")
            .viewport_state(&self.viewport_state);
        if let Some(tokens) = self.scrollbar_tokens {
            viewer = viewer.scrollbar_tokens(tokens);
        }
        let viewer = viewer.backdrop(self.backdrop).into_element();
        let document: Element<'_, PagedTextNavigation> = match self.shape {
            HostShape::Stable => viewer,
            HostShape::ConditionalTailSpacer => {
                let pad = self.caller_side_pad_rows();
                self.pad_history.borrow_mut().push(pad);
                if pad == 0 {
                    viewer
                } else {
                    column![
                        Space::new().height(Length::Fixed(pad as f32 * 22.0)),
                        viewer
                    ]
                    .into()
                }
            }
        };
        column![
            focus_button(text("sibling"), PagedTextNavigation::Wheel(0))
                .width(Length::Fill)
                .height(Length::Fixed(24.0)),
            document,
        ]
        .into()
    }
    fn caller_side_pad_rows(&self) -> usize {
        let source_len = self.state.source_len();
        if source_len == 0 {
            return 0;
        }
        let chunk = policy().read_chunk_size as u64;
        let page_start = (self.viewport.anchor_byte.min(source_len - 1) / chunk) * chunk;
        if page_start == 0 || page_start.saturating_add(chunk) < source_len {
            return 0;
        }
        let drawn = self.viewport.materialize(&self.state).rows.len();
        let capacity = self.viewport.visible_rows + 2 * policy().overscan_rows;
        capacity.saturating_sub(drawn)
    }
    fn apply(&mut self, movement: PagedTextNavigation) {
        self.viewport
            .navigate(movement, self.state.source_len(), policy().read_chunk_size);
        for _ in 0..8 {
            match self.viewport.resolve(&self.state) {
                NavigateOutcome::Ready => return,
                NavigateOutcome::NeedsFetch(anchor) => {
                    let Some(operation) = self.state.request(anchor) else {
                        return;
                    };
                    if !self.state.apply(operation.execute()) {
                        return;
                    }
                }
            }
        }
        panic!("bounded reads did not settle");
    }
    fn announced(&self) -> String {
        let visible = self
            .viewport_state
            .visible_rows_or(self.viewport.visible_rows);
        PagedTextViewModel::from_state(&self.state, &self.viewport, visible).range_announcement
    }
}
struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    doc: Doc,
}

impl Host {
    fn new(bytes: Vec<u8>, shape: HostShape) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
            doc: Doc::new(bytes, shape),
        }
    }
    // Retaining the UI cache carries document focus across view rebuilds.
    fn frame(&mut self, events: &[Event], cursor: mouse::Cursor) -> Vec<PagedTextNavigation> {
        let mut messages = iced::advanced::shell::Bus::new();
        {
            let Self {
                renderer,
                cache,
                doc,
            } = self;
            let mut ui = UserInterface::build(doc.view(), SURFACE, std::mem::take(cache), renderer);
            let _ = ui.update(
                &window::Headless,
                &shell::Waker::noop(),
                events,
                cursor,
                renderer,
                &mut messages,
            );
            let theme = iced::Theme::Dark;
            let base = <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
            ui.draw(
                renderer,
                &theme,
                &iced::advanced::renderer::Style {
                    text_color: base.text_color,
                },
                cursor,
            );
            *cache = ui.into_cache();
        }
        let published = self
            .doc
            .viewport_state
            .visible_rows_or(self.doc.viewport.visible_rows);
        self.doc.viewport.visible_rows = published.max(1);
        for message in &messages {
            self.doc.apply(*message);
        }
        messages.into_iter().collect()
    }
    fn pixels(&mut self) -> Frame {
        let _ = self.frame(&[], mouse::Cursor::Available(CENTER));
        Frame(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::screenshot(
                &mut self.renderer,
                iced::Size::new(SURFACE.width as u32, SURFACE.height as u32),
                1.0,
                iced::Color::BLACK,
            ),
        )
    }
    // Home at byte zero settles keyboard focus paint without moving the document.
    fn focus_document(&mut self) {
        let _ = self.click(CENTER);
        assert_eq!(
            self.key(Named::Home),
            vec![PagedTextNavigation::Home],
            "the clicked document must own the keyboard"
        );
        assert_eq!(self.doc.viewport.anchor_byte, 0, "the primer must not move");
    }

    fn click(&mut self, at: Point) -> Vec<PagedTextNavigation> {
        let cursor = mouse::Cursor::Available(at);
        self.frame(
            &[
                Event::Mouse(mouse::Event::CursorMoved { position: at }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            cursor,
        )
    }

    fn key(&mut self, named: Named) -> Vec<PagedTextNavigation> {
        let key = keyboard::Key::Named(named);
        self.frame(
            &[
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key.clone(),
                    physical_key: keyboard::key::Physical::Unidentified(
                        keyboard::key::NativeCode::Unidentified,
                    ),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::default(),
                    text: None,
                    repeat: false,
                }),
                Event::Keyboard(keyboard::Event::KeyReleased {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: keyboard::key::Physical::Unidentified(
                        keyboard::key::NativeCode::Unidentified,
                    ),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::default(),
                }),
            ],
            mouse::Cursor::Available(CENTER),
        )
    }

    fn announced(&self) -> String {
        self.doc.announced()
    }
}
#[test]
fn paged_viewer_navigation_follows_focus() {
    let mut host = Host::new(dense_fixture(4_000), HostShape::Stable);
    host.focus_document();
    let top = host.pixels();
    assert_eq!(
        host.key(Named::PageDown),
        vec![PagedTextNavigation::PageDown],
        "the focused document owns Page Down"
    );
    let paged = host.pixels();
    assert_ne!(paged, top);
    let paged_body = paged.body();
    let paged_range = host.announced();

    assert_eq!(
        host.click(SIBLING),
        vec![PagedTextNavigation::Wheel(0)],
        "the sibling button activates and takes focus"
    );
    assert_eq!(
        host.key(Named::PageDown),
        Vec::new(),
        "a document that does not own focus must not eat navigation keys"
    );
    assert_eq!(
        host.pixels().body(),
        paged_body,
        "the unfocused document must not move (its focus ring may)"
    );
    assert_eq!(
        host.announced(),
        paged_range,
        "and it must still announce the range it was left at"
    );

    let _ = host.click(CENTER);
    assert_eq!(
        host.key(Named::Home),
        vec![PagedTextNavigation::Home],
        "clicking the document must give the keys back to it"
    );
    assert_eq!(
        host.pixels().body(),
        top.body(),
        "and Home must return to the top"
    );
}
#[derive(Clone, PartialEq, Eq)]
struct Frame(Vec<u8>);

impl Frame {
    // Exclude sibling and focus chrome so comparisons isolate document movement.
    fn body(&self) -> Vec<u8> {
        let width = SURFACE.width as usize;
        let mut out = Vec::new();
        let start = SIBLING_H as usize + 4;
        let end = SURFACE.height as usize - 4;
        for y in start..end {
            let row = (y * width + 72) * 4;
            out.extend_from_slice(&self.0[row..row + (width - 80) * 4]);
        }
        out
    }
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in &self.0 {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        write!(f, "frame#{hash:016x}({} bytes)", self.0.len())
    }
}
