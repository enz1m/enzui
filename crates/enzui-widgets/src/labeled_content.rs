//! Label and value or control rows that stack in narrow component widths.
//!
//! Auto-compact defaults to a strict 320 px breakpoint measured on the row's
//! offered width. An explicit layout overrides it, and disabling auto-compact
//! keeps the wide arrangement. Fixed controls retain their widget tree across
//! reflow; value text and control factories may rebuild during layout.
//! A control factory takes precedence over a fixed control, which takes
//! precedence over value text, regardless of setter order. Nonempty labels
//! name an accessibility group containing the row's children.

use crate::enz_responsive::enz_responsive;
use crate::reflow_pair::{PairArm, reflow_pair};
use enzui_theme::{Palette, SizingTokens, TypographyScale};
use iced::widget::{Space, column, container, row};
use iced::{Alignment, Color, Element, Length, Padding, Size};
/// Default component-width threshold in logical pixels; equality stays wide.
pub const DEFAULT_BREAKPOINT: f32 = 320.0;
/// Fallback text size in logical pixels for invalid resolved metrics.
pub const DEFAULT_LABEL_SIZE: f32 = 13.0;
/// Default wide-layout label-column width in logical pixels.
pub const DEFAULT_LABEL_WIDTH: f32 = 140.0;
/// Fallback wide-row height in logical pixels for invalid resolved metrics.
pub const DEFAULT_ROW_HEIGHT: f32 = 32.0;
#[inline]
fn sane(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        fallback
    }
}

/// Explicit label and value ink overrides; unset rows follow the runtime palette.
#[derive(Debug, Clone, Copy)]
pub struct LabeledContentTokens {
    /// Ink for the label caption.
    pub label_color: Color,
    /// Ink for read-only value text; supplied controls retain their own styles.
    pub value_color: Color,
}

impl LabeledContentTokens {
    /// Use muted label ink and primary value ink from this palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            label_color: p.text_muted,
            value_color: p.text,
        }
    }

    /// Use the fixed dark-palette colours.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Use the fixed light-palette colours.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Pinned arrangement of a labeled row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabeledContentLayout {
    /// Label beside the right cell; the default pinned arrangement.
    #[default]
    Wide,
    /// Label above the right cell.
    Compact,
}

/// Builder for a labeled value or control row with optional width-based reflow.
#[must_use]
pub struct EnzLabeledContent<'a, Message> {
    label: String,
    control: Option<Element<'a, Message>>,
    control_view: Option<Box<dyn Fn(Size) -> Element<'a, Message> + 'a>>,
    value: String,
    label_size: f32,
    label_width: f32,
    row_height: f32,
    padding_x: f32,
    row_spacing: f32,
    layout: Option<LabeledContentLayout>,
    auto_compact: bool,
    breakpoint: f32,
    allow_overflow: bool,
    label_size_explicit: bool,
    row_height_explicit: bool,
    typography: Option<TypographyScale>,
    sizing: Option<SizingTokens>,
    tokens: Option<LabeledContentTokens>,
}

/// Create a row with automatic reflow below 320 logical pixels.
/// Defaults to a 140 px label column, 12 px horizontal padding, an 8 px gap and
/// clipped cells. Unpinned text uses ambient body typography, and unpinned wide
/// height uses the published control density at the desktop text scale.
pub fn enz_labeled_content<'a, Message: 'a>(
    label: impl Into<String>,
) -> EnzLabeledContent<'a, Message> {
    EnzLabeledContent {
        label: label.into(),
        control: None,
        control_view: None,
        value: String::new(),
        label_size: DEFAULT_LABEL_SIZE,
        label_width: DEFAULT_LABEL_WIDTH,
        row_height: DEFAULT_ROW_HEIGHT,
        padding_x: 12.0,
        row_spacing: 8.0,
        layout: None,
        auto_compact: true,
        breakpoint: DEFAULT_BREAKPOINT,
        allow_overflow: false,
        label_size_explicit: false,
        row_height_explicit: false,
        typography: None,
        sizing: None,
        tokens: None,
    }
}

