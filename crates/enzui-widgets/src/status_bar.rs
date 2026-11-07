//! Single-message status bars and interactive strips with leading/trailing groups.
//!
//! Text and stat cells publish labels; actions publish named buttons with focus
//! rings and muted-to-accent hover/press ink. Labels elide on one line. Stat
//! cells reserve caption and value line boxes, growing the requested band height
//! independently of their strings. Parent constraints can still limit geometry.
//!
//! Metrics are resolved when constructing the element: explicit setters win over
//! forwarded scales, then defaults. Rebuild with updated scales after theme changes.
//! Colors and radius tokens resolve from the live theme when drawn; field pins
//! override only their own channels. Embedded strips use `palette.background`
//! as their ink backdrop unless a background is pinned.
//!
//! [`Distribution::EdgeAligned`] uses the overflow-row priorities and optional
//! atomic trailing group. [`Distribution::Even`] inserts equal flexible gaps;
//! [`Distribution::EqualColumns`] gives non-separator items equal flexible widths.
//! Spreading layouts concatenate leading items before trailing items and reject
//! edge-cluster overflow settings in debug builds.

use crate::enz_text::{TextTokens, TextTone, enz_text, line_box_px};
use crate::focus_button::{FocusStatus, focus_button};
use crate::surface::RadiusSize;
use crate::theme_lookup;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{Palette, RadiusScale, SizingTokens, SpacingScale, TypographyScale};
use iced::advanced::widget::Id;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{container, row};
use iced::{Background, Border, Color, Element, Length, Padding};

/// Semantic tone applied to the status text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusBarTone {
    /// `text_muted` (default).
    #[default]
    Muted,
    /// `accent`.
    Accent,
    /// `tone_success_fg`.
    Success,
    /// `tone_warning_fg`.
    Warning,
    /// `tone_error_fg` (destructive).
    Danger,
    /// Error tone, rendered identically to [`StatusBarTone::Danger`].
    Error,
}

impl StatusBarTone {
    fn text_tone(self) -> TextTone {
        match self {
            Self::Muted => TextTone::Muted,
            Self::Accent => TextTone::Accent,
            Self::Success => TextTone::Success,
            Self::Warning => TextTone::Warning,
            Self::Danger | Self::Error => TextTone::Danger,
        }
    }
}

/// Default bar height in logical pixels.
pub const DEFAULT_BAR_HEIGHT: f32 = 28.0;
/// Base default text size in logical pixels, adjusted by the UI text scale
/// when no explicit size or typography scale is supplied.
pub const DEFAULT_TEXT_SIZE: f32 = 12.0;
/// Default horizontal padding (`spacing-lg` = 16 px).
pub const DEFAULT_HORIZONTAL_PADDING: f32 = 16.0;

fn sane(value: f32, default: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        default
    }
}

// Zero must fall back here before enz_text would choose its different Body default.
fn sane_text_size(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        DEFAULT_TEXT_SIZE
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct StatusBarStyle {
    background: Option<Color>,
    border_color: Option<Color>,
    text_color: Option<Color>,
    // Resolve embedded ink against the host backdrop rather than the transparent fill.
    embedded: bool,
}

impl StatusBarStyle {
    fn background(&self, palette: &enzui_theme::Palette) -> Color {
        if self.embedded && self.background.is_none() {
            Color::TRANSPARENT
        } else {
            self.background.unwrap_or(palette.surface_alt)
        }
    }

    fn ink_backdrop(&self, palette: &enzui_theme::Palette) -> Color {
        if let Some(pin) = self.background {
            return pin;
        }
        if self.embedded {
            palette.background
        } else {
            palette.surface_alt
        }
    }

    fn border(&self, _palette: &enzui_theme::Palette) -> Color {
        self.border_color.unwrap_or(Color::TRANSPARENT)
    }

    fn text(&self, palette: &enzui_theme::Palette, tone: TextTone) -> Color {
        self.text_color.unwrap_or_else(|| {
            TextTokens::from_palette_on(palette, self.ink_backdrop(palette)).color_for(tone)
        })
    }
}

/// A themed bottom-of-window status strip. Background, border, and text
/// resolve from the runtime theme every render.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzStatusBar {
    text: String,
    tone: StatusBarTone,
    bar_height: f32,
    text_size: f32,
    horizontal_padding: f32,
    bar_height_explicit: bool,
    text_size_explicit: bool,
    horizontal_padding_explicit: bool,
    sizing: Option<SizingTokens>,
    typography: Option<TypographyScale>,
    spacing_scale: Option<SpacingScale>,
    announce: bool,
    style: StatusBarStyle,
}

