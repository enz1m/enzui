//! Navigation breadcrumbs: a trail of path segments as links, separated by
//! a chevron (or any `.separator(..)` text), with the middle collapsible when
//! the trail is long.
//!
//! # Editable location
//!
//! [`editable`](EnzBreadcrumb::editable) adds a location field the caller
//! owns, so a global Ctrl+L or F6 can enter the same mode. A press on the
//! trail's empty tail emits [`Begin`](BreadcrumbEditEvent::Begin); crumbs keep
//! navigating. Enter emits [`Commit`](BreadcrumbEditEvent::Commit) and Escape
//! [`Cancel`](BreadcrumbEditEvent::Cancel); the caller feeds the resulting mode
//! back in the next view. After applying an event, run
//! [`edit_focus_operation`]: it focuses the field on Begin and restores focus
//! to the trail on Commit or Cancel. Wrap the bar in [`track_edit_bounds`] and
//! the whole view in [`editable_focus_scope`] so a click or focus move
//! elsewhere ends editing without stealing that interaction.
//!
//! [`autocomplete_editor`](EnzBreadcrumb::autocomplete_editor) uses the shared
//! typed suggestion field in edit mode. Its caller supplies queries/results and
//! handles typed selections; Escape still emits the bar's Cancel event.
//!
//! Both modes occupy the same rectangle with the same fill, border and inset,
//! so switching changes only the content: a list of path items, or one text
//! input. Search is not a third mode; an app composes an
//! [`EnzSearchField`](crate::search_field::EnzSearchField) in the same slot.

use crate::focus_button::FocusStatus;
use enzui_theme::Palette;
use iced::widget::button::Style;
use iced::{Border, Color, Element, Length, Padding};

/// The measured bounds of an editable breadcrumb, published by
/// [`track_edit_bounds`] and checked by [`editable_focus_scope`] to end editing
/// on an outside press or focus move.
pub type EditBounds = crate::focus_dismiss::DismissBounds;

/// Publish the editable breadcrumb's real translated bounds.
pub fn track_edit_bounds<'a, Message: 'a>(
    breadcrumb: impl Into<Element<'a, Message>>,
    bounds: &'a EditBounds,
) -> Element<'a, Message> {
    crate::focus_dismiss::track_bounds(breadcrumb, bounds)
}

/// Dismissal of an editable breadcrumb by focus moving away or an outside
/// press. `child` must be the complete containing view, not just the
/// breadcrumb, so outside controls can receive the press or focus
/// operation that dismisses the editor. `on_dismiss` is separate from
/// [`Cancel`](BreadcrumbEditEvent::Cancel) on purpose: Escape restores focus
/// to the trail, while an outside press or focus transfer keeps focus at its
/// destination.
pub fn editable_focus_scope<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    active: bool,
    edit_bounds: &'a EditBounds,
    bar_id: iced::advanced::widget::Id,
    on_dismiss: Message,
) -> Element<'a, Message> {
    crate::focus_dismiss::dismiss_scope(child, active, edit_bounds, bar_id, on_dismiss)
}

/// A user action in an editable breadcrumb location.
///
/// The caller owns the draft and `editing` boolean. Apply the event to that
/// state, rebuild the view, then queue [`edit_focus_operation`] with the same
/// bar id to complete the focus handoff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BreadcrumbEditEvent {
    /// Replace the trail with its editable text field.
    Begin,
    /// The text field draft changed.
    Changed(String),
    /// Accept the draft and return to the trail.
    Commit,
    /// Discard the draft and return to the trail.
    Cancel,
}

type EditorFactory<'a, Message> = Box<
    dyn FnOnce(
            iced::advanced::widget::Id,
            Option<Element<'a, Message>>,
            std::rc::Rc<dyn Fn(BreadcrumbEditEvent) -> Message + 'a>,
        ) -> Element<'a, Message>
        + 'a,
>;

struct Editable<'a, Message> {
    editing: bool,
    value: String,
    on_event: std::rc::Rc<dyn Fn(BreadcrumbEditEvent) -> Message + 'a>,
}

