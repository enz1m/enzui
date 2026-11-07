//! A text field that turns what is typed into chips: tags, recipients,
//! keywords (GTK token entries, Qt's tag inputs, Material chip input).
//!
//! The application owns the chips and the text being typed (the draft) and
//! applies every [`ChipInputEvent`] with [`apply_chip_event`]: Enter, or a
//! separator (a comma by default) typed or pasted, turns the draft into a
//! chip; Backspace in an empty draft removes the last chip; each chip has a
//! remove button named `Remove <chip>`. Chips wrap onto more lines as they
//! fill the field. Assistive technology sees a group named by the field's
//! label holding the chips' remove buttons and the text input.

use crate::field_chrome::{ChromePaint, ChromeStatus, FieldChrome};
use crate::focus_button::{FocusStatus, focus_button_maybe};
use crate::text_field::TextFieldSize;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{container, row, text, text_input};
use iced::{Background, Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector};
use std::rc::Rc;

/// What a chip input asks its application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChipInputEvent {
    /// The draft is now this text (typing, pasting).
    Draft(String),
    /// Turn the draft into a chip (Enter).
    Commit,
    /// Remove the chip at this index.
    Remove(usize),
}

/// Apply `event` to the application's `chips` and `draft`: separators in
/// the draft and Commit turn its trimmed, non-empty pieces into chips (a
/// piece already present is not added twice); Remove drops one chip.
pub fn apply_chip_event(
    chips: &mut Vec<String>,
    draft: &mut String,
    event: &ChipInputEvent,
    separators: &[char],
) {
    let add = |piece: &str, chips: &mut Vec<String>| {
        let piece = piece.trim();
        if !piece.is_empty() && !chips.iter().any(|chip| chip == piece) {
            chips.push(piece.to_owned());
        }
    };
    match event {
        ChipInputEvent::Draft(text) => {
            let mut pieces: Vec<&str> = text.split(separators).collect();
            let rest = pieces.pop().unwrap_or_default();
            for piece in pieces {
                add(piece, chips);
            }
            rest.clone_into(draft);
        }
        ChipInputEvent::Commit => {
            add(draft, chips);
            draft.clear();
        }
        ChipInputEvent::Remove(index) => {
            if *index < chips.len() {
                let _ = chips.remove(*index);
            }
        }
    }
}

/// The namespace a chip input's label-derived id is made in.
pub const DEFAULT_ID_PREFIX: &str = "enz-chip-input";

/// The separator a chip input splits on unless told otherwise.
pub const DEFAULT_SEPARATORS: &[char] = &[','];

/// Builder for [`enz_chip_input`].
#[must_use]
pub struct EnzChipInput<'a, Message> {
    chips: &'a [String],
    draft: &'a str,
    on_event: Rc<dyn Fn(ChipInputEvent) -> Message + 'a>,
    placeholder: String,
    label: Option<String>,
    enabled: bool,
    id: Option<Id>,
    size: TextFieldSize,
}

/// A chip input showing `chips` and the `draft` being typed; every event
/// goes through `on_event`, and the application applies it with
/// [`apply_chip_event`].
pub fn enz_chip_input<'a, Message: Clone + 'a>(
    chips: &'a [String],
    draft: &'a str,
    on_event: impl Fn(ChipInputEvent) -> Message + 'a,
) -> EnzChipInput<'a, Message> {
    EnzChipInput {
        chips,
        draft,
        on_event: Rc::new(on_event),
        placeholder: String::new(),
        label: None,
        enabled: true,
        id: None,
        size: TextFieldSize::Md,
    }
}