/// Create a row pinned to the stacked arrangement at every width.
pub fn enz_labeled_content_compact<'a, Message: 'a>(
    label: impl Into<String>,
) -> EnzLabeledContent<'a, Message> {
    EnzLabeledContent {
        layout: Some(LabeledContentLayout::Compact),
        ..enz_labeled_content(label)
    }
}

impl<'a, Message: 'a> EnzLabeledContent<'a, Message> {
    /// Set read-only value text, ignored when either control source is set.
    /// Nonempty values elide on one line and align right when wide, left when stacked.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Set a fixed control that replaces value text and retains its tree across reflow.
    /// A control factory takes precedence regardless of setter order.
    pub fn control(mut self, control: impl Into<Element<'a, Message>>) -> Self {
        self.control = Some(control.into());
        self
    }

    /// Set a rebuildable control factory that takes precedence over fixed controls and value text.
    /// Automatic layout supplies the component size. Pinned layout or disabled
    /// auto-compact supplies infinite width and the resolved row height. The factory
    /// may run repeatedly during layout and must derive its view without side effects.
    pub fn control_view(mut self, view: impl Fn(Size) -> Element<'a, Message> + 'a) -> Self {
        self.control_view = Some(Box::new(view));
        self
    }

    /// Stack below the breakpoint when enabled; defaults to true.
    /// An explicit layout takes precedence; disabling this keeps wide layout.
    pub fn auto_compact(mut self, on: bool) -> Self {
        self.auto_compact = on;
        self
    }

    /// Set the component-width threshold in logical pixels; defaults to 320.
    /// Negative or non-finite values fall back to 320 during conversion. Zero disables
    /// stacking; equality and non-positive or non-finite width probes remain wide.
    pub fn breakpoint(mut self, px: f32) -> Self {
        self.breakpoint = px;
        self
    }

    /// Allow label and control cells to paint beyond their own bounds.
    /// Defaults to false; ancestor clipping still applies.
    pub fn allow_overflow(mut self, allow: bool) -> Self {
        self.allow_overflow = allow;
        self
    }

    /// Pin label and value text size in logical pixels, overriding forwarded typography.
    /// Negative or non-finite values fall back to 13 during metric resolution.
    pub fn label_size(mut self, size: f32) -> Self {
        self.label_size = size;
        self.label_size_explicit = true;
        self
    }

    /// Set the wide-layout label-column width in logical pixels; defaults to 140.
    /// Negative or non-finite values fall back to 140 during conversion.
    pub fn label_width(mut self, width: f32) -> Self {
        self.label_width = width;
        self
    }

    /// Set horizontal padding in logical pixels; defaults to 12.
    /// Negative or non-finite values leave the preceding padding unchanged; zero is allowed.
    pub fn padding_x(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.padding_x = px;
        }
        self
    }

    /// Set the horizontal wide-layout or vertical stacked-layout gap in logical pixels.
    /// Defaults to 8; negative or non-finite values leave the preceding gap unchanged.
    pub fn row_spacing(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.row_spacing = px;
        }
        self
    }

    /// Pin wide-row and factory or explicitly stacked control-cell height, overriding sizing.
    /// Heights use logical pixels.
    /// Negative or non-finite values fall back to 32 during metric resolution.
    /// An automatic fixed-control stack uses its child's offered intrinsic height.
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height = height;
        self.row_height_explicit = true;
        self
    }

    /// Supply body typography at construction time when text size is unpinned.
    /// Explicit text size wins in either setter order. Without this, ambient body
    /// typography applies; later theme changes require rebuilding the configuration.
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }

    /// Supply control height at construction time when row height is unpinned.
    /// Explicit row height wins in either setter order. Without this, the published
    /// control density is scaled to the desktop text size.
    pub fn sizing(mut self, sizing: SizingTokens) -> Self {
        self.sizing = Some(sizing);
        self
    }

    /// Pin an arrangement, overriding automatic width-based reflow.
    pub fn layout(mut self, layout: LabeledContentLayout) -> Self {
        self.layout = Some(layout);
        self
    }

    /// Pin label and value ink; supplied controls keep their own styles.
    pub fn tokens(mut self, tokens: LabeledContentTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Build the row and, for a nonblank label, its named accessibility group.
    /// The group associates children without assigning a label to a supplied control.
    pub fn into_element(self) -> Element<'a, Message> {
        let label_size_px = if self.label_size_explicit {
            self.label_size
        } else {
            self.typography.map_or_else(
                || enzui_theme::TypographyScale::ambient().body_size,
                |t| t.body_size,
            )
        };
        let row_height_px = if self.row_height_explicit {
            self.row_height
        } else {
            self.sizing.map_or_else(
                || {
                    enzui_theme::text_scale::ui_px(
                        enzui_theme::text_defaults::sizing().control_height,
                    )
                },
                |s| s.control_height,
            )
        };
        let m = Metrics {
            label_size: sane(label_size_px, DEFAULT_LABEL_SIZE),
            label_width: sane(self.label_width, DEFAULT_LABEL_WIDTH),
            row_height: sane(row_height_px, DEFAULT_ROW_HEIGHT),
            padding_x: sane(self.padding_x, 12.0),
            row_spacing: sane(self.row_spacing, 8.0),
            clip: !self.allow_overflow,
        };
        let pinned = self.tokens;
        let label = self.label;
        let breakpoint = sane(self.breakpoint, DEFAULT_BREAKPOINT);
        let explicit = self.layout;
        let auto_compact = self.auto_compact;
        let right_cell = match (self.control_view, self.control) {
            (Some(view), _) => RightCell::Factory(view),
            (None, Some(ctrl)) => RightCell::Fixed(ctrl),
            (None, None) => RightCell::Value(self.value),
        };
        let is_factory = matches!(right_cell, RightCell::Factory(_));
        let pinned_layout = match explicit {
            Some(layout) => Some(layout),
            None if !auto_compact => Some(LabeledContentLayout::Wide),
            None => None,
        };
        let group_name = if label.trim().is_empty() {
            None
        } else {
            Some(label.clone())
        };
        let inner: Element<'a, Message> = if let Some(layout) = pinned_layout {
            let label_el = label_element(label, pinned, m.label_size);
            let right =
                right_cell.into_element(layout, Size::new(f32::INFINITY, m.row_height), pinned, m);
            assemble(layout, label_el, right, m)
        } else if !is_factory {
            match right_cell {
                RightCell::Value(v) if !v.is_empty() => {
                    let clip = m.clip;
                    let label_size = m.label_size;
                    let row_height = m.row_height;
                    let label_width = m.label_width;
                    let padding_x = m.padding_x;
                    let row_spacing = m.row_spacing;
                    let bp = breakpoint;
                    let pinned_for_value = pinned;
                    let label_for_closure = label;
                    enz_responsive(move |size: Size| -> Element<'a, Message> {
                        // Match ReflowPair classification so value alignment follows its geometry.
                        let stacked = size.width.is_finite() && size.width > 0.0 && size.width < bp;
                        let layout = if stacked {
                            LabeledContentLayout::Compact
                        } else {
                            LabeledContentLayout::Wide
                        };
                        let label_el = label_cell(
                            label_element(label_for_closure.clone(), pinned_for_value, label_size),
                            m,
                        );
                        let value_el =
                            value_element(v.clone(), pinned_for_value, label_size, layout, clip);
                        reflow_pair(
                            label_el,
                            control_cell(value_el, m),
                            bp,
                            PairArm {
                                padding: Padding {
                                    top: 0.0,
                                    right: padding_x,
                                    bottom: 0.0,
                                    left: padding_x,
                                },
                                spacing: row_spacing,
                                align: Alignment::Center,
                                height: Length::Fixed(row_height),
                                leading_width: Length::Fixed(label_width),
                            },
                            PairArm {
                                padding: Padding {
                                    top: 0.0,
                                    right: padding_x,
                                    bottom: 0.0,
                                    left: padding_x,
                                },
                                spacing: row_spacing,
                                align: Alignment::Start,
                                height: Length::Shrink,
                                leading_width: Length::Fill,
                            },
                        )
                        .into()
                    })
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .into()
                }
                other => {
                    let label_el = label_cell(label_element(label, pinned, m.label_size), m);
                    let right = other.into_element(
                        LabeledContentLayout::Wide,
                        Size::new(f32::INFINITY, m.row_height),
                        pinned,
                        m,
                    );
                    reflow_pair(
                        label_el,
                        control_cell(right, m),
                        breakpoint,
                        PairArm {
                            padding: Padding {
                                top: 0.0,
                                right: m.padding_x,
                                bottom: 0.0,
                                left: m.padding_x,
                            },
                            spacing: m.row_spacing,
                            align: Alignment::Center,
                            height: Length::Fixed(m.row_height),
                            leading_width: Length::Fixed(m.label_width),
                        },
                        PairArm {
                            padding: Padding {
                                top: 0.0,
                                right: m.padding_x,
                                bottom: 0.0,
                                left: m.padding_x,
                            },
                            spacing: m.row_spacing,
                            align: Alignment::Start,
                            height: Length::Shrink,
                            leading_width: Length::Fill,
                        },
                    )
                    .into()
                }
            }
        } else {
            Self::factory_responsive_inner(label, right_cell, pinned, m, breakpoint)
        };
        if let Some(name) = group_name {
            crate::semantics_container::SemanticsContainer::new(
                inner,
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                    .label(name),
            )
            .into()
        } else {
            inner
        }
    }
    fn factory_responsive_inner(
        label: String,
        right_cell: RightCell<'a, Message>,
        pinned: Option<LabeledContentTokens>,
        m: Metrics,
        breakpoint: f32,
    ) -> Element<'a, Message> {
        // Shrink height keeps the responsive wrapper from inflating intrinsic row height.
        enz_responsive(move |size: Size| {
            let layout = if size.width > 0.0 && size.width < breakpoint {
                LabeledContentLayout::Compact
            } else {
                LabeledContentLayout::Wide
            };
            let label_el = label_element(label.clone(), pinned, m.label_size);
            let right = right_cell.rebuild(layout, size, pinned, m);
            assemble(layout, label_el, right, m)
        })
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
    }
}
enum RightCell<'a, Message> {
    Factory(Box<dyn Fn(Size) -> Element<'a, Message> + 'a>),
    Fixed(Element<'a, Message>),
    Value(String),
}