/// Focus handoff for a [`BreadcrumbEditEvent`].
///
/// Run this operation after applying the event and rebuilding the view.
/// Begin targets the text input now carrying `bar_id`. Commit and Cancel use
/// [`enzui_core::focus::RestoreFocus`], so a missing replacement trail
/// does not destructively clear focus. Draft changes need no handoff.
#[must_use]
pub fn edit_focus_operation<T: 'static>(
    event: &BreadcrumbEditEvent,
    bar_id: iced::advanced::widget::Id,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>> {
    use iced::advanced::widget::operation;

    match event {
        BreadcrumbEditEvent::Begin => Some(Box::new(operation::focusable::focus(bar_id))),
        BreadcrumbEditEvent::Commit | BreadcrumbEditEvent::Cancel => {
            Some(Box::new(enzui_core::focus::RestoreFocus::new(bar_id)))
        }
        BreadcrumbEditEvent::Changed(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// The text-field size the editable location, its idle trail surface and a
/// sibling titlebar search field all share.
///
/// One constant so the three location modes cannot drift into three different
/// heights, radii and insets (contract items 1a/1b/1c).
pub const EDIT_FIELD_SIZE: crate::text_field::TextFieldSize = crate::text_field::TextFieldSize::Md;

/// Color tokens for the breadcrumb bar.
#[derive(Debug, Clone, Copy)]
pub struct BreadcrumbTokens {
    /// Text color for the current (last) crumb — `text-color = text`.
    pub text_color: Color,
    /// The resting colour of ancestor crumbs, the body text colour (`text`).
    /// The current location is set apart by weight and selected state, not
    /// hue; hover shifts a crumb to [`hover_color`](Self::hover_color).
    pub link_color: Color,
    /// Previous-crumb hover colors — `hover-color = accent-hover`: darkened-accent-
    /// never distinct-palette roles. Normal-ink-resting ancestors make hover whole-
    /// affordance-supplying alone; invisible `text` -> `text` resting-to-hover steps.
    /// Hover-*only* accents: standard actionable-text reveals costing rest-nothing.
    pub hover_color: Color,
    /// Separator glyph color — `separator-color = disabled`.
    pub separator_color: Color,
}

impl BreadcrumbTokens {
    /// Derive tokens from a palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            text_color: p.text,
            // Ancestors rest at the same ink as the current crumb. The trail
            // is one readable path, and the current location is distinguished
            // by Bold weight + `selected(true)` rather than by hue -- neither
            // hyperlink blue nor muted secondary ink at rest.
            link_color: p.text,
            // Hover carries the entire actionable affordance, so it must
            // differ from the resting ink: the accent-hover role, on
            // pointer-over only.
            hover_color: p.accent_hover,
            separator_color: p.disabled,
        }
    }

    /// Dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Light preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// The cloneable half of a trail, rebuilt per layout pass by the measured
/// narrow collapse ([`EnzBreadcrumb::min_trail_width`]).
type CrumbDecorator<'a, Message> =
    std::rc::Rc<dyn Fn(usize, Element<'a, Message>) -> Element<'a, Message> + 'a>;

struct TrailParts<'a, Message> {
    items: Vec<String>,
    on_select: std::rc::Rc<dyn Fn(usize) -> Message + 'a>,
    text_size: f32,
    item_gap: f32,
    separator: Option<String>,
    direction: Option<enzui_core::placement::Direction>,
    max_visible: usize,
    tokens: Option<BreadcrumbTokens>,
    id_prefix: String,
    id_override: Option<String>,
    fill_terminal: bool,
    decorator: Option<CrumbDecorator<'a, Message>>,
}

impl<'a, Message: Clone + 'a> TrailParts<'a, Message> {
    /// Rebuild the trail builder, optionally collapsed to the current segment.
    ///
    /// `items` is passed whole in both cases, so `KeyedIds` still derives every
    /// crumb id and reconciliation key over the FULL trail: narrowing the
    /// window hides ancestors without renumbering the ones that remain.
    fn rebuild(&self, current_only: bool) -> EnzBreadcrumb<'a, Message> {
        let on_select = std::rc::Rc::clone(&self.on_select);
        let mut bar = enz_breadcrumb(self.items.clone(), move |index| on_select(index));
        bar.text_size = self.text_size;
        bar.item_gap = self.item_gap;
        bar.separator.clone_from(&self.separator);
        bar.direction = self.direction;
        bar.max_visible = self.max_visible;
        bar.tokens = self.tokens;
        bar.id_prefix.clone_from(&self.id_prefix);
        bar.id_override.clone_from(&self.id_override);
        bar.fill_terminal = self.fill_terminal;
        bar.decorator.clone_from(&self.decorator);
        bar.current_only = current_only;
        bar
    }
}

/// Builder for an enzUI-styled breadcrumb bar.
#[must_use]
pub struct EnzBreadcrumb<'a, Message> {
    items: Vec<String>,
    on_select: std::rc::Rc<dyn Fn(usize) -> Message + 'a>,
    /// `body-size` (13 px).
    text_size: f32,
    /// `spacing-sm` (8 px).
    item_gap: f32,
    /// Separator text; `None` (the default) draws a chevron icon pointing
    /// along the reading direction, as file managers' path bars do.
    separator: Option<String>,
    /// `max-visible`. 0 = show all; >= 2 = collapse.
    max_visible: usize,
    tokens: Option<BreadcrumbTokens>,
    /// Namespace for the crumb-derived link ids. Each clickable crumb
    /// gets `{id_prefix}-{crumb}` via [`enzui_core::identity::KeyedIds`],
    /// so an AT Focus/Click request follows the *path segment* rather than a
    /// slot. Defaults to `"enz-breadcrumb"`; set a distinct prefix when several
    /// bars share a screen (two trails both starting at "Home" would otherwise
    /// derive the same id — a collision no widget can see on its own).
    id_prefix: String,
    /// Caller override. See [`Self::id`]: it replaces the *namespace*, keeping
    /// the crumb-keyed suffix, so an explicit id never reintroduces positional
    /// identity.
    id_override: Option<String>,
    editable: Option<Editable<'a, Message>>,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    autocomplete_editor: Option<EditorFactory<'a, Message>>,
    trailing: Option<Element<'a, Message>>,
    fill_terminal: bool,
    decorator: Option<CrumbDecorator<'a, Message>>,
    /// Measured width below which the trail shows only the current segment.
    /// `0.0` disables the collapse. See [`Self::min_trail_width`].
    min_trail_width: f32,
    /// Render only the terminal crumb. Set by the measured collapse, never by
    /// a caller: crumb ids stay derived from the FULL trail either way.
    current_only: bool,
}