impl EnzStatusBar {
    /// Override the semantic tone (default `Muted`).
    pub fn tone(mut self, tone: StatusBarTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the requested height, overriding forwarded sizing (default 28 logical pixels).
    /// Nonfinite or negative values use the default; zero is accepted.
    pub fn bar_height(mut self, height: f32) -> Self {
        self.bar_height = sane(height, DEFAULT_BAR_HEIGHT);
        self.bar_height_explicit = true;
        self
    }

    /// Override the text size (default 12 px). Sanitized: finite, strictly
    /// positive values are honoured; `0.0`, NaN, negative, and infinite
    /// inputs coerce back to [`DEFAULT_TEXT_SIZE`]. The composed [`enz_text`]
    /// discards a non-positive `size_px` and falls back to its 13 px Body
    /// ramp, so `.text_size(0.0)` renders at 12 px rather than 13 px.
    pub fn text_size(mut self, size: f32) -> Self {
        self.text_size = sane_text_size(size);
        self.text_size_explicit = true;
        self
    }

    /// Set horizontal padding, overriding the spacing scale (default 16 logical pixels).
    /// Nonfinite or negative values use the default; zero is accepted.
    pub fn horizontal_padding(mut self, padding: f32) -> Self {
        self.horizontal_padding = sane(padding, DEFAULT_HORIZONTAL_PADDING);
        self.horizontal_padding_explicit = true;
        self
    }

    /// Supply sizing tokens for element construction; explicit height wins.
    ///
    /// Rebuild with updated tokens after theme changes; layout does not read the live theme.
    pub fn sizing(mut self, sizing: SizingTokens) -> Self {
        self.sizing = Some(sizing);
        self
    }

    /// Forward the active runtime [`TypographyScale`]. Uses
    /// [`TypographyScale::statusbar_text_size`] without changing the UI font
    /// family. Precedence: explicit [`Self::text_size`] > this scale >
    /// [`DEFAULT_TEXT_SIZE`].
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }

    /// Forward the active runtime [`SpacingScale`]. The horizontal padding
    /// follows `SpacingScale::lg()` (16 px by default). Precedence: explicit
    /// [`Self::horizontal_padding`] > this scale > [`DEFAULT_HORIZONTAL_PADDING`].
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Include the status label in collected semantics (default `true`).
    ///
    /// `false` suppresses that label, for example when a parent supplies its name.
    /// This setting does not create a live announcement region.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }

    /// Pin the band background colour, overriding the runtime `surface_alt`
    /// for this instance only. The border and text still track the live theme
    /// unless separately pinned.
    pub fn background(mut self, color: Color) -> Self {
        self.style.background = Some(color);
        self
    }

    /// Pin the band border color, replacing its transparent default.
    /// Background and text still resolve independently.
    pub fn border_color(mut self, color: Color) -> Self {
        self.style.border_color = Some(color);
        self
    }

    /// Pin the status text ink, overriding the tone's runtime-resolved
    /// `tone-*-fg` for this instance only. Background and border still track
    /// the live theme unless separately pinned.
    pub fn text_color(mut self, color: Color) -> Self {
        self.style.text_color = Some(color);
        self
    }

    /// Resolve the background pin or `surface_alt`, before compositing.
    pub fn resolved_background(&self, palette: &enzui_theme::Palette) -> Color {
        self.style.background(palette)
    }

    /// Returns the band border this bar resolves to under `palette` (pin,
    /// else transparent). Installed as the container border resolver by
    /// [`Self::into_element`].
    pub fn resolved_border(&self, palette: &enzui_theme::Palette) -> Color {
        self.style.border(palette)
    }

    /// Returns the status text ink this bar resolves to under `palette`
    /// (pin, else the tone's audited `tone-*-fg`). Installed as the text
    /// colour resolver by [`Self::into_element`].
    pub fn resolved_text_color(&self, palette: &enzui_theme::Palette) -> Color {
        self.style.text(palette, self.tone.text_tone())
    }

