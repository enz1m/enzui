use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_core::theme_file::ThemeDocument;
use enzui_theme::{EnzTheme, EnzThemeMode};
use enzui_widgets::button::{ButtonKind, ButtonSize, ButtonTokens, enz_button};
use enzui_widgets::button_group::{EnzButtonGroupItem, enz_button_group};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::{
    Id, Operation,
    operation::{self, Outcome, focusable},
};
use iced::widget::container;
use iced::{Color, Element, Event, Length, Rectangle, Size, keyboard, mouse};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 140;
const RADII: [f32; 5] = [7.0, 9.0, 11.0, 14.0, 18.0];

#[derive(Clone, Copy, Debug)]
enum Subject {
    Button(Option<ButtonSize>),
    Group(usize),
}

#[derive(Clone, Copy, Debug)]
struct Config {
    subject: Subject,
    pin: Option<f32>,
    palette_pin: bool,
    enabled: bool,
    selected: bool,
    kind: Option<ButtonKind>,
}
impl Config {
    fn new(subject: Subject) -> Self {
        Self {
            subject,
            pin: None,
            palette_pin: false,
            enabled: true,
            selected: false,
            kind: None,
        }
    }
}

fn theme(radii: [f32; 5], mode: EnzThemeMode) -> EnzTheme {
    let doc = ThemeDocument::parse(&format!(
        "version = 1\n[shape]\nbutton = \"md\"\nbutton_group = \"md\"\n\
         [radius]\nxs = {}\nsm = {}\nmd = {}\nlg = {}\nxl = {}\n",
        radii[0], radii[1], radii[2], radii[3], radii[4]
    ))
    .unwrap();
    assert!(doc.diagnostics().is_empty());
    doc.resolve(mode)
}

fn view(config: Config) -> Element<'static, usize> {
    let control: Element<'static, usize> = match config.subject {
        Subject::Button(size) => {
            let mut button = enz_button("Action", 0)
                .id(Id::new("button"))
                .width(Length::Fixed(180.0))
                .enabled(config.enabled);
            if let Some(size) = size {
                button = button.size(size);
            }
            if let Some(kind) = config.kind {
                button = button.kind(kind);
            }
            if let Some(radius) = config.pin {
                let mut tokens = size.unwrap_or_default().tokens();
                tokens.radius = radius;
                button = button.size_tokens(tokens).size(ButtonSize::Xl);
            }
            if config.palette_pin {
                button = button.tokens(ButtonTokens::from_palette(&EnzTheme::dark().palette));
            }
            button.into()
        }
        Subject::Group(count) => {
            let items = (0..count)
                .map(|i| EnzButtonGroupItem::new(format!("cell-{i}"), format!("Choice {i}")))
                .collect();
            let mut group = enz_button_group(items, |i| i)
                .id_prefix("group")
                .group_enabled(config.enabled)
                .selectable(config.selected)
                .selected(config.selected.then_some(0))
                .block(true);
            if let Some(kind) = config.kind {
                group = group.kind(kind);
            }
            if let Some(radius) = config.pin {
                group = group.corner_radius(radius);
            }
            container(group).width(320.0).into()
        }
    };
    container(control).padding(12).into()
}

struct Host {
    ui: UserInterface<'static, usize, iced::Theme, Renderer>,
    renderer: Renderer,
    buttons: Vec<CollectedNode>,
    bounds: Rectangle,
    cursor: mouse::Cursor,
}
impl Host {
    fn new(config: Config) -> Self {
        let mut renderer = crate::honest_frame_driver::new_renderer();
        let mut ui = UserInterface::build(
            view(config),
            Size::new(WIDTH as f32, HEIGHT as f32),
            Cache::default(),
            &mut renderer,
        );
        let buttons = collect(&mut ui, &renderer);
        let count = match config.subject {
            Subject::Button(_) => 1,
            Subject::Group(count) => count,
        };
        assert_eq!(buttons.len(), count);
        for node in &buttons {
            assert!(node.id.is_some());
            assert!(node.bounds.width > 50.0 && node.bounds.height >= 22.0);
        }
        let bounds = buttons
            .iter()
            .skip(1)
            .fold(buttons[0].bounds, |b, n| b.union(&n.bounds));
        assert!(bounds.x >= 8.0 && bounds.y >= 8.0 && bounds.x + bounds.width < WIDTH as f32 - 8.0);
        assert!(bounds.y + bounds.height < HEIGHT as f32 - 8.0);
        Self {
            ui,
            renderer,
            buttons,
            bounds,
            cursor: mouse::Cursor::Unavailable,
        }
    }
    fn send(&mut self, events: &[Event]) -> Vec<usize> {
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = self.ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            self.cursor,
            &mut self.renderer,
            &mut messages,
        );
        messages.into_iter().collect()
    }
    fn draw(&mut self, theme: &EnzTheme) -> Vec<u8> {
        assert!(
            self.send(&[Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now()
            ))])
            .is_empty()
        );
        self.ui.draw(
            &mut self.renderer,
            &theme.iced_theme(),
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            self.cursor,
        );
        let nodes = collect(&mut self.ui, &self.renderer);
        assert_eq!(nodes.len(), self.buttons.len());
        for (a, b) in nodes.iter().zip(&self.buttons) {
            assert_eq!(a.id, b.id);
            assert_eq!(a.bounds, b.bounds);
        }
        self.renderer
            .screenshot(Size::new(WIDTH, HEIGHT), 1.0, Color::BLACK)
    }
    fn hover(&mut self, index: usize) {
        let position = self.buttons[index].bounds.center();
        self.cursor = mouse::Cursor::Available(position);
        assert!(
            self.send(&[Event::Mouse(mouse::Event::CursorMoved { position })])
                .is_empty()
        );
    }
    fn focus(&mut self, index: usize) {
        self.cursor = mouse::Cursor::Unavailable;
        let id = self.buttons[index].id.clone().unwrap();
        self.ui
            .operate(&self.renderer, &mut focusable::focus::<()>(id.clone()));
        let mut find = focusable::find_focused();
        self.ui
            .operate(&self.renderer, &mut operation::black_box(&mut find));
        assert!(matches!(find.finish(), Outcome::Some(found) if found == id));
        assert!(self.send(&[key(keyboard::key::Named::Tab)]).is_empty());
    }
}

