//! Labelled settings rows with built-in or caller-supplied controls.
//!
//! Rows begin as noninteractive displays. Action, switch, entry, password,
//! value-menu, and custom-control setters share one kind slot: the last wins.
//! [`crate::preferences_group`] supplies a surrounding settings section.
//!
//! Action rows use a whole-row list-item button. Switch rows replay an unclaimed
//! row press onto their single focusable toggle. Entry and password controls own
//! their interactions. Value menus use a title-named combo-box trigger with
//! application-owned open state. Custom controls own their name and disabled state.
//!
//! Long subtitles wrap by default. Row-height tokens are floors when content
//! grows or accessories stack; disabling subtitle wrapping does not prevent
//! title wrapping or tall controls. Narrow rows move the complete accessory
//! below the label. The threshold depends on its slot or measured width, gap,
//! and padding, rather than a fixed application breakpoint. Explicit
//! [`EnzPreferencesRow::accessory_width`] and [`EnzPreferencesRow::label_min_width`]
//! pins let compact controls share a row with a narrower readable label.
//!
//! Geometry defaults use ambient density, typography, and text scale; colors
//! resolve through themed child primitives. Custom tokens override geometry.

use crate::enz_text::{TextAlign, TextTone, enz_text};
use crate::min_height::min_height;
use iced::advanced::widget::Id;
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length, Padding};
use std::borrow::Cow;

/// Preferences-row geometry in logical pixels; colors come from themed child primitives.
/// Defaults derive row heights, text sizes, and icon sizes from ambient density,
/// typography, and text scale. Padding, gaps, and accessory widths use fixed defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreferencesRowTokens {
    /// Single-line row-height floor.
    pub row_height: f32,
    /// Row-height floor when a subtitle is present.
    pub row_height_with_subtitle: f32,
    /// Corner radius (`radius-md` = 6px).
    pub row_radius: f32,
    /// Horizontal padding (`spacing-5` = 10px).
    pub padding_x: f32,
    /// Inter-slot gap (`spacing-5` = 10px).
    pub gap: f32,
    /// Total top/bottom padding added when a subtitle is present and wrapping is enabled.
    /// Split equally between the two edges.
    pub wrap_vertical_padding: f32,
    /// Leading icon glyph size.
    pub icon_size: f32,
    /// Leading icon slot width.
    pub icon_slot: f32,
    /// Trailing chevron glyph size (`icon-xs` = 12px).
    pub chevron_size: f32,
    /// Trailing chevron slot width.
    pub chevron_slot: f32,
    /// Title text size (`body-size` = 13px).
    pub title_size: f32,
    /// Subtitle text size (`label-size` = 11px).
    pub subtitle_size: f32,
    /// Trailing value text size (`body-size` = 13px).
    pub value_size: f32,
    /// Inline field preferred width.
    pub inline_field_width: f32,
    /// Compatibility metric retained in the token set; row layout uses `inline_field_width` instead.
    pub inline_field_max_width: f32,
    /// Switch track width.
    pub switch_width: f32,
    /// Switch track height.
    pub switch_height: f32,
    /// Switch thumb diameter.
    pub switch_thumb: f32,
}

impl Default for PreferencesRowTokens {
    fn default() -> Self {
        let sizing = enzui_theme::text_defaults::sizing();
        let ramp = enzui_theme::TypographyScale::ambient();
        let ui = enzui_theme::text_scale::ui_px;
        Self {
            row_height: ui(sizing.control_height + 10.0),
            row_height_with_subtitle: ui(sizing.control_height + 22.0),
            row_radius: 6.0,
            padding_x: 10.0,
            gap: 10.0,
            wrap_vertical_padding: 16.0,
            icon_size: ui(18.0),
            icon_slot: ui(24.0),
            chevron_size: ui(12.0),
            chevron_slot: ui(20.0),
            title_size: ramp.body_size,
            subtitle_size: ramp.label_size,
            value_size: ramp.body_size,
            inline_field_width: 160.0,
            inline_field_max_width: 200.0,
            switch_width: 42.0,
            switch_height: 24.0,
            switch_thumb: 20.0,
        }
    }
}

