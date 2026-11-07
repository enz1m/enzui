//! Menu command rows with icons, shortcut hints, descriptions and checkmarks.
//!
//! Standalone rows use [`crate::focus_button::FocusButton`] for pointer and
//! keyboard activation. [`EnzMenuRow::embedded`] leaves keyed keyboard handling
//! and focus ownership to the menu. [`enz_menu_separator`] is inert chrome.
//!
//! Unpinned colours resolve from the live theme; geometry is chosen at view
//! construction. Tokens can pin both. Destructive rows tint their icon, or the
//! label when no icon is configured. Labels and descriptions stay on one line
//! and elide; descriptions select the taller row height. Trailing icons precede
//! checkmarks, submenu chevrons and shortcut hints.
//!
//! Hover follows the theme's motion policy or a per-row reduced-motion override;
//! pressed and highlighted fills switch immediately.

use crate::focus_button::{FocusStatus, focus_button_maybe, focus_button_maybe_with};
use crate::row_motion::{RowMotion, RowMotionProgress, lerp_color};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{column, container, row, text};
use iced::{Background, Border, Color, Length, Padding};
use std::cell::Cell;
use std::rc::Rc;

/// Colour + geometry tokens for a menu row.
#[derive(Debug, Clone, Copy)]
pub struct MenuRowTokens {
    /// Primary label colour.
    pub text: Color,
    /// Second-line description colour.
    pub description: Color,
    /// Trailing shortcut hint colour.
    pub trailing: Color,
    /// Destructive-tone label colour.
    pub destructive: Color,
    /// Disabled label / description / trailing colour.
    pub disabled: Color,
    /// Background painted when hovered.
    pub hover_background: Color,
    /// 1 px separator line colour.
    pub separator: Color,
    /// Selected-checkmark accent; pinned tokens override the live accent.
    pub check_accent: Color,
    /// Keyboard focus-ring colour.
    pub focus_ring: Color,
    /// Single-line row height.
    pub row_height: f32,
    /// Two-line row height (label + description).
    pub row_height_two_line: f32,
    /// Corner radius for hover background.
    pub row_radius: f32,
    /// Vertical background inset in pixels; defaults to 1 without changing layout or hit bounds.
    pub background_inset_y: f32,
    /// Horizontal padding on each side.
    pub padding_x: f32,
    /// Body text size.
    pub body_size: f32,
    /// Description text size.
    pub description_size: f32,
    /// Leading/trailing icon size (`icon-size` = 16).
    pub icon_size: f32,
    /// Gap between row cells (icon ↔ label ↔ trailing), `spacing-sm` = 8.
    pub row_gap: f32,
    /// Gap between the label and its description line, `spacing-1` = 1.
    pub description_gap: f32,
    /// Vertical inset above/below a separator line, `spacing-xs` = 4.
    pub separator_inset: f32,
}

impl MenuRowTokens {
    /// Resolve palette colours and ambient typography with scaled row heights and icon size.
    pub fn from_palette(p: &Palette) -> Self {
        use enzui_theme::text_scale::ui_px;
        let ramp = enzui_theme::TypographyScale::ambient();
        Self {
            text: p.text,
            description: p.text_muted,
            trailing: p.text_muted,
            destructive: p.destructive,
            disabled: p.disabled,
            hover_background: p.surface_hover,
            separator: p.border,
            check_accent: p.accent,
            focus_ring: p.focus_ring,
            row_height: ui_px(28.0),
            row_height_two_line: ui_px(46.0),
            row_radius: 6.0,
            background_inset_y: 1.0,
            padding_x: 12.0,
            body_size: ramp.body_size,
            description_size: ramp.label_size,
            icon_size: ui_px(16.0),
            row_gap: 8.0,
            description_gap: 1.0,
            separator_inset: 4.0,
        }
    }
    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replace negative or non-finite geometry with current dark-token defaults.
    /// Zero is preserved except for body and description text sizes, which must be
    /// positive. Fallbacks include ambient typography and text scaling; colours pass through.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        let d = Self::dark();
        Self {
            row_height: FiniteNonNeg::new_or(self.row_height, d.row_height).get(),
            row_height_two_line: FiniteNonNeg::new_or(
                self.row_height_two_line,
                d.row_height_two_line,
            )
            .get(),
            row_radius: FiniteNonNeg::new_or(self.row_radius, d.row_radius).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, d.padding_x).get(),
            background_inset_y: FiniteNonNeg::new_or(self.background_inset_y, d.background_inset_y)
                .get(),
            // Zero text sizes can panic in the text renderer; geometry extents may remain zero.
            body_size: FinitePositive::new_or(self.body_size, d.body_size).get(),
            description_size: FinitePositive::new_or(self.description_size, d.description_size)
                .get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, d.icon_size).get(),
            row_gap: FiniteNonNeg::new_or(self.row_gap, d.row_gap).get(),
            description_gap: FiniteNonNeg::new_or(self.description_gap, d.description_gap).get(),
            separator_inset: FiniteNonNeg::new_or(self.separator_inset, d.separator_inset).get(),
            ..*self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LabelTone {
    Default,
    Destructive,
    Disabled,
}