    /// Convert into an iced [`Element`].
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let height = if self.bar_height_explicit {
            self.bar_height
        } else {
            sane(
                self.sizing
                    .map_or(DEFAULT_BAR_HEIGHT, |s| s.statusbar_height),
                DEFAULT_BAR_HEIGHT,
            )
        };
        let text_size = if self.text_size_explicit {
            self.text_size
        } else {
            sane_text_size(self.typography.map_or_else(
                || enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
                |t| t.statusbar_text_size,
            ))
        };
        let padding = if self.horizontal_padding_explicit {
            self.horizontal_padding
        } else {
            sane(
                self.spacing_scale
                    .map_or(DEFAULT_HORIZONTAL_PADDING, |s| s.lg()),
                DEFAULT_HORIZONTAL_PADDING,
            )
        };
        let style = self.style;
        let tone = self.tone.text_tone();

        let mut label = enz_text(self.text)
            .size_px(text_size)
            .single_line_elided()
            .full_width()
            .announce(self.announce);
        label = label.color_resolved(move |theme| style.text(&theme_lookup::palette(theme), tone));

        container(label)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .padding(Padding::from([0.0, padding]))
            .align_y(iced::alignment::Vertical::Center)
            .style(move |theme: &iced::Theme| {
                let p = theme_lookup::palette(theme);
                container::Style {
                    background: Some(Background::Color(style.background(&p))),
                    border: Border {
                        color: style.border(&p),
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a, Message: 'a> From<EnzStatusBar> for Element<'a, Message> {
    fn from(bar: EnzStatusBar) -> Self {
        bar.into_element()
    }
}

/// Construct a themed bottom status bar with the given message.
pub fn enz_status_bar<S: Into<String>>(text: S) -> EnzStatusBar {
    EnzStatusBar {
        text: text.into(),
        tone: StatusBarTone::default(),
        bar_height: DEFAULT_BAR_HEIGHT,
        text_size: DEFAULT_TEXT_SIZE,
        horizontal_padding: DEFAULT_HORIZONTAL_PADDING,
        bar_height_explicit: false,
        text_size_explicit: false,
        horizontal_padding_explicit: false,
        sizing: None,
        typography: None,
        spacing_scale: None,
        announce: true,
        style: StatusBarStyle::default(),
    }
}

const DEFAULT_ITEM_SPACING: f32 = 8.0;

const ACTION_RING_RADIUS: f32 = 4.0;

fn stat_caption_size(text_size: f32) -> f32 {
    sane_text_size(text_size - 1.0)
}

fn stat_cell_height(text_size: f32) -> f32 {
    line_box_px(text_size) + line_box_px(stat_caption_size(text_size))
}

fn center_in_band<'a, Message: 'a>(
    element: Element<'a, Message>,
    bar_height: f32,
) -> Element<'a, Message> {
    container(element)
        .height(Length::Fixed(bar_height))
        .align_y(iced::alignment::Vertical::Center)
        .into()
}

fn action_ink(status: FocusStatus, style: StatusBarStyle, palette: &Palette) -> Color {
    let tone = match status {
        FocusStatus::Hovered | FocusStatus::Pressed => TextTone::Accent,
        FocusStatus::Active | FocusStatus::Focused | FocusStatus::Disabled => TextTone::Muted,
    };
    style.text(palette, tone)
}

fn action_chrome(status: FocusStatus, palette: &Palette) -> ButtonStyle {
    let border = match status {
        FocusStatus::Focused => Border {
            color: palette.focus_ring,
            width: enzui_theme::text_defaults::focus_ring_width(),
            radius: ACTION_RING_RADIUS.into(),
        },
        FocusStatus::Active
        | FocusStatus::Hovered
        | FocusStatus::Pressed
        | FocusStatus::Disabled => Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: ACTION_RING_RADIUS.into(),
        },
    };
    ButtonStyle {
        background: Some(Background::Color(Color::TRANSPARENT)),
        border,
        ..ButtonStyle::default()
    }
}

type AnchorAttachment<'a, Message> =
    Box<dyn FnOnce(Element<'a, Message>) -> Element<'a, Message> + 'a>;

/// A passive label, stacked stat, named action or separator in a status strip.
///
/// Action identities support focus restoration; anchored attachments wrap the
/// finished trigger while leaving popup state and dismissal with the caller.
pub enum StatusBarItem<'a, Message> {
    /// A passive text readout ("Ln 12, Col 4").
    Text {
        /// Display label.
        label: String,
    },
    /// A stacked caption-over-value stat cell.
    Stat {
        /// The muted caption line, above the value ("Processes").
        caption: String,
        /// The value line, below the caption ("460").
        value: String,
    },
    /// A clickable region that emits `message` on press.
    Action {
        /// Display label (also the AT name unless overridden by `aria_label`).
        label: String,
        /// Message published on activation.
        message: Message,
        /// Optional explicit AT name (overrides `label` for screen readers).
        aria_label: Option<String>,
        /// Stable identity attached directly to this action, never an index.
        id: Option<Id>,
        /// Caller-owned attachment point for an anchored overlay.
        anchor: Option<AnchorAttachment<'a, Message>>,
    },
    /// A 1 px vertical hairline separator.
    Separator,
}

impl<Message> std::fmt::Debug for StatusBarItem<'_, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text { label } => f.debug_struct("Text").field("label", label).finish(),
            Self::Stat { caption, value } => f
                .debug_struct("Stat")
                .field("caption", caption)
                .field("value", value)
                .finish(),
            Self::Action {
                label,
                aria_label,
                id,
                anchor,
                ..
            } => f
                .debug_struct("Action")
                .field("label", label)
                .field("aria_label", aria_label)
                .field("id", id)
                .field("anchored", &anchor.is_some())
                .finish_non_exhaustive(),
            Self::Separator => f.write_str("Separator"),
        }
    }
}

