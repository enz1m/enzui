//! Formatting controls for [`RichEditController`].
//!
//! [`enz_rich_edit_toolbar`] offers the five inline-style toggles; its caller
//! forwards the chosen style to [`RichEditController::toggle_style`]. Multiple
//! active styles are selected simultaneously.
//!
//! [`EnzRichEdit`](super::EnzRichEdit) also offers a single-select paragraph
//! alignment group by default, with optional colour chips and caller controls.
//! Groups share a wrapping row: trailing controls remain beside the preceding
//! groups only when their combined measured width fits. Compact density lowers
//! control height without removing controls or guaranteeing a single line.

use super::document::{ListKind, ParagraphAlign};
use super::{InlineStyle, RichEditAction, RichEditController, ToolbarMenu};
use crate::button_group::{EnzButtonGroupItem, enz_button_group};
use crate::color_swatch::{enz_color_swatch, enz_color_swatch_none};
use crate::flow::enz_flow;
use crate::surface_stack::SpaceToken;
use iced::{Color, Element};

/// Build a formatting toolbar for a [`RichEditController`].
///
/// Returns an `Element` containing one button per [`InlineStyle`] variant.
/// The `on_toggle` callback receives the [`InlineStyle`] that was toggled.
/// Active styles (per [`RichEditController::active_styles`]) are visually
/// highlighted.
///
/// The toolbar is a thin wrapper around [`enz_button_group`] in segmented
/// multi-select mode — each button fires independently, and the caller
/// reflects the toggle back into the controller. Every active style lights
/// up: the toolbar hands the group the full set of active-style indices via
/// [`selected_set`](crate::button_group::EnzButtonGroup::selected_set), so
/// bold-and-italic text lights both cells at once, not just the first.
pub fn enz_rich_edit_toolbar<'a, Message: Clone + 'a>(
    controller: &RichEditController,
    on_toggle: impl Fn(InlineStyle) -> Message + 'a,
) -> Element<'a, Message> {
    enz_rich_edit_toolbar_builder(controller, on_toggle).into_element()
}

/// The two cell heights the style row is built at.
///
/// The compact rung is for a toolbar sharing a line with other controls in a
/// host whose subject is the editor below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RichEditToolbarDensity {
    /// The standalone 28 px row.
    #[default]
    Standard,
    /// The 24 px row for a toolbar sharing a line with other controls.
    Compact,
}

impl RichEditToolbarDensity {
    fn button_height(self) -> f32 {
        match self {
            RichEditToolbarDensity::Standard => 28.0,
            RichEditToolbarDensity::Compact => 24.0,
        }
    }

    fn swatch_size(self) -> f32 {
        match self {
            RichEditToolbarDensity::Standard => 28.0,
            RichEditToolbarDensity::Compact => 24.0,
        }
    }

    fn corner_radius(self) -> f32 {
        match self {
            RichEditToolbarDensity::Standard => 6.0,
            RichEditToolbarDensity::Compact => 5.0,
        }
    }
}

/// One offered ink in a toolbar colour strip.
///
/// The label is the swatch's accessible name. A `None` colour is the clear
/// chip, which reverts a run to the editor's ambient ink rather than painting
/// some particular grey.
#[derive(Debug, Clone, PartialEq)]
pub struct RichEditSwatch {
    /// The ink this chip applies, or `None` to clear an authored ink.
    pub color: Option<Color>,
    /// The accessible name for the chip.
    pub label: String,
}

impl RichEditSwatch {
    /// An ink chip.
    pub fn ink(color: Color, label: impl Into<String>) -> Self {
        Self {
            color: Some(color),
            label: label.into(),
        }
    }

    /// The clear chip.
    pub fn clear(label: impl Into<String>) -> Self {
        Self {
            color: None,
            label: label.into(),
        }
    }
}