impl<'a, Message: 'a> RightCell<'a, Message> {
    fn blank(m: Metrics) -> Element<'a, Message> {
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(m.row_height))
            .into()
    }
    fn into_element(
        self,
        layout: LabeledContentLayout,
        size: Size,
        pinned: Option<LabeledContentTokens>,
        m: Metrics,
    ) -> Element<'a, Message> {
        match self {
            Self::Factory(view) => view(size),
            Self::Fixed(ctrl) => ctrl,
            Self::Value(value) if !value.is_empty() => {
                value_element(value, pinned, m.label_size, layout, m.clip)
            }
            Self::Value(_) => Self::blank(m),
        }
    }
    fn rebuild(
        &self,
        layout: LabeledContentLayout,
        size: Size,
        pinned: Option<LabeledContentTokens>,
        m: Metrics,
    ) -> Element<'a, Message> {
        match self {
            Self::Factory(view) => view(size),
            Self::Value(value) if !value.is_empty() => {
                value_element(value.clone(), pinned, m.label_size, layout, m.clip)
            }
            Self::Value(_) | Self::Fixed(_) => Self::blank(m),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Metrics {
    label_size: f32,
    label_width: f32,
    row_height: f32,
    padding_x: f32,
    row_spacing: f32,
    clip: bool,
}
fn label_element<'a, Message: 'a>(
    label: String,
    pinned: Option<LabeledContentTokens>,
    size: f32,
) -> Element<'a, Message> {
    match pinned {
        None => crate::enz_text::enz_text(label)
            .size_px(size)
            .weight_rung(enzui_theme::WeightRung::Medium)
            .tone(crate::enz_text::TextTone::Muted)
            .single_line_elided()
            .into(),
        Some(t) => crate::enz_text::enz_text(label)
            .size_px(size)
            .weight_rung(enzui_theme::WeightRung::Medium)
            .single_line_elided()
            .color(t.label_color)
            .into(),
    }
}
fn value_element<'a, Message: 'a>(
    value: String,
    pinned: Option<LabeledContentTokens>,
    size: f32,
    layout: LabeledContentLayout,
    clip: bool,
) -> Element<'a, Message> {
    let (align, text_align) = if layout == LabeledContentLayout::Compact {
        (Alignment::Start, crate::enz_text::TextAlign::Left)
    } else {
        (Alignment::End, crate::enz_text::TextAlign::Right)
    };
    let inner: Element<'a, Message> = match pinned {
        None => crate::enz_text::enz_text(value)
            .size_px(size)
            .tone(crate::enz_text::TextTone::Default)
            .align(text_align)
            .single_line_elided()
            .full_width()
            .into(),
        Some(t) => crate::enz_text::enz_text(value)
            .size_px(size)
            .align(text_align)
            .single_line_elided()
            .full_width()
            .color(t.value_color)
            .into(),
    };
    container(inner)
        .width(Length::Fill)
        .align_x(align)
        .clip(clip)
        .into()
}
fn label_cell<'a, Message: 'a>(label: Element<'a, Message>, m: Metrics) -> Element<'a, Message> {
    container(label).clip(m.clip).into()
}
fn control_cell<'a, Message: 'a>(
    control: Element<'a, Message>,
    m: Metrics,
) -> Element<'a, Message> {
    container(control).width(Length::Fill).clip(m.clip).into()
}
fn assemble<'a, Message: 'a>(
    layout: LabeledContentLayout,
    label: Element<'a, Message>,
    right: Element<'a, Message>,
    m: Metrics,
) -> Element<'a, Message> {
    let pad = Padding {
        top: 0.0,
        right: m.padding_x,
        bottom: 0.0,
        left: m.padding_x,
    };
    match layout {
        LabeledContentLayout::Wide => container(
            row![
                container(label)
                    .width(Length::Fixed(m.label_width))
                    .clip(m.clip)
                    .center_y(Length::Fixed(m.row_height)),
                // Cell clipping blocks paint into the label even inside the row's outer bounds.
                container(right)
                    .width(Length::Fill)
                    .clip(m.clip)
                    .center_y(Length::Fixed(m.row_height)),
            ]
            .spacing(m.row_spacing),
        )
        .padding(pad)
        .height(Length::Fixed(m.row_height))
        .clip(m.clip)
        .into(),

        LabeledContentLayout::Compact => container(
            column![
                container(label)
                    .height(Length::Fixed(m.label_size + 4.0))
                    .clip(m.clip)
                    .align_y(Alignment::Center),
                container(right)
                    .width(Length::Fill)
                    .height(Length::Fixed(m.row_height))
                    .clip(m.clip),
            ]
            .spacing(m.row_spacing),
        )
        .padding(pad)
        .clip(m.clip)
        .into(),
    }
}

impl<'a, Message: 'a> From<EnzLabeledContent<'a, Message>> for Element<'a, Message> {
    fn from(row: EnzLabeledContent<'a, Message>) -> Self {
        row.into_element()
    }
}