impl<'a, Message> StatusBarItem<'a, Message> {
    /// A passive text readout.
    pub fn text(label: impl Into<String>) -> Self {
        Self::Text {
            label: label.into(),
        }
    }

    /// Stack a muted caption above a value, with independently elided text runs.
    ///
    /// Both line boxes are reserved even when either string is empty.
    pub fn stat(caption: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Stat {
            caption: caption.into(),
            value: value.into(),
        }
    }

    /// A clickable region that emits `message` on press.
    pub fn action(label: impl Into<String>, message: Message) -> Self {
        Self::Action {
            label: label.into(),
            message,
            aria_label: None,
            id: None,
            anchor: None,
        }
    }

    /// A clickable region with a distinct AT name.
    pub fn action_labelled(
        label: impl Into<String>,
        aria_label: impl Into<String>,
        message: Message,
    ) -> Self {
        Self::Action {
            label: label.into(),
            message,
            aria_label: Some(aria_label.into()),
            id: None,
            anchor: None,
        }
    }

    /// Attach a stable widget identity to this action so focus restoration
    /// targets it directly. Because the identity lives on the item rather
    /// than in an index-keyed strip map, inserting or removing a neighbouring
    /// item cannot silently retarget focus. No-op for passive items.
    #[must_use]
    pub fn id(mut self, item_id: Id) -> Self {
        if let Self::Action { id, .. } = &mut self {
            *id = Some(item_id);
        }
        self
    }

    /// Give the caller this action's rendered trigger to attach an anchored
    /// overlay (or another anchor-aware wrapper) around it. The strip owns
    /// neither popup state nor dismissal policy; the callback receives the
    /// finished, named `Role::Button` trigger and returns the caller's
    /// wrapper, so popup contents may borrow from the caller for `'a`. No-op
    /// for passive items.
    #[must_use]
    pub fn anchored_overlay(
        mut self,
        attach: impl FnOnce(Element<'a, Message>) -> Element<'a, Message> + 'a,
    ) -> Self {
        if let Self::Action { anchor, .. } = &mut self {
            *anchor = Some(Box::new(attach));
        }
        self
    }

    /// A 1 px vertical hairline separator.
    pub fn separator() -> Self {
        Self::Separator
    }
}

/// Arrangement of status-strip items within the available width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Distribution {
    /// Default. Leading cluster pinned to the leading edge, trailing cluster
    /// pinned to the trailing edge, middle absorbs the slack (the composed
    /// [`enz_overflow_row`](crate::overflow_row) shape). The leading and
    /// trailing overflow knobs ([`EnzStatusBarStrip::leading_priority`],
    /// [`EnzStatusBarStrip::trailing_priority`],
    /// [`EnzStatusBarStrip::trailing_atomic`]) apply here.
    #[default]
    EdgeAligned,
    /// Insert equal flexible gaps between intrinsic-width items.
    /// A single item sits at the leading edge; gaps shrink before items under pressure.
    /// Edge-cluster overflow settings are ignored in release and rejected in debug.
    Even,
    /// Center non-separator items in equal flexible columns; separators keep
    /// their intrinsic width. Edge-cluster settings have the same restriction as `Even`.
    EqualColumns,
}

impl Distribution {
    fn spreads_across_the_bar(self) -> bool {
        matches!(self, Self::Even | Self::EqualColumns)
    }
}

