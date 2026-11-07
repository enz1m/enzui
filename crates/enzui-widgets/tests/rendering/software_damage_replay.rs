//! Frames the software renderer presents into aged buffers, drawing only the
//! damage since the frame a buffer last held, match a full redraw.
//!
//! The compositor's present loop is replayed exactly: buffers rotate, each
//! knows how many presents ago it was shown, damage is diffed against the
//! frame recorded for that age, and only the damage is drawn.

use std::collections::VecDeque;

use enzui_widgets::button::{ButtonKind, enz_button};
use enzui_widgets::switch::switch;
use enzui_widgets::text_field::enz_text_field;
use iced::advanced::widget::{Id, Operation};
use iced::widget::{column, text};
use iced::{Color, Element, Rectangle, Size};
use iced_graphics::Viewport;
use iced_test::runtime::user_interface::{Cache, UserInterface};
use iced_tiny_skia::Layer;

const SIZE: Size<u32> = Size::new(400, 300);
const BG: Color = Color::from_rgb(0.1, 0.1, 0.12);

#[derive(Debug, Clone)]
enum Msg {
    Noop,
}

fn card<'a>(title: &'a str, rows: usize, disabled: bool) -> Element<'a, Msg> {
    let mut body = column![text(title).size(18)].spacing(8);
    for row in 0..rows {
        body = body.push(text(format!("{title} detail {row}")));
        body = body.push(enz_text_field("Name", "", "Alex", |_| Msg::Noop));
        let button = enz_button("Change picture…", Msg::Noop).kind(ButtonKind::Secondary);
        body = body.push(if disabled {
            button.enabled(false)
        } else {
            button
        });
        body = body.push(switch(row % 2 == 0, |_| Msg::Noop));
    }
    enzui_widgets::card::enz_card(body).into()
}

/// Draws its child inside a group: an alpha group, as a reveal or fade
/// does, or a rounded clip over a translucent fill, as a settings card does.
struct Fade<'a> {
    child: Element<'a, Msg>,
    alpha: f32,
    rounded: bool,
}

impl iced::advanced::Widget<Msg, iced::Theme, iced::Renderer> for Fade<'_> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<iced::Length> {
        self.child.as_widget().size()
    }
    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        if self.rounded {
            renderer.start_layer_rounded(bounds, iced::border::Radius::new(12.0));
            renderer.fill_quad(
                iced::advanced::renderer::Quad {
                    bounds,
                    ..Default::default()
                },
                Color::from_rgba(1.0, 1.0, 1.0, 0.06),
            );
            self.child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
            renderer.end_layer();
            return;
        }
        renderer.with_layer_alpha(bounds, self.alpha, |renderer| {
            self.child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        });
    }
}

fn page<'a>(expanded: Option<f32>, tick: usize) -> Element<'a, Msg> {
    // A rounded card whose one label changes every frame: small damage.
    let accounts = Element::new(Fade {
        child: column![
            text("Accounts"),
            text(format!("Selected {}", tick % 3)),
            text("Alex Morgan")
        ]
        .spacing(8)
        .padding(12)
        .into(),
        alpha: 1.0,
        rounded: true,
    });
    let mut cards = column![accounts, card("Your profile", 2, true)].spacing(16);
    if let Some(alpha) = expanded {
        cards = cards.push(Element::new(Fade {
            child: card("Add a local user", 3, false),
            alpha,
            rounded: false,
        }));
    }
    cards = cards.push(card("Add user", 1, false));
    enzui_widgets::scroll_view::enz_scroll_view(cards.padding(12))
        .id(Id::new("page"))
        .into()
}

struct Surface {
    buffers: Vec<Vec<u32>>,
    /// Presents since each buffer was last shown; 0 before its first.
    ages: Vec<u8>,
    next: usize,
    frames: VecDeque<(Color, Vec<Layer>)>,
    max_age: u8,
    clip_mask: tiny_skia::Mask,
}

fn software(renderer: &mut iced::Renderer) -> &mut iced_tiny_skia::Renderer {
    match renderer {
        iced::Renderer::Secondary(software) => software,
        iced::Renderer::Primary(_) => unreachable!("asked for tiny-skia"),
    }
}

fn draw(
    renderer: &mut iced_tiny_skia::Renderer,
    buffer: &mut [u32],
    mask: &mut tiny_skia::Mask,
    damage: &[Rectangle],
) {
    let viewport = Viewport::with_physical_size(SIZE, Default::default());
    renderer.draw(
        &mut tiny_skia::PixmapMut::from_bytes(
            bytemuck::cast_slice_mut(buffer),
            SIZE.width,
            SIZE.height,
        )
        .expect("pixmap"),
        mask,
        &viewport,
        damage,
        BG,
    );
}

impl Surface {
    fn new(count: usize) -> Self {
        Self {
            buffers: vec![vec![0; (SIZE.width * SIZE.height) as usize]; count],
            ages: vec![0; count],
            next: 0,
            frames: VecDeque::new(),
            max_age: 0,
            clip_mask: tiny_skia::Mask::new(SIZE.width, SIZE.height).expect("mask"),
        }
    }

