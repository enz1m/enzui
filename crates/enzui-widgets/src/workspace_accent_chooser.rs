//! Theme-key workspace accent selection with one keyboard focus owner.
//!
//! Eight named radio choices include Default and the palette's seven accents.
//! [`EnzWorkspaceAccentChooser::columns`] arranges them in rows; the default
//! remains one row. Resolve the palette again when the application theme changes.
//! Left/Right wrap through choices; multi-row Up/Down move within a column,
//! skipping absent cells. Home/End choose endpoints and Enter/Space commit.
//! Ctrl/Alt/Logo chords and Escape remain available to the containing host.

use enzui_core::semantics::{Orientation as SemOrientation, Role, Semantics};
use enzui_theme::{WorkspaceAccentKey, WorkspaceAccentPalette};
use iced::Element;
use iced::widget::{column, row};
use std::rc::Rc;

use crate::color_swatch::{enz_color_swatch, enz_color_swatch_none};
use crate::keyboard_capture::{RovingGridOwner, RovingRing, roving_grid};

/// Default swatch edge / diameter for the chooser (WCAG 2.2 SC 2.5.8: 24 px).
pub const DEFAULT_SWATCH_SIZE: f32 = 24.0;

/// Default horizontal spacing between swatches.
pub const DEFAULT_SPACING: f32 = 8.0;

/// Default accessible label of the chooser radio group.
pub const DEFAULT_ARIA_LABEL: &str = "Workspace colour";

/// Accessible name of the *None / default* chip.
pub const NONE_LABEL: &str = "Default";

const OPTION_COUNT: usize = 1 + WorkspaceAccentKey::ALL.len();

fn selected_index(selected: Option<WorkspaceAccentKey>) -> usize {
    selected
        .and_then(|selected| {
            WorkspaceAccentKey::ALL
                .iter()
                .position(|key| *key == selected)
        })
        .map_or(0, |index| index + 1)
}