/// A multi-item bottom status strip with leading and trailing groups. See
/// the [module docs](self) for the height, overflow, and semantics contracts.
/// Compose with [`enz_status_bar_strip`].
#[derive(Debug)]
#[must_use]
pub struct EnzStatusBarStrip<'a, Message> {
    leading: Vec<StatusBarItem<'a, Message>>,
    trailing: Vec<StatusBarItem<'a, Message>>,
    bar_height: f32,
    text_size: f32,
    horizontal_padding: f32,
    item_spacing: f32,
    bar_height_explicit: bool,
    text_size_explicit: bool,
    horizontal_padding_explicit: bool,
    sizing: Option<SizingTokens>,
    typography: Option<TypographyScale>,
    spacing_scale: Option<SpacingScale>,
    style: StatusBarStyle,
    radius: RadiusSize,
    distribution: Distribution,
    leading_priority: Option<i32>,
    trailing_priority: Option<i32>,
    trailing_atomic: bool,
}

/// Construct a multi-item status bar strip.
pub fn enz_status_bar_strip<'a, Message>() -> EnzStatusBarStrip<'a, Message> {
    EnzStatusBarStrip {
        leading: Vec::new(),
        trailing: Vec::new(),
        bar_height: DEFAULT_BAR_HEIGHT,
        text_size: DEFAULT_TEXT_SIZE,
        horizontal_padding: DEFAULT_HORIZONTAL_PADDING,
        item_spacing: DEFAULT_ITEM_SPACING,
        bar_height_explicit: false,
        text_size_explicit: false,
        horizontal_padding_explicit: false,
        sizing: None,
        typography: None,
        spacing_scale: None,
        style: StatusBarStyle::default(),
        radius: RadiusSize::None,
        distribution: Distribution::default(),
        leading_priority: None,
        trailing_priority: None,
        trailing_atomic: false,
    }
}

