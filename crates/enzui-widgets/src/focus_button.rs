//! Button primitive with pointer, touch, keyboard and routed activation.
//!
//! Standalone enabled buttons join the focus ring. Enter activates on press;
//! Space activates on release. Embedded buttons delegate keyboard and focus
//! registration to their owner while retaining pointer and routed activation.
//!
//! Attach semantics and a stable ID for routed accessibility actions. Styles
//! receive [`FocusStatus`]; hover fill and a focus-visible ring can coexist.
//! The neutral default uses live-theme ink and hover/press washes.
//!
//! Transparent controls cannot inspect ancestor paint. Supply
//! [`FocusButton::disabled_on`] when the composing caller owns an opaque
//! backdrop; default-ring colour assumes the palette page background.

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, PressFocus, activation_from_key, press_focus_from_cursor};
use enzui_core::semantics::Semantics;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::widget::button::{Catalog, Style, StyleFn};
use iced::{
    Background, Border, Color, Element, Event, Length, Padding, Rectangle, Size, keyboard, mouse,
    touch, window,
};
mod builder;
mod ring;
mod status;
mod widget;
pub(crate) use widget::inset_background_bounds;

pub use builder::*;
pub use ring::*;
pub use status::*;

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::layout::Measure as _;
    use iced::widget::text;

    #[derive(Debug, Clone, PartialEq)]
    enum PointMsg {
        Clicked(Option<iced::Point>),
    }
    #[test]
    fn replacement_identity_cancels_inherited_interaction() {
        use iced::advanced::renderer::Headless;
        use iced_test::runtime::user_interface::{Cache, UserInterface};
        fn step(
            renderer: &mut iced::Renderer,
            cache: Cache,
            id: &'static str,
            color: Color,
            event: Event,
        ) -> (Cache, Vec<u8>, Vec<()>) {
            let view = focus_button(iced::widget::Space::new().width(80).height(30), ())
                .id(iced::advanced::widget::Id::new(id))
                .style_focus(move |_, _| Style {
                    background: Some(color.into()),
                    ..Style::default()
                });
            let mut ui = UserInterface::build(view, Size::new(200.0, 100.0), cache, renderer);
            let cursor = mouse::Cursor::Available(iced::Point::new(20.0, 15.0));
            let mut bus = iced::advanced::shell::Bus::new();
            ui.update(
                &iced::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[event],
                cursor,
                renderer,
                &mut bus,
            );
            ui.draw(renderer, &iced::Theme::Dark, &Default::default(), cursor);
            let pixels = renderer.screenshot(Size::new(200, 100), 1.0, Color::BLACK);
            (ui.into_cache(), pixels, bus.into_iter().collect())
        }
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as Headless>::new(Default::default(), Some("software")),
        )
        .expect("headless renderer");
        let press = || Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
        let release = || Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
        let red = Color::from_rgb(1.0, 0.0, 0.0);
        let blue = Color::from_rgb(0.0, 0.0, 1.0);
        let (cache, first, messages) =
            step(&mut renderer, Cache::default(), "original", red, press());
        assert!(messages.is_empty());
        let (cache, same, messages) = step(&mut renderer, cache, "original", red, release());
        assert_eq!(messages, vec![()]);
        assert_eq!(first, same);
        let (cache, _, _) = step(&mut renderer, cache, "original", red, press());
        let (cache, replaced, messages) =
            step(&mut renderer, cache, "replacement", blue, release());
        assert!(
            messages.is_empty(),
            "a displaced press cannot activate its replacement"
        );
        let pixel = (15 * 200 + 20) * 4;
        assert_eq!(
            &replaced[pixel..pixel + 4],
            &[0, 0, 255, 255],
            "the replacement paints its own fill on its first frame"
        );
        let (cache, _, _) = step(&mut renderer, cache, "replacement", blue, press());
        let (_, _, messages) = step(&mut renderer, cache, "replacement", blue, release());
        assert_eq!(messages, vec![()]);
    }

    #[test]
    fn keyboard_activation_reports_no_point() {
        use iced::advanced::widget::Tree;
        use iced::{Event, keyboard};

        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");

        let mut el: Element<'static, PointMsg> = focus_button(text("Ev"), PointMsg::Clicked(None))
            .padding(20)
            .on_press_with_point(PointMsg::Clicked)
            .into();
        let mut tree = Tree::new(&mut el);
        el.as_widget_mut().diff(&mut tree);
        tree.state.downcast_mut::<State>().focus.focus();
        assert!(
            tree.state.downcast_ref::<State>().focus.is_focused(),
            "sanity: the button must hold focus for Enter to activate it"
        );

        let node = el.as_widget_mut().measure_snapshot(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(400.0, 300.0)),
        );
        let mut messages: iced::advanced::shell::Bus<PointMsg> = iced::advanced::shell::Bus::new();
        let mut shell = Shell::new(
            &iced::advanced::graphics::core::window::Headless,
            iced::advanced::graphics::core::shell::Waker::noop(),
            &mut messages,
        );
        el.as_widget_mut().update(
            &mut tree,
            &Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Enter),
                modified_key: keyboard::Key::Named(keyboard::key::Named::Enter),
                physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Enter),
                location: keyboard::Location::Standard,
                modifiers: keyboard::Modifiers::default(),
                text: None,
                repeat: false,
            }),
            Layout::of(&node),
            mouse::Cursor::Available(iced::Point::new(5000.0, 5000.0)),
            &renderer,
            &mut shell,
            &Rectangle::with_size(Size::new(400.0, 300.0)),
        );

        assert_eq!(
            messages,
            vec![PointMsg::Clicked(None)],
            "Enter activation must report no press point (None), not a stale cursor"
        );
    }
}