/// Build a standalone formatting toolbar from the active document selection.
/// Inline style toggles are enabled by default. Add [`EnzRichEditToolbar::alignment`]
/// and [`EnzRichEditToolbar::actions`] for paragraph alignment, lists, links,
/// and clear formatting. [`EnzRichEditToolbar::colors`] enables the colour
/// and highlight popover when actions are installed.
pub fn enz_rich_edit_toolbar_builder<'a, Message: Clone + 'a>(
    controller: &RichEditController,
    on_toggle: impl Fn(InlineStyle) -> Message + 'a,
) -> EnzRichEditToolbar<'a, Message> {
    EnzRichEditToolbar {
        active_styles: controller.active_styles(),
        active_color: controller.active_color(),
        on_toggle: Box::new(on_toggle),
        styles: InlineStyle::ALL.to_vec(),
        density: RichEditToolbarDensity::Standard,
        swatches: Vec::new(),
        on_color: None,
        leading: Vec::new(),
        trailing: Vec::new(),
        active_align: controller.active_paragraph_align(),
        on_align: None,
        on_action: None,
        menu: controller.menu(),
        link_draft: controller.link_draft().to_owned(),
        active_link: controller.active_link().map(str::to_owned),
        active_highlight: controller.active_highlight(),
        active_list: controller.active_list(),
    }
}

/// The id of the link popover's URL field, which a host focuses when the
/// popover opens (see [`RichEditController::menu`]).
pub fn link_field_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("{DEFAULT_TOOLBAR_ID_PREFIX}-link-url"))
}

/// The marker colours the colour popover offers behind text.
fn highlight_swatches() -> Vec<RichEditSwatch> {
    vec![
        RichEditSwatch::clear(enzui_core::localization::text(
            "enzui-rich-edit-no-highlight",
        )),
        RichEditSwatch::ink(
            Color::from_rgb8(0xFF, 0xF1, 0x76),
            enzui_core::localization::text("enzui-rich-edit-yellow-highlight"),
        ),
        RichEditSwatch::ink(
            Color::from_rgb8(0xB9, 0xF6, 0xCA),
            enzui_core::localization::text("enzui-rich-edit-green-highlight"),
        ),
        RichEditSwatch::ink(
            Color::from_rgb8(0xB3, 0xE5, 0xFC),
            enzui_core::localization::text("enzui-rich-edit-blue-highlight"),
        ),
        RichEditSwatch::ink(
            Color::from_rgb8(0xF8, 0xBB, 0xD0),
            enzui_core::localization::text("enzui-rich-edit-pink-highlight"),
        ),
    ]
}

/// The namespace toolbar control ids are derived under.
///
/// A colour chip appends `-color` and its own colour key, so
/// `rich-edit-toolbar-color-#D01B1B` addresses one ink and
/// `rich-edit-toolbar-color-none` the clear chip.
pub const DEFAULT_TOOLBAR_ID_PREFIX: &str = "rich-edit-toolbar";

/// Standalone wrapping formatting row constructed with [`enz_rich_edit_toolbar_builder`].
///
/// The row is a wrapping [`enz_flow`](crate::flow::enz_flow), so a colour strip
/// and caller controls added beside the style toggles reflow onto a second line
/// at a narrow width instead of clipping.
#[must_use]
pub struct EnzRichEditToolbar<'a, Message: Clone + 'a> {
    active_styles: Vec<InlineStyle>,
    active_color: Option<Color>,
    on_toggle: Box<dyn Fn(InlineStyle) -> Message + 'a>,
    styles: Vec<InlineStyle>,
    density: RichEditToolbarDensity,
    swatches: Vec<RichEditSwatch>,
    on_color: Option<Box<dyn Fn(Option<Color>) -> Message + 'a>>,
    leading: Vec<Element<'a, Message>>,
    trailing: Vec<Element<'a, Message>>,
    active_align: Option<ParagraphAlign>,
    on_align: Option<Box<dyn Fn(ParagraphAlign) -> Message + 'a>>,
    on_action: Option<std::rc::Rc<dyn Fn(RichEditAction) -> Message + 'a>>,
    menu: Option<ToolbarMenu>,
    link_draft: String,
    active_link: Option<String>,
    active_highlight: Option<Color>,
    active_list: Option<ListKind>,
}

