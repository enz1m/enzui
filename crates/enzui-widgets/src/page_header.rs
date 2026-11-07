//! Responsive page titles with optional secondary and primary actions.
//!
//! Auto stacks below the header's allocated breakpoint (stock 480 px); zero
//! width uses the wide arrangement. Always and Never force a layout. Fit
//! measures the wide candidate and falls back to compact when it cannot fit.
//! Only the selected arrangement participates in events and semantics.
//!
//! Secondary Quiet actions precede primary filled Primary actions. The wide
//! primary has a growable 160 px floor, subject to parent constraints; compact
//! primary fills the remaining action-row width. Titles publish a Page heading.
//! Repeated commands across headers need distinct id_prefix values.
//!
//! Geometry resolves at construction: tokens win over forwarded spacing, then
//! stock spacing. Title colour follows the live palette. The default title uses
//! ambient Title1 typography; a title-size token differing from 28 px pins its size.

use crate::button::{ButtonKind, ButtonSize, enz_button};
use crate::enz_responsive::enz_responsive;
use crate::enz_text::{TextSize, TextTone, enz_text};
use crate::view_that_fits::{CandidateFn, enz_view_that_fits};
use enzui_core::semantics::HeadingLevel;
use enzui_theme::{BreakpointScale, SpacingScale};
use iced::widget::{column, container, row};
use iced::{Length, Padding, Size};

/// Header geometry in logical pixels. Explicit token overrides require finite,
/// positive fields; zero, negative and non-finite fields fall back to stock defaults.
#[derive(Debug, Clone, Copy)]
pub struct PageHeaderTokens {
    /// Default comparison size, 28 px. Values differing from 28 pin the title size;
    /// the default uses the ambient Title1 size and line height.
    pub title_size: f32,
    /// Growable primary-button width floor in wide layout; defaults to 160 px.
    /// Compact layout ignores this field.
    pub primary_min_width: f32,
    /// Gap between secondary and primary actions; defaults to stock sm (8 px).
    pub action_spacing: f32,
    /// Title-to-action-row gap in compact layout; defaults to stock md (12 px).
    pub compact_spacing: f32,
    /// Auto stacks at positive widths strictly below this value; stock default is 480 px.
    /// Fit, Always and Never ignore it.
    pub compact_breakpoint: f32,
}

impl PageHeaderTokens {
    /// Derive action and compact gaps from the supplied scale, with stock title,
    /// primary-width and breakpoint values. This constructor does not sanitize the scale.
    pub fn from_spacing(scale: &SpacingScale) -> Self {
        Self {
            title_size: TextSize::Title1.px(),
            primary_min_width: 160.0,
            action_spacing: scale.sm(),
            compact_spacing: scale.md(),
            compact_breakpoint: BreakpointScale::default().sm(),
        }
    }
    /// Return geometry from the stock spacing and breakpoint scales.
    pub fn defaults() -> Self {
        Self::from_spacing(&SpacingScale::default())
    }
    fn sanitized(self) -> Self {
        let d = Self::defaults();
        let fix = |v: f32, fb: f32| if v.is_finite() && v > 0.0 { v } else { fb };
        Self {
            title_size: fix(self.title_size, d.title_size),
            primary_min_width: fix(self.primary_min_width, d.primary_min_width),
            action_spacing: fix(self.action_spacing, d.action_spacing),
            compact_spacing: fix(self.compact_spacing, d.compact_spacing),
            compact_breakpoint: fix(self.compact_breakpoint, d.compact_breakpoint),
        }
    }
}

impl Default for PageHeaderTokens {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Choose the arrangement using allocated width, measured content or a forced layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactPolicy {
    /// Stack at positive allocated widths below the breakpoint; use wide otherwise.
    /// This is the default.
    #[default]
    Auto,
    /// Always stack the title above the action row.
    Always,
    /// Always place title and actions beside each other.
    Never,
    /// Measure the wide arrangement against allocated width, using compact as fallback.
    Fit,
}

/// Builder for an announced page title and up to two canonical action buttons.
#[must_use]
pub struct EnzPageHeader<'a, Message> {
    title: String,
    primary_action: Option<Action<Message>>,
    secondary_action: Option<Action<Message>>,
    compact: CompactPolicy,
    tokens: Option<PageHeaderTokens>,
    spacing_scale: Option<SpacingScale>,
    id_prefix: Option<String>,
    _phantom: std::marker::PhantomData<&'a ()>,
}

use crate::action_id::derive_action_id;
use enzui_core::Action;