impl<'a, Message: Clone + 'a> EnzChipInput<'a, Message> {
    /// Text shown in the empty draft.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// The field's accessible name ("Tags").
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set availability: a disabled input shows its chips without remove
    /// buttons and takes no typing.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// The text input's id, for programmatic focus.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// The field's size tier; default medium.
    pub fn size(mut self, size: TextFieldSize) -> Self {
        self.size = size;
        self
    }

    /// Build the field.
    pub fn into_element(self) -> Element<'a, Message> {
        let tokens = self.size.tokens();
        let enabled = self.enabled;
        let chips: Vec<Element<'a, Message>> = self
            .chips
            .iter()
            .enumerate()
            .map(|(index, chip)| chip_view(chip, index, enabled, &self.on_event, tokens.text_size))
            .collect();
        let mut input = text_input(&self.placeholder, self.draft)
            .size(tokens.text_size)
            .padding(Padding::from([tokens.padding_y(), 0.0]))
            .width(Length::Fill)
            .style(|theme: &iced::Theme, status| {
                let palette = crate::theme_lookup::palette(theme);
                text_input::Style {
                    background: Background::Color(Color::TRANSPARENT),
                    border: Border::default(),
                    ..crate::text_field::text_field_style(
                        &palette,
                        status,
                        false,
                        &TextFieldSize::Md.tokens(),
                        crate::tokens::FieldVariant::default(),
                        crate::tokens::FieldVariant::default(),
                        false,
                    )
                }
            });
        // A stable id keeps focus and the caret across rebuilds: the given
        // one, else one derived from the label, else from the placeholder,
        // as text fields do.
        let key = self
            .label
            .clone()
            .filter(|label| !label.is_empty())
            .or_else(|| (!self.placeholder.is_empty()).then(|| self.placeholder.clone()));
        let id = self
            .id
            .clone()
            .or_else(|| key.map(|key| enzui_core::identity::derive_id(DEFAULT_ID_PREFIX, &key)));
        if let Some(id) = id {
            input = input.id(id);
        }
        if enabled {
            let typed = Rc::clone(&self.on_event);
            let committed = Rc::clone(&self.on_event);
            input = input
                .on_input(move |text| typed(ChipInputEvent::Draft(text)))
                .on_submit(committed(ChipInputEvent::Commit));
        }
        let remove_last = (enabled && self.draft.is_empty())
            .then(|| self.chips.len().checked_sub(1))
            .flatten()
            .map(|last| (self.on_event)(ChipInputEvent::Remove(last)));
        let input: Element<'a, Message> = Element::new(BackspaceGuard {
            child: input.into(),
            remove_last,
        });
        let content = ChipFlow { input, chips };
        let mut semantics = Semantics::new(Role::Group);
        if let Some(label) = self.label {
            semantics = semantics.label(label);
        }
        let field = FieldChrome::new(
            content,
            tokens.height,
            Padding::from([4.0, tokens.padding_x]),
            move |theme: &iced::Theme, status: ChromeStatus| {
                let palette = crate::theme_lookup::palette(theme);
                let mapped = match status {
                    ChromeStatus::Active => text_input::Status::Active,
                    ChromeStatus::Hovered => text_input::Status::Hovered,
                    ChromeStatus::Focused => text_input::Status::Focused { is_hovered: false },
                    ChromeStatus::Inert => text_input::Status::Disabled,
                };
                let style = crate::text_field::text_field_style(
                    &palette,
                    mapped,
                    false,
                    &tokens,
                    crate::tokens::FieldVariant::default(),
                    crate::tokens::FieldVariant::default(),
                    false,
                );
                ChromePaint {
                    background: match style.background {
                        Background::Color(color) => color,
                        _ => palette.surface,
                    },
                    border_color: style.border.color,
                    border_width: style.border.width,
                    radius: tokens.radius,
                }
            },
        )
        .height_length(Length::Shrink)
        .interactive(enabled);
        crate::semantics_container::SemanticsContainer::new(field, semantics).into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzChipInput<'a, Message>> for Element<'a, Message> {
    fn from(input: EnzChipInput<'a, Message>) -> Self {
        input.into_element()
    }
}