    /// Presents the renderer's current layers the way the compositor does,
    /// returning the buffer it drew into.
    fn present(&mut self, renderer: &mut iced_tiny_skia::Renderer) -> Vec<u32> {
        let index = self.next;
        self.next = (self.next + 1) % self.buffers.len();
        let age = self.ages[index];
        self.max_age = self.max_age.max(age);
        self.frames.truncate(self.max_age as usize);
        let last = (age > 0)
            .then(|| self.frames.get(age as usize - 1))
            .flatten()
            .cloned();
        let full = Rectangle::with_size(Size::new(SIZE.width as f32, SIZE.height as f32));
        let current = renderer.layers().to_vec();
        let damage = last
            .as_ref()
            .and_then(|(background, layers)| {
                (*background == BG).then(|| {
                    iced_graphics::damage::diff(
                        layers,
                        &current,
                        |layer| vec![layer.bounds],
                        Layer::damage,
                    )
                })
            })
            .unwrap_or_else(|| vec![full]);
        if damage.is_empty() {
            if let Some(last) = last {
                self.frames.push_front(last);
            }
        } else {
            self.frames.push_front((BG, current));
            let damage = iced_graphics::damage::group(damage, full);
            draw(
                renderer,
                &mut self.buffers[index],
                &mut self.clip_mask,
                &damage,
            );
        }
        for (i, age) in self.ages.iter_mut().enumerate() {
            if i == index {
                *age = 1;
            } else if *age > 0 {
                *age += 1;
            }
        }
        self.buffers[index].clone()
    }
}

fn full_draw(renderer: &mut iced_tiny_skia::Renderer) -> Vec<u32> {
    let mut buffer = vec![0; (SIZE.width * SIZE.height) as usize];
    let mut mask = tiny_skia::Mask::new(SIZE.width, SIZE.height).expect("mask");
    let full = Rectangle::with_size(Size::new(SIZE.width as f32, SIZE.height as f32));
    draw(renderer, &mut buffer, &mut mask, &[full]);
    buffer
}

fn new_renderer() -> iced::Renderer {
    iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("tiny-skia"),
        ),
    )
    .expect("software renderer")
}

fn scrolled(
    ui: &mut UserInterface<'_, Msg, iced::Theme, iced::Renderer>,
    renderer: &iced::Renderer,
    offset: f32,
) {
    use iced::advanced::widget::operation::scrollable as scroll_op;
    let mut scroll: Box<dyn Operation<()>> = Box::new(scroll_op::scroll_to(
        Id::new("page"),
        scroll_op::AbsoluteOffset {
            x: None,
            y: Some(offset),
        },
        iced::advanced::widget::operation::Animation::Instant,
    ));
    ui.operate(renderer, scroll.as_mut());
}

/// The frame a new window would draw at this state: no carried state.
fn fresh(expanded: Option<f32>, tick: usize, offset: f32) -> Vec<u32> {
    let mut renderer = new_renderer();
    let mut ui = UserInterface::build(
        page(expanded, tick),
        Size::new(SIZE.width as f32, SIZE.height as f32),
        Cache::default(),
        &mut renderer,
    );
    scrolled(&mut ui, &renderer, offset);
    ui.draw(
        &mut renderer,
        &iced::Theme::Dark,
        &iced::advanced::renderer::Style {
            text_color: Color::WHITE,
        },
        iced::mouse::Cursor::Unavailable,
    );
    full_draw(software(&mut renderer))
}

#[test]
fn partial_redraws_into_aged_buffers_match_full_redraws() {
    for buffer_count in [1, 2, 3] {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("tiny-skia"),
            ),
        )
        .expect("software renderer");
        let mut surface = Surface::new(buffer_count);
        let mut cache = Cache::default();
        // (expanded, scroll offset) per frame: scroll, expand, scroll down, back up.
        // (reveal, scroll offset) per frame: scroll, reveal over three
        // frames while scrolling it into view, scroll down, back up.
        let frames = [
            (None, 0.0),
            (None, 0.0),
            (None, 0.0),
            (None, 0.0),
            (None, 120.0),
            (Some(0.2), 160.0),
            (Some(0.6), 260.0),
            (Some(1.0), 340.0),
            (Some(1.0), 420.0),
            (Some(1.0), 60.0),
            (Some(1.0), 0.0),
        ];
        for (step, (expanded, offset)) in frames.into_iter().enumerate() {
            let mut ui = UserInterface::build(
                page(expanded, step),
                Size::new(SIZE.width as f32, SIZE.height as f32),
                cache,
                &mut renderer,
            );
            use iced::advanced::widget::operation::scrollable as scroll_op;
            let mut scroll: Box<dyn Operation<()>> = Box::new(scroll_op::scroll_to(
                Id::new("page"),
                scroll_op::AbsoluteOffset {
                    x: None,
                    y: Some(offset),
                },
                iced::advanced::widget::operation::Animation::Instant,
            ));
            ui.operate(&renderer, scroll.as_mut());
            ui.draw(
                &mut renderer,
                &iced::Theme::Dark,
                &iced::advanced::renderer::Style {
                    text_color: Color::WHITE,
                },
                iced::mouse::Cursor::Unavailable,
            );
            cache = ui.into_cache();
            let presented = surface.present(software(&mut renderer));
            let expected = fresh(expanded, step, offset);
            let wrong = presented
                .iter()
                .zip(&expected)
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                wrong, 0,
                "{buffer_count} buffers, frame {step} (expanded {expanded:?}, scrolled {offset}): \
                 {wrong} pixels differ from a full redraw"
            );
        }
    }
}