impl<'a, Message: Clone + 'a> EnzRichEditToolbar<'a, Message> {
    /// Offer inline style toggles in the given order; defaults to [`InlineStyle::ALL`].
    /// Omitted styles remain in the document and HTML.
    pub fn styles(mut self, styles: Vec<InlineStyle>) -> Self {
        self.styles = styles;
        self
    }

    /// Choose the row height. Defaults to [`RichEditToolbarDensity::Standard`].
    pub fn density(mut self, density: RichEditToolbarDensity) -> Self {
        self.density = density;
        self
    }

    /// Offer an inline colour strip driven by `swatches`.
    ///
    /// The chip matching [`RichEditController::active_color`] is drawn
    /// selected, so the row reports the ink the next edit would carry rather
    /// than only offering a choice.
    pub fn colors(
        mut self,
        swatches: Vec<RichEditSwatch>,
        on_color: impl Fn(Option<Color>) -> Message + 'a,
    ) -> Self {
        self.swatches = swatches;
        self.on_color = Some(Box::new(on_color));
        self
    }

    /// Offer the paragraph-alignment control after the style toggles.
    ///
    /// Single-select, unlike the style group: a paragraph has exactly one
    /// alignment. The cell matching
    /// [`RichEditController::active_paragraph_align`] is drawn selected, and a
    /// selection spanning paragraphs that disagree lights none — the same
    /// answer the colour strip gives for a selection spanning two inks.
    pub fn alignment(mut self, on_align: impl Fn(ParagraphAlign) -> Message + 'a) -> Self {
        self.on_align = Some(Box::new(on_align));
        self
    }

    /// Publish list, link and clear-formatting commands through one callback.
    /// With [`Self::colors`], colours and highlights appear in a popover;
    /// without actions, colours remain an inline strip.
    pub fn actions(mut self, on_action: impl Fn(RichEditAction) -> Message + 'a) -> Self {
        self.on_action = Some(std::rc::Rc::new(on_action));
        self
    }

    /// Prepend a caller-owned control to the same row.
    pub fn leading(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.leading.push(element.into());
        self
    }

    /// Append a caller-owned control to the wrapping row.
    pub fn trailing(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.trailing.push(element.into());
        self
    }

    /// Build the row into an [`Element`].
    pub fn into_element(self) -> Element<'a, Message> {
        let styles = style_group(
            &self.styles,
            &self.active_styles,
            self.on_toggle,
            self.density,
            DEFAULT_TOOLBAR_ID_PREFIX,
        );
        let mut row = enz_flow()
            .wrap(true)
            .gap(SpaceToken::Xs)
            .cross_gap(SpaceToken::Xs)
            .align_items(iced::Alignment::Center);
        for element in self.leading {
            row = row.push(element);
        }
        row = row.push(styles);

        if let Some(on_align) = self.on_align {
            row = row.push(alignment_group(
                self.active_align,
                on_align,
                self.density,
                DEFAULT_TOOLBAR_ID_PREFIX,
            ));
        }

        if let Some(on_action) = self.on_action.clone() {
            row = row.push(list_group(
                self.active_list,
                std::rc::Rc::clone(&on_action),
                self.density,
            ));
            let tools = tools_group(
                self.menu,
                self.active_link.is_some(),
                self.on_color.is_some(),
                std::rc::Rc::clone(&on_action),
                self.density,
            );
            let popup = match self.menu {
                Some(ToolbarMenu::Link) => Some(link_popover(
                    &self.link_draft,
                    self.active_link.is_some(),
                    std::rc::Rc::clone(&on_action),
                )),
                Some(ToolbarMenu::Color) => self.on_color.as_ref().map(|on_color| {
                    color_popover(
                        &self.swatches,
                        self.active_color,
                        on_color,
                        self.active_highlight,
                        std::rc::Rc::clone(&on_action),
                        self.density,
                    )
                }),
                None => None,
            };
            let dismiss = std::rc::Rc::clone(&on_action);
            row = row.push(crate::anchored_overlay::dismissible_anchored_menu(
                tools,
                popup,
                enzui_core::placement::Placement::new(enzui_core::placement::Side::Bottom)
                    .alignment(enzui_core::placement::Alignment::Start)
                    .gap(4.0),
                move || dismiss(RichEditAction::CloseMenu),
            ));
        } else if let Some(on_color) = self.on_color {
            let mut strip = enz_flow()
                .wrap(true)
                .gap(SpaceToken::Xxs)
                .cross_gap(SpaceToken::Xxs)
                .align_items(iced::Alignment::Center);
            for swatch in self.swatches {
                let selected = swatch.color == self.active_color;
                let message = on_color(swatch.color);
                let chip = match swatch.color {
                    Some(color) => enz_color_swatch(color, message),
                    None => enz_color_swatch_none(message),
                };
                strip = strip.push(
                    chip.aria_label(swatch.label)
                        .selected(selected)
                        .size(self.density.swatch_size())
                        .id_prefix(format!("{DEFAULT_TOOLBAR_ID_PREFIX}-color"))
                        .into_element(),
                );
            }
            row = row.push(strip.into_element());
        }

        for element in self.trailing {
            row = row.push(element);
        }

        row.into_element()
    }
}