/// Creates a new enzUI-styled breadcrumb bar.
///
/// `on_select(index)` is fired when the user clicks a non-terminal crumb.
/// The final crumb is rendered as plain text (current location) and does
/// not emit a message.
pub fn enz_breadcrumb<'a, Message: 'a>(
    items: Vec<String>,
    on_select: impl Fn(usize) -> Message + 'a,
) -> EnzBreadcrumb<'a, Message> {
    EnzBreadcrumb {
        items,
        on_select: std::rc::Rc::new(on_select),
        // Idle, edit, and search presentations share `EDIT_FIELD_SIZE`'s text
        // size. A literal here would create a second source for one band and
        // silently survive a future retune of the field tier.
        // Reading the same tokens makes shared typography structural rather
        // than a coincidence that today's numbers happen to agree on.
        // An explicit `.text_size(..)` still overrides this default.
        text_size: EDIT_FIELD_SIZE.tokens().text_size,
        item_gap: 8.0,
        separator: None,
        max_visible: 0,
        tokens: None,
        id_prefix: "enz-breadcrumb".into(),
        id_override: None,
        editable: None,
        direction: None,
        autocomplete_editor: None,
        trailing: None,
        fill_terminal: false,
        decorator: None,
        min_trail_width: 0.0,
        current_only: false,
    }
}