fn disabled_ink(theme: &iced::Theme, preferred: Color, backdrop: Color) -> Color {
    crate::theme_lookup::palette(theme).disabled_on_with_preferred(preferred, backdrop)
}

impl LabelTone {
    fn pick(self, t: &MenuRowTokens) -> Color {
        match self {
            LabelTone::Default => t.text,
            LabelTone::Destructive => t.destructive,
            LabelTone::Disabled => t.disabled,
        }
    }
}

/// Choose normal ink or a destructive icon, falling back to a destructive icon-less label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuRowTone {
    /// Regular action — label uses the theme `text` token (the default).
    #[default]
    Default,
    /// Dangerous action — the icon, or an icon-less label, uses `destructive`
    /// (e.g. "Delete Account").
    Destructive,
}

/// Place the icon before the label or in the trailing lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MenuRowIconPosition {
    /// Icon at the left edge, before the label.
    #[default]
    Leading,
    /// Icon after the label and before checkmarks, submenu chevrons and shortcut hints.
    Trailing,
}

/// Menu-row builder.
#[must_use]
pub struct EnzMenuRow<'a, Message> {
    label: String,
    trailing: String,
    description: String,
    icon: String,
    icon_source: Option<crate::icon::IconSource>,
    icon_position: MenuRowIconPosition,
    /// The icon's own colour, over the row's ink.
    icon_tint: Option<Color>,
    on_activated: Option<Message>,
    on_activated_lazy: Option<Box<dyn Fn() -> Message + 'a>>,
    tokens: Option<MenuRowTokens>,
    tone: MenuRowTone,
    enabled: bool,
    separator: bool,
    selected: bool,
    reserve_leading: bool,
    highlighted: bool,
    checkable: bool,
    radio: bool,
    embedded: bool,
    submenu_indicator: bool,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    id: Option<iced::advanced::widget::Id>,
    set_position: Option<enzui_core::semantics::SetPosition>,
    reduced_motion: Option<bool>,
    measure_shrink: bool,
    mnemonic: bool,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a, Message: Clone + 'a> EnzMenuRow<'a, Message> {
    /// Reads an underscore in the label as the access key marker
    /// (`"_Open"`, see [`crate::mnemonic`]): the marker is not shown, and the
    /// letter is underlined while Alt is held. The menu decides what the key
    /// does (the menu bar chooses the row).
    pub fn mnemonic(mut self, enabled: bool) -> Self {
        self.mnemonic = enabled;
        self
    }

    /// Set the trailing shortcut hint and its native accessibility property
    /// (for example, `"Ctrl+S"`). An empty hint is omitted from both.
    pub fn trailing(mut self, s: impl Into<String>) -> Self {
        self.trailing = s.into();
        self
    }

    /// Leading icon (Lucide canonical name or semantic alias resolved by
    /// `enzui-icons`). Empty string omits the icon.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// Paint the icon in `color` (a container's or an account's colour)
    /// instead of the row's ink. The label keeps its ink, and a disabled row
    /// shows its disabled ink on the icon too.
    pub fn icon_tint(mut self, color: Color) -> Self {
        self.icon_tint = Some(color);
        self
    }

    /// Use an app-owned icon source; nonempty bundled names override [`Self::icon`].
    /// An empty bundled name falls back to the configured icon. Bundled icons use row
    /// tint; SVG and image handles retain their colours. The icon remains decorative.
    /// Any configured source marks the row as icon-bearing for destructive tone, even
    /// an empty bundled name.
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.icon_source = Some(source.into());
        self
    }

    /// Place the icon before the label by default, or before trailing checks and hints.
    pub fn icon_position(mut self, pos: MenuRowIconPosition) -> Self {
        self.icon_position = pos;
        self
    }

    /// Set single-line description text and its accessibility description.
    /// Any nonempty string selects the taller row height, including whitespace.
    pub fn description(mut self, s: impl Into<String>) -> Self {
        self.description = s.into();
        self
    }

    /// Override the live theme's hover-motion policy for this row.
    /// True snaps the hover transition; false enables it even under reduced motion.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Pin row colours and geometry after [`MenuRowTokens::sanitized`] normalization.
    /// Disabled ink can still be adjusted for contrast against the live elevated surface.
    pub fn tokens(mut self, t: MenuRowTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }

    /// Set the tone (default vs destructive).
    pub fn tone(mut self, t: MenuRowTone) -> Self {
        self.tone = t;
        self
    }

    /// Enable / disable the row.
    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }

    /// Render as a horizontal 1-px separator instead of a tappable row.
    /// Prefer [`enz_menu_separator`] which needs no label/message.
    pub fn separator(mut self, on: bool) -> Self {
        self.separator = on;
        self
    }

    /// Reserve an icon-width leading gutter to align labels with icon-bearing rows.
    /// Selected checkmarks use the trailing lane and do not reserve this gutter.
    pub fn reserve_leading(mut self, on: bool) -> Self {
        self.reserve_leading = on;
        self
    }

    /// Mark the row as selected: a trailing accent checkmark is drawn in the
    /// trailing lane after any trailing icon and before the shortcut hint.
    /// Does not reserve the leading gutter.
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// Paint the active-row fill immediately; embedded rows also receive the focus-visible flag.
    pub fn highlighted(mut self, on: bool) -> Self {
        self.highlighted = on;
        self
    }

    /// Leave the row's Tab stop and Enter/Space handling to its owning roving menu.
    /// Pointer activation remains available; [`Self::highlighted`] supplies the focus-visible flag.
    pub fn embedded(mut self, on: bool) -> Self {
        self.embedded = on;
        self
    }

    pub(crate) fn measure_shrink(mut self, on: bool) -> Self {
        self.measure_shrink = on;
        self
    }

    /// Attach a stable widget id, so an owning surface can name this row as its
    /// active descendant, and so AT action requests route back here.
    ///
    /// Required for the `aria-activedescendant` pattern: a query field or menu
    /// trigger points at this row by id while keeping keyboard focus itself.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Publish a 1-based position and total count within the menu.
    pub fn set_position(mut self, position: enzui_core::semantics::SetPosition) -> Self {
        self.set_position = Some(position);
        self
    }

    /// Publish [`Role::MenuItemCheckBox`] instead of a command role.
    /// Pair with [`Self::selected`] for checked state and the trailing checkmark.
    pub fn checkable(mut self, on: bool) -> Self {
        self.checkable = on;
        self
    }

    /// Publish [`Role::MenuItemRadio`]: a checkable row that is one choice
    /// of several.
    pub fn radio(mut self, on: bool) -> Self {
        self.radio = on;
        self
    }

    /// Override the writing direction. By default the row follows the
    /// locale; right-to-left lays it out from the right and points the
    /// submenu chevron left.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Mark the row as a submenu opener with a trailing chevron, after any
    /// trailing icon and checkmark and before the shortcut text. It is tinted
    /// like the row's icon (dimmed when disabled, danger when destructive).
    /// Separators never draw it.
    pub fn submenu_indicator(mut self, on: bool) -> Self {
        self.submenu_indicator = on;
        self
    }

    fn label_tone(&self) -> LabelTone {
        if !self.enabled {
            LabelTone::Disabled
        } else {
            match self.tone {
                MenuRowTone::Destructive => LabelTone::Destructive,
                MenuRowTone::Default => LabelTone::Default,
            }
        }
    }

    /// Convert into an iced `iced::Element`.
    pub fn into_element(mut self) -> iced::Element<'a, Message> {
        let hover_progress: RowMotionProgress = Rc::new(Cell::new(0.0));
        let reduced_motion = self.reduced_motion;
        let is_separator = self.separator;
        let t = self.tokens.unwrap_or_else(MenuRowTokens::dark);
        let pinned = self.tokens;
        let color_of = move |theme: &iced::Theme, pick: fn(&MenuRowTokens) -> Color| -> Color {
            let tk = pinned.unwrap_or_else(|| {
                crate::theme_lookup::resolve(theme, MenuRowTokens::from_palette)
            });
            pick(&tk)
        };
        if self.separator {
            let inset = t.separator_inset;
            let line = container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(move |theme: &iced::Theme| container::Style {
                    background: Some(color_of(theme, |t| t.separator).into()),
                    ..Default::default()
                });
            let rule = container(line)
                .width(Length::Fill)
                .padding(Padding::from([inset, 0.0]));
            // A rule between groups, announced as one.
            return crate::semantics_container::SemanticsContainer::leaf(
                rule,
                enzui_core::semantics::Semantics::new(Role::Separator),
            )
            .into();
        }

        let has_desc = !self.description.is_empty();
        let row_height = if has_desc {
            t.row_height_two_line
        } else {
            t.row_height
        };

        let mnemonic = self
            .mnemonic
            .then(|| enzui_core::mnemonic::Mnemonic::parse(&self.label));
        if let Some(mnemonic) = &mnemonic {
            self.label.clone_from(&mnemonic.text);
        }
        let a11y_label = self.label.clone();
        let a11y_description = self.description.clone();
        let a11y_shortcut = (!self.trailing.is_empty()).then(|| self.trailing.clone());
        let selected = self.selected;
        let label_tone = self.label_tone();
        let resolve_tokens = move |theme: &iced::Theme| -> MenuRowTokens {
            crate::theme_lookup::resolve_pinned(
                crate::theme_lookup::PIN_SITE_MENU_ROW,
                pinned,
                theme,
                MenuRowTokens::from_palette,
            )
        };
        let has_icon = self.icon_source.is_some() || !self.icon.is_empty();
        let text_tone = match label_tone {
            LabelTone::Destructive if has_icon => LabelTone::Default,
            tone => tone,
        };
        let label_color_fn = move |theme: &iced::Theme| -> Color {
            let tokens = resolve_tokens(theme);
            if matches!(text_tone, LabelTone::Disabled) {
                let p = crate::theme_lookup::palette(theme);
                disabled_ink(theme, tokens.disabled, p.surface_elevated)
            } else {
                text_tone.pick(&tokens)
            }
        };
        let body_size = t.body_size;
        let desc_size = t.description_size;
        // Single-line elision preserves the trailing controls under width compression.
        let direction = enzui_core::placement::resolve_direction(self.direction);
        let start = if direction == enzui_core::placement::Direction::RightToLeft {
            iced::alignment::Horizontal::Right
        } else {
            iced::alignment::Horizontal::Left
        };
        let label_font = enzui_theme::text_defaults::font(
            enzui_theme::FontRole::Ui,
            enzui_theme::WeightRung::Medium,
        );
        let label_width = if self.measure_shrink {
            Length::Shrink
        } else {
            Length::Fill
        };
        let label_color_fn = std::rc::Rc::new(label_color_fn);
        let plain_color = std::rc::Rc::clone(&label_color_fn);
        let plain: iced::Element<'a, Message> = text(self.label.clone())
            .align_x(start)
            .size(body_size)
            .width(label_width)
            .ellipsis(iced::widget::text::Ellipsis::End)
            .wrapping(iced::widget::text::Wrapping::None)
            .font(label_font)
            .style(move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(plain_color(theme)),
            })
            .into();
        let label_widget: iced::Element<'a, Message> = match mnemonic
            .and_then(|mnemonic| mnemonic.at)
            .filter(|at| self.label.get(at.clone()).is_some())
        {
            Some(at) => {
                let label = &self.label;
                let spans: [iced::advanced::text::Span<'_, ()>; 3] = [
                    iced::widget::span(label.get(..at.start).unwrap_or_default().to_owned()),
                    iced::widget::span(label.get(at.clone()).unwrap_or_default().to_owned())
                        .underline(true),
                    iced::widget::span(label.get(at.end..).unwrap_or_default().to_owned()),
                ];
                let marked_color = std::rc::Rc::clone(&label_color_fn);
                let marked = iced::widget::rich_text(spans)
                    .align_x(start)
                    .size(body_size)
                    .width(label_width)
                    .ellipsis(iced::widget::text::Ellipsis::End)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .font(label_font)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(marked_color(theme)),
                    });
                crate::mnemonic::mnemonic_label(plain, marked.into())
            }
            None => plain,
        };

        let stack: iced::Element<'a, Message> = if has_desc {
            let enabled = self.enabled;
            let desc_style = move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(if !enabled {
                    let p = crate::theme_lookup::palette(theme);
                    disabled_ink(theme, color_of(theme, |t| t.disabled), p.surface_elevated)
                } else {
                    color_of(theme, |t| t.description)
                }),
            };
            column![
                label_widget,
                text(self.description)
                    .align_x(start)
                    .size(desc_size)
                    .width(if self.measure_shrink {
                        Length::Shrink
                    } else {
                        Length::Fill
                    })
                    .ellipsis(iced::widget::text::Ellipsis::End)
                    .wrapping(iced::widget::text::Wrapping::None)
                    .style(desc_style),
            ]
            .spacing(t.description_gap)
            .into()
        } else {
            label_widget
        };
        let trailing: Option<iced::Element<'a, Message>> = if self.trailing.is_empty() {
            None
        } else {
            let enabled = self.enabled;
            let trailing_style = move |theme: &iced::Theme| iced::widget::text::Style {
                color: Some(if !enabled {
                    let p = crate::theme_lookup::palette(theme);
                    disabled_ink(theme, color_of(theme, |t| t.disabled), p.surface_elevated)
                } else {
                    color_of(theme, |t| t.trailing)
                }),
            };
            Some(
                text(self.trailing)
                    .size(body_size)
                    .style(trailing_style)
                    .into(),
            )
        };
        let icon_size = t.icon_size;
        let icon_tint = self.icon_tint;
        let icon_color = move |theme: &iced::Theme| -> Color {
            let tokens = resolve_tokens(theme);
            if matches!(label_tone, LabelTone::Disabled) {
                let p = crate::theme_lookup::palette(theme);
                disabled_ink(theme, tokens.disabled, p.surface_elevated)
            } else {
                icon_tint.unwrap_or_else(|| label_tone.pick(&tokens))
            }
        };
        let icon_position = self.icon_position;
        let chosen_icon: Option<iced::Element<'a, Message>> = match self.icon_source {
            Some(source) if !source.is_empty() => {
                Some(source.into_slot_element(icon_size, |icon| icon.tint_resolved(icon_color)))
            }
            Some(_) | None => {
                if !self.icon.is_empty() {
                    Some(
                        crate::icon::enz_icon(&self.icon)
                            .size(icon_size)
                            .tint_resolved(icon_color)
                            .into(),
                    )
                } else {
                    None
                }
            }
        };
        let (leading_icon_el, trailing_icon_el): (
            Option<iced::Element<'a, Message>>,
            Option<iced::Element<'a, Message>>,
        ) = if let Some(el) = chosen_icon {
            match icon_position {
                MenuRowIconPosition::Leading => (Some(el), None),
                MenuRowIconPosition::Trailing => (None, Some(el)),
            }
        } else {
            (None, None)
        };

        let leading: Option<iced::Element<'a, Message>> = if leading_icon_el.is_some() {
            leading_icon_el
        } else if self.reserve_leading {
            Some(
                container(iced::widget::Space::new())
                    .width(Length::Fixed(icon_size))
                    .into(),
            )
        } else {
            None
        };

        let trailing_icon: Option<iced::Element<'a, Message>> = trailing_icon_el;
        let selected_check: Option<iced::Element<'a, Message>> = if self.selected {
            let enabled = self.enabled;
            let check_color = move |theme: &iced::Theme| -> Color {
                if enabled {
                    color_of(theme, |t| t.check_accent)
                } else {
                    let p = crate::theme_lookup::palette(theme);
                    disabled_ink(theme, color_of(theme, |t| t.disabled), p.surface_elevated)
                }
            };
            Some(
                crate::icon::enz_icon("selection.check")
                    .size(icon_size)
                    .tint_resolved(check_color)
                    .into(),
            )
        } else {
            None
        };
        let submenu_chevron: Option<iced::Element<'a, Message>> =
            if self.submenu_indicator && !self.separator {
                Some(
                    crate::icon::enz_icon_for_direction("navigation.forward", direction)
                        .size(icon_size)
                        .tint_resolved(icon_color)
                        .into(),
                )
            } else {
                None
            };
        let lane_width = if self.measure_shrink {
            Length::Shrink
        } else {
            Length::Fill
        };
        let mut cells: Vec<iced::Element<'a, Message>> = Vec::new();
        if let Some(l) = leading {
            cells.push(l);
        }
        cells.push(container(stack).width(lane_width).into());
        if let Some(ti) = trailing_icon {
            cells.push(ti);
        }
        if let Some(chk) = selected_check {
            cells.push(chk);
        }
        if let Some(chev) = submenu_chevron {
            cells.push(chev);
        }
        if let Some(tr) = trailing {
            cells.push(tr);
        }
        let content: iced::Element<'a, Message> = row(cells)
            .mirrored(direction == enzui_core::placement::Direction::RightToLeft)
            .spacing(t.row_gap)
            .align_y(iced::Alignment::Center)
            .into();

        let radius = t.row_radius;
        let enabled = self.enabled;
        let highlighted = self.highlighted;
        let on_press_eager = if enabled { self.on_activated } else { None };
        let on_press_lazy = if enabled {
            self.on_activated_lazy
        } else {
            None
        };
        let role = if self.checkable && self.radio {
            Role::MenuItemRadio
        } else if self.checkable {
            Role::MenuItemCheckBox
        } else {
            Role::MenuItem
        };
        let mut semantics = Semantics::new(role)
            .default_action()
            .label(a11y_label)
            .destructive_action(matches!(label_tone, LabelTone::Destructive))
            .disabled(!enabled);
        if !a11y_description.is_empty() {
            semantics = semantics.description(a11y_description);
        }
        if let Some(shortcut) = &a11y_shortcut {
            semantics = semantics.keyboard_shortcut(shortcut.clone());
        }
        // A check or radio row is checked or not (`aria-checked`); only a
        // plain row reports selection.
        if self.checkable {
            semantics = semantics.toggled(if selected {
                enzui_core::semantics::Toggle::True
            } else {
                enzui_core::semantics::Toggle::False
            });
        } else if selected {
            semantics = semantics.selected(true);
        }
        if let Some(position) = self.set_position {
            semantics = semantics.set_position(position);
        }

        let embedded = self.embedded;
        let btn_width = if self.measure_shrink {
            Length::Shrink
        } else {
            Length::Fill
        };
        let mut btn = match on_press_lazy {
            Some(factory) => focus_button_maybe_with(content, Some(factory)),
            None => focus_button_maybe(content, on_press_eager),
        }
        .width(btn_width)
        .height(Length::Fixed(row_height))
        .padding(Padding::from([0.0, t.padding_x]))
        .background_inset_y(if is_separator {
            0.0
        } else {
            t.background_inset_y
        })
        .embedded(embedded)
        .focus_visible(embedded && highlighted)
        .semantics(semantics)
        .content_text_maybe(a11y_shortcut)
        // Row motion animates this style itself.
        .reduced_motion(true)
        .style_focus({
            let hover_progress = Rc::clone(&hover_progress);
            move |theme: &iced::Theme, status: FocusStatus| {
                let hover_bg = color_of(theme, |t| t.hover_background);
                let focus_ring = color_of(theme, |t| t.focus_ring);
                let bg = match status {
                    FocusStatus::Pressed => Some(Background::Color(hover_bg)),
                    _ if highlighted => Some(Background::Color(hover_bg)),
                    _ => {
                        let p = hover_progress.get();
                        if p <= 0.0 {
                            None
                        } else {
                            Some(Background::Color(lerp_color(
                                Color::TRANSPARENT,
                                hover_bg,
                                p,
                            )))
                        }
                    }
                };
                let border = if matches!(status, FocusStatus::Focused) {
                    Border {
                        color: focus_ring,
                        width: 1.5,
                        radius: radius.into(),
                    }
                } else {
                    Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: radius.into(),
                    }
                };
                ButtonStyle {
                    background: bg,
                    text_color: label_color_fn(theme),
                    border,
                    ..ButtonStyle::default()
                }
            }
        });
        if let Some(id) = self.id {
            btn = btn.id(id);
        }
        if is_separator {
            return btn.into();
        }
        RowMotion::new(btn.into(), false, reduced_motion, hover_progress)
            .track_hover(true)
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzMenuRow<'a, Message>> for iced::Element<'a, Message> {
    fn from(m: EnzMenuRow<'a, Message>) -> Self {
        m.into_element()
    }
}

