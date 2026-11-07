//! Access keys: Alt and an underlined letter activate a control.
//!
//! Mark the letter with an underscore in the label (`"_Save"`); see
//! `enzui_core::mnemonic`. [`EnzButton::mnemonic`](crate::button::EnzButton::mnemonic)
//! does the rest for a button: the letter is underlined while Alt is held,
//! and Alt with the letter presses the button. For any other control,
//! [`with_mnemonic`] adds the activation and [`mnemonic_label`] the
//! underlined label.
//!
//! The first control in the window with a matching key takes the press;
//! give each control in a window its own letter.

use enzui_core::mnemonic::Mnemonic;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard;
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::rc::Rc;

/// Whether Alt is held, as the last event any access-key label saw said:
/// a label created while Alt is down (a menu opened with Alt and a letter)
/// starts underlined.
static ALT_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether `event` holds or releases Alt; `None` when it says nothing.
fn alt_from(event: &Event) -> Option<bool> {
    let held = match event {
        Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => modifiers.alt(),
        Event::Window(iced::window::Event::Unfocused) => false,
        _ => return None,
    };
    ALT_HELD.store(held, std::sync::atomic::Ordering::Relaxed);
    Some(held)
}

/// A label showing `plain` normally and `marked` (the same text with the
/// access key underlined) while Alt is held. Both are laid out in the same
/// place; only one is drawn.
pub fn mnemonic_label<'a, Message: 'a>(
    plain: Element<'a, Message>,
    marked: Element<'a, Message>,
) -> Element<'a, Message> {
    Element::new(MnemonicLabel { plain, marked })
}

struct MnemonicLabel<'a, Message> {
    plain: Element<'a, Message>,
    marked: Element<'a, Message>,
}

#[derive(Debug, Default)]
struct AltState {
    held: bool,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MnemonicLabel<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<AltState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(AltState {
            held: ALT_HELD.load(std::sync::atomic::Ordering::Relaxed),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        if tree.children.len() != 2 {
            tree.children = vec![
                Tree::new(self.plain.as_widget()),
                Tree::new(self.marked.as_widget()),
            ];
        }
        tree.children[0].diff(self.plain.as_widget_mut());
        tree.children[1].diff(self.marked.as_widget_mut());
        tree.refresh_frame_children();
    }

    fn size(&self) -> Size<Length> {
        self.plain.as_widget().size()
    }

    fn frame_interest(&self, _tree: &Tree) -> bool {
        false
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        use iced::advanced::layout::Measure as _;
        let plain = self
            .plain
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        let marked = self
            .marked
            .as_widget_mut()
            .measure(&mut tree.children[1], renderer, limits);
        layout::Node::with_children(plain.size(), vec![plain, marked]).apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if let Some(held) = alt_from(event) {
            let state = tree.state.downcast_mut::<AltState>();
            if state.held != held {
                state.held = held;
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let held = tree.state.downcast_ref::<AltState>().held;
        let index = usize::from(held);
        let Some(child) = layout.children(&tree.children).nth(index) else {
            return;
        };
        let widget = if held { &self.marked } else { &self.plain };
        widget.as_widget().draw(
            &tree.children[index],
            renderer,
            theme,
            style,
            child,
            cursor,
            viewport,
        );
    }
}

/// `content` that also activates on Alt and `label`'s access key: `on_key`
/// makes the message. Nothing changes when the label marks no key.
pub fn with_mnemonic<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
    label: &str,
    on_key: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    let mnemonic = Mnemonic::parse(label);
    let content = content.into();
    if mnemonic.key.is_none() {
        return content;
    }
    Element::new(MnemonicActivator {
        content,
        mnemonic,
        on_key: Rc::new(on_key),
    })
}

/// [`with_mnemonic`] with a shared message factory.
pub(crate) fn with_mnemonic_rc<'a, Message: 'a>(
    content: Element<'a, Message>,
    mnemonic: Mnemonic,
    on_key: Rc<dyn Fn() -> Message + 'a>,
) -> Element<'a, Message> {
    Element::new(MnemonicActivator {
        content,
        mnemonic,
        on_key,
    })
}

struct MnemonicActivator<'a, Message> {
    content: Element<'a, Message>,
    mnemonic: Mnemonic,
    on_key: Rc<dyn Fn() -> Message + 'a>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MnemonicActivator<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modifiers,
            repeat: false,
            ..
        }) = event
            && modifiers.alt()
            && !modifiers.control()
            && !modifiers.logo()
            && self.mnemonic.matches(key)
            && !shell.is_event_captured()
        {
            shell.publish((self.on_key)());
            shell.capture_event();
            return;
        }
        self.content.as_widget_mut().update(
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
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children
            .first()
            .is_none_or(|content| self.content.as_widget().frame_interest(content))
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