fn key(named: keyboard::key::Named) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        modified_key: keyboard::Key::Named(named),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: keyboard::Modifiers::empty(),
        text: None,
        repeat: false,
    })
}

fn collect(
    ui: &mut UserInterface<'_, usize, iced::Theme, Renderer>,
    renderer: &Renderer,
) -> Vec<CollectedNode> {
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector);
    collector
        .into_collected()
        .into_iter()
        .filter(|n| matches!(n.semantics.role, Role::Button | Role::Tab))
        .collect()
}

fn equal(a: &[u8], b: &[u8], context: impl std::fmt::Debug) {
    assert_eq!(a.len(), b.len());
    let changed = a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(changed, 0, "{context:?}");
}

fn changed_at_corners(a: &[u8], b: &[u8], bounds: Rectangle, radius: f32) -> usize {
    let mut changed = 0;
    let area = radius.ceil() + 5.0;
    for (i, (a, b)) in a
        .as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0.iter())
        .enumerate()
    {
        if a == b {
            continue;
        }
        let x = (i as u32 % WIDTH) as f32 + 0.5;
        let y = (i as u32 / WIDTH) as f32 + 0.5;
        assert!(
            x >= bounds.x - 4.0
                && x <= bounds.x + bounds.width + 4.0
                && y >= bounds.y - 4.0
                && y <= bounds.y + bounds.height + 4.0
        );
        assert!(
            (x < bounds.x + area || x > bounds.x + bounds.width - area)
                && (y < bounds.y + area || y > bounds.y + bounds.height - area),
            "non-corner pixel changed at {x},{y}"
        );
        changed += 1;
    }
    let maximum = (4.0 * area * area) as usize;
    assert!(
        changed > 12 && changed < maximum,
        "{changed} changed pixels; upper bound {maximum}"
    );
    changed
}

#[test]
fn focus_rings_and_real_activation_survive_retheming() {
    for subject in [Subject::Button(None), Subject::Group(3)] {
        let config = Config::new(subject);
        let square = theme([0.0; 5], EnzThemeMode::Dark);
        let round = theme(RADII, EnzThemeMode::Dark);
        let mut host = Host::new(config);
        let rest = host.draw(&square);
        host.focus(0);
        let focused = host.draw(&square);
        let b = host.buttons[0].bounds;
        for (edge, length) in [
            (Rectangle { height: 3.0, ..b }, b.width),
            (
                Rectangle {
                    y: b.y + b.height - 3.0,
                    height: 3.0,
                    ..b
                },
                b.width,
            ),
            (Rectangle { width: 3.0, ..b }, b.height),
            (
                Rectangle {
                    x: b.x + b.width - 3.0,
                    width: 3.0,
                    ..b
                },
                b.height,
            ),
        ] {
            let changed = rest
                .as_chunks::<4>()
                .0
                .iter()
                .zip(focused.as_chunks::<4>().0.iter())
                .enumerate()
                .filter(|(i, (a, b))| {
                    a != b
                        && edge.contains(iced::Point::new(
                            (*i as u32 % WIDTH) as f32 + 0.5,
                            (*i as u32 / WIDTH) as f32 + 0.5,
                        ))
                })
                .count();
            assert!(
                changed as f32 >= length - 8.0,
                "{subject:?}: focus edge {edge:?}: only {changed} changed pixels"
            );
        }
        equal(&focused, &host.draw(&square), "same focused input");
        let after = host.draw(&round);
        changed_at_corners(&focused, &after, host.bounds, RADII[2]);
        let mut pinned = Host::new(Config {
            pin: Some(RADII[2]),
            ..config
        });
        pinned.draw(&square);
        pinned.focus(0);
        equal(&after, &pinned.draw(&square), (subject, "focus shape"));
        assert_eq!(host.send(&[key(keyboard::key::Named::Enter)]), vec![0]);
        for index in 0..host.buttons.len() {
            host.hover(index);
            assert!(
                host.send(&[Event::Mouse(mouse::Event::ButtonPressed(
                    mouse::Button::Left
                ))])
                .is_empty()
            );
            assert_eq!(
                host.send(&[Event::Mouse(mouse::Event::ButtonReleased(
                    mouse::Button::Left
                ))]),
                vec![index]
            );
        }
        let mut disabled = Host::new(Config {
            enabled: false,
            ..config
        });
        disabled.draw(&round);
        disabled.hover(0);
        assert!(
            disabled
                .send(&[
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                    key(keyboard::key::Named::Enter)
                ])
                .is_empty()
        );
    }
}