fn choice_at(index: usize) -> Option<Option<WorkspaceAccentKey>> {
    if index == 0 {
        Some(None)
    } else {
        WorkspaceAccentKey::ALL.get(index - 1).copied().map(Some)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChooserKeyAction {
    Next,
    Previous,
    First,
    Last,
    Activate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChooserKeyClaim {
    key: iced::keyboard::key::Named,
    action: ChooserKeyAction,
}

const CHOOSER_KEY_CLAIMS: &[ChooserKeyClaim] = &[
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::ArrowRight,
        action: ChooserKeyAction::Next,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::ArrowDown,
        action: ChooserKeyAction::Next,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::ArrowLeft,
        action: ChooserKeyAction::Previous,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::ArrowUp,
        action: ChooserKeyAction::Previous,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::Home,
        action: ChooserKeyAction::First,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::End,
        action: ChooserKeyAction::Last,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::Enter,
        action: ChooserKeyAction::Activate,
    },
    ChooserKeyClaim {
        key: iced::keyboard::key::Named::Space,
        action: ChooserKeyAction::Activate,
    },
];

fn choice_for_key(
    selected: Option<WorkspaceAccentKey>,
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    columns: usize,
) -> Option<Option<WorkspaceAccentKey>> {
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }

    let iced::keyboard::Key::Named(named) = key else {
        return None;
    };
    let action = CHOOSER_KEY_CLAIMS
        .iter()
        .find(|claim| claim.key == *named)?
        .action;

    let current = selected_index(selected);
    let target = if columns < OPTION_COUNT
        && matches!(
            named,
            iced::keyboard::key::Named::ArrowUp | iced::keyboard::key::Named::ArrowDown
        ) {
        let row_count = OPTION_COUNT.div_ceil(columns);
        let column = current % columns;
        let mut target_row = current / columns;
        loop {
            target_row = if *named == iced::keyboard::key::Named::ArrowDown {
                (target_row + 1) % row_count
            } else {
                (target_row + row_count - 1) % row_count
            };
            let target = target_row * columns + column;
            if target < OPTION_COUNT {
                break target;
            }
        }
    } else {
        match action {
            ChooserKeyAction::Next => (current + 1) % OPTION_COUNT,
            ChooserKeyAction::Previous => (current + OPTION_COUNT - 1) % OPTION_COUNT,
            ChooserKeyAction::First => 0,
            ChooserKeyAction::Last => OPTION_COUNT - 1,
            ChooserKeyAction::Activate => current,
        }
    };
    choice_at(target)
}

/// Construct a named radio chooser for a theme-key workspace tint.
pub fn enz_workspace_accent_chooser<Message, F>(
    palette: WorkspaceAccentPalette,
    selected: Option<WorkspaceAccentKey>,
    on_choose: F,
) -> EnzWorkspaceAccentChooser<Message, F>
where
    Message: Clone + 'static,
    F: Fn(Option<WorkspaceAccentKey>) -> Message + 'static,
{
    EnzWorkspaceAccentChooser {
        palette,
        selected,
        on_choose,
        aria_label: enzui_core::localization::text("enzui-workspace-colour"),
        swatch_size: DEFAULT_SWATCH_SIZE,
        spacing: DEFAULT_SPACING,
        columns: OPTION_COUNT,
        id_prefix: "enz-workspace-accent".to_owned(),
        _phantom: std::marker::PhantomData,
    }
}

/// Builder for [`enz_workspace_accent_chooser`].
pub struct EnzWorkspaceAccentChooser<Message, F> {
    palette: WorkspaceAccentPalette,
    selected: Option<WorkspaceAccentKey>,
    on_choose: F,
    aria_label: String,
    swatch_size: f32,
    spacing: f32,
    columns: usize,
    id_prefix: String,
    _phantom: std::marker::PhantomData<fn() -> Message>,
}

impl<Message, F> EnzWorkspaceAccentChooser<Message, F>
where
    Message: Clone + 'static,
    F: Fn(Option<WorkspaceAccentKey>) -> Message + 'static,
{
    /// Override the accessible name of the radio-group container. Defaults to
    /// [`DEFAULT_ARIA_LABEL`].
    #[must_use]
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = label.into();
        self
    }

    /// Override each swatch's edge / diameter in px. Defaults to
    /// [`DEFAULT_SWATCH_SIZE`] (24). Finite values are clamped to that 24 px
    /// target floor; non-finite / negative values are ignored. A compact
    /// chooser may shrink its painted mark in a future token, but never its
    /// interactive target.
    #[must_use]
    pub fn swatch_size(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.swatch_size = px.max(DEFAULT_SWATCH_SIZE);
        }
        self
    }

    /// Override the inter-swatch spacing in px. Defaults to [`DEFAULT_SPACING`].
    #[must_use]
    pub fn spacing(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.spacing = px;
        }
        self
    }

    /// Limit each row to this many choices; defaults to all eight in one row.
    ///
    /// Values clamp to 1–8. Up/Down retain linear navigation in the default
    /// single row, and follow columns with wrap in a multi-row arrangement.
    #[must_use]
    pub fn columns(mut self, columns: usize) -> Self {
        self.columns = columns.clamp(1, OPTION_COUNT);
        self
    }

    /// Namespace the derived widget ids so two chooser instances on one screen
    /// (light and dark previews on a settings page, say) do not collide.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element<'a>(self) -> Element<'a, Message>
    where
        Message: 'a,
        F: 'a,
    {
        let radius = self.swatch_size * 0.5;
        let selected = self.selected;
        let cursor = selected_index(selected);
        let id_prefix = self.id_prefix;
        let on_choose = Rc::new(self.on_choose);
        let columns = self.columns;
        let none_id = enzui_core::identity::derive_id(&id_prefix, "none");
        let mut choices: Vec<Element<'a, Message>> = vec![
            enz_color_swatch_none((on_choose)(None))
                .radio_option()
                .aria_label(enzui_core::localization::text("enzui-workspace-default"))
                .selected(selected.is_none())
                .size(self.swatch_size)
                .radius(radius)
                .id(none_id.clone())
                .into(),
        ];
        for key in WorkspaceAccentKey::ALL {
            choices.push(
                enz_color_swatch(self.palette.resolve(*key), (on_choose)(Some(*key)))
                    .radio_option()
                    .aria_label(key.label())
                    .selected(selected == Some(*key))
                    .size(self.swatch_size)
                    .radius(radius)
                    // Persisted keys keep routing stable when theme pigments change.
                    .id(enzui_core::identity::derive_id(&id_prefix, key.id_str()))
                    .into(),
            );
        }
        let grid: Element<'a, Message> = if columns == OPTION_COUNT {
            row(choices)
                .spacing(self.spacing)
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            let mut rows = column![].spacing(self.spacing);
            let mut choices = choices.into_iter();
            loop {
                let cells: Vec<_> = choices.by_ref().take(columns).collect();
                if cells.is_empty() {
                    break;
                }
                rows = rows.push(
                    row(cells)
                        .spacing(self.spacing)
                        .align_y(iced::Alignment::Center),
                );
            }
            rows.into()
        };

        let active_id = match selected {
            None => none_id,
            Some(key) => enzui_core::identity::derive_id(&id_prefix, key.id_str()),
        };
        let owner_id = enzui_core::identity::derive_id(&id_prefix, "group");
        let semantics = Semantics::new(Role::RadioGroup)
            .label(self.aria_label)
            .orientation(SemOrientation::Horizontal)
            .active_descendant(active_id);
        let on_key = on_choose;

        roving_grid(
            grid,
            true,
            RovingGridOwner::new(owner_id, semantics, false),
            move |layout, tree| {
                if columns == OPTION_COUNT {
                    layout
                        .children(&tree.children)
                        .nth(cursor)
                        .map(|child| child.bounds())
                } else {
                    let row_index = cursor / columns;
                    let row_layout = layout.children(&tree.children).nth(row_index)?;
                    row_layout
                        .children(&tree.children[row_index].children)
                        .nth(cursor % columns)
                        .map(|child| child.bounds())
                }
            },
            move |theme| {
                let palette = crate::theme_lookup::palette(theme);
                RovingRing {
                    color: palette.focus_ring,
                    width: crate::theme_lookup::focus_ring_width(theme),
                    radius,
                }
            },
            move |key, modifiers, _cursor_bounds| {
                choice_for_key(selected, key, modifiers, columns).map(|choice| (on_key)(choice))
            },
        )
    }
}

impl<'a, Message, F> From<EnzWorkspaceAccentChooser<Message, F>> for Element<'a, Message>
where
    Message: Clone + 'static + 'a,
    F: Fn(Option<WorkspaceAccentKey>) -> Message + 'static + 'a,
{
    fn from(w: EnzWorkspaceAccentChooser<Message, F>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
