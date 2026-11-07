//! A desktop menu bar (File, Edit, View) built from a [`MenuBarModel`].
//!
//! The application owns the model and applies each [`MenuBarEvent`] with
//! [`MenuBarModel::apply`], which returns the command a user chose. Titles
//! open their menu on a press, and while a menu is open moving over another
//! title opens that one. F10 opens the first menu from anywhere in the window;
//! with a menu open, arrows move through it and between menus, Right opens a
//! submenu, Enter chooses and Escape closes. Rows show their shortcut, check
//! or radio mark and submenu chevron from the model.
//!
//! Titles and rows take access keys: an underscore marks the letter
//! (`"_File"`, `"_Open…"`; `"__"` is an underscore). Alt and a title's letter
//! open its menu, and in an open menu a row's letter chooses it; the letters
//! are underlined while Alt is held.
//!
//! Accessibility: the bar is a `MenuBar` whose titles are menu items that
//! report whether their menu is expanded; each open menu is a `Menu` of its
//! rows.

use enzui_core::menu_bar::{MenuBarEvent, MenuBarMenu, MenuBarModel};
use enzui_core::menu_model::{EntryId, EntryKind, MenuModel};
use enzui_core::placement::{Alignment, Direction, Placement, Side};
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{container, row, text};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::rc::Rc;

/// The menu bar's title height in logical pixels, before text scaling.
const TITLE_HEIGHT: f32 = 28.0;

/// Builder for [`enz_menu_bar`].
#[must_use]
pub struct EnzMenuBar<'a, Message> {
    model: &'a MenuBarModel,
    on_event: Rc<dyn Fn(MenuBarEvent) -> Message + 'a>,
    id_prefix: String,
    direction: Option<Direction>,
}

/// A menu bar for `model`; every event goes through `on_event` to the
/// application, which applies it to the model.
pub fn enz_menu_bar<'a, Message: Clone + 'a>(
    model: &'a MenuBarModel,
    on_event: impl Fn(MenuBarEvent) -> Message + 'a,
) -> EnzMenuBar<'a, Message> {
    EnzMenuBar {
        model,
        on_event: Rc::new(on_event),
        id_prefix: String::from("menu-bar"),
        direction: None,
    }
}

