//! Fixed-height window chrome with back, start actions, title and end actions.
//!
//! Defaults are a 48px band, 8px side padding, 6px control gaps and Fill width.
//! Measured action zones clamp the title around the full bar's centre. Controls
//! use small Quiet buttons; the title delegates typography to [`crate::window_title`].
//! Backgrounds and floating edges follow the live palette.
//!
//! Distinct action keys retain child state within each zone. Repeated command
//! IDs are disambiguated by occurrence across start and end actions. A namespace
//! separates forwarded action IDs between bars. Active Back defaults to
//! navigation.back; explicit back IDs override that derivation.
//!
//! [`crate::toolbar_view`] and [`crate::navigation_outlet`]
//! compose this bar for their respective shells.

use crate::button::{ButtonIconPosition, ButtonKind, ButtonSize, enz_button};
use enzui_core::Action;
use enzui_theme::palette::Palette;
use iced::widget::{Space, container};
use iced::{Alignment, Border, Color, Element, Length, Padding, border};

/// Live palette surface treatment for the band.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderBarStyle {
    /// Use surface_alt without a border; this is the default.
    #[default]
    Flat,

    /// Use surface_elevated without a border.
    Raised,

    /// Use surface with a 1px derived edge and configured corner radius.
    Floating,
}

/// Canonical action paired with its header presentation.
#[derive(Debug, Clone)]
pub struct HeaderAction<Message> {
    action: Action<Message>,
    icon_only: bool,
}

impl<Message> HeaderAction<Message> {
    /// Show the action label and optional icon.
    pub fn new(action: Action<Message>) -> Self {
        Self {
            action,
            icon_only: false,
        }
    }

    /// Hide the visual label while retaining it as the accessible name.
    pub fn icon_only(action: Action<Message>) -> Self {
        Self {
            action,
            icon_only: true,
        }
    }

    /// Borrow the presented action.
    pub fn action(&self) -> &Action<Message> {
        &self.action
    }

    /// Recover the presented action.
    pub fn into_action(self) -> Action<Message> {
        self.action
    }

    /// Return whether the visual label is omitted.
    pub fn is_icon_only(&self) -> bool {
        self.icon_only
    }
}

impl<Message> From<Action<Message>> for HeaderAction<Message> {
    fn from(action: Action<Message>) -> Self {
        Self::new(action)
    }
}

/// Band geometry; colours resolve from the live theme.
#[derive(Debug, Clone, Copy)]
pub struct HeaderBarTokens {
    /// Band height in logical pixels; defaults to 48.
    pub bar_height: f32,

    /// Side padding in logical pixels; defaults to 8.
    pub padding_x: f32,

    /// Gap between controls in logical pixels; defaults to 6.
    pub gap: f32,

    /// Floating-band corner radius in logical pixels; defaults to 10.
    pub radius_lg: f32,

    /// Compatibility value, defaulting to 72; this does not affect layout.
    pub action_min_width: f32,
}

impl HeaderBarTokens {
    /// Stock geometry values.
    pub const DEFAULT: Self = Self {
        bar_height: 48.0,
        padding_x: 8.0,
        gap: 6.0,
        radius_lg: 10.0,
        action_min_width: 72.0,
    };

    /// Replace negative or non-finite geometry with stock defaults; preserve zero.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::DEFAULT;
        Self {
            bar_height: FiniteNonNeg::new_or(self.bar_height, d.bar_height).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, d.padding_x).get(),
            gap: FiniteNonNeg::new_or(self.gap, d.gap).get(),
            radius_lg: FiniteNonNeg::new_or(self.radius_lg, d.radius_lg).get(),
            action_min_width: FiniteNonNeg::new_or(self.action_min_width, d.action_min_width).get(),
        }
    }
}

impl Default for HeaderBarTokens {
    fn default() -> Self {
        Self::DEFAULT
    }
}
fn bar_bg(style: HeaderBarStyle, p: &Palette) -> Color {
    match style {
        HeaderBarStyle::Flat => p.surface_alt,
        HeaderBarStyle::Raised => p.surface_elevated,
        HeaderBarStyle::Floating => p.surface,
    }
}

/// Window header builder with keyed action zones and a centre-clamped title.
#[must_use]
pub struct EnzHeaderBar<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    title: String,
    subtitle: Option<String>,
    start_actions: Vec<HeaderAction<Message>>,
    end_actions: Vec<HeaderAction<Message>>,
    show_back_button: bool,
    show_title: bool,
    style: HeaderBarStyle,
    on_back: Option<Message>,
    tokens: HeaderBarTokens,
    width: Length,
    title_tokens: Option<crate::window_title::WindowTitleTokens>,
    id_prefix: Option<String>,
    back_id: Option<iced::advanced::widget::Id>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Own a title with stock geometry, Flat style, Fill width and no actions or Back.