impl<'a, Message: Clone + 'a> From<EnzRichEditToolbar<'a, Message>> for Element<'a, Message> {
    fn from(builder: EnzRichEditToolbar<'a, Message>) -> Self {
        builder.into_element()
    }
}

// Reading order, with justify last: it is the least reached for, and the row
// wraps from its trailing edge.
const OFFERED_ALIGNMENTS: [ParagraphAlign; 4] = [
    ParagraphAlign::Left,
    ParagraphAlign::Center,
    ParagraphAlign::Right,
    ParagraphAlign::Justify,
];

fn alignment_group<'a, Message: Clone + 'a>(
    active: Option<ParagraphAlign>,
    on_align: Box<dyn Fn(ParagraphAlign) -> Message + 'a>,
    density: RichEditToolbarDensity,
    id_prefix: &str,
) -> Element<'a, Message> {
    // The label is the accessible name, as in the style group: `.icon_only`
    // paints the glyph and keeps the word for assistive technology, so the cell
    // announces "Align centre" rather than an undecodable bar glyph.
    let items: Vec<EnzButtonGroupItem> = OFFERED_ALIGNMENTS
        .iter()
        .map(|&align| {
            // The group prefixes each item id with its own, so the cells are
            // `rich-edit-toolbar-align-left` and not `…-align-align-left`.
            let (id, label, icon) = match align {
                ParagraphAlign::Left => (
                    "left",
                    enzui_core::localization::text("enzui-rich-edit-align-left"),
                    "align-left",
                ),
                ParagraphAlign::Center => (
                    "center",
                    enzui_core::localization::text("enzui-rich-edit-align-centre"),
                    "align-center",
                ),
                ParagraphAlign::Right => (
                    "right",
                    enzui_core::localization::text("enzui-rich-edit-align-right"),
                    "align-right",
                ),
                ParagraphAlign::Justify => (
                    "justify",
                    enzui_core::localization::text("enzui-rich-edit-justify"),
                    "align-justify",
                ),
            };
            EnzButtonGroupItem::new(id, label).icon(icon)
        })
        .collect();

    let group = enz_button_group(items, move |index| on_align(OFFERED_ALIGNMENTS[index]))
        .button_height(density.button_height())
        .corner_radius(density.corner_radius())
        .icon_only(true)
        .id_prefix(format!("{id_prefix}-align"));

    // A selection whose paragraphs disagree has no chosen alignment, so the
    // group renders selectable with no cell lit rather than lying about one.
    let selected: Vec<usize> = active
        .and_then(|align| {
            OFFERED_ALIGNMENTS
                .iter()
                .position(|offered| *offered == align)
        })
        .into_iter()
        .collect();

    group.selectable(true).selected_set(selected).into()
}

const OFFERED_LISTS: [ListKind; 2] = [ListKind::Bullet, ListKind::Numbered];

fn list_group<'a, Message: Clone + 'a>(
    active: Option<ListKind>,
    on_action: std::rc::Rc<dyn Fn(RichEditAction) -> Message + 'a>,
    density: RichEditToolbarDensity,
) -> Element<'a, Message> {
    let items = vec![
        EnzButtonGroupItem::new(
            "bullets",
            enzui_core::localization::text("enzui-rich-edit-bulleted-list"),
        )
        .icon("list"),
        EnzButtonGroupItem::new(
            "numbers",
            enzui_core::localization::text("enzui-rich-edit-numbered-list"),
        )
        .icon("list-ordered"),
    ];
    let selected: Vec<usize> = active
        .and_then(|kind| OFFERED_LISTS.iter().position(|offered| *offered == kind))
        .into_iter()
        .collect();
    enz_button_group(items, move |index| {
        on_action(RichEditAction::ToggleList(OFFERED_LISTS[index]))
    })
    .button_height(density.button_height())
    .corner_radius(density.corner_radius())
    .icon_only(true)
    .id_prefix(format!("{DEFAULT_TOOLBAR_ID_PREFIX}-list"))
    .selectable(true)
    .selected_set(selected)
    .into()
}