impl<'a, Message: Clone + 'a> EnzBreadcrumb<'a, Message> {
    /// When the trail's measured width falls below `minimum`, show only the
    /// current segment, untruncated, and drop the ancestors and ellipsis. The
    /// ancestors stay reachable through the location editor, and crumb ids do
    /// not change. `0.0` (and negative or non-finite values) disables this.
    pub fn min_trail_width(mut self, minimum: f32) -> Self {
        self.min_trail_width = if minimum.is_finite() && minimum > 0.0 {
            minimum
        } else {
            0.0
        };
        self
    }

    /// Sets the text size, overriding the trail's default.
    ///
    /// The default is [`EDIT_FIELD_SIZE`]'s field text size, so an untouched
    /// breadcrumb matches its own edit mode and a sibling search field. A
    /// non-finite or non-positive argument falls back to that same default
    /// rather than to a literal, so the rejection path cannot reintroduce the
    /// drift the default just removed.
    pub fn text_size(mut self, size: f32) -> Self {
        let fallback = EDIT_FIELD_SIZE.tokens().text_size;
        self.text_size = enzui_core::metrics::FinitePositive::new_or(size, fallback).get();
        self
    }

    /// Sets the horizontal gap between items.
    pub fn item_gap(mut self, gap: f32) -> Self {
        self.item_gap = enzui_core::metrics::FiniteNonNeg::new_or(gap, 8.0).get();
        self
    }

    /// Override the writing direction. By default the trail follows the
    /// locale and runs from right to left in a right-to-left one.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    fn is_rtl(&self) -> bool {
        enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft
    }

    /// Separates crumbs with `sep` text (e.g. `"/"`, `"›"`, `"→"`) instead
    /// of the default chevron icon.
    pub fn separator(mut self, sep: impl Into<String>) -> Self {
        self.separator = Some(sep.into());
        self
    }

    /// The maximum number of visible crumbs; `0` shows all, and `>= 2`
    /// collapses the middle into an ellipsis. This is the only overflow
    /// mechanism: the bar does not measure its width, so set it to suit the
    /// space available.
    pub fn max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    /// Pin color tokens explicitly.
    pub fn tokens(mut self, tokens: BreadcrumbTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Namespace for the auto-derived crumb ids. Set a distinct prefix
    /// when several breadcrumb bars share a screen; the crumb strings alone are
    /// not unique across instances.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the derived identity for this bar.
    ///
    /// A breadcrumb is a *collection*, so there is no single id to set: each
    /// clickable crumb is its own AT target. The override therefore replaces
    /// the namespace, and every crumb id becomes `{id}-{crumb}`. This is
    /// deliberately not a per-crumb positional setter — handing out
    /// `{id}-0`, `{id}-1` would make focus follow a slot, which is worse than
    /// no id at all.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id_override = Some(id.into());
        self
    }

    /// Enables the caller-owned editable location. `editing` selects the
    /// mode and `value` is the current draft; the callback receives Begin,
    /// field changes, Commit and Cancel. See the module docs for the focus
    /// operation to run after each event.
    pub fn editable(
        mut self,
        editing: bool,
        value: impl Into<String>,
        on_event: impl Fn(BreadcrumbEditEvent) -> Message + 'a,
    ) -> Self {
        self.editable = Some(Editable {
            editing,
            value: value.into(),
            on_event: std::rc::Rc::new(on_event),
        });
        self
    }

    /// Replace the plain location input with a shared typed autocomplete field.
    ///
    /// Supply a field with caller-owned query text, input/selection callbacks
    /// and ranked results; keep its query aligned with [`Self::editable`]'s draft.
    /// This slot is used only while editing and retains the bar identity, Md size,
    /// Escape cancellation and trailing controls. Standalone label/error captions
    /// and responsive collapse are suppressed; error semantics remain available.
    /// After a typed selection, apply the caller's chosen draft/mode and use
    /// [`edit_focus_operation`] for the resulting Commit or Cancel.
    pub fn autocomplete_editor<T: 'a>(
        mut self,
        editor: crate::autocomplete_field::EnzAutocompleteField<'a, T, Message>,
    ) -> Self {
        self.autocomplete_editor = Some(Box::new(move |id, trailing, on_event| {
            editor.breadcrumb_editor(id, trailing, move || on_event(BreadcrumbEditEvent::Cancel))
        }));
        self
    }

    /// Places controls at the trailing edge in both trail and edit modes.
    ///
    /// In trail mode the controls remain children of the full-width click
    /// surface and consume their own presses before its empty-tail Begin
    /// action. In edit mode they occupy the text field's standard trailing
    /// slot. Each interactive child must publish its own accessible name.
    pub fn trailing(mut self, trailing: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// Wrap each visible crumb, for example with a native file drop target.
    /// The index belongs to the full trail; hidden ancestors and separators
    /// never acquire a drop area. Edit mode exposes only its text editor.
    pub fn decorate_crumb(
        mut self,
        decorator: impl Fn(usize, Element<'a, Message>) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.decorator = Some(std::rc::Rc::new(decorator));
        self
    }

    /// Converts this builder into an iced `Element`.
    pub fn into_element(self) -> Element<'a, Message> {
        self.into_collection()
    }

    /// The widget id of each crumb, in trail order, as the trail mounts
    /// them: what a caller passes to
    /// [`EnzDragItemSurface::items_at_widgets`](crate::drag_item_surface::EnzDragItemSurface::items_at_widgets)
    /// to make the ancestors drop targets. The current location, the last
    /// entry, mounts with no id, so nothing can be dropped onto it; a crumb
    /// collapsed into the ellipsis is not mounted at all.
    #[must_use]
    pub fn crumb_ids(&self) -> Vec<iced::advanced::widget::Id> {
        let mut keyed = enzui_core::identity::KeyedIds::new(
            self.id_override
                .clone()
                .unwrap_or_else(|| self.id_prefix.clone()),
        );
        self.items.iter().map(|crumb| keyed.id_for(crumb)).collect()
    }

    /// The trail as a [`KeyedRow`](crate::keyed_row::KeyedRow), so retained
    /// child state (focus, press, hover) follows each crumb rather than its
    /// slot when the trail changes.
    ///
    /// Keys: `crumb:{segment}` (repeats suffixed `#2`, `#3`), `sep:{key of the
    /// child after it}`, `ellipsis`, and `current` for the terminal location.
    /// They derive from the whole trail like the crumb ids, so the two cannot
    /// drift, and they do not use the joined ancestor path, which would change
    /// for every descendant when an ancestor is inserted.
    pub fn into_keyed_row(self) -> crate::keyed_row::KeyedRow<'a, Message> {
        let item_gap = self.item_gap;
        let rtl = self.is_rtl();
        crate::keyed_row::keyed_row(self.keyed_children())
            .mirrored(rtl)
            .spacing(item_gap)
            .align_y(iced::Alignment::Start)
    }

    fn into_collection(mut self) -> Element<'a, Message> {
        use enzui_core::semantics::{Role, Semantics};

        let rtl = self.is_rtl();

        let Some(editable) = self.editable.take() else {
            let row: Element<'a, Message> = self.into_keyed_row().into();
            // The non-editable breadcrumb stays shrink-wrapped.
            return crate::semantics_container::SemanticsContainer::new(
                row,
                Semantics::new(Role::ListBox).label(enzui_core::localization::text(
                    "enzui-breadcrumb-collection",
                )),
            )
            .into();
        };

        // The bar identity stays stable across modes; crumb identities retain their keyed suffixes.
        let namespace = self
            .id_override
            .clone()
            .unwrap_or_else(|| self.id_prefix.clone());
        let bar_id = iced::advanced::widget::Id::from(namespace);

        if editable.editing {
            let changed = std::rc::Rc::clone(&editable.on_event);
            let committed = std::rc::Rc::clone(&editable.on_event);
            let escaped = std::rc::Rc::clone(&editable.on_event);

            let field: Element<'a, Message> = if let Some(editor) = self.autocomplete_editor.take()
            {
                editor(bar_id, self.trailing.take(), editable.on_event)
            } else {
                let mut field = crate::text_field::enz_text_field_owned(
                    "",
                    enzui_core::localization::text("enzui-breadcrumb-location"),
                    editable.value,
                    move |value| changed(BreadcrumbEditEvent::Changed(value)),
                )
                .aria_label(enzui_core::localization::text(
                    "enzui-breadcrumb-collection",
                ))
                .id(bar_id)
                .size(EDIT_FIELD_SIZE)
                .on_submit_with(move || committed(BreadcrumbEditEvent::Commit));
                if let Some(trailing) = self.trailing.take() {
                    field = field.trailing(trailing);
                }
                field.into_element()
            };
            // Centre the field in the band, as the idle trail is centred.
            let field: Element<'a, Message> = iced::widget::container(field)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(iced::Alignment::Center)
                .into();

            // Editing owns unclaimed Escape; an open autocomplete popup handles its own cancellation.
            crate::keyboard_capture::keyboard_capture_unscoped(field, move |key, modifiers| {
                (modifiers.is_empty()
                    && matches!(
                        key,
                        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                    ))
                .then(|| escaped(BreadcrumbEditEvent::Cancel))
            })
        } else {
            let began = editable.on_event;
            let trailing = self.trailing.take();
            // In a full-width editable location bar the current segment is the
            // flexible child. A single unusually long directory name must
            // elide into the allocated residual instead of reporting its full
            // natural width and starving a sibling titlebar action region.
            self.fill_terminal = true;
            // The narrow collapse is resolved from the trail's own allocation.
            // Below `min_trail_width` the ancestors and the ellipsis are
            // dropped so the current folder keeps the whole residual and stays
            // readable, rather than sharing it with `Home › … ›` and eliding.
            let row: Element<'a, Message> = if self.min_trail_width > 0.0 {
                let minimum = self.min_trail_width;
                let parts = self.trail_parts();
                let branches = [true, false].map(|current_only| {
                    parts
                        .rebuild(current_only)
                        .into_keyed_row()
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_y(iced::Alignment::Center)
                        .into()
                });
                Element::new(crate::item_strip::Row::width_choice(minimum, branches))
            } else {
                self.into_keyed_row()
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_y(iced::Alignment::Center)
                    .into()
            };
            // The trail takes the residual width; the current segment elides.
            // Its inset matches the field's, read from the same size tokens,
            // so the content does not move between modes.
            let inset = EDIT_FIELD_SIZE.tokens().padding_x;
            // This container supplies only the field-parity horizontal inset;
            // the fixed-height focus surface below owns the vertical geometry.
            let trail = iced::widget::container(row)
                .width(Length::Fill)
                .padding(Padding::ZERO.left(inset).right(inset));
            let content: Element<'a, Message> = match trailing {
                Some(trailing) => iced::widget::container(
                    iced::widget::row![trail, trailing]
                        .mirrored(rtl)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_y(iced::Alignment::Center),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(if rtl {
                    Padding::ZERO.left(inset)
                } else {
                    Padding::ZERO.right(inset)
                })
                .into(),
                None => iced::widget::container(trail)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
            };
            // The collection wraps the complete location control, including
            // its trailing actions, so its stable id reports the same outer
            // bounds as the editor that replaces it. The child crumb nodes
            // retain their ListItem roles and independent ids.
            let collection = crate::semantics_container::SemanticsContainer::new(
                content,
                Semantics::new(Role::ListBox)
                    .label(enzui_core::localization::text(
                        "enzui-breadcrumb-collection",
                    ))
                    .default_action(),
            )
            .id(bar_id.clone());
            // Paint the idle field at the text field's intrinsic height, then
            // centre that complete surface in the titlebar band. Previously
            // `.height(Fill)` made the idle background 44px tall while edit
            // mode painted a 32px field at y=6; switching modes visibly
            // changed both size and chrome even though they are one control.
            let control: Element<'a, Message> =
                crate::focus_button::focus_button_maybe(collection, None)
                    .on_press_with(move || began(BreadcrumbEditEvent::Begin))
                    .width(Length::Fill)
                    .height(Length::Fixed(EDIT_FIELD_SIZE.tokens().height))
                    .padding(Padding::ZERO)
                    .id(bar_id)
                    .style_focus(editable_trail_style)
                    .into();
            iced::widget::container(control)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(iced::Alignment::Center)
                .into()
        }
    }

    /// Snapshot everything the crumb row is built from, so the measured
    /// collapse can rebuild it inside a `responsive` closure.
    ///
    /// Only the trail is captured. `trailing` and `editable` are siblings of
    /// the collection and are never rebuilt per layout pass, which is why an
    /// un-cloneable `Element` in those slots is not a problem here.
    fn trail_parts(&self) -> TrailParts<'a, Message> {
        TrailParts {
            items: self.items.clone(),
            on_select: std::rc::Rc::clone(&self.on_select),
            text_size: self.text_size,
            item_gap: self.item_gap,
            separator: self.separator.clone(),
            direction: self.direction,
            max_visible: self.max_visible,
            tokens: self.tokens,
            id_prefix: self.id_prefix.clone(),
            id_override: self.id_override.clone(),
            fill_terminal: self.fill_terminal,
            decorator: self.decorator.clone(),
        }
    }

    /// The complete visual sequence as `(reconciliation key, child)` pairs, in
    /// document order. See [`Self::into_keyed_row`] for the key scheme.
    fn keyed_children(self) -> Vec<(String, Element<'a, Message>)> {
        let text_size = self.text_size;
        let pinned = self.tokens;
        let separator_str = Separator {
            text: self.separator.clone(),
            rtl: self.is_rtl(),
        };
        let fill_terminal = self.fill_terminal;
        let on_select = self.on_select;
        let decorator = self.decorator;
        // Crumb ids come from the segment text, with repeated segments
        // disambiguated by occurrence over the full trail, so collapsing the
        // bar never repoints an id.
        let crumb_ids: Vec<iced::advanced::widget::Id> = {
            let mut keyed =
                enzui_core::identity::KeyedIds::new(self.id_override.unwrap_or(self.id_prefix));
            self.items.iter().map(|c| keyed.id_for(c)).collect()
        };

        // Compute visible indices per `collapsed`, `tail-start`.
        //
        // `current_only` is the measured narrow collapse ([`min_trail_width`]):
        // no ancestors and no ellipsis, so the entire residual is the current
        // folder's. It is deliberately not expressed as `max_visible == 1` --
        // the collapse rule requires `max_visible >= 2`, so a `1` there
        // means "no collapse" and would render the whole trail.
        let n = self.items.len();
        let current_only = self.current_only && n > 1;
        let collapsed = !current_only && self.max_visible >= 2 && n > self.max_visible;
        let tail_start = if current_only {
            n.saturating_sub(1)
        } else if collapsed {
            n.saturating_sub(self.max_visible - 1)
        } else {
            0
        };
        let last = n.saturating_sub(1);

        // Reconciliation keys follow the same occurrence rule through
        // `KeyedIds::key_for`, which also escapes a literal `#`.
        let crumb_keys: Vec<String> = {
            let mut keyed = enzui_core::identity::KeyedIds::new("crumb");
            self.items
                .iter()
                .map(|c| format!("crumb:{}", keyed.key_for(c)))
                .collect()
        };

        let mut children: Vec<(String, Element<'a, Message>)> = Vec::new();
        // The key of the last non-separator child pushed. A separator is keyed
        // by what precedes it, so it is stable for as long as that neighbour
        // is, and two separators can never share a key (each child emits at
        // most one trailing separator).
        let mut previous_key = String::new();

        for (index, crumb) in self.items.into_iter().enumerate() {
            let shown = if current_only {
                index >= tail_start
            } else {
                !collapsed || index == 0 || index >= tail_start
            };
            let ellipsis_here = collapsed && index == tail_start;

            if ellipsis_here {
                // The trail reads "root / … / tail": the ellipsis has its own
                // leading separator, and the following tail crumb supplies the
                // trailing one.
                children.push((
                    format!("sep:{previous_key}"),
                    sep_widget(&separator_str, text_size, pinned),
                ));
                children.push((ELLIPSIS_KEY.to_owned(), ellipsis_widget(text_size, pinned)));
                ELLIPSIS_KEY.clone_into(&mut previous_key);
                children.push((
                    format!("sep:{previous_key}"),
                    sep_widget(&separator_str, text_size, pinned),
                ));
            }

            if !shown {
                continue;
            }

            // Leading separator between visible crumbs (skip before the
            // first crumb). Also skip in the narrow `current_only` collapse:
            // `tail_start = n - 1`, so `index > 0` fires for the only visible
            // crumb and an extra chevron leaked into a mode whose contract is
            // "only the current folder". `current_only` implies no
            // prior crumbs have been pushed, so there is nothing for the
            // separator to sit between.
            if index > 0 && !current_only && (index != tail_start || !collapsed) {
                children.push((
                    format!("sep:{previous_key}"),
                    sep_widget(&separator_str, text_size, pinned),
                ));
            }

            // Terminal crumb: plain BOLD text (Original leaf `weight: "bold"`,
            // `text-color`). Non-terminal: a focusable link (FocusButton) so
            // it's a Tab stop, activates on Space/Enter, draws a focus ring,
            // and publishes list-item semantics + a default action.
            if index == last {
                CURRENT_KEY.clone_into(&mut previous_key);
                let mut leaf = crumb_run(crumb.clone(), text_size)
                    .weight_rung(enzui_theme::WeightRung::Bold)
                    .color_resolved(move |theme: &iced::Theme| {
                        pinned
                            .unwrap_or_else(|| {
                                crate::theme_lookup::resolve(theme, BreadcrumbTokens::from_palette)
                            })
                            .text_color
                    });
                if fill_terminal {
                    leaf = leaf.width(Length::Fill);
                }
                //  the current location is a member of the list,
                // not disconnected prose. It is a non-actionable `ListItem`
                // (no `default_action`, no focusable), marked `selected(true)`
                // — "you are here" — and positioned last (`n of n`). Published
                // through a grouping `SemanticsContainer` so the crumb's own
                // `text` node lands inside it and is de-duplicated against the
                // item's label, rather than surfacing as a second
                // root-level `Role::Label` for the same string.
                let semantics =
                    enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::ListItem)
                        .label(crumb)
                        .selected(true)
                        .set_position(enzui_core::semantics::SetPosition { index: n, count: n });
                // The terminal crumb is the only run in the trail that changes
                // face. Ancestors, separators and the ellipsis stay at the
                // trail weight, so their font is the baseline every crumb must
                // paint on; centring alone aligns line boxes, not baselines.
                let seated = crate::text_baseline::seat_on_baseline_of(
                    leaf,
                    crumb_run(String::new(), text_size).committed_font(),
                );
                children.push((
                    CURRENT_KEY.to_owned(),
                    // No widget id: the current location is not a routable
                    // target, and an id would publish it as one.
                    crate::semantics_container::SemanticsContainer::new(seated, semantics).into(),
                ));
                // `enz_text` publishes its own `Role::Label` node during
                // `operate`, which the SemanticsContainer above de-duplicates
                // against the ListItem label. The terminal crumb keeps
                // announcing normally (no `.announce(false)`).
            } else {
                // Lazy activation: the message is produced once per real
                // activation, not on every view rebuild.
                let on_select = std::rc::Rc::clone(&on_select);
                let crumb_id = crumb_ids[index].clone();
                let crumb_label = crumb.clone();
                previous_key.clone_from(&crumb_keys[index]);
                //  a list-item announces "item i of n" only if it
                // carries its position within the set. `SetPosition` is 1-based
                // (ARIA `aria-posinset`), and the count spans the whole trail
                // (every crumb including the current one), so the current
                // location is "n of n".
                let semantics =
                    enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::ListItem)
                        .label(crumb_label)
                        .set_position(enzui_core::semantics::SetPosition {
                            index: index + 1,
                            count: n,
                        })
                        .default_action();
                // `inherit_color` keeps the link ink driven by the enclosing
                // FocusButton's status `text_color` (accent at rest,
                // accent-hover on hover) rather than a pinned tone.
                let link_label = crumb_run(crumb, text_size).inherit_color(true);
                children.push((
                    crumb_keys[index].clone(),
                    crate::focus_button::focus_button_maybe(
                        Element::<'a, Message>::from(link_label),
                        None,
                    )
                    .on_press_with(move || (on_select)(index))
                    .padding(Padding::ZERO)
                    .id(crumb_id)
                    .semantics(semantics)
                    .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                        crumb_link_style(theme, status, pinned)
                    })
                    .into(),
                ));
            }
        }

        if let Some(decorator) = decorator {
            children
                .into_iter()
                .map(|(key, content)| {
                    let index = if key == CURRENT_KEY {
                        Some(n - 1)
                    } else {
                        crumb_keys.iter().position(|crumb| *crumb == key)
                    };
                    let content = match index {
                        Some(index) => decorator(index, content),
                        None => content,
                    };
                    (key, content)
                })
                .collect()
        } else {
            children
        }
    }
}

