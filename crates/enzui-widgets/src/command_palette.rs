//! A controlled command query with a bounded, theme-aware result list.
//!
//! The query input is the only Tab stop; highlighted rows are exposed through
//! its active-descendant relation. The host applies events to [`PaletteState`]
//! and focuses the input when opening the palette.
//! Keyboard claims run before children without checking focus, so the host
//! must scope the palette to the active keyboard presentation.

use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use enzui_core::action::Action;
use enzui_core::identity::derive_id;
use enzui_core::metrics::FiniteNonNeg;
use enzui_core::palette::{Catalogue, Match, PaletteState, Window};
use iced::advanced::widget::Id;
use iced::widget::{column, container, text};
use iced::{Background, Border, Element, Length, Padding};
use std::rc::Rc;
/// Default identity namespace; use distinct prefixes for simultaneously mounted palettes.
pub const ID_PREFIX: &str = "enz-command-palette";
/// Default maximum number of realized result rows.
pub const DEFAULT_CAPACITY: usize = 12;
const ROW_HEIGHT: f32 = 32.0;
/// Events for the host to apply to controlled palette state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPaletteEvent {
    /// Update the query with [`PaletteState::set_query`].
    Query(String),
    /// Move through results with [`PaletteState::move_highlight`].
    MoveHighlight(isize),
    /// Select a specific action with [`PaletteState::highlight`].
    /// Available for host-driven selection; the widget does not emit this on hover.
    Highlight(&'static str),
    /// Request activation of an enabled row, carrying its action id and build-time generation.
    /// Before execution, the host must validate [`PaletteState::accepts_activation`]
    /// and the action's current enabled state.
    Activate(&'static str, u64),
    /// Request that the host close the palette.
    Dismissed,
}
/// A controlled palette that realizes a retained window of matching actions.
pub struct EnzCommandPalette<'a, Message> {
    actions: &'a [Action<Message>],
    catalogue: &'a Catalogue,
    state: &'a PaletteState,
    capacity: usize,
    on_event: Rc<dyn Fn(CommandPaletteEvent) -> Message + 'a>,
    placeholder: String,
    empty_text: String,
    label: String,
    id_prefix: String,
    input_id: Option<Id>,
    row_height: f32,
}
/// Build a palette with twelve result rows, a 32 px row height, and shrink height.
/// `actions` must be the snapshot used to build `catalogue`; the host applies
/// query and highlight events to `state` before rebuilding.
pub fn enz_command_palette<'a, Message>(
    actions: &'a [Action<Message>],
    catalogue: &'a Catalogue,
    state: &'a PaletteState,
    on_event: impl Fn(CommandPaletteEvent) -> Message + 'a,
) -> EnzCommandPalette<'a, Message> {
    EnzCommandPalette {
        actions,
        catalogue,
        state,
        capacity: DEFAULT_CAPACITY,
        on_event: Rc::new(on_event),
        placeholder: enzui_core::localization::text("enzui-command-palette-type-a-command"),
        empty_text: enzui_core::localization::text("enzui-command-palette-no-matching-commands"),
        label: enzui_core::localization::text("enzui-command-palette-commands"),
        id_prefix: ID_PREFIX.to_owned(),
        input_id: None,
        row_height: enzui_theme::text_defaults::sizing().control_height,
    }
}

impl<'a, Message> EnzCommandPalette<'a, Message> {
    /// Set the maximum realized rows; zero realizes none and shows the empty-state text.
    #[must_use]
    pub fn capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }
    /// Set the empty input hint; the default is “Type a command…”.
    #[must_use]
    pub fn placeholder(mut self, s: impl Into<String>) -> Self {
        self.placeholder = s.into();
        self
    }
    /// Set text shown when no rows are realized; the default is “No matching commands”.
    #[must_use]
    pub fn empty_text(mut self, s: impl Into<String>) -> Self {
        self.empty_text = s.into();
        self
    }
    /// Set the result list's accessible name; the default is “Commands”.
    #[must_use]
    pub fn label(mut self, s: impl Into<String>) -> Self {
        self.label = s.into();
        self
    }
    /// Set the namespace for row ids and the default query-input id.
    /// With a derived input id, reserve the action id `"query"` to avoid an identity collision.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }
    /// Set the query-input id for host focus operations.
    /// The default derives `"query"` from this palette's [`id_prefix`](Self::id_prefix).
    #[must_use]
    pub fn input_id(mut self, id: Id) -> Self {
        self.input_id = Some(id);
        self
    }
    /// Set row height in pixels; the default is 32.
    /// Negative or non-finite values use the default; zero is accepted.
    #[must_use]
    pub fn row_height(mut self, px: f32) -> Self {
        self.row_height = FiniteNonNeg::new_or(px, ROW_HEIGHT).get();
        self
    }
    /// Derive a row identity from the palette namespace and stable action id.
    #[must_use]
    pub fn row_id(&self, action_id: &str) -> Id {
        derive_id(&self.id_prefix, action_id)
    }
    /// Copy all current matches and return their realization window.
    /// Allocates the result vector and updates the state's retained window start.
    #[must_use]
    pub fn visible(&self) -> (Vec<Match>, Window) {
        self.state
            .realize(self.catalogue, self.capacity, |results, w| {
                (results.to_vec(), w)
            })
    }
    fn label_for(&self, m: &Match) -> &str {
        self.actions
            .get(m.index)
            .map_or("", |action| action.label())
    }
    fn enabled_for(&self, m: &Match) -> bool {
        self.actions
            .get(m.index)
            .is_none_or(enzui_core::action::Action::is_enabled)
    }
    fn activatable_highlight(&self, results: &[Match]) -> Option<&'static str> {
        let id = self.state.highlighted()?;
        let m = results.iter().find(|m| m.id == id)?;
        self.enabled_for(m).then_some(id)
    }
}