pub fn enz_header_bar<'a, Message: 'a + Clone>(
    title: impl Into<String>,
) -> EnzHeaderBar<'a, Message> {
    EnzHeaderBar {
        material: None,
        title: title.into(),
        subtitle: None,
        start_actions: Vec::new(),
        end_actions: Vec::new(),
        show_back_button: false,
        show_title: true,
        style: HeaderBarStyle::default(),
        on_back: None,
        tokens: HeaderBarTokens::DEFAULT,
        width: Length::Fill,
        title_tokens: None,
        id_prefix: None,
        back_id: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a + Clone> EnzHeaderBar<'a, Message> {
    /// Set the secondary title line; an empty string clears it.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        self.subtitle = if s.is_empty() { None } else { Some(s) };
        self
    }

    /// Replace actions after Back in the leading zone; defaults to empty.
    pub fn start_actions(mut self, actions: Vec<HeaderAction<Message>>) -> Self {
        self.start_actions = actions;
        self
    }

    /// Replace trailing actions; defaults to empty.
    pub fn end_actions(mut self, actions: Vec<HeaderAction<Message>>) -> Self {
        self.end_actions = actions;
        self
    }

    /// Show Back; defaults to false. Without on_back it is disabled.
    pub fn show_back_button(mut self, s: bool) -> Self {
        self.show_back_button = s;
        self
    }

    /// Show the title and subtitle; defaults to true.
    pub fn show_title(mut self, s: bool) -> Self {
        self.show_title = s;
        self
    }

    /// Select the band surface treatment; defaults to Flat.
    pub fn style(mut self, s: HeaderBarStyle) -> Self {
        self.style = s;
        self
    }

    /// Emit this message when the shown Back control is activated.
    pub fn on_back(mut self, msg: Message) -> Self {
        self.on_back = Some(msg);
        self
    }

    /// Override active Back identity.
    /// Without an override, Back derives navigation.back using the optional namespace.
    /// The override is used verbatim; disabled Back has no identity.
    pub fn back_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.back_id = Some(id);
        self
    }

    /// Set geometry after replacing negative or non-finite fields with stock defaults.
    /// Zero is preserved. action_min_width is retained but does not affect layout.
    pub fn tokens(mut self, t: HeaderBarTokens) -> Self {
        self.tokens = t.sanitized();
        self
    }

    /// Forward title overrides to [`crate::window_title::EnzWindowTitle::tokens`].
    pub fn title_tokens(mut self, t: crate::window_title::WindowTitleTokens) -> Self {
        self.title_tokens = Some(t);
        self
    }

    /// Set band width; defaults to Fill.
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Namespace action IDs and the default active Back ID.
    /// The first occurrence preserves its canonical action ID without a namespace;
    /// repeats use occurrence keys across both zones. Explicit back_id is unaffected.
    /// Back is counted separately, so an action named navigation.back needs a distinct
    /// explicit Back ID when both are present.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// Wrap the band in liquid glass and default its controls to that material.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Compose the clipped band and optional glass presentation.
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.material;
        let radius: f32 = 0.0;
        crate::glass::wrap(self.build_element(), material, radius)
    }

    fn build_element(self) -> Element<'a, Message> {
        let tokens = self.tokens;
        let style = self.style;
        let is_floating = matches!(style, HeaderBarStyle::Floating);
        let mut start_children: Vec<(String, Element<'a, Message>)> = Vec::new();
        let mut end_children: Vec<(String, Element<'a, Message>)> = Vec::new();
        // Reconciliation keys stay local to each zone; forwarded IDs serve accessibility routing.
        let mut keys = enzui_core::identity::KeyedIds::new("enz-header-bar");
        let id_prefix = self.id_prefix;
        if self.show_back_button {
            start_children.push((
                format!("{:?}", keys.id_for("navigation.back")),
                back_button(
                    self.on_back.clone(),
                    self.back_id.clone(),
                    id_prefix.as_deref(),
                ),
            ));
        }
        // Count action occurrences across both zones so repeated commands publish distinct IDs.
        let mut at_ids = enzui_core::identity::KeyedIds::new("");
        for a in self.start_actions {
            let key = format!("{:?}", keys.id_for(a.action().id()));
            let at_key = at_ids.key_for(a.action().id());
            start_children.push((key, action_button(a, id_prefix.as_deref(), &at_key)));
        }
        for a in self.end_actions {
            let key = format!("{:?}", keys.id_for(a.action().id()));
            let at_key = at_ids.key_for(a.action().id());
            end_children.push((key, action_button(a, id_prefix.as_deref(), &at_key)));
        }

        let start_zone = crate::keyed_row::keyed_row(start_children)
            .balance_shrink()
            .spacing(tokens.gap)
            .align_y(Alignment::Center);
        let end_zone = crate::keyed_row::keyed_row(end_children)
            .balance_shrink()
            .spacing(tokens.gap)
            .align_y(Alignment::Center);
        let content: Element<'a, Message> = if self.show_title {
            let mut wt = crate::window_title::enz_window_title(self.title).centered(true);
            if let Some(sub) = self.subtitle {
                wt = wt.subtitle(sub);
            }
            if let Some(tt) = self.title_tokens {
                wt = wt.tokens(tt);
            }
            // Clip title paint to its measured band when parent height or width is constrained.
            let title_col: Element<'a, Message> = container(wt.into_element::<Message>())
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .clip(true)
                .into();

            crate::center_clamp_row::center_clamp_row(start_zone.into(), title_col, end_zone.into())
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            crate::center_clamp_row::center_clamp_row(
                start_zone.into(),
                Space::new()
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into(),
                end_zone.into(),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
        };

        container(content)
            .padding(
                Padding::new(0.0)
                    .left(tokens.padding_x)
                    .right(tokens.padding_x),
            )
            .width(self.width)
            .height(Length::Fixed(tokens.bar_height))
            .clip(true)
            .style(move |theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                container::Style {
                    background: Some(iced::Background::Color(bar_bg(style, &p))),
                    border: Border {
                        color: if is_floating {
                            crate::surface_edge::edge_color(
                                bar_bg(style, &p),
                                crate::surface_edge::DEFAULT_EDGE_DEPTH,
                            )
                        } else {
                            Color::TRANSPARENT
                        },
                        width: if is_floating { 1.0 } else { 0.0 },
                        radius: border::Radius::new(if is_floating {
                            tokens.radius_lg
                        } else {
                            0.0
                        }),
                    },
                    ..container::Style::default()
                }
            })
            .into()
    }
}