/// The idle trail's surface: the text-field family's fill and border, so the
/// click-to-edit tail reads as part of a control.
fn editable_trail_style(theme: &iced::Theme, status: FocusStatus) -> Style {
    use crate::focus_button::FocusRingTheme;

    // The idle trail is an editable control, not blank titlebar space. Match
    // the text-field family's filled surface and control boundary so the
    // click-to-edit tail remains visibly attributable without nesting a card.
    let background = crate::theme_lookup::resolve(theme, crate::tokens::field_fill);
    let border_color = match status {
        FocusStatus::Focused => theme.enz_focus_ring_color(Some(background)),
        FocusStatus::Hovered | FocusStatus::Pressed => {
            crate::theme_lookup::resolve(theme, |palette| palette.field_border_strong)
        }
        FocusStatus::Active | FocusStatus::Disabled => {
            crate::theme_lookup::resolve(theme, |palette| palette.field_border)
        }
    };
    let border_width = if matches!(status, FocusStatus::Focused) {
        theme.enz_focus_ring_width()
    } else {
        1.0
    };

    Style {
        background: Some(background.into()),
        text_color: crate::theme_lookup::resolve(theme, BreadcrumbTokens::from_palette).text_color,
        border: Border {
            color: border_color,
            width: border_width,
            radius: EDIT_FIELD_SIZE.tokens().radius.into(),
        },
        ..Style::default()
    }
}

