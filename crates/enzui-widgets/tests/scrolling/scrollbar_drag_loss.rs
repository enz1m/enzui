use enzui_core::table::{ColumnKey, TableProvider};
use enzui_core::virtual_sequence::{
    HeightModel, ItemProvider, KeyedProvider, SequenceKey, VariableHeightModel,
};
use enzui_widgets::enz_table;
use iced::advanced::widget::{Operation, Tree, Widget, tree};
use iced::advanced::{Layout, Shell, layout, mouse, overlay, renderer};
use iced::widget::{text, text_editor};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector, window};
use std::cell::Cell;
use std::rc::Rc;

const W: f32 = 320.0;
const H: f32 = 240.0;
const PRESS: Point = Point::new(314.0, 12.0);
const AWAY: Point = Point::new(20.0, 66.0);
const FAR: Point = Point::new(20.0, 220.0);

#[derive(Debug, Clone)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Offset(f32),
    Editor(text_editor::Action),
    Table(f32),
}

#[derive(Default)]
struct LossCounts {
    left: Cell<u32>,
    unfocused: Cell<u32>,
}
struct LossProbe<'a> {
    child: Element<'a, Msg>,
    counts: Rc<LossCounts>,
}

impl Widget<Msg, Theme, iced::Renderer> for LossProbe<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Msg>,
        viewport: &Rectangle,
    ) {
        match event {
            Event::Mouse(mouse::Event::CursorLeft) => {
                self.counts.left.set(self.counts.left.get() + 1);
            }
            Event::Window(window::Event::Unfocused) => {
                self.counts.unfocused.set(self.counts.unfocused.get() + 1);
            }
            _ => {}
        }
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Msg, Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a> From<LossProbe<'a>> for Element<'a, Msg> {
    fn from(value: LossProbe<'a>) -> Self {
        Element::new(value)
    }
}

fn probe<'a>(child: impl Into<Element<'a, Msg>>, counts: Rc<LossCounts>) -> Element<'a, Msg> {
    LossProbe {
        child: child.into(),
        counts,
    }
    .into()
}

struct Grid {
    rows: u64,
}

impl ItemProvider<SequenceKey> for Grid {
    fn len(&self) -> u64 {
        self.rows
    }

    fn item(&self, index: u64) -> Option<SequenceKey> {
        (index < self.rows).then(|| SequenceKey::new(format!("row-{index}")))
    }
}

impl KeyedProvider<SequenceKey> for Grid {
    fn key(&self, index: u64) -> Option<SequenceKey> {
        self.item(index)
    }
}

impl TableProvider<String> for Grid {
    fn column_count(&self) -> usize {
        3
    }

    fn column_key(&self, column: usize) -> Option<ColumnKey> {
        (column < 3).then(|| ColumnKey::new(format!("col-{column}")))
    }

    fn cell(&self, row: u64, column: usize) -> Option<String> {
        (row < self.rows && column < 3).then(|| format!("r{row}c{column}"))
    }
}

fn table_widget(counts: Rc<LossCounts>, rows: u64) -> Element<'static, Msg> {
    let provider: &'static Grid = Box::leak(Box::new(Grid { rows }));
    enz_table(
        provider,
        HeightModel::new(rows as usize, 24.0),
        VariableHeightModel::uniform(3, 100.0),
        move |_row, _column, value: &String| {
            probe(text(value.clone()).size(10), Rc::clone(&counts))
        },
    )
    .width(Length::Fixed(W))
    .height(Length::Fixed(H))
    .header_height(0.0)
    .row_overscan(0)
    .column_overscan(0)
    .scroll_y(0.0)
    .on_scroll_to(Msg::Table)
    .into_element()
}

fn left() -> Event {
    Event::Mouse(mouse::Event::CursorLeft)
}

fn unfocused() -> Event {
    Event::Window(window::Event::Unfocused)
}

fn press() -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
}

fn release() -> Event {
    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
}

fn moved(point: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position: point })
}

fn offsets(messages: impl Iterator<Item = Msg>, owner: &str) -> Vec<f32> {
    messages
        .filter_map(|message| match (owner, message) {
            ("offset", Msg::Offset(value)) | ("table", Msg::Table(value)) => Some(value),
            ("editor", Msg::Editor(text_editor::Action::ScrollTo { offset })) => Some(offset.y),
            _ => None,
        })
        .collect()
}

fn assert_loss_contract<'a>(
    element: Element<'a, Msg>,
    owner: &str,
    press_at: Point,
    loss: Event,
    counts: &LossCounts,
) {
    // Published offsets stay unapplied so a fresh press exposes any local offset mutation.
    let mut ui = iced_test::Simulator::with_size(
        iced_test::core::Settings::default(),
        Size::new(W, H),
        element,
    );
    // Drawing warms editor metrics and latches runtime scrollbar hit geometry.
    let _ = ui.snapshot(&Theme::Dark).expect("initial frame");

    ui.point_at(press_at);
    assert_eq!(ui.simulate([press()]), vec![iced::event::Status::Captured]);
    ui.point_at(AWAY);
    assert_eq!(
        ui.simulate([moved(AWAY)]),
        vec![iced::event::Status::Captured],
        "{owner}: an out-of-lane move inside the window must continue the drag"
    );
    let loss_status = ui.simulate([loss]);
    assert_eq!(
        loss_status,
        vec![iced::event::Status::Ignored],
        "{owner}: the scrollbar must not capture pointer/window loss"
    );
    ui.point_at(FAR);
    assert_eq!(
        ui.simulate([moved(FAR)]),
        vec![iced::event::Status::Ignored],
        "{owner}: a post-loss move must not be captured"
    );
    ui.point_at(press_at);
    assert_eq!(ui.simulate([press()]), vec![iced::event::Status::Captured]);
    let fresh_target = Point::new(AWAY.x, 105.0);
    ui.point_at(fresh_target);
    assert_eq!(
        ui.simulate([moved(fresh_target)]),
        vec![iced::event::Status::Captured]
    );
    assert_eq!(
        ui.simulate([release()]),
        vec![iced::event::Status::Captured]
    );

    let values = offsets(ui.into_messages(), owner);
    assert_eq!(
        values.len(),
        4,
        "{owner}: press+move before loss and fresh press+move are the only publishes: {values:?}"
    );
    assert!(
        values[1] > values[0] && values[3] > values[2],
        "{owner}: both the pre-loss and freshly armed drags must change the absolute offset: {values:?}"
    );
    assert!(
        (values[2] - values[0]).abs() < 0.01,
        "{owner}: loss must neither mutate the caller-owned offset nor publish a snap-back: {values:?}"
    );
    match owner {
        "offset" | "editor" => {
            assert_eq!(counts.left.get() + counts.unfocused.get(), 1);
        }
        "table" => assert!(counts.left.get() + counts.unfocused.get() > 0),
        _ => unreachable!(),
    }
}

#[test]
fn public_table_cancels_on_each_mouse_loss_and_fresh_drag_still_works() {
    for loss in [left(), unfocused()] {
        let counts = Rc::new(LossCounts::default());
        assert_loss_contract(
            table_widget(Rc::clone(&counts), 100),
            "table",
            PRESS,
            loss,
            &counts,
        );
    }
}