impl<'a, Message: 'a + Clone> From<EnzHeaderBar<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzHeaderBar<'a, Message>) -> Self {
        w.into_element()
    }
}
fn back_button<'a, Message: 'a + Clone>(
    on_back: Option<Message>,
    back_id: Option<iced::advanced::widget::Id>,
    id_prefix: Option<&str>,
) -> Element<'a, Message> {
    match on_back {
        Some(msg) => {
            // Active Back needs a stable address for default-action routing.
            let id = back_id.unwrap_or_else(|| {
                crate::action_id::derive_action_id(id_prefix, "navigation.back")
            });
            enz_button(enzui_core::localization::text("enzui-header-bar-back"), msg)
                .kind(ButtonKind::Quiet)
                .size(ButtonSize::Sm)
                .icon("navigation.back")
                .icon_position(ButtonIconPosition::Only)
                .aria_label(enzui_core::localization::text("enzui-header-bar-back"))
                .id(id)
                .into()
        }
        None => crate::button::enz_inert_button::<Message>(
            enzui_core::localization::text("enzui-header-bar-back"),
            ButtonKind::Quiet,
        )
        .size(ButtonSize::Sm)
        .icon("navigation.back")
        .icon_position(ButtonIconPosition::Only)
        .aria_label(enzui_core::localization::text("enzui-header-bar-back"))
        .into(),
    }
}
fn action_button<'a, Message: 'a + Clone>(
    a: HeaderAction<Message>,
    id_prefix: Option<&str>,
    at_key: &str,
) -> Element<'a, Message> {
    let icon_only = a.icon_only;
    let label = a.action.label().to_owned();
    // Canonical names collapse unknown/custom identities before the resolver can diagnose them.
    let icon = a.action.icon().map(|i| i.requested_name().to_owned());
    let enabled = a.action.is_enabled();
    let id = crate::action_id::derive_action_id_keyed(id_prefix, at_key);

    let icon_position = if icon_only {
        ButtonIconPosition::Only
    } else {
        ButtonIconPosition::Leading
    };
    let builder = if enabled {
        enz_button(label.clone(), a.action.into_message())
    } else {
        crate::button::enz_inert_button::<Message>(label.clone(), ButtonKind::Quiet)
    };

    let mut b = builder
        .kind(ButtonKind::Quiet)
        .size(ButtonSize::Sm)
        .icon_position(icon_position)
        .id(id)
        .aria_label(label);
    if let Some(name) = icon {
        b = b.icon(name);
    }
    b.into()
}