/// The style of an ancestor crumb for the live theme and interaction state,
/// with the shared contrast-aware focus ring.
pub(crate) fn crumb_link_style(
    theme: &iced::Theme,
    status: FocusStatus,
    pinned: Option<BreadcrumbTokens>,
) -> Style {
    use crate::focus_button::FocusRingTheme;
    let tokens = crate::theme_lookup::resolve_pinned(
        crate::theme_lookup::PIN_SITE_BREADCRUMB,
        pinned,
        theme,
        BreadcrumbTokens::from_palette,
    );
    let color = match status {
        FocusStatus::Hovered | FocusStatus::Pressed => tokens.hover_color,
        _ => tokens.link_color,
    };
    let border = if matches!(status, FocusStatus::Focused) {
        Border {
            color: theme.enz_focus_ring_color(None),
            width: theme.enz_focus_ring_width(),
            radius: 2.0.into(),
        }
    } else {
        Border::default()
    };
    Style {
        background: None,
        text_color: color,
        border,
        ..Style::default()
    }
}

/// Reconciliation key for the collapse ellipsis. There is at most one per bar.
const ELLIPSIS_KEY: &str = "ellipsis";

/// Reconciliation key for the terminal (current-location) text. Deliberately
/// its own namespace rather than a crumb key: the terminal is a plain `text`
/// run while a crumb is a `FocusButton`, and a key that named one widget type
/// last frame and the other this frame would hand a text state to a button.
const CURRENT_KEY: &str = "current";