fn title_element<'a, Message: 'a>(title: &str, t: &PageHeaderTokens) -> iced::Element<'a, Message> {
    let mut t_el = enz_text(title.to_owned())
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
        .size(TextSize::Title1)
        .weight_rung(enzui_theme::WeightRung::ExtraBold)
        .tone(TextTone::Default)
        .heading(HeadingLevel::Page);
    // An equal raw size pin would bypass the semantic line-height ramp.
    if (t.title_size - TextSize::Title1.px()).abs() > f32::EPSILON {
        t_el = t_el.size_px(t.title_size);
    }
    t_el.into()
}

impl<'a, Message: Clone + 'a> EnzPageHeader<'a, Message> {
    /// Set the trailing Primary action; wide uses a growable width floor, compact fills.
    pub fn primary_action(mut self, action: Action<Message>) -> Self {
        self.primary_action = Some(action);
        self
    }
    /// Set a Quiet action before the primary in traversal order.
    pub fn secondary_action(mut self, action: Action<Message>) -> Self {
        self.secondary_action = Some(action);
        self
    }
    /// Namespace action widget ids with the shared action-id derivation.
    /// Without a prefix, canonical action ids are forwarded unchanged. Use distinct
    /// prefixes for headers containing the same command; the two actions within one
    /// header also need distinct ids.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }
    /// Force compact when true or wide when false; use compact_policy(Auto) for responsiveness.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = if compact {
            CompactPolicy::Always
        } else {
            CompactPolicy::Never
        };
        self
    }
    /// Choose layout policy; defaults to Auto.
    pub fn compact_policy(mut self, policy: CompactPolicy) -> Self {
        self.compact = policy;
        self
    }
    /// Pin the whole token set, overriding forwarded spacing in either call order.
    /// Zero, negative and non-finite fields fall back to stock positive defaults.
    pub fn tokens(mut self, t: PageHeaderTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }
    /// Forward spacing for action and compact gaps at construction.
    /// Explicit tokens take precedence. Forwarded scale values are not sanitized here;
    /// rebuild the view to follow spacing changes.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
    /// Return the configured layout policy.
    pub fn policy(&self) -> CompactPolicy {
        self.compact
    }
    /// Report whether Always forces a compact layout.
    /// Auto and Fit return false even when their eventual layout stacks.
    pub fn is_compact(&self) -> bool {
        self.compact == CompactPolicy::Always
    }
    /// Return the resolved Auto breakpoint, including token overrides.
    pub fn compact_breakpoint(&self) -> f32 {
        self.resolved_tokens().compact_breakpoint
    }
    fn resolved_tokens(&self) -> PageHeaderTokens {
        if let Some(t) = self.tokens {
            return t;
        }
        match &self.spacing_scale {
            Some(scale) => PageHeaderTokens::from_spacing(scale),
            None => PageHeaderTokens::defaults(),
        }
    }
    fn build_title(&self, t: &PageHeaderTokens) -> iced::Element<'a, Message> {
        title_element(&self.title, t)
    }

    fn build_secondary(
        act: Action<Message>,
        id_prefix: Option<&str>,
    ) -> iced::Element<'a, Message> {
        let label = act.label().to_owned();
        let icon = act.icon().copied();
        let enabled = act.is_enabled();
        let id = derive_action_id(id_prefix, act.id());
        let mut button = enz_button(label, act.into_message())
            .kind(ButtonKind::Quiet)
            .size(ButtonSize::Md)
            .id(id);
        if let Some(icon) = icon {
            // Preserve unknown/custom icon identity across the button boundary.
            button = button.icon(icon.requested_name());
        }
        if !enabled {
            button = button.enabled(false);
        }
        button.into()
    }

    fn build_primary(
        act: Action<Message>,
        id_prefix: Option<&str>,
    ) -> crate::button::EnzButton<'a, Message> {
        let label = act.label().to_owned();
        let icon = act.icon().copied();
        let enabled = act.is_enabled();
        let id = derive_action_id(id_prefix, act.id());
        let mut button = enz_button(label, act.into_message())
            .kind(ButtonKind::Primary)
            .size(ButtonSize::Md)
            .id(id);
        if let Some(icon) = icon {
            // Preserve unknown/custom icon identity across the button boundary.
            button = button.icon(icon.requested_name());
        }
        if !enabled {
            button = button.enabled(false);
        }
        button
    }
    fn compact_view(
        title: iced::Element<'a, Message>,
        primary: Option<Action<Message>>,
        secondary: Option<Action<Message>>,
        t: &PageHeaderTokens,
        id_prefix: Option<&str>,
    ) -> iced::Element<'a, Message> {
        let mut keyed: Vec<(String, iced::Element<'a, Message>)> = Vec::new();
        if let Some(act) = secondary {
            let key = act.id().to_owned();
            keyed.push((key, Self::build_secondary(act, id_prefix)));
        }
        if let Some(act) = primary {
            let key = act.id().to_owned();
            keyed.push((
                key,
                Self::build_primary(act, id_prefix).fill_width(true).into(),
            ));
        }
        let actions_row = crate::keyed_row::keyed_row(keyed)
            .spacing(t.action_spacing)
            .width(Length::Fill);
        column![title, actions_row]
            .spacing(t.compact_spacing)
            .width(Length::Fill)
            .into()
    }
    fn wide_view(
        title: iced::Element<'a, Message>,
        primary: Option<Action<Message>>,
        secondary: Option<Action<Message>>,
        t: &PageHeaderTokens,
        id_prefix: Option<&str>,
    ) -> iced::Element<'a, Message> {
        let mut keyed: Vec<(String, iced::Element<'a, Message>)> = Vec::new();
        if let Some(act) = secondary {
            let key = act.id().to_owned();
            keyed.push((key, Self::build_secondary(act, id_prefix)));
        }
        if let Some(act) = primary {
            let key = act.id().to_owned();
            keyed.push((
                key,
                // A wrapper floor would leave the button content leading-aligned.
                Self::build_primary(act, id_prefix)
                    .min_width(t.primary_min_width)
                    .into(),
            ));
        }
        let actions_row: iced::Element<'a, Message> = crate::keyed_row::keyed_row(keyed)
            .spacing(t.action_spacing)
            .into();

        row![container(title).width(Length::Fill), actions_row]
            .align_y(iced::Alignment::Center)
            .padding(Padding::ZERO)
            .into()
    }
    /// Build the header; Auto and Fit rebuild their selected arrangement during layout.
    pub fn into_element(self) -> iced::Element<'a, Message> {
        let t = self.resolved_tokens();

        match self.compact {
            CompactPolicy::Always => {
                let title = self.build_title(&t);
                let p = self.id_prefix;
                Self::compact_view(
                    title,
                    self.primary_action,
                    self.secondary_action,
                    &t,
                    p.as_deref(),
                )
            }
            CompactPolicy::Never => {
                let title = self.build_title(&t);
                let p = self.id_prefix;
                Self::wide_view(
                    title,
                    self.primary_action,
                    self.secondary_action,
                    &t,
                    p.as_deref(),
                )
            }
            CompactPolicy::Fit => {
                // Layout can rebuild each candidate, so closures must retain clonable inputs.
                let wide_title = self.title.clone();
                let wide_primary = self.primary_action.clone();
                let wide_secondary = self.secondary_action.clone();
                let wide_prefix = self.id_prefix.clone();

                let compact_title = self.title;
                let compact_primary = self.primary_action;
                let compact_secondary = self.secondary_action;
                let compact_prefix = self.id_prefix;

                let candidates: Vec<CandidateFn<'a, Message>> = vec![
                    Box::new(move || {
                        Self::wide_view(
                            title_element(&wide_title, &t),
                            wide_primary.clone(),
                            wide_secondary.clone(),
                            &t,
                            wide_prefix.as_deref(),
                        )
                    }),
                    Box::new(move || {
                        Self::compact_view(
                            title_element(&compact_title, &t),
                            compact_primary.clone(),
                            compact_secondary.clone(),
                            &t,
                            compact_prefix.as_deref(),
                        )
                    }),
                ];

                enz_view_that_fits(candidates)
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into()
            }
            CompactPolicy::Auto => {
                let title = self.title;
                let primary = self.primary_action;
                let secondary = self.secondary_action;
                let id_prefix = self.id_prefix;
                let breakpoint = t.compact_breakpoint;
                enz_responsive(move |size: Size| {
                    let title_el = title_element(&title, &t);
                    // Zero width is the responsive pre-layout probe, not a compact allocation.
                    if size.width > 0.0 && size.width < breakpoint {
                        Self::compact_view(
                            title_el,
                            primary.clone(),
                            secondary.clone(),
                            &t,
                            id_prefix.as_deref(),
                        )
                    } else {
                        Self::wide_view(
                            title_el,
                            primary.clone(),
                            secondary.clone(),
                            &t,
                            id_prefix.as_deref(),
                        )
                    }
                })
                .width(Length::Fill)
                .height(Length::Shrink)
                .into()
            }
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzPageHeader<'a, Message>> for iced::Element<'a, Message> {
    fn from(h: EnzPageHeader<'a, Message>) -> Self {
        h.into_element()
    }
}

/// Build an Auto header with a Page heading, no actions and stock geometry.
pub fn enz_page_header<'a, Message: Clone + 'a>(
    title: impl Into<String>,
) -> EnzPageHeader<'a, Message> {
    EnzPageHeader {
        title: title.into(),
        primary_action: None,
        secondary_action: None,
        compact: CompactPolicy::Auto,
        tokens: None,
        spacing_scale: None,
        id_prefix: None,
        _phantom: std::marker::PhantomData,
    }
}