impl<'a, Message: Clone + 'a> EnzStatusBarStrip<'a, Message> {
    /// Append an item to the leading (left) group.
    pub fn push_leading(mut self, item: StatusBarItem<'a, Message>) -> Self {
        self.leading.push(item);
        self
    }

    /// Append an item to the trailing (right) group.
    pub fn push_trailing(mut self, item: StatusBarItem<'a, Message>) -> Self {
        self.trailing.push(item);
        self
    }

    /// Make the trailing cluster an all-or-nothing overflow unit: when
    /// enabled, the complete cluster is shown at intrinsic width or the
    /// complete cluster is shed, so a narrow bar never leaves one orphaned
    /// action from a related set. The leading readout remains independently
    /// laid out.
    pub fn trailing_atomic(mut self, atomic: bool) -> Self {
        self.trailing_atomic = atomic;
        self
    }

    /// Set the requested minimum band height (default 28 logical pixels).
    /// Stat cells can grow it; nonfinite or negative values use the default.
    pub fn bar_height(mut self, height: f32) -> Self {
        self.bar_height = sane(height, DEFAULT_BAR_HEIGHT);
        self.bar_height_explicit = true;
        self
    }

    /// Set text size in logical pixels; nonpositive or nonfinite values use 12.
    /// Overrides forwarded typography.
    pub fn text_size(mut self, size: f32) -> Self {
        self.text_size = sane_text_size(size);
        self.text_size_explicit = true;
        self
    }

    /// Set horizontal padding, overriding forwarded spacing (default 16 logical pixels).
    /// Nonfinite or negative values use the default.
    pub fn horizontal_padding(mut self, padding: f32) -> Self {
        self.horizontal_padding = sane(padding, DEFAULT_HORIZONTAL_PADDING);
        self.horizontal_padding_explicit = true;
        self
    }

    /// Override inter-item spacing within a group (default 8 px).
    pub fn item_spacing(mut self, spacing: f32) -> Self {
        self.item_spacing = sane(spacing, DEFAULT_ITEM_SPACING);
        self
    }

    /// Forward runtime [`SizingTokens`].
    pub fn sizing(mut self, sizing: SizingTokens) -> Self {
        self.sizing = Some(sizing);
        self
    }

    /// Forward runtime [`TypographyScale`].
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }

    /// Forward runtime [`SpacingScale`].
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Pin the band background.
    pub fn background(mut self, color: Color) -> Self {
        self.style.background = Some(color);
        self
    }

    /// Pin the band border colour.
    pub fn border_color(mut self, color: Color) -> Self {
        self.style.border_color = Some(color);
        self
    }

    /// Suppress unpinned band fill and border, resolving ink against `palette.background`.
    ///
    /// Background and border pins still paint their channels. Supply a background
    /// pin when the host surface differs from the palette root.
    pub fn embedded(mut self) -> Self {
        self.style.embedded = true;
        self
    }

    /// Set the live-theme radius token (default [`RadiusSize::None`]).
    /// Applies to painted backgrounds or pinned borders; an unpinned embedded strip paints neither.
    pub fn radius(mut self, radius: RadiusSize) -> Self {
        self.radius = radius;
        self
    }

    /// Choose how items are spread across the bar.
    /// [`EdgeAligned`](Distribution::EdgeAligned)-default leading/trailing clusters;
    /// [`Even`](Distribution::Even) laid-out-width-gapped even spreads; [`EqualColumns`](Distribution::EqualColumns)
    /// equal non-separator columns. [`Distribution`] documents one-item +
    /// overflow policies.
    pub fn distribute(mut self, distribution: Distribution) -> Self {
        self.distribution = distribution;
        self
    }

    /// Returns the strip's corner radius under `scale`, in px: the
    /// [`RadiusSize`] token resolved against the runtime [`RadiusScale`].
    /// [`Self::into_element`] installs the same value as the container
    /// border radius when the strip paints a band, so this matches the
    /// painted corner exactly.
    pub fn resolved_radius(&self, scale: &RadiusScale) -> f32 {
        self.radius.from_scale(scale)
    }

    /// Set the layout priority of the leading group for overflow. Higher
    /// priority retains its measured width first.
    pub fn leading_priority(mut self, p: i32) -> Self {
        self.leading_priority = Some(p);
        self
    }

    /// Set the layout priority of the trailing group for overflow.
    pub fn trailing_priority(mut self, p: i32) -> Self {
        self.trailing_priority = Some(p);
        self
    }

    fn resolved_metrics(&self) -> (f32, f32, f32) {
        let height = if self.bar_height_explicit {
            self.bar_height
        } else {
            sane(
                self.sizing
                    .map_or(DEFAULT_BAR_HEIGHT, |s| s.statusbar_height),
                DEFAULT_BAR_HEIGHT,
            )
        };
        let text_size = if self.text_size_explicit {
            self.text_size
        } else {
            sane_text_size(self.typography.map_or_else(
                || enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
                |t| t.statusbar_text_size,
            ))
        };
        let padding = if self.horizontal_padding_explicit {
            self.horizontal_padding
        } else {
            sane(
                self.spacing_scale
                    .map_or(DEFAULT_HORIZONTAL_PADDING, |s| s.lg()),
                DEFAULT_HORIZONTAL_PADDING,
            )
        };
        (height, text_size, padding)
    }

    fn render_item(
        item: StatusBarItem<'a, Message>,
        text_size: f32,
        bar_height: f32,
        style: StatusBarStyle,
    ) -> Element<'a, Message> {
        let item_ink =
            move |theme: &iced::Theme| style.text(&theme_lookup::palette(theme), TextTone::Muted);
        match item {
            StatusBarItem::Text { label } => center_in_band(
                enz_text(label)
                    .size_px(text_size)
                    .single_line_elided()
                    .color_resolved(item_ink)
                    .into(),
                bar_height,
            ),
            StatusBarItem::Stat { caption, value } => {
                let value_ink = move |theme: &iced::Theme| {
                    style.text(&theme_lookup::palette(theme), TextTone::Default)
                };
                let caption_el: Element<'a, Message> = enz_text(caption)
                    .size_px(stat_caption_size(text_size))
                    .single_line_elided()
                    .color_resolved(item_ink)
                    .into();
                let value_el: Element<'a, Message> = enz_text(value)
                    .size_px(text_size)
                    .single_line_elided()
                    .color_resolved(value_ink)
                    .into();
                center_in_band(
                    iced::widget::Column::with_children(vec![caption_el, value_el])
                        .spacing(0.0)
                        .align_x(iced::alignment::Horizontal::Center)
                        .into(),
                    bar_height,
                )
            }
            StatusBarItem::Action {
                label,
                message,
                aria_label,
                id,
                anchor,
            } => {
                // The button style publishes status before its child resolves the same-frame ink.
                let status_cell = std::rc::Rc::new(std::cell::Cell::new(FocusStatus::Active));
                let ink_cell = std::rc::Rc::clone(&status_cell);
                let text_el: Element<'a, Message> = enz_text(label.clone())
                    .size_px(text_size)
                    .single_line_elided()
                    .color_resolved(move |theme: &iced::Theme| {
                        action_ink(ink_cell.get(), style, &theme_lookup::palette(theme))
                    })
                    .into();
                // A Fill wrapper centers the natural label inside the fixed-height button.
                let centred: Element<'a, Message> = container(text_el)
                    .height(Length::Fill)
                    .align_y(iced::alignment::Vertical::Center)
                    .into();
                let name = aria_label.unwrap_or(label);
                let semantics = Semantics::new(Role::Button).default_action().label(name);
                let mut btn = focus_button(centred, message)
                    .height(Length::Fixed(bar_height))
                    .padding(Padding::from([0.0, 6.0]))
                    .content_align_y(iced::Alignment::Center)
                    .semantics(semantics)
                    .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                        status_cell.set(status);
                        action_chrome(status, &theme_lookup::palette(theme))
                    });
                if let Some(id) = id {
                    btn = btn.id(id);
                }
                let trigger: Element<'a, Message> = btn.into();
                let trigger = match anchor {
                    Some(attach) => attach(trigger),
                    None => trigger,
                };
                center_in_band(trigger, bar_height)
            }
            StatusBarItem::Separator => {
                let inset = 4.0f32;
                let rule_height = (bar_height - 2.0 * inset).max(1.0);
                center_in_band(
                    container(
                        iced::widget::Space::new()
                            .width(Length::Fixed(1.0))
                            .height(Length::Fixed(rule_height)),
                    )
                    .width(Length::Fixed(1.0))
                    .height(Length::Fixed(rule_height))
                    .style(move |theme: &iced::Theme| {
                        let p = theme_lookup::palette(theme);
                        container::Style {
                            background: Some(Background::Color(p.border)),
                            ..Default::default()
                        }
                    })
                    .into(),
                    bar_height,
                )
            }
        }
    }

    fn build_group(
        items: Vec<StatusBarItem<'a, Message>>,
        text_size: f32,
        bar_height: f32,
        item_spacing: f32,
        style: StatusBarStyle,
    ) -> Element<'a, Message> {
        let mut r = row![]
            .spacing(item_spacing)
            .align_y(iced::Alignment::Center);
        for item in items {
            r = r.push(Self::render_item(item, text_size, bar_height, style));
        }
        r.into()
    }

    /// Resolve the selected background: its pin, transparent when embedded,
    /// otherwise `surface_alt`. Returns a style color before compositing.
    pub fn resolved_background(&self, palette: &enzui_theme::Palette) -> Color {
        self.style.background(palette)
    }

    /// Resolve muted text and resting-action ink against the strip's effective backdrop.
    ///
    /// Hovered and pressed actions use accent ink instead. Embedded unpinned
    /// strips resolve against `palette.background`, despite painting no fill.
    pub fn resolved_item_text_color(&self, palette: &enzui_theme::Palette) -> Color {
        self.style.text(palette, TextTone::Muted)
    }

    /// Construct the strip element with the selected distribution and current metrics.
    ///
    /// # Panics
    ///
    /// In debug builds, panics if `Even` or `EqualColumns` is combined with
    /// trailing atomicity or an explicit leading/trailing priority.
    pub fn into_element(self) -> Element<'a, Message> {
        let (height, text_size, padding) = self.resolved_metrics();
        let item_spacing = self.item_spacing;
        let style = self.style;

        let has_stat = self
            .leading
            .iter()
            .chain(self.trailing.iter())
            .any(|item| matches!(item, StatusBarItem::Stat { .. }));
        let band_height = if has_stat {
            height.max(stat_cell_height(text_size))
        } else {
            height
        };

        let radius = self.radius;
        let distribution = self.distribution;

        if distribution.spreads_across_the_bar() {
            debug_assert!(
                !(self.trailing_atomic
                    || self.leading_priority.is_some()
                    || self.trailing_priority.is_some()),
                "a spreading Distribution and the edge-cluster overflow knobs \
                 (trailing_atomic / leading_priority / trailing_priority) are \
                 mutually exclusive: a spread bar has no leading/trailing \
                 clusters to prioritise. Drop the knobs or use \
                 Distribution::EdgeAligned."
            );
            let inner = if distribution == Distribution::EqualColumns {
                self.build_equal_columns(text_size, band_height, style)
            } else {
                self.build_even(text_size, band_height, style)
            };
            return Self::band_container(inner, band_height, padding, style, radius);
        }

        let has_leading = !self.leading.is_empty();
        let has_trailing = !self.trailing.is_empty();

        let leading_el: Option<Element<'a, Message>> = if has_leading {
            Some(Self::build_group(
                self.leading,
                text_size,
                band_height,
                item_spacing,
                style,
            ))
        } else {
            None
        };

        let trailing_items: Vec<Element<'a, Message>> = if has_trailing {
            self.trailing
                .into_iter()
                .map(|item| Self::render_item(item, text_size, band_height, style))
                .collect()
        } else {
            Vec::new()
        };
        use crate::overflow_row::enz_overflow_row;

        let spacer: Element<'a, Message> = iced::widget::Space::new().width(Length::Fill).into();

        let mut overflow = if self.trailing_atomic && !trailing_items.is_empty() {
            let complete_group: Element<'a, Message> = row(trailing_items)
                .spacing(item_spacing)
                .align_y(iced::Alignment::Center)
                .into();
            enz_overflow_row(leading_el, spacer, None::<Element<'a, Message>>)
                .trailing_atomic(vec![complete_group], item_spacing)
                .height(Length::Fixed(band_height))
        } else {
            let trailing_el: Option<Element<'a, Message>> = if trailing_items.is_empty() {
                None
            } else {
                Some(
                    row(trailing_items)
                        .spacing(item_spacing)
                        .align_y(iced::Alignment::Center)
                        .into(),
                )
            };
            enz_overflow_row(leading_el, spacer, trailing_el).height(Length::Fixed(band_height))
        };

        if let Some(lp) = self.leading_priority {
            overflow = overflow.leading_priority(lp);
        }
        if let Some(tp) = self.trailing_priority {
            overflow = overflow.trailing_priority(tp);
        }

        let inner: Element<'a, Message> = overflow.into_element();

        Self::band_container(inner, band_height, padding, style, radius)
    }

    fn build_even(
        self,
        text_size: f32,
        band_height: f32,
        style: StatusBarStyle,
    ) -> Element<'a, Message> {
        let items: Vec<Element<'a, Message>> = self
            .leading
            .into_iter()
            .chain(self.trailing)
            .map(|item| Self::render_item(item, text_size, band_height, style))
            .collect();
        let mut r = row![]
            .align_y(iced::Alignment::Center)
            .height(Length::Fixed(band_height));
        for (i, item) in items.into_iter().enumerate() {
            if i > 0 {
                r = r.push(iced::widget::Space::new().width(Length::Fill));
            }
            r = r.push(item);
        }
        r.into()
    }

    fn build_equal_columns(
        self,
        text_size: f32,
        band_height: f32,
        style: StatusBarStyle,
    ) -> Element<'a, Message> {
        let mut r = row![]
            .align_y(iced::Alignment::Center)
            .height(Length::Fixed(band_height));
        for item in self.leading.into_iter().chain(self.trailing) {
            let separator = matches!(item, StatusBarItem::Separator);
            let cell = Self::render_item(item, text_size, band_height, style);
            r = r.push(if separator {
                cell
            } else {
                container(cell)
                    .width(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .into()
            });
        }
        r.into()
    }

    fn band_container(
        inner: Element<'a, Message>,
        band_height: f32,
        padding: f32,
        style: StatusBarStyle,
        radius: RadiusSize,
    ) -> Element<'a, Message> {
        container(inner)
            .width(Length::Fill)
            .height(Length::Fixed(band_height))
            .padding(Padding::from([0.0, padding]))
            .style(move |theme: &iced::Theme| {
                let p = theme_lookup::palette(theme);
                let (background, border_width) =
                    if style.embedded && style.background.is_none() && style.border_color.is_none()
                    {
                        (None, 0.0)
                    } else {
                        // A transparent border still takes its width out of
                        // the fill, so only a pinned colour gets one.
                        let width = if style.border_color.is_some() {
                            1.0
                        } else {
                            0.0
                        };
                        (Some(Background::Color(style.background(&p))), width)
                    };
                let corner = if background.is_some() {
                    radius.from_scale(&theme_lookup::radius(theme))
                } else {
                    0.0
                };
                container::Style {
                    background,
                    border: Border {
                        color: style.border(&p),
                        width: border_width,
                        radius: corner.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzStatusBarStrip<'a, Message>> for Element<'a, Message> {
    fn from(strip: EnzStatusBarStrip<'a, Message>) -> Self {
        strip.into_element()
    }
}