/// One chip: its text and, while enabled, a remove button.
fn chip_view<'a, Message: Clone + 'a>(
    chip: &str,
    index: usize,
    enabled: bool,
    on_event: &Rc<dyn Fn(ChipInputEvent) -> Message + 'a>,
    text_size: f32,
) -> Element<'a, Message> {
    let mut body = row![text(chip.to_owned()).size(text_size - 1.0)]
        .spacing(4)
        .align_y(iced::alignment::Vertical::Center);
    if enabled {
        // A small filled circle with the cross inside, as mail recipient
        // tokens and input chips draw their remove action.
        let remove = focus_button_maybe(
            container(crate::icon::enz_icon("x").size(REMOVE_ICON))
                .center_x(Length::Fixed(REMOVE_SIZE))
                .center_y(Length::Fixed(REMOVE_SIZE)),
            Some(on_event(ChipInputEvent::Remove(index))),
        )
        .padding(0)
        .width(Length::Fixed(REMOVE_SIZE))
        .height(Length::Fixed(REMOVE_SIZE))
        .semantics(
            Semantics::new(Role::Button)
                .label(enzui_core::localized!("enzui-chip-input-remove", "chip" => chip))
                .default_action(),
        )
        .style_focus(|theme: &iced::Theme, status: FocusStatus| {
            let palette = crate::theme_lookup::palette(theme);
            let active = matches!(status, FocusStatus::Hovered | FocusStatus::Pressed);
            iced::widget::button::Style {
                background: Some(Background::Color(palette.text.scale_alpha(if active {
                    0.28
                } else {
                    0.14
                }))),
                text_color: if active {
                    palette.text
                } else {
                    palette.text_muted
                },
                border: Border {
                    color: if status == FocusStatus::Focused {
                        palette.focus_ring
                    } else {
                        Color::TRANSPARENT
                    },
                    width: if status == FocusStatus::Focused {
                        2.0
                    } else {
                        0.0
                    },
                    radius: (REMOVE_SIZE / 2.0).into(),
                },
                ..Default::default()
            }
        });
        body = body.push(remove);
    }
    let padding = if enabled {
        Padding::from([3.0, 3.0]).left(10.0)
    } else {
        Padding::from([3.0, 10.0])
    };
    container(body)
        .padding(padding)
        .style(|theme: &iced::Theme| {
            let palette = crate::theme_lookup::palette(theme);
            container::Style {
                background: Some(Background::Color(palette.surface_selected)),
                text_color: Some(palette.text),
                border: Border {
                    // The filter chip's corners: the theme's button step.
                    radius: crate::theme_lookup::shape_radius(theme, |shape| shape.button).into(),
                    ..Border::default()
                },
                ..Default::default()
            }
        })
        .into()
}

/// The remove button's circle and the cross inside it.
const REMOVE_SIZE: f32 = 16.0;
const REMOVE_ICON: f32 = 10.0;

/// Hands Backspace in an empty, focused draft to "remove the last chip"
/// before the text input, which would otherwise take it.
struct BackspaceGuard<'a, Message> {
    child: Element<'a, Message>,
    remove_last: Option<Message>,
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for BackspaceGuard<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        if let Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Backspace),
            ..
        }) = event
            && let Some(message) = &self.remove_last
            && crate::field_chrome::focused_input_in::<iced::Renderer>(tree) == Some(true)
        {
            shell.publish(message.clone());
            shell.capture_event();
            return;
        }
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

/// The narrowest the text input gets at the end of a line before it moves
/// to a line of its own.
const MIN_INPUT_WIDTH: f32 = 120.0;

/// The gap between chips, and between lines.
const FLOW_SPACING: f32 = 6.0;

/// Lays the chips out in lines and the input after them, filling the rest of
/// the last line. The input is the first child in the tree, so adding or
/// removing a chip never moves its state (focus, caret, typed text).
struct ChipFlow<'a, Message> {
    input: Element<'a, Message>,
    chips: Vec<Element<'a, Message>>,
}