impl<'a, Message: Clone + 'a> EnzCommandPalette<'a, Message> {
    /// Build the query input and windowed result list using the state's reusable match storage.
    /// Activation callbacks run on events, including row clicks, rather than during construction.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.state
            .realize(self.catalogue, self.capacity, |results, win| {
                self.build_element(results, win)
            })
    }
    fn build_element(&self, results: &[Match], win: Window) -> Element<'a, Message> {
        let highlighted = self.state.highlighted();
        // The input relation must name a row realized in this same tree.
        let total = results.len();
        let mut rows: Vec<Element<'a, Message>> = Vec::with_capacity(win.len);
        let mut active_row_id: Option<Id> = None;

        for index in win.start..win.end() {
            let Some(m) = results.get(index) else {
                break;
            };
            let id = m.id;
            let row_id = self.row_id(id);
            let is_highlighted = highlighted == Some(id);
            if is_highlighted {
                active_row_id = Some(row_id.clone());
            }

            let label = self.label_for(m).to_owned();
            let enabled = self.enabled_for(m);
            let label_widget = text(label.clone())
                .size(enzui_theme::text_scale::ui_px(13.0))
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                .style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(if enabled {
                        crate::theme_lookup::palette(theme).text
                    } else {
                        crate::theme_lookup::palette(theme).disabled
                    }),
                });
            let on_activate = Rc::clone(&self.on_event);
            let generation = self.state.generation();
            let factory: Option<Box<dyn Fn() -> Message + 'a>> = enabled.then(|| {
                Box::new(move || (on_activate)(CommandPaletteEvent::Activate(id, generation)))
                    as Box<dyn Fn() -> Message + 'a>
            });

            let row = focus_button_maybe_with(label_widget, factory)
                .width(Length::Fill)
                .height(Length::Fixed(self.row_height))
                .padding(Padding::from([0.0, 10.0]))
                .content_align_x(iced::alignment::Horizontal::Left)
                // Result rows expose selection without becoming separate focus stops.
                .embedded(true)
                .id(row_id)
                .semantics(crate::listbox_shape::listbox_option(
                    label,
                    is_highlighted,
                    index + 1,
                    total,
                ))
                .style_focus(move |theme: &iced::Theme, _status: FocusStatus| {
                    let p = crate::theme_lookup::palette(theme);
                    iced::widget::button::Style {
                        background: is_highlighted.then_some(Background::Color(p.surface_hover)),
                        text_color: if enabled { p.text } else { p.disabled },
                        border: Border {
                            color: iced::Color::TRANSPARENT,
                            width: 0.0,
                            radius: crate::theme_lookup::shape_radius(theme, |s| s.row).into(),
                        },
                        ..iced::widget::button::Style::default()
                    }
                });
            rows.push(row.into());
        }
        let list: Element<'a, Message> = if rows.is_empty() {
            let empty = self.empty_text.clone();
            container(
                text(empty)
                    .size(enzui_theme::text_scale::ui_px(13.0))
                    .style(|theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(crate::theme_lookup::palette(theme).text_muted),
                    }),
            )
            .width(Length::Fill)
            .padding(Padding::from([8.0, 10.0]))
            .into()
        } else {
            column(rows).spacing(2.0).width(Length::Fill).into()
        };
        let list_semantics = crate::listbox_shape::listbox(self.label.clone());
        let list_group: Element<'a, Message> =
            crate::semantics_container::SemanticsContainer::new(list, list_semantics).into();
        let on_query = Rc::clone(&self.on_event);
        let input_id = self
            .input_id
            .clone()
            .unwrap_or_else(|| derive_id(&self.id_prefix, "query"));
        // Enter belongs to the palette; an input submit callback would claim it first.
        let input = crate::search_field::enz_search_field(self.state.query(), move |q| {
            (on_query)(CommandPaletteEvent::Query(q))
        })
        .responsive(false)
        .placeholder(self.placeholder.clone())
        .active_descendant(active_row_id)
        .id(input_id);

        let body = column![input, list_group]
            .spacing(enzui_theme::text_defaults::spacing().sm())
            .width(Length::Fill);

        let surface = container(body)
            .padding(enzui_theme::text_defaults::spacing().sm())
            .width(Length::Fill)
            .style(|theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                container::Style {
                    background: Some(Background::Color(p.surface_elevated)),
                    border: Border {
                        color: crate::surface_edge::edge_color(
                            p.surface_elevated,
                            crate::surface_edge::DEFAULT_EDGE_DEPTH,
                        ),
                        width: 1.0,
                        radius: crate::theme_lookup::shape_radius(theme, |s| s.popup).into(),
                    },
                    ..container::Style::default()
                }
            });
        PaletteKeys {
            child: surface.into(),
            on_event: Rc::clone(&self.on_event),
            highlighted: self.activatable_highlight(results),
            generation: self.state.generation(),
        }
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzCommandPalette<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzCommandPalette<'a, Message>) -> Self {
        w.into_element()
    }
}
/// A key's navigation, activation, or dismissal class before modifier and repeat gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteKeyClaim {
    /// Repeatable movement; Ctrl, Alt, or Logo chords are left to the host.
    Movement(isize),
    /// Single-press activation; Ctrl, Alt, or Logo chords are left to the host.
    Activation,
    /// Single-press dismissal under any modifiers.
    Dismissal,
}
/// Named key claims used by [`event_for_key`]; the first entry for a key wins.
pub const PALETTE_KEYS: &[(iced::keyboard::key::Named, PaletteKeyClaim)] = {
    use iced::keyboard::key::Named;
    &[
        (Named::ArrowDown, PaletteKeyClaim::Movement(1)),
        (Named::ArrowUp, PaletteKeyClaim::Movement(-1)),
        (Named::Enter, PaletteKeyClaim::Activation),
        (Named::Escape, PaletteKeyClaim::Dismissal),
    ]
};
/// Look up a named key's first claim in [`PALETTE_KEYS`].
#[must_use]
pub fn palette_key_claim(named: iced::keyboard::key::Named) -> Option<PaletteKeyClaim> {
    PALETTE_KEYS
        .iter()
        .find(|(k, _)| *k == named)
        .map(|(_, claim)| *claim)
}
/// Decode a named key using the palette's modifier and repeat rules.
/// Movement repeats; activation requires a highlight and does not repeat.
/// Ctrl, Alt, or Logo suppress movement and activation, while Shift alone permits them.
/// Escape dismisses under any modifiers without repeating. Activation carries
/// the supplied build-time generation for host validation.
#[must_use]
pub fn event_for_key(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    highlighted: Option<&'static str>,
    repeat: bool,
    generation: u64,
) -> Option<CommandPaletteEvent> {
    let iced::keyboard::Key::Named(named) = key else {
        return None;
    };
    let claim = palette_key_claim(*named)?;
    let app_chord = modifiers.control() || modifiers.alt() || modifiers.logo();
    match claim {
        PaletteKeyClaim::Movement(delta) => {
            (!app_chord).then_some(CommandPaletteEvent::MoveHighlight(delta))
        }
        PaletteKeyClaim::Activation => {
            if !repeat && !app_chord {
                highlighted.map(|id| CommandPaletteEvent::Activate(id, generation))
            } else {
                None
            }
        }
        PaletteKeyClaim::Dismissal => (!repeat).then_some(CommandPaletteEvent::Dismissed),
    }
}
struct PaletteKeys<'a, Message> {
    child: Element<'a, Message>,
    on_event: Rc<dyn Fn(CommandPaletteEvent) -> Message + 'a>,
    highlighted: Option<&'static str>,
    generation: u64,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for PaletteKeys<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> iced::Size<Length> {
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
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key,
            modifiers,
            repeat,
            ..
        }) = event
            && let Some(palette_event) =
                event_for_key(key, *modifiers, self.highlighted, *repeat, self.generation)
        {
            shell.publish((self.on_event)(palette_event));
            shell.capture_event();
            return;
        }
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
}

impl<'a, Message: 'a> From<PaletteKeys<'a, Message>> for Element<'a, Message> {
    fn from(w: PaletteKeys<'a, Message>) -> Self {
        Element::new(w)
    }
}