impl PreferencesRowTokens {
    /// Ignores the palette and returns ambient default geometry.
    pub fn from_palette(_p: &enzui_theme::Palette) -> Self {
        Self::default()
    }
    /// Returns ambient default geometry.
    pub fn dark() -> Self {
        Self::default()
    }
    /// Returns ambient default geometry.
    pub fn light() -> Self {
        Self::default()
    }

    /// Replaces non-finite or negative geometry with corresponding ambient defaults.
    /// Zero values are preserved.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::default();
        Self {
            row_height: FiniteNonNeg::new_or(self.row_height, d.row_height).get(),
            row_height_with_subtitle: FiniteNonNeg::new_or(
                self.row_height_with_subtitle,
                d.row_height_with_subtitle,
            )
            .get(),
            row_radius: FiniteNonNeg::new_or(self.row_radius, d.row_radius).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, d.padding_x).get(),
            gap: FiniteNonNeg::new_or(self.gap, d.gap).get(),
            wrap_vertical_padding: FiniteNonNeg::new_or(
                self.wrap_vertical_padding,
                d.wrap_vertical_padding,
            )
            .get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, d.icon_size).get(),
            icon_slot: FiniteNonNeg::new_or(self.icon_slot, d.icon_slot).get(),
            chevron_size: FiniteNonNeg::new_or(self.chevron_size, d.chevron_size).get(),
            chevron_slot: FiniteNonNeg::new_or(self.chevron_slot, d.chevron_slot).get(),
            title_size: FiniteNonNeg::new_or(self.title_size, d.title_size).get(),
            subtitle_size: FiniteNonNeg::new_or(self.subtitle_size, d.subtitle_size).get(),
            value_size: FiniteNonNeg::new_or(self.value_size, d.value_size).get(),
            inline_field_width: FiniteNonNeg::new_or(self.inline_field_width, d.inline_field_width)
                .get(),
            inline_field_max_width: FiniteNonNeg::new_or(
                self.inline_field_max_width,
                d.inline_field_max_width,
            )
            .get(),
            switch_width: FiniteNonNeg::new_or(self.switch_width, d.switch_width).get(),
            switch_height: FiniteNonNeg::new_or(self.switch_height, d.switch_height).get(),
            switch_thumb: FiniteNonNeg::new_or(self.switch_thumb, d.switch_thumb).get(),
        }
    }
}

mod responsive;
mod switch_press;
use responsive::PreferencesResponsive;

/// Default namespace for title-derived entry and password identities.
///
/// It is distinct from the bare field namespaces. [`EnzPreferencesRow::id`]
/// and [`EnzPreferencesRow::id_prefix`] override this fallback.
pub const DEFAULT_FIELD_ID_PREFIX: &str = "enz-preferences-row-field";

/// Default namespace for a title-derived value-menu trigger identity.
pub const DEFAULT_VALUE_MENU_ID_PREFIX: &str = "enz-preferences-row-value-menu";

enum Kind<'a, Message> {
    Action {
        value: Cow<'a, str>,
        on_click: Message,
    },
    Switch {
        checked: bool,
        on_toggle: Box<dyn Fn(bool) -> Message + 'a>,
    },
    Entry {
        value: &'a str,
        placeholder: Cow<'a, str>,
        on_edit: Box<dyn Fn(String) -> Message + 'a>,
    },
    Password {
        value: &'a str,
        placeholder: Cow<'a, str>,
        revealed: bool,
        on_edit: Box<dyn Fn(String) -> Message + 'a>,
        on_reveal_toggle: Message,
    },
    Control {
        control: Element<'a, Message>,
    },
    ValueMenu {
        current: Cow<'a, str>,
        open: bool,
        on_trigger: Message,
        on_dismiss: Message,
        options: Vec<(String, Message)>,
    },
}