// Link, colour and clear formatting: the first two open a popover, so each
// lights while its popover is open; the link cell also lights inside a link.
fn tools_group<'a, Message: Clone + 'a>(
    menu: Option<ToolbarMenu>,
    in_link: bool,
    colors: bool,
    on_action: std::rc::Rc<dyn Fn(RichEditAction) -> Message + 'a>,
    density: RichEditToolbarDensity,
) -> Element<'a, Message> {
    let mut items = vec![
        EnzButtonGroupItem::new(
            "link",
            enzui_core::localization::text("enzui-rich-edit-link"),
        )
        .icon("link"),
    ];
    let mut actions = vec![RichEditAction::ToggleMenu(ToolbarMenu::Link)];
    if colors {
        items.push(
            EnzButtonGroupItem::new(
                "color",
                enzui_core::localization::text("enzui-rich-edit-text-colour-and-highlight"),
            )
            .icon("palette"),
        );
        actions.push(RichEditAction::ToggleMenu(ToolbarMenu::Color));
    }
    items.push(
        EnzButtonGroupItem::new(
            "clear",
            enzui_core::localization::text("enzui-rich-edit-clear-formatting"),
        )
        .icon("remove-formatting"),
    );
    actions.push(RichEditAction::ClearFormatting);

    let mut selected = Vec::new();
    if in_link || menu == Some(ToolbarMenu::Link) {
        selected.push(0);
    }
    if colors && menu == Some(ToolbarMenu::Color) {
        selected.push(1);
    }
    enz_button_group(items, move |index| on_action(actions[index].clone()))
        .button_height(density.button_height())
        .corner_radius(density.corner_radius())
        .icon_only(true)
        .id_prefix(DEFAULT_TOOLBAR_ID_PREFIX)
        .selectable(true)
        .selected_set(selected)
        .into()
}

fn popover_card<'a, Message: 'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    crate::surface::enz_surface(iced::widget::container(content).padding(10)).into()
}

fn link_popover<'a, Message: Clone + 'a>(
    draft: &str,
    in_link: bool,
    on_action: std::rc::Rc<dyn Fn(RichEditAction) -> Message + 'a>,
) -> Element<'a, Message> {
    let edit = std::rc::Rc::clone(&on_action);
    let submit = std::rc::Rc::clone(&on_action);
    let field = crate::text_field::enz_text_field_owned(
        enzui_core::localization::text("enzui-rich-edit-link"),
        "https://example.com",
        draft.to_owned(),
        move |text| edit(RichEditAction::EditLink(text)),
    )
    .on_submit_with(move || submit(RichEditAction::ApplyLink))
    .id(link_field_id());
    let mut actions = iced::widget::row![].spacing(6);
    if in_link {
        actions = actions.push(
            crate::button::enz_button(
                enzui_core::localization::text("enzui-rich-edit-remove-link"),
                on_action(RichEditAction::RemoveLink),
            )
            .kind(crate::button::ButtonKind::Quiet)
            .size(crate::button::ButtonSize::Sm),
        );
    }
    actions = actions.push(
        crate::button::enz_button(
            enzui_core::localization::text("enzui-rich-edit-apply"),
            on_action(RichEditAction::ApplyLink),
        )
        .kind(crate::button::ButtonKind::Primary)
        .size(crate::button::ButtonSize::Sm),
    );
    popover_card(
        iced::widget::column![
            iced::widget::container(field).width(iced::Length::Fixed(280.0)),
            actions
        ]
        .spacing(8)
        .align_x(iced::Alignment::End)
        .into(),
    )
}