/// Construct a menu row.
pub fn enz_menu_row<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    on_activated: Message,
) -> EnzMenuRow<'a, Message> {
    enz_menu_row_maybe(label, Some(on_activated))
}

/// Build a row with optional eager activation.
/// None disables dispatch independently of [`EnzMenuRow::enabled`]; set enabled
/// false as well for disabled paint and semantics.
pub fn enz_menu_row_maybe<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    on_activated: Option<Message>,
) -> EnzMenuRow<'a, Message> {
    EnzMenuRow {
        label: label.into(),
        trailing: String::new(),
        description: String::new(),
        icon: String::new(),
        icon_source: None,
        icon_position: MenuRowIconPosition::Leading,
        icon_tint: None,
        on_activated,
        on_activated_lazy: None,
        tokens: None,
        tone: MenuRowTone::Default,
        enabled: true,
        separator: false,
        selected: false,
        reserve_leading: false,
        highlighted: false,
        checkable: false,
        radio: false,
        embedded: false,
        submenu_indicator: false,
        direction: None,
        id: None,
        set_position: None,
        reduced_motion: None,
        measure_shrink: false,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
}

/// Build a row with an optional factory invoked on activation, never during view construction.
pub fn enz_menu_row_maybe_with<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    on_activated: Option<Box<dyn Fn() -> Message + 'a>>,
) -> EnzMenuRow<'a, Message> {
    EnzMenuRow {
        label: label.into(),
        trailing: String::new(),
        description: String::new(),
        icon: String::new(),
        icon_source: None,
        icon_position: MenuRowIconPosition::Leading,
        icon_tint: None,
        on_activated: None,
        on_activated_lazy: on_activated,
        tokens: None,
        tone: MenuRowTone::Default,
        enabled: true,
        separator: false,
        selected: false,
        reserve_leading: false,
        highlighted: false,
        checkable: false,
        radio: false,
        embedded: false,
        submenu_indicator: false,
        direction: None,
        id: None,
        set_position: None,
        reduced_motion: None,
        measure_shrink: false,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
}

/// Build an inert 1 px horizontal line with no label, message or row semantics.
pub fn enz_menu_separator<'a, Message: Clone + 'a>() -> iced::Element<'a, Message> {
    enz_menu_separator_builder().into_element()
}

/// Construct a separator builder for composites that need to apply the same
/// token override to command rows and separator spacing.
pub fn enz_menu_separator_builder<'a, Message: Clone + 'a>() -> EnzMenuRow<'a, Message> {
    EnzMenuRow {
        label: String::new(),
        trailing: String::new(),
        description: String::new(),
        icon: String::new(),
        icon_source: None,
        icon_position: MenuRowIconPosition::Leading,
        icon_tint: None,
        on_activated: None,
        on_activated_lazy: None,
        tokens: None,
        tone: MenuRowTone::Default,
        enabled: true,
        separator: true,
        selected: false,
        reserve_leading: false,
        highlighted: false,
        checkable: false,
        radio: false,
        embedded: false,
        submenu_indicator: false,
        direction: None,
        id: None,
        set_position: None,
        reduced_motion: None,
        measure_shrink: false,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
}