/// One crumb's text run, before its per-role ink and weight.
///
/// The run is single-line and end-elided: a segment containing a hyphen would
/// otherwise word-wrap and burst the enclosing pill.
///
/// Both crumb kinds are built here, and so is the reference font the terminal
/// crumb's baseline seat measures, so the trail has one typography source. A
/// crumb constructed another way could be seated on a font no other crumb
/// paints with.
fn crumb_run(text: String, text_size: f32) -> crate::text::EnzText {
    crate::enz_text::enz_text(text)
        .size_px(text_size)
        .single_line_elided()
}

/// A separator or collapse ellipsis, hidden from assistive technology.
///
/// A plain `text` publishes a `Role::Label` node (`collect.rs`), so screen
/// reader users would hear "slash" or "ellipsis" between crumbs.
/// `enz_text(..).announce(false)` renders the same but publishes no node, as
/// the banner and inspector do for their decorative runs.
fn decorative_glyph<'a, Message: 'a>(
    glyph: &str,
    text_size: f32,
    pinned: Option<BreadcrumbTokens>,
) -> Element<'a, Message> {
    crate::enz_text::enz_text(glyph.to_owned())
        .size_px(text_size)
        .color_resolved(move |theme: &iced::Theme| {
            pinned
                .unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, BreadcrumbTokens::from_palette)
                })
                .separator_color
        })
        .announce(false)
        .into()
}