fn color_popover<'a, Message: Clone + 'a>(
    swatches: &[RichEditSwatch],
    active_color: Option<Color>,
    on_color: &(dyn Fn(Option<Color>) -> Message + 'a),
    active_highlight: Option<Color>,
    on_action: std::rc::Rc<dyn Fn(RichEditAction) -> Message + 'a>,
    density: RichEditToolbarDensity,
) -> Element<'a, Message> {
    let chips = |offered: &[RichEditSwatch],
                 active: Option<Color>,
                 message: &dyn Fn(Option<Color>) -> Message,
                 key: &str| {
        let mut strip = iced::widget::row![].spacing(4);
        for swatch in offered {
            let chip = match swatch.color {
                Some(color) => enz_color_swatch(color, message(swatch.color)),
                None => enz_color_swatch_none(message(None)),
            };
            strip = strip.push(
                chip.aria_label(swatch.label.clone())
                    .selected(swatch.color == active)
                    .size(density.swatch_size())
                    .id_prefix(format!("{DEFAULT_TOOLBAR_ID_PREFIX}-{key}"))
                    .into_element(),
            );
        }
        strip
    };
    let caption = |text: String| crate::text::enz_text(text).size(crate::text::TextSize::Caption);
    let highlight = move |color| on_action(RichEditAction::SetHighlight(color));
    popover_card(
        iced::widget::column![
            caption(enzui_core::localization::text("enzui-rich-edit-text")),
            chips(swatches, active_color, on_color, "color"),
            caption(enzui_core::localization::text("enzui-rich-edit-highlight")),
            chips(
                &highlight_swatches(),
                active_highlight,
                &highlight,
                "highlight"
            ),
        ]
        .spacing(6)
        .into(),
    )
}

fn style_group<'a, Message: Clone + 'a>(
    offered: &[InlineStyle],
    active: &[InlineStyle],
    on_toggle: Box<dyn Fn(InlineStyle) -> Message + 'a>,
    density: RichEditToolbarDensity,
    id_prefix: &str,
) -> Element<'a, Message> {
    // Build items with icon glyphs. The `label` is the ACCESSIBLE name,
    // not visible text: `.icon_only(true)` on the group drops the label from the
    // painted cell but keeps it as the cell's `aria_label`. So the row shows the
    // Lucide glyph while the a11y tree announces the full word — "Code" instead
    // of the old undecodable `<>` digraph.
    let items: Vec<EnzButtonGroupItem> = offered
        .iter()
        .map(|&style| {
            let (id, label, icon) = match style {
                InlineStyle::Bold => (
                    "bold",
                    enzui_core::localization::text("enzui-rich-edit-bold"),
                    "bold",
                ),
                InlineStyle::Italic => (
                    "italic",
                    enzui_core::localization::text("enzui-rich-edit-italic"),
                    "italic",
                ),
                InlineStyle::Underline => (
                    "underline",
                    enzui_core::localization::text("enzui-rich-edit-underline"),
                    "underline",
                ),
                InlineStyle::Strikethrough => (
                    "strike",
                    enzui_core::localization::text("enzui-rich-edit-strikethrough"),
                    "strikethrough",
                ),
                InlineStyle::Code => (
                    "code",
                    enzui_core::localization::text("enzui-rich-edit-code"),
                    "code",
                ),
            };
            EnzButtonGroupItem::new(id, label).icon(icon)
        })
        .collect();

    // We use the button group's activation callback to map the index back
    // to the InlineStyle and fire the caller's on_toggle. The index addresses
    // the OFFERED list, not `InlineStyle::ALL`: a row that omits a style would
    // otherwise fire the wrong toggle for every cell after the gap.
    let offered_styles: Vec<InlineStyle> = offered.to_vec();
    let toolbar = enz_button_group(items, move |idx| on_toggle(offered_styles[idx]))
        .button_height(density.button_height())
        .corner_radius(density.corner_radius())
        .icon_only(true)
        .id_prefix(id_prefix);

    // light every active style, not just the first. Inline styles are
    // independently active, so we map each active style to its index in
    // `InlineStyle::ALL` and hand the group the whole set via `selected_set`.
    // An empty set (no active styles) still renders in selectable mode with no
    // cell lit.
    let selected: Vec<usize> = active
        .iter()
        .filter_map(|style| offered.iter().position(|s| s == style))
        .collect();

    toolbar.selectable(true).selected_set(selected).into()
}