impl<'a, Message> ChipFlow<'a, Message> {
    /// The children in tree order: the input, then the chips.
    fn children(&self) -> impl Iterator<Item = &Element<'a, Message>> {
        std::iter::once(&self.input).chain(self.chips.iter())
    }

    fn children_mut(&mut self) -> Vec<&mut Element<'a, Message>> {
        std::iter::once(&mut self.input)
            .chain(self.chips.iter_mut())
            .collect()
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ChipFlow<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        let mut children = self.children_mut();
        tree.diff_children_custom(
            &mut children,
            |tree, child| tree.diff(child.as_widget_mut()),
            |child| Tree::new(child.as_widget()),
        );
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        use iced::advanced::layout::Measure as _;
        let width = limits.max().width;
        let width = if width.is_finite() { width } else { 480.0 };
        let unbounded =
            |width: f32| layout::Limits::new(Size::ZERO, Size::new(width, f32::INFINITY));
        // (tree index, position, size, line)
        let mut placed: Vec<(usize, iced::Point, Size, usize)> = Vec::new();
        let mut lines: Vec<(f32, f32)> = Vec::new();
        let (mut x, mut y, mut line_height) = (0.0_f32, 0.0_f32, 0.0_f32);
        for (index, chip) in self.chips.iter_mut().enumerate() {
            let size = chip
                .as_widget_mut()
                .measure(&mut tree.children[index + 1], renderer, &unbounded(width))
                .size();
            if x > 0.0 && x + size.width > width {
                lines.push((y, line_height));
                y += line_height + FLOW_SPACING;
                x = 0.0;
                line_height = 0.0;
            }
            placed.push((index + 1, iced::Point::new(x, y), size, lines.len()));
            x += size.width + FLOW_SPACING;
            line_height = line_height.max(size.height);
        }
        if x > 0.0 && width - x < MIN_INPUT_WIDTH {
            lines.push((y, line_height));
            y += line_height + FLOW_SPACING;
            x = 0.0;
            line_height = 0.0;
        }
        let input = self
            .input
            .as_widget_mut()
            .measure(
                &mut tree.children[0],
                renderer,
                &unbounded((width - x).max(0.0)),
            )
            .size();
        placed.push((0, iced::Point::new(x, y), input, lines.len()));
        line_height = line_height.max(input.height);
        lines.push((y, line_height));
        // Centre every child on its line.
        for (index, position, size, line) in placed {
            let (top, height) = lines[line];
            tree.children[index].translation =
                Vector::new(position.x, top + (height - size.height) / 2.0);
        }
        tree.size = Size::new(width, y + line_height);
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
        for ((child, child_tree), child_layout) in self
            .children()
            .zip(tree.children.iter())
            .zip(layout.children(&tree.children))
        {
            child.as_widget().draw(
                child_tree,
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.children()
            .zip(tree.children.iter())
            .zip(layout.children(&tree.children))
            .map(|((child, child_tree), child_layout)| {
                child.as_widget().mouse_interaction(
                    child_tree,
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_default()
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        // Reading order: the chips, then the input.
        operation.traverse(&mut |operation| {
            let mut layouts: Vec<_> = layout.iter_mut(&mut tree.children).collect();
            let (input_layout, input_tree) = layouts.remove(0);
            for (chip, (chip_layout, chip_tree)) in self.chips.iter_mut().zip(layouts) {
                chip.as_widget_mut()
                    .operate(chip_tree, chip_layout, viewport, renderer, operation);
            }
            self.input.as_widget_mut().operate(
                input_tree,
                input_layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        let mut children = self.children_mut();
        for (child, (child_layout, child_tree)) in
            children.iter_mut().zip(layout.iter_mut(&mut tree.children))
        {
            child.as_widget_mut().update(
                child_tree,
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            child_tree.refresh_frame_interest(child.as_widget());
        }
        tree.refresh_frame_children();
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let Tree {
            children: child_trees,
            frame_children,
            ..
        } = tree;
        let mut children = self.children_mut();
        let mut route = 0;
        while route < frame_children.len() {
            let index = frame_children[route];
            let child_layout = layout.child(child_trees, index);
            let child_tree = &mut child_trees[index];
            children[index].as_widget_mut().update_frame(
                child_tree,
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            child_tree.refresh_frame_interest(children[index].as_widget());
            if child_tree.has_frame_interest() {
                route += 1;
            } else {
                let _ = frame_children.remove(route);
            }
        }
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
        std::iter::once(&mut self.input)
            .chain(self.chips.iter_mut())
            .zip(layout.iter_mut(&mut tree.children))
            .flat_map(|(child, (child_layout, child_tree))| {
                child.as_widget_mut().overlay(
                    child_tree,
                    child_layout,
                    renderer,
                    viewport,
                    translation,
                    window,
                )
            })
            .collect()
    }
}

impl<'a, Message: 'a> From<ChipFlow<'a, Message>> for Element<'a, Message> {
    fn from(flow: ChipFlow<'a, Message>) -> Self {
        Element::new(flow)
    }
}