/// What goes between crumbs: `text`, or a chevron along the reading
/// direction.
struct Separator {
    text: Option<String>,
    rtl: bool,
}

fn sep_widget<'a, Message: 'a>(
    sep: &Separator,
    text_size: f32,
    pinned: Option<BreadcrumbTokens>,
) -> Element<'a, Message> {
    if let Some(text) = &sep.text {
        return decorative_glyph(text, text_size, pinned);
    }
    let direction = if sep.rtl {
        enzui_core::placement::Direction::RightToLeft
    } else {
        enzui_core::placement::Direction::LeftToRight
    };
    let chevron = crate::icon::enz_icon_for_direction("navigation.forward", direction)
        .size(text_size.round())
        .tint_resolved(move |theme: &iced::Theme| {
            pinned
                .unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, BreadcrumbTokens::from_palette)
                })
                .separator_color
        })
        .into_element();
    // Centred on the crumbs' line, which is taller than the icon.
    iced::widget::container(chevron)
        .height(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

fn ellipsis_widget<'a, Message: 'a>(
    text_size: f32,
    pinned: Option<BreadcrumbTokens>,
) -> Element<'a, Message> {
    decorative_glyph("…", text_size, pinned)
}

impl<'a, Message: Clone + 'a> From<EnzBreadcrumb<'a, Message>> for Element<'a, Message> {
    fn from(b: EnzBreadcrumb<'a, Message>) -> Self {
        b.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