/// Preferences-row builder.
///
/// Display text accepts borrowed or owned strings. Entry and password values
/// remain borrowed so editing continues to use the caller's canonical buffer.
#[must_use]
pub struct EnzPreferencesRow<'a, Message> {
    title: Cow<'a, str>,
    subtitle: Cow<'a, str>,
    subtitle_wrap: bool,
    switch_hover: bool,
    accessory_width: Option<f32>,
    label_min_width: Option<f32>,
    /// Writing direction of the row; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    icon: &'a str,
    property_style: bool,
    enabled: bool,
    field_size: crate::text_field::TextFieldSize,
    field_error: Option<&'a str>,
    id: Option<Id>,
    id_prefix: Option<String>,
    kind: Option<Kind<'a, Message>>,
    tokens: Option<PreferencesRowTokens>,
}

/// Creates a noninteractive display row with wrapping subtitles and enabled styling.
/// Choose a kind to add a built-in or custom control.
pub fn enz_preferences_row<'a, Message: Clone + 'a>(
    title: impl Into<Cow<'a, str>>,
) -> EnzPreferencesRow<'a, Message> {
    EnzPreferencesRow {
        title: title.into(),
        subtitle: Cow::Borrowed(""),
        subtitle_wrap: true,
        switch_hover: true,
        accessory_width: None,
        label_min_width: None,
        direction: None,
        icon: "",
        property_style: false,
        enabled: true,
        field_size: crate::text_field::TextFieldSize::Sm,
        field_error: None,
        id: None,
        id_prefix: None,
        kind: None,
        tokens: None,
    }
}