impl<'a, Message: Clone + 'a> EnzMenuBar<'a, Message> {
    /// Namespace the widget ids of the titles and rows (two menu bars in one
    /// window need different prefixes).
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the writing direction; by default the locale's.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Build the bar.
    pub fn into_element(self) -> Element<'a, Message> {
        let model = self.model;
        let open = model.open_menu();
        let direction = enzui_core::placement::resolve_direction(self.direction);
        let height = enzui_theme::text_scale::ui_px(TITLE_HEIGHT);
        let mut titles = row![].spacing(2.0);
        for (index, menu) in model.menus().iter().enumerate() {
            let is_open = open == Some(index);
            let on_event = Rc::clone(&self.on_event);
            let press = if is_open {
                MenuBarEvent::Close
            } else {
                MenuBarEvent::OpenUnhighlighted(index)
            };
            let title_text = enzui_core::mnemonic::Mnemonic::parse(menu.title());
            let size = enzui_theme::text_scale::ui_px(13.0);
            let plain: Element<'a, Message> = text(title_text.text.clone()).size(size).into();
            let label = match title_text.at.clone() {
                Some(at) => {
                    let shown = &title_text.text;
                    let spans: [iced::advanced::text::Span<'_, ()>; 3] = [
                        iced::widget::span(shown.get(..at.start).unwrap_or_default().to_owned()),
                        iced::widget::span(shown.get(at.clone()).unwrap_or_default().to_owned())
                            .underline(true),
                        iced::widget::span(shown.get(at.end..).unwrap_or_default().to_owned()),
                    ];
                    crate::mnemonic::mnemonic_label(
                        plain,
                        iced::widget::rich_text(spans).size(size).into(),
                    )
                }
                None => plain,
            };
            let title = crate::focus_button::focus_button_maybe_with(
                container(label)
                    .padding([0.0, 10.0])
                    .height(height)
                    .center_y(height),
                Some(Box::new(move || on_event(press.clone())) as Box<dyn Fn() -> Message + 'a>),
            )
            .semantics(
                Semantics::new(Role::MenuItem)
                    .label(title_text.text.clone())
                    .expanded(is_open)
                    .default_action(),
            )
            .id(enzui_core::identity::derive_id(
                &self.id_prefix,
                &format!("title-{}", menu.title()),
            ))
            .style_focus(move |theme, status| title_style(theme, status, is_open));
            // While a menu is open, moving onto another title opens it.
            let title: Element<'a, Message> = if open.is_some() && !is_open {
                let on_event = Rc::clone(&self.on_event);
                iced::widget::mouse_area(title)
                    .on_enter(on_event(MenuBarEvent::OpenUnhighlighted(index)))
                    .into()
            } else {
                title.into()
            };
            let popup = is_open.then(|| {
                menu_popup(
                    model,
                    menu,
                    menu.model(),
                    &format!("{}-{}", self.id_prefix, menu.title()),
                    &self.on_event,
                    direction,
                )
            });
            let on_event = Rc::clone(&self.on_event);
            titles = titles.push(
                crate::anchored_overlay::anchored_overlay(title, popup)
                    .placement(crate::menu_button::menu_placement(Side::Bottom, direction))
                    .dismiss_on_outside_press(move || on_event(MenuBarEvent::Close)),
            );
        }
        let bar = crate::semantics_container::SemanticsContainer::new(
            container(titles).width(Length::Fill),
            Semantics::new(Role::MenuBar),
        );
        Element::new(MenuBarKeys {
            content: bar.into(),
            model,
            on_event: Rc::clone(&self.on_event),
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzMenuBar<'a, Message>> for Element<'a, Message> {
    fn from(bar: EnzMenuBar<'a, Message>) -> Self {
        bar.into_element()
    }
}

/// A title's look: like a quiet button, held pressed while its menu is open.
fn title_style(
    theme: &iced::Theme,
    status: crate::focus_button::FocusStatus,
    open: bool,
) -> iced::widget::button::Style {
    use crate::focus_button::FocusStatus;
    let palette = crate::theme_lookup::palette(theme);
    let background = match status {
        _ if open => Some(palette.surface_selected),
        FocusStatus::Hovered | FocusStatus::Pressed => Some(palette.surface_hover),
        _ => None,
    };
    iced::widget::button::Style {
        background: background.map(iced::Background::Color),
        text_color: palette.text,
        border: iced::Border::default().rounded(6.0),
        ..Default::default()
    }
}

/// The popup listing `level`'s entries; the row of an open submenu carries
/// that submenu beside it.
fn menu_popup<'a, Message: Clone + 'a>(
    model: &'a MenuBarModel,
    menu: &'a MenuBarMenu,
    level: &'a MenuModel,
    row_prefix: &str,
    on_event: &Rc<dyn Fn(MenuBarEvent) -> Message + 'a>,
    direction: Direction,
) -> Element<'a, Message> {
    let mut popup =
        crate::menu::enz_menu().label(enzui_core::mnemonic::Mnemonic::parse(menu.title()).text);
    let reserve_leading = level
        .entries()
        .iter()
        .any(|entry| entry.icon_name().is_some() || entry.check_state().is_checkable());
    let entries = level.dedup_entries();
    let total = entries.iter().filter(|entry| entry.is_focusable()).count();
    let mut seen = 0;
    for entry in entries {
        if entry.kind() == EntryKind::Separator {
            popup = popup.push(crate::menu_row::enz_menu_separator::<Message>());
            continue;
        }
        let position = entry.is_focusable().then(|| {
            seen += 1;
            enzui_core::semantics::SetPosition {
                index: seen,
                count: total,
            }
        });
        let activate = {
            let on_event = Rc::clone(on_event);
            let id = entry.id().clone();
            Box::new(move || on_event(MenuBarEvent::Activate(id.clone())))
                as Box<dyn Fn() -> Message + 'a>
        };
        let row = crate::menu_button::model_row(
            entry,
            level.active() == Some(entry.id()),
            reserve_leading,
            row_prefix,
            position,
            activate,
        )
        .mnemonic(true)
        .into_element();
        let submenu = submenu_of(model, menu, entry.id()).map(|child| {
            menu_popup(
                model,
                menu,
                child,
                &format!("{row_prefix}-{}", entry.id().as_str()),
                on_event,
                direction,
            )
        });
        let row: Element<'a, Message> = match submenu {
            Some(child) => crate::anchored_overlay::anchored_overlay(row, Some(child))
                .placement(
                    Placement::new(Side::Right)
                        .alignment(Alignment::Start)
                        .direction(direction),
                )
                .into(),
            None => row,
        };
        popup = popup.push(row);
    }
    crate::swallow_press::SwallowPress::new(popup.into_element()).into()
}

/// The open submenu's entries, when `entry` is the row that opened it.
fn submenu_of<'a>(
    model: &MenuBarModel,
    menu: &'a MenuBarMenu,
    entry: &EntryId,
) -> Option<&'a MenuModel> {
    (model.open_submenu() == Some(entry))
        .then(|| menu.submenu_model(entry))
        .flatten()
}

/// Hands the window's key presses to the menu bar: F10 always, and the menu
/// keys while a menu is open. A key the bar does not answer passes on.
struct MenuBarKeys<'a, Message> {
    content: Element<'a, Message>,
    model: &'a MenuBarModel,
    on_event: Rc<dyn Fn(MenuBarEvent) -> Message + 'a>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MenuBarKeys<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.content.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content.as_widget_mut().layout(tree, renderer, limits);
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
        self.content
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
        self.content
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
        self.content
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
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
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
        // The bar answers its keys before the rest of its subtree: a focused
        // title must not turn Enter into a second press.
        if let Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            && !shell.is_event_captured()
            && let Some(menu_event) = self.model.event_for_key(key, *modifiers)
        {
            shell.publish((self.on_event)(menu_event));
            shell.capture_event();
            return;
        }
        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
}