impl<'a, Message: Clone + 'a> EnzPreferencesRow<'a, Message> {
    /// Secondary line under the title. Borrowed and owned strings are accepted.
    pub fn subtitle(mut self, s: impl Into<Cow<'a, str>>) -> Self {
        self.subtitle = s.into();
        self
    }
    /// Enables subtitle wrapping and its vertical padding, defaulting to true.
    /// False elides the subtitle on one line; titles, tall controls, and stacking can still grow the row.
    pub fn subtitle_wrap(mut self, w: bool) -> Self {
        self.subtitle_wrap = w;
        self
    }
    /// Paint whole-row hover feedback for switch rows, defaulting to true.
    ///
    /// False keeps row activation, switch hover, keyboard focus and semantics.
    /// Other row kinds retain their own interaction styling.
    pub fn switch_hover(mut self, enabled: bool) -> Self {
        self.switch_hover = enabled;
        self
    }

    /// Pin the accessory slot width in logical pixels instead of its kind's default.
    ///
    /// Narrow rows still stack and may shrink the slot to available width.
    /// Non-finite or negative values are ignored; display-only rows have no slot.
    pub fn accessory_width(mut self, width: f32) -> Self {
        if width.is_finite() && width >= 0.0 {
            self.accessory_width = Some(width);
        }
        self
    }

    /// Reserve this label-column width before stacking, in logical pixels.
    ///
    /// Defaults to the accessory width. Derive explicit pins from current text
    /// metrics when they should follow typography scaling. Labels and descriptions
    /// may still wrap; non-finite or negative values are ignored.
    pub fn label_min_width(mut self, width: f32) -> Self {
        if width.is_finite() && width >= 0.0 {
            self.label_min_width = Some(width);
        }
        self
    }

    /// Lay the row out for `direction` instead of the locale's: the label
    /// leads from the start edge and the accessory trails, so in
    /// right-to-left text the label sits at the right. `None` (the default)
    /// follows the ambient locale.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Leading Lucide icon (empty hides it).
    pub fn icon(mut self, name: &'a str) -> Self {
        self.icon = name;
        self
    }
    /// Swaps title/subtitle sizes, weights, and enabled tones while preserving title-first order.
    pub fn property_style(mut self, p: bool) -> Self {
        self.property_style = p;
        self
    }
    /// Enables built-in interactions, defaulting to true, and controls row label styling.
    /// An opaque custom control owns its disabled state. A value-menu popup supplied
    /// as open is still constructed; this flag disables its trigger, not its choices.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Action (drill-in) kind with a borrowed or owned trailing value:
    /// whole-row button plus chevron.
    /// The trailing value stays on one line and ellipsizes to fit its slot.
    /// The decorative chevron yields when the value needs the whole slot.
    pub fn action(mut self, value: impl Into<Cow<'a, str>>, on_click: Message) -> Self {
        self.kind = Some(Kind::Action {
            value: value.into(),
            on_click,
        });
        self
    }
    /// Sets a switch kind whose toggle owns the focus stop.
    /// An unclaimed mouse or touch press elsewhere on an enabled row is replayed at
    /// the toggle center to activate and focus the same control.
    pub fn switch(mut self, checked: bool, on_toggle: impl Fn(bool) -> Message + 'a) -> Self {
        self.kind = Some(Kind::Switch {
            checked,
            on_toggle: Box::new(on_toggle),
        });
        self
    }
    /// Sets the shared entry/password size tier, defaulting to [`Sm`](crate::text_field::TextFieldSize::Sm).
    /// The field height contributes to the row floor; it does not change accessory widths.
    pub fn field_size(mut self, size: crate::text_field::TextFieldSize) -> Self {
        self.field_size = size;
        self
    }

    /// Set validation text for the built-in entry or password field.
    ///
    /// A non-empty value marks the field invalid and is shown and announced by
    /// the leaf field. It has no effect when the final row kind is action,
    /// switch, value-menu, custom control, or no control.
    pub fn error(mut self, error: &'a str) -> Self {
        self.field_error = (!error.is_empty()).then_some(error);
        self
    }

    /// Assign the stable identity of the row's built-in control.
    ///
    /// This identity reaches the action, switch, entry, password, or
    /// value-menu trigger. Password reveal and value-menu popup descendants
    /// derive their own identities from it. It has no effect on an opaque
    /// control supplied through [`Self::control`].
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the title-derived identity of the row's built-in control.
    ///
    /// Use a stable model key in the prefix when repeated rows have the same
    /// visible title. An explicit [`Self::id`] takes precedence.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// Entry kind: inline editable text field.
    pub fn entry(
        mut self,
        text: &'a str,
        placeholder: impl Into<Cow<'a, str>>,
        on_edit: impl Fn(String) -> Message + 'a,
    ) -> Self {
        self.kind = Some(Kind::Entry {
            value: text,
            placeholder: placeholder.into(),
            on_edit: Box::new(on_edit),
        });
        self
    }
    /// Password kind: inline masked field with a reveal toggle. `revealed`
    /// unmasks the text; `on_reveal_toggle` fires when the reveal control is
    /// pressed.
    pub fn password(
        mut self,
        text: &'a str,
        placeholder: impl Into<Cow<'a, str>>,
        revealed: bool,
        on_edit: impl Fn(String) -> Message + 'a,
        on_reveal_toggle: Message,
    ) -> Self {
        self.kind = Some(Kind::Password {
            value: text,
            placeholder: placeholder.into(),
            revealed,
            on_edit: Box::new(on_edit),
            on_reveal_toggle,
        });
        self
    }
    /// Sets a caller-owned trailing control; its accessible name and disabled state remain caller-owned.
    /// A nonblank row title publishes a named group around it. Inline the control uses
    /// `inline_field_width`; when stacked it receives the full content width. The last kind wins.
    pub fn control(mut self, control: impl Into<Element<'a, Message>>) -> Self {
        self.kind = Some(Kind::Control {
            control: control.into(),
        });
        self
    }

    /// Sets a text-only current-value trigger with application-owned menu state.
    ///
    /// The title names its combo-box node and `current` supplies the accessible value.
    /// An open popup uses a roving menu and returns focus to an enabled trigger
    /// after keyboard dismissal or a committed choice. Option labels also serve as keys
    /// and mark choices matching `current`.
    /// The application must update `open` and `current` from the supplied messages; the last kind wins.
    pub fn value_menu(
        mut self,
        current: impl Into<Cow<'a, str>>,
        open: bool,
        on_trigger: Message,
        on_dismiss: Message,
        options: Vec<(String, Message)>,
    ) -> Self {
        self.kind = Some(Kind::ValueMenu {
            current: current.into(),
            open,
            on_trigger,
            on_dismiss,
            options,
        });
        self
    }
    /// Overrides geometry after sanitization; child colors continue to track the runtime theme.
    pub fn tokens(mut self, t: PreferencesRowTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }

    /// Convert into an iced [`Element`].
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: the chevron is pushed just before it is taken"
    )]
    pub fn into_element(self) -> Element<'a, Message> {
        let t = self.tokens.unwrap_or_default();
        let enabled = self.enabled;
        let has_subtitle = !self.subtitle.is_empty();
        let title = self.title.into_owned();
        let subtitle = self.subtitle.into_owned();
        let control_id = self.id.clone();
        let control_id_prefix = self.id_prefix.clone();

        let (title_tone, subtitle_tone) = if !enabled {
            (TextTone::Disabled, TextTone::Disabled)
        } else if self.property_style {
            (TextTone::Muted, TextTone::Default)
        } else {
            (TextTone::Default, TextTone::Muted)
        };
        let (title_weight, subtitle_weight) = if self.property_style {
            (
                enzui_theme::WeightRung::Regular,
                enzui_theme::WeightRung::Semibold,
            )
        } else {
            (
                enzui_theme::WeightRung::Semibold,
                enzui_theme::WeightRung::Regular,
            )
        };
        let (title_size, subtitle_size) = if self.property_style {
            (t.subtitle_size, t.title_size)
        } else {
            (t.title_size, t.subtitle_size)
        };

        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;
        // Full width, so the start alignment puts it at the leading edge in
        // either direction.
        let title_widget = enz_text(title.clone())
            .size_px(title_size)
            .weight_rung(title_weight)
            .tone(title_tone)
            .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
            .full_width();
        let mut text_col = column![title_widget];
        if has_subtitle {
            text_col = text_col.push(
                enz_text(subtitle)
                    .size_px(subtitle_size)
                    .weight_rung(subtitle_weight)
                    .tone(subtitle_tone)
                    .wrap(self.subtitle_wrap)
                    .wrapping(if self.subtitle_wrap {
                        iced::widget::text::Wrapping::WordOrGlyph
                    } else {
                        iced::widget::text::Wrapping::None
                    })
                    .full_width(),
            );
        }
        let text_col = text_col.spacing(2.0);

        let mut label_children: Vec<Element<'a, Message>> = Vec::new();

        if !self.icon.is_empty() {
            let icon_tone = if enabled {
                TextTone::Default
            } else {
                TextTone::Disabled
            };
            let glyph = crate::icon::enz_icon(self.icon)
                .size(t.icon_size)
                .tint_resolved(move |theme: &iced::Theme| {
                    let p = crate::theme_lookup::palette(theme);
                    crate::enz_text::TextTokens::from_palette(&p).color_for(icon_tone)
                });
            label_children.push(
                container(glyph)
                    .width(Length::Fixed(t.icon_slot))
                    .align_x(Alignment::Center)
                    .into(),
            );
        }

        label_children.push(container(text_col).width(Length::Fill).into());
        let label_block: Element<'a, Message> = row(label_children)
            .mirrored(rtl)
            .spacing(t.gap)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .into();

        let mut trailing_children: Vec<Element<'a, Message>> = Vec::new();
        let mut trailing_min_width: Option<f32> = None;
        let mut expand_when_stacked = false;
        let kind = self.kind;
        let field_size = self.field_size;
        let field_height_floor = match &kind {
            Some(Kind::Entry { .. } | Kind::Password { .. }) => field_size.tokens().height,
            _ => 0.0,
        };
        let action_on_click: Option<Message> = match &kind {
            Some(Kind::Action { on_click, .. }) => Some(on_click.clone()),
            _ => None,
        };
        let is_custom_control = matches!(kind, Some(Kind::Control { .. }));
        let is_switch = matches!(kind, Some(Kind::Switch { .. }));
        match kind {
            Some(Kind::Action { value, .. }) => {
                if !value.is_empty() {
                    trailing_children.push(
                        enz_text(value.into_owned())
                            .size_px(t.value_size)
                            .no_wrap()
                            .tone(if enabled {
                                TextTone::Muted
                            } else {
                                TextTone::Disabled
                            })
                            .align(TextAlign::Right)
                            .into(),
                    );
                }
                let chevron_tone = if enabled {
                    TextTone::Muted
                } else {
                    TextTone::Disabled
                };
                let chevron = crate::icon::enz_icon("navigation.forward")
                    .size(t.chevron_size)
                    .tint_resolved(move |theme: &iced::Theme| {
                        let p = crate::theme_lookup::palette(theme);
                        crate::enz_text::TextTokens::from_palette(&p).color_for(chevron_tone)
                    });
                trailing_children.push(
                    container(chevron)
                        .width(Length::Fixed(t.chevron_slot))
                        .align_x(Alignment::Center)
                        .into(),
                );
                if trailing_children.len() == 2 {
                    let chevron = trailing_children.pop().expect("chevron");
                    let value = trailing_children.pop().expect("value");
                    trailing_children.push(
                        PreferencesResponsive::action_value(value, chevron, t.gap, t.value_size)
                            .rtl(rtl)
                            .into(),
                    );
                }
            }
            Some(Kind::Switch { checked, on_toggle }) => {
                let geom = crate::toggle::SwitchGeometry {
                    track_width: t.switch_width,
                    track_height: t.switch_height,
                    thumb: t.switch_thumb,
                    inset: 2.0,
                };
                trailing_min_width = Some(t.switch_width);
                // The row paints its title; give the labelless toggle an accessibility-only name.
                let mut toggle = crate::toggle::enz_toggle(checked, "", on_toggle)
                    .aria_label(title.clone())
                    .geometry(geom)
                    .enabled(enabled);
                if let Some(id) = control_id.clone() {
                    toggle = toggle.id(id);
                } else if let Some(prefix) = control_id_prefix.clone() {
                    toggle = toggle.id_prefix(prefix);
                }
                trailing_children.push(toggle.into());
            }
            Some(Kind::Entry {
                value,
                placeholder,
                on_edit,
            }) => {
                let mut field = crate::text_field::enz_text_field("", placeholder, value, on_edit)
                    .size(field_size)
                    .aria_label(title.clone())
                    .id_prefix(DEFAULT_FIELD_ID_PREFIX)
                    .enabled(enabled);
                if let Some(error) = self.field_error {
                    field = field.error(error);
                }
                if let Some(id) = control_id.clone() {
                    field = field.id(id);
                } else if let Some(prefix) = control_id_prefix.clone() {
                    field = field.id_prefix(prefix);
                }
                trailing_min_width = Some(t.inline_field_width);
                expand_when_stacked = true;
                trailing_children.push(container(field).width(Length::Fill).into());
            }
            Some(Kind::Password {
                value,
                placeholder,
                revealed,
                on_edit,
                on_reveal_toggle,
            }) => {
                let mut field = crate::secure_field::enz_secure_field(
                    value,
                    on_edit,
                    revealed,
                    on_reveal_toggle,
                )
                .placeholder(placeholder)
                .aria_label(title.clone())
                .id_prefix(DEFAULT_FIELD_ID_PREFIX)
                .size(field_size)
                .enabled(enabled);
                if let Some(error) = self.field_error {
                    field = field.error(error);
                }
                if let Some(id) = control_id.clone() {
                    field = field.id(id);
                } else if let Some(prefix) = control_id_prefix.clone() {
                    field = field.id_prefix(prefix);
                }
                trailing_min_width = Some(t.inline_field_width);
                expand_when_stacked = true;
                trailing_children.push(container(field).width(Length::Fill).into());
            }
            Some(Kind::Control { control }) => {
                // A narrow trailing control (a switch, a stepper) stays inline while it fits.
                // One that fills its width (a combo box) has no width of its own,
                // so it gets an entry field's, or the shrink-sized trailing lane
                // would lay it out zero wide.
                trailing_min_width = control
                    .as_widget()
                    .size()
                    .width
                    .is_fill()
                    .then_some(t.inline_field_width);
                expand_when_stacked = true;
                trailing_children.push(
                    container(control)
                        .width(Length::Fill)
                        .align_x(Alignment::End)
                        .into(),
                );
            }
            Some(Kind::ValueMenu {
                current,
                open,
                on_trigger,
                on_dismiss,
                options,
            }) => {
                let value_tone = if enabled {
                    TextTone::Muted
                } else {
                    TextTone::Disabled
                };
                let caption = enz_text(current.to_string())
                    .size_px(t.value_size)
                    .tone(value_tone)
                    .align(TextAlign::Right)
                    .single_line_elided();
                let chevron = crate::icon::enz_icon("navigation.down")
                    .size(t.chevron_size)
                    .tint_resolved(move |theme: &iced::Theme| {
                        let p = crate::theme_lookup::palette(theme);
                        crate::enz_text::TextTokens::from_palette(&p).color_for(value_tone)
                    });
                let trigger_body = container(
                    row![container(caption).width(Length::Fill), chevron]
                        .spacing(t.gap / 2.0)
                        .align_y(Alignment::Center),
                )
                .padding(Padding::from([2.0, 6.0]));

                let radius = t.row_radius;
                let trigger = crate::focus_button::focus_button_maybe(
                    trigger_body,
                    enabled.then(|| on_trigger.clone()),
                )
                .clip(true)
                .style_focus(move |theme: &iced::Theme, status| {
                    use crate::focus_button::FocusStatus;
                    let p = crate::theme_lookup::palette(theme);
                    let bg = match status {
                        FocusStatus::Hovered | FocusStatus::Pressed => {
                            iced::Background::Color(p.surface_hover)
                        }
                        _ => iced::Background::Color(iced::Color::TRANSPARENT),
                    };
                    iced::widget::button::Style {
                        background: Some(bg),
                        text_color: p.text,
                        border: iced::Border {
                            color: if matches!(status, FocusStatus::Focused) {
                                p.focus_ring
                            } else {
                                iced::Color::TRANSPARENT
                            },
                            width: if matches!(status, FocusStatus::Focused) {
                                1.5
                            } else {
                                0.0
                            },
                            radius: radius.into(),
                        },
                        ..iced::widget::button::Style::default()
                    }
                })
                .semantics({
                    let s = enzui_core::semantics::Semantics::new(
                        enzui_core::semantics::Role::ComboBox,
                    )
                    .label(title.clone())
                    .value(current.to_string())
                    .default_action()
                    .disabled(!enabled);
                    // Disabled triggers announce expansion state without exposing an expand action.
                    if enabled {
                        s.expanded(open)
                    } else {
                        s.expanded_state_only(open)
                    }
                });
                let value_menu_id = control_id.clone().unwrap_or_else(|| {
                    enzui_core::identity::derive_id(
                        control_id_prefix
                            .as_deref()
                            .unwrap_or(DEFAULT_VALUE_MENU_ID_PREFIX),
                        &title,
                    )
                });
                let trigger_id = enabled.then_some(value_menu_id.clone());
                let trigger = if let Some(id) = trigger_id.clone() {
                    // Open frames relinquish trigger focus; closed frames reclaim the popup return.
                    trigger
                        .expand_alias(id.clone())
                        .id(id.clone())
                        .relinquish_focus_while(open)
                        .reclaim_focus_on_return((!open).then_some(id))
                } else {
                    trigger
                };

                let popup: Option<Element<'a, Message>> = open.then(|| {
                    let menu_namespace = format!("{value_menu_id:?}-menu");
                    let mut menu = crate::menu::enz_menu()
                        .hug_content(true)
                        .hug_max_width(0.0)
                        .label(title.clone())
                        .id(enzui_core::identity::derive_id(&menu_namespace, "surface"))
                        .id_prefix(format!("{menu_namespace}-item"))
                        .roving_self(true)
                        .active(current.to_string());
                    if let Some(id) = trigger_id.clone() {
                        menu = menu.focus_return_to(id);
                    }
                    for (label, msg) in options {
                        let checked = label == current.as_ref();
                        menu = menu.push_item(
                            crate::menu::EnzMenuItem::command(label.clone(), label, msg)
                                .check(if checked {
                                    enzui_core::menu_model::CheckState::Checked
                                } else {
                                    enzui_core::menu_model::CheckState::Unchecked
                                })
                                .radio(true),
                        );
                    }
                    menu.into()
                });

                let placement =
                    enzui_core::placement::Placement::new(enzui_core::placement::Side::Bottom)
                        .alignment(enzui_core::placement::Alignment::End)
                        .gap(4.0);
                let anchored = crate::anchored_overlay::dismissible_anchored_menu(
                    trigger,
                    popup,
                    placement,
                    move || on_dismiss.clone(),
                );
                trailing_min_width = Some(t.inline_field_width);
                trailing_children.push(
                    container(anchored)
                        .width(Length::Fill)
                        .align_x(Alignment::End)
                        .into(),
                );
            }
            None => {}
        }

        let can_stack = !trailing_children.is_empty();
        let inner: Element<'a, Message> = if can_stack {
            let trailing: Element<'a, Message> = row(trailing_children)
                .mirrored(rtl)
                .spacing(t.gap)
                .align_y(Alignment::Center)
                .into();
            PreferencesResponsive::new(
                label_block,
                trailing,
                t.gap,
                self.accessory_width.or(trailing_min_width),
                expand_when_stacked,
            )
            .label_min_width(self.label_min_width)
            .rtl(rtl)
            .into()
        } else {
            label_block
        };

        let base_h = if has_subtitle {
            t.row_height_with_subtitle
        } else {
            t.row_height
        }
        .max(field_height_floor);

        if let Some(on_click) = action_on_click {
            // This token is a total across both edges, not padding per edge.
            let v_pad = if self.subtitle_wrap && has_subtitle {
                t.wrap_vertical_padding / 2.0
            } else {
                0.0
            };
            let content = container(inner)
                .padding(Padding::from([v_pad, t.padding_x]))
                .align_y(Alignment::Center);
            let row_radius = t.row_radius;
            let mut surface = crate::list_row_surface::enz_list_row_surface(content, on_click)
                .content_padding(0.0)
                .label(title.clone())
                .enabled(enabled)
                .row_height(base_h)
                // Preferences rows supply their own horizontal inset.
                .outer_gutter(0.0)
                .tokens_resolved(move |theme: &iced::Theme| {
                    crate::list_row_surface::ListRowSurfaceTokens {
                        row_radius,
                        ..crate::theme_lookup::resolve_live(
                            theme,
                            crate::list_row_surface::ListRowSurfaceTokens::from_palette,
                        )
                    }
                });
            if (self.subtitle_wrap && has_subtitle) || can_stack {
                surface = surface.content_height(true);
            }
            if let Some(id) = control_id {
                surface = surface.id(id);
            } else if let Some(prefix) = control_id_prefix {
                surface = surface.id_prefix(prefix);
            }
            let mut el: Element<'a, Message> = surface.into();
            if (self.subtitle_wrap && has_subtitle) || can_stack {
                el = min_height(el, base_h).into();
            }
            return el;
        }

        let v_pad = if self.subtitle_wrap && has_subtitle {
            t.wrap_vertical_padding / 2.0
        } else {
            0.0
        };
        let padded = container(inner)
            .padding(Padding::from([v_pad, t.padding_x]))
            .align_y(Alignment::Center)
            .width(Length::Fill);

        let el: Element<'a, Message> = if (self.subtitle_wrap && has_subtitle) || can_stack {
            min_height(padded, base_h).into()
        } else {
            padded.height(Length::Fixed(base_h)).into()
        };
        let el = if is_switch && enabled {
            switch_press::switch_press(el, t.row_radius, self.switch_hover)
        } else {
            el
        };

        if is_custom_control && !title.trim().is_empty() {
            return crate::semantics_container::SemanticsContainer::new(
                el,
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                    .label(title)
                    .disabled(!enabled),
            )
            .into();
        }
        el
    }
}

impl<'a, Message: Clone + 'a> From<EnzPreferencesRow<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzPreferencesRow<'a, Message>) -> Self {
        w.into_element()
    }
}
