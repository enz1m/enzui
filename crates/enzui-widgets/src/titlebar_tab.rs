//! Align document tabs with titlebar window controls using a shared height.
//!
//! [`enz_titlebar_document_tab`] bounds the tab to
//! [`window_controls::DEFAULT_CONTROL_SIZE`] and centers it vertically in its slot.
//! The slot must itself be centered in the titlebar band for the tab and controls
//! to share a centerline. The wrapper leaves the cell’s styling and event wiring intact.
//!
//! The ordinary constructor shrink-wraps width. [`enz_titlebar_document_tab_fill`]
//! fixes the outer width to the cell’s maximum width and fills the inner slot;
//! use equal minimum/maximum widths and `fill_width(true)` for packed browser tabs
//! whose close buttons sit at their trailing edges.
//! [`enz_titlebar_document_tab_with`] matches a custom control height.

use crate::anchored_overlay::dismissible_anchored_menu;
use crate::document_tab::{self, EnzDocumentTab, enz_document_tab};
use crate::menu::{EnzMenuItem, enz_menu};
use crate::toolbar_icon::enz_toolbar_icon;
use crate::window_controls;
use enzui_core::menu_model::EntryId;
use enzui_core::placement::{Alignment as PlacementAlignment, Placement, Side};
use iced::alignment::Vertical;
use iced::widget::{container, responsive, row};
use iced::{Alignment, Element, Length};

/// Center a content-sized tab in its slot at the default window-control height.
///
/// The cell’s own style and callbacks are preserved. See the [module example](self).
pub fn enz_titlebar_document_tab<'a, Message: Clone + 'a>(
    cell: EnzDocumentTab<'a, Message>,
) -> Element<'a, Message> {
    compose_titlebar_document_tab(
        cell,
        window_controls::DEFAULT_CONTROL_SIZE,
        TabWidthMode::Shrink,
        None,
        crate::tooltip::default_tooltip_delay(),
    )
}

/// Center a tab at window-control height in a fixed-width slot.
///
/// The outer slot uses the cell’s maximum width; the inner layer fills it. Use a
/// cell with equal minimum/maximum widths and `fill_width(true)` to place its close
/// button at the trailing edge. The slot must fit within the parent’s constraints.
#[must_use]
pub fn enz_titlebar_document_tab_fill<'a, Message: Clone + 'a>(
    cell: EnzDocumentTab<'a, Message>,
) -> Element<'a, Message> {
    compose_titlebar_document_tab(
        cell,
        window_controls::DEFAULT_CONTROL_SIZE,
        TabWidthMode::Fill,
        None,
        crate::tooltip::default_tooltip_delay(),
    )
}

/// [`enz_titlebar_document_tab_fill`] with a hover description, which hangs
/// from the painted tab rather than from the band the tab is centred in.
pub(crate) fn described_titlebar_document_tab_fill<'a, Message: Clone + 'a>(
    cell: EnzDocumentTab<'a, Message>,
    description: Option<&String>,
    delay: std::time::Duration,
) -> Element<'a, Message> {
    compose_titlebar_document_tab(
        cell,
        window_controls::DEFAULT_CONTROL_SIZE,
        TabWidthMode::Fill,
        description.cloned(),
        delay,
    )
}

/// Center a content-sized tab using a custom control height in logical pixels.
///
/// Negative, zero or non-finite heights use [`window_controls::DEFAULT_CONTROL_SIZE`].
pub fn enz_titlebar_document_tab_with<'a, Message: Clone + 'a>(
    cell: EnzDocumentTab<'a, Message>,
    control_size: f32,
) -> Element<'a, Message> {
    compose_titlebar_document_tab(
        cell,
        control_size,
        TabWidthMode::Shrink,
        None,
        crate::tooltip::default_tooltip_delay(),
    )
}

#[derive(Debug, Clone, Copy)]
enum TabWidthMode {
    Shrink,
    Fill,
}

impl TabWidthMode {
    fn resolve<Message: Clone>(self, cell: &EnzDocumentTab<'_, Message>) -> (Length, Length) {
        match self {
            TabWidthMode::Shrink => (Length::Shrink, Length::Shrink),
            TabWidthMode::Fill => {
                // Fixed slots retain the requested tab width instead of sharing remaining row space.
                (Length::Fixed(cell.width_range().hi()), Length::Fill)
            }
        }
    }
}

fn compose_titlebar_document_tab<'a, Message: Clone + 'a>(
    cell: EnzDocumentTab<'a, Message>,
    control_size: f32,
    mode: TabWidthMode,
    description: Option<String>,
    delay: std::time::Duration,
) -> Element<'a, Message> {
    let control_size = if control_size.is_finite() && control_size > 0.0 {
        control_size
    } else {
        window_controls::DEFAULT_CONTROL_SIZE
    };
    // Resolve the two layers' widths from the cell before it is consumed by
    // `into_element()`. The layers differ in fill mode (fixed outer, fill
    // inner) — see `TabWidthMode`.
    let (outer_width, inner_width) = mode.resolve(&cell);
    // Inner container: bounds the tab's painted height to `control_size` by
    // handing `TabHeightFill` a finite `max.height`. In the ordinary
    // mode `Shrink` leaves width ownership to the tab's min/max clamps. In
    // browser fill mode `Fill` fills the fixed outer slot so the tab's own
    // fill spacer can reach its trailing edge.
    let bounded: Element<'a, Message> = container(cell.into_element())
        .height(Length::Fixed(control_size))
        .width(inner_width)
        .into();
    let bounded = match description {
        Some(description) => crate::tooltip::enz_tooltip_with_delay(
            bounded,
            description,
            iced::widget::tooltip::Position::Bottom,
            delay,
        ),
        None => bounded,
    };
    // Outer container: fills the vertical slot and centres the bounded pill in
    // it. In fill mode its width is a FIXED pin (not `Fill`): a `Fill` outer
    // would let a row split N tabs evenly and clamp each pinned floor,
    // and a `Shrink` outer would starve the inner fill spacer.
    container(bounded)
        .height(Length::Fill)
        .width(outer_width)
        .align_y(Vertical::Center)
        .into()
}

/// Default ceiling for a variable-width titlebar document tab.
pub const DEFAULT_ROW_TAB_MAX_WIDTH: f32 = 200.0;
/// Hard share-safe width floor. A planned width never exceeds its share.
pub const DEFAULT_ROW_TAB_HARD_MIN_WIDTH: f32 = 28.0;
/// Width below which inactive tabs hide their close button.
pub const DEFAULT_ROW_CLOSE_HIDE_BELOW: f32 = 80.0;
/// Minimum width retained by the active, closable tab.
pub const DEFAULT_ROW_ACTIVE_MIN_WIDTH: f32 = 80.0;
/// Width reserved for the tab-list trigger when documents fold.
pub const DEFAULT_ROW_LIST_RESERVE: f32 = 34.0;
/// Default width reserved for an optional trailing affordance such as “new tab”.
pub const DEFAULT_ROW_TRAILING_RESERVE: f32 = 34.0;
/// Default filename sample used when deciding whether a drawn tab is identifiable.
pub const DEFAULT_ROW_LEGIBILITY_SAMPLE: &str = "nn…";
/// Default gap in pixels drawn between neighbouring titlebar document tabs.
///
/// A titlebar row of pinned-width browser tabs painted edge to edge at
/// `spacing(0.0)` gave no visual break between adjacent pills. A minimal
/// 2 px gutter separates each tab from the next without breaking the
/// share-safe legibility floor: the planner reserves `(count - 1) *
/// DEFAULT_ROW_TAB_GAP` from the tab area before widths are chosen, so a
/// row that fits `n` tabs at gap = 0 still fits `n` tabs at this gap
/// with only a proportional per-tab shrink.
pub const DEFAULT_ROW_TAB_GAP: f32 = 2.0;
/// Default width of the complete-document overflow menu.
pub const DEFAULT_ROW_MENU_WIDTH: f32 = 240.0;
/// Accessible-name prefix of the overflow trigger.
pub const ROW_LIST_LABEL: &str = "List all documents";

type TrailingFn<'a, Message> = Box<dyn Fn() -> Element<'a, Message> + 'a>;

/// One document supplied to [`enz_titlebar_tab_row`].
///
/// The row owns tab sizing, close visibility, folding and overflow reachability;
/// the caller supplies only product data and messages.
#[derive(Debug, Clone)]
pub struct TitlebarTabItem<Message> {
    key: String,
    title: String,
    icon: String,
    modified: bool,
    activate: Message,
    close: Option<Message>,
    description: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    workspace_accent: Option<iced::Color>,
    tab_background: Option<iced::Color>,
}

impl<Message> TitlebarTabItem<Message> {
    /// Create an item with a stable key, visible title and activation message.
    pub fn new(key: impl Into<String>, title: impl Into<String>, activate: Message) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            icon: String::new(),
            modified: false,
            activate,
            close: None,
            description: None,
            tooltip_delay: None,
            workspace_accent: None,
            tab_background: None,
        }
    }

    /// Set the favicon/semantic icon name.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Set the close message. Without one, this item draws no close affordance.
    #[must_use]
    pub fn on_close(mut self, message: Message) -> Self {
        self.close = Some(message);
        self
    }

    /// Mark the document as having unsaved changes: the tab shows a filled
    /// dot in its title's ink in place of the icon. See
    /// [`EnzDocumentTab::modified`](crate::document_tab::EnzDocumentTab::modified).
    #[must_use]
    pub fn modified(mut self, modified: bool) -> Self {
        self.modified = modified;
        self
    }

    /// Override the app's hover delay; zero starts revealing immediately.
    #[must_use]
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Set the hover description, commonly the document's full path.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the inactive resting fill; defaults to the document tab's transparent fill.
    ///
    /// Resolve this color from the current palette when rebuilding the row.
    /// Selection keeps its own fill, hover composites over this fill, and an
    /// optional workspace accent washes the resulting state independently.
    #[must_use]
    pub fn tab_background(mut self, background: iced::Color) -> Self {
        self.tab_background = Some(background);
        self
    }

    /// Wash resolved state fills with an optional workspace accent.
    ///
    /// Resolve the persisted accent key against the current theme before building
    /// the row. `None` preserves the document tab's untinted colors.
    #[must_use]
    pub fn workspace_accent(mut self, accent: Option<iced::Color>) -> Self {
        self.workspace_accent = accent;
        self
    }
}

/// A variable-width titlebar tab-row layout decision.
#[derive(Debug, Clone, PartialEq)]
pub struct TitlebarTabPlan {
    /// First item in the contiguous drawn window.
    pub start: usize,
    /// Width of each drawn tab, in window order.
    pub widths: Vec<f32>,
    /// Width shared by the drawn run after any overflow reserve.
    pub strip_area: f32,
    /// Total item count, including folded documents.
    pub item_count: usize,
}

impl TitlebarTabPlan {
    /// Number of tabs drawn inline.
    #[must_use]
    pub fn shown(&self) -> usize {
        self.widths.len()
    }

    /// Number of documents reachable only through the complete-document menu.
    #[must_use]
    pub fn hidden(&self) -> usize {
        self.item_count.saturating_sub(self.shown())
    }

    /// Whether the complete-document trigger is present.
    #[must_use]
    pub fn is_folded(&self) -> bool {
        self.hidden() > 0
    }
}

/// Responsive variable-width document tab strip for a titlebar.
///
/// The widget keeps the active tab visible, compensates it for close chrome,
/// folds only when the measured filename sample no longer fits, and puts every
/// document in a named overflow menu. The menu is controlled: callers feed
/// [`Self::list_open`] back from the toggle message.
pub struct TitlebarTabRow<'a, Message> {
    items: Vec<TitlebarTabItem<Message>>,
    active_index: usize,
    on_toggle_list: Message,
    on_dismiss: Option<Message>,
    list_open: bool,
    legibility_sample: String,
    id_prefix: String,
    tooltip_delay: Option<std::time::Duration>,
    trigger_id: Option<iced::advanced::widget::Id>,
    trailing: Option<TrailingFn<'a, Message>>,
}

/// Build a responsive variable-width document tab strip.
///
/// `active_index` is clamped to the available items. `on_toggle_list` controls
/// the overflow menu; feed the resulting state back through
/// [`TitlebarTabRow::list_open`].
pub fn enz_titlebar_tab_row<'a, Message: Clone + 'a>(
    items: Vec<TitlebarTabItem<Message>>,
    active_index: usize,
    on_toggle_list: Message,
) -> TitlebarTabRow<'a, Message> {
    TitlebarTabRow {
        items,
        active_index,
        on_toggle_list,
        on_dismiss: None,
        list_open: false,
        legibility_sample: DEFAULT_ROW_LEGIBILITY_SAMPLE.to_owned(),
        id_prefix: "enz-titlebar-tab-row".to_owned(),
        tooltip_delay: None,
        trigger_id: None,
        trailing: None,
    }
}

impl<'a, Message: Clone + 'a> TitlebarTabRow<'a, Message> {
    /// Override the app's hover delay; zero starts revealing immediately.
    #[must_use]
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Whether the complete-document menu is open. Caller-owned.
    #[must_use]
    pub fn list_open(mut self, open: bool) -> Self {
        self.list_open = open;
        self
    }

    /// Set an idempotent dismissal message. Unset, dismissal uses the toggle.
    #[must_use]
    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }

    /// Override the product-specific filename sample used for legibility.
    ///
    /// The default is [`DEFAULT_ROW_LEGIBILITY_SAMPLE`] (`"nn…"`). A sample is
    /// a product fact, so callers with different identifiers can be explicit;
    /// an empty sample falls back to the default.
    #[must_use]
    pub fn fold_on_legibility_sample(mut self, sample: impl Into<String>) -> Self {
        let sample = sample.into();
        if !sample.is_empty() {
            self.legibility_sample = sample;
        }
        self
    }

    /// Namespace used for tab and menu-row accessibility ids.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Give the complete-document trigger a stable widget id.
    #[must_use]
    pub fn trigger_id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.trigger_id = Some(id.into());
        self
    }

    /// Add a trailing affordance (normally “new tab”).
    ///
    /// The row reserves [`DEFAULT_ROW_TRAILING_RESERVE`] before planning tabs,
    /// matching the toolbar-icon slot used by app titlebars. At an extreme
    /// width the affordance folds as a whole; it is never flex-compressed below
    /// the canonical control and glyph geometry. A needed document-list trigger
    /// has priority over this optional trailing action.
    #[must_use]
    pub fn trailing(mut self, build: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.trailing = Some(Box::new(build));
        self
    }

    /// Measured minimum tab width for this row's legibility sample.
    #[must_use]
    pub fn legibility_min_width(&self) -> f32 {
        use crate::enz_text::TextSize;
        use crate::text_measure::{MeasureTypography, enz_text_measure_width};
        let typo =
            MeasureTypography::new(TextSize::Body).pin_size_px(document_tab::DEFAULT_TITLE_SIZE);
        // The planner adds the close action where one shows; the leading slot
        // (an icon or the modified dot) is reserved whenever any tab has one.
        let leading = if self
            .items
            .iter()
            .any(|item| !item.icon.is_empty() || item.modified)
        {
            document_tab::DEFAULT_FAVICON_SIZE + document_tab::DEFAULT_ROW_SPACING
        } else {
            0.0
        };
        enz_text_measure_width(&self.legibility_sample, typo)
            + document_tab::DEFAULT_CONTENT_PADDING_X
            + document_tab::DEFAULT_CONTENT_PADDING_TRAILING
            + leading
    }

    fn owned_policy(&self) -> iced::advanced::layout::tab_strip::Specification {
        iced::advanced::layout::tab_strip::Specification {
            closable: self.items.iter().map(|item| item.close.is_some()).collect(),
            active: self.active_index,
            legibility: self.legibility_min_width(),
            maximum: DEFAULT_ROW_TAB_MAX_WIDTH,
            hard_minimum: DEFAULT_ROW_TAB_HARD_MIN_WIDTH,
            close_threshold: DEFAULT_ROW_CLOSE_HIDE_BELOW,
            active_minimum: DEFAULT_ROW_ACTIVE_MIN_WIDTH,
            close_chrome: document_tab::default_close_chrome_width(),
            gap: DEFAULT_ROW_TAB_GAP,
            list_width: DEFAULT_ROW_LIST_RESERVE,
            trailing_width: self.trailing.as_ref().map(|_| DEFAULT_ROW_TRAILING_RESERVE),
        }
    }

    /// Plan the tab run in `available` pixels.
    ///
    /// `available` includes an optional trailing slot; the planner removes that
    /// reserve first, then removes the list-trigger reserve only when folding.
    #[must_use]
    pub fn plan(&self, available: f32) -> TitlebarTabPlan {
        // The trailing slot is placed one gap after the tab run, as the
        // layout-owned strip plans it.
        let trailing = if self.trailing.is_some() {
            DEFAULT_ROW_TRAILING_RESERVE + DEFAULT_ROW_TAB_GAP
        } else {
            0.0
        };
        self.plan_tab_area((available.max(0.0) - trailing).max(0.0))
    }

    fn plan_tab_area(&self, available: f32) -> TitlebarTabPlan {
        let plan = self.owned_policy().tab_area(available);
        TitlebarTabPlan {
            start: plan.start,
            widths: plan.widths,
            strip_area: plan.strip_area,
            item_count: self.items.len(),
        }
    }

    fn list_trigger(&self, hidden: usize) -> Element<'a, Message> {
        let label = enzui_core::localized!("enzui-titlebar-tab-not-shown", "hidden" => hidden);
        let return_token = self.trigger_id.clone().unwrap_or_else(|| {
            enzui_core::identity::derive_id(&self.id_prefix, "document-list-trigger")
        });
        let mut trigger = enz_toolbar_icon("chevron-down", self.on_toggle_list.clone())
            .aria_label(label.clone())
            .tooltip(label)
            .tooltip_delay(
                self.tooltip_delay
                    .unwrap_or_else(crate::tooltip::default_tooltip_delay),
            )
            .expanded(self.list_open)
            .relinquish_focus_while(self.list_open)
            .reclaim_focus_on_return((!self.list_open).then(|| return_token.clone()));
        if let Some(id) = self.trigger_id.clone() {
            trigger = trigger.id(id);
        }
        let popup: Option<Element<'a, Message>> = if self.list_open {
            let active = self
                .items
                .get(self.active_index.min(self.items.len().saturating_sub(1)))
                .map(|item| EntryId::from(item.key.clone()));
            let mut menu = enz_menu()
                .width(DEFAULT_ROW_MENU_WIDTH)
                .label(enzui_core::localization::text(
                    "enzui-titlebar-tab-open-documents",
                ))
                .id_prefix(format!("{}-menu", self.id_prefix))
                .active_maybe(active)
                .roving_self(true)
                .focus_return_to(return_token);
            for item in &self.items {
                menu = menu.push_item(EnzMenuItem::command(
                    EntryId::from(item.key.clone()),
                    item.title.clone(),
                    item.activate.clone(),
                ));
            }
            Some(menu.into())
        } else {
            None
        };
        let dismiss = self
            .on_dismiss
            .clone()
            .unwrap_or_else(|| self.on_toggle_list.clone());
        container(dismissible_anchored_menu(
            trigger,
            popup,
            Placement::new(Side::Bottom)
                .gap(4.0)
                .alignment(PlacementAlignment::Start),
            move || dismiss.clone(),
        ))
        .width(Length::Fixed(DEFAULT_ROW_LIST_RESERVE))
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(Vertical::Center)
        .into()
    }

    fn row_at(&self, available: f32) -> Element<'a, Message> {
        // Optional titlebar controls fold as whole canonical affordances. The
        // document list has priority over New because it is the only route to
        // already-open documents. Never hand two fixed controls less than the
        // two slots they require: flex compression would preserve their nodes
        // while shrinking their glyphs, which is worse than an explicit fold.
        let wants_trailing = self.trailing.is_some();
        let mut show_trailing = wants_trailing && available >= DEFAULT_ROW_TRAILING_RESERVE;
        let mut plan = if show_trailing {
            self.plan(available)
        } else {
            self.plan_tab_area(available.max(0.0))
        };
        if plan.is_folded()
            && show_trailing
            && available < DEFAULT_ROW_LIST_RESERVE + DEFAULT_ROW_TRAILING_RESERVE
        {
            show_trailing = false;
            plan = self.plan_tab_area(available.max(0.0));
        }
        let show_list = plan.is_folded() && available >= DEFAULT_ROW_LIST_RESERVE;
        if !show_list && plan.is_folded() {
            // At a width too small for even one canonical control, draw no
            // compressed stub. The titlebar's higher-priority window controls
            // remain usable and this optional row disappears honestly.
            plan.widths.clear();
        }
        let active = if self.items.is_empty() {
            0
        } else {
            self.active_index.min(self.items.len() - 1)
        };
        let mut children = Vec::with_capacity(plan.shown() + 2);
        for (offset, width) in plan.widths.iter().copied().enumerate() {
            let index = plan.start + offset;
            let item = &self.items[index];
            let selected = index == active;
            let show_close =
                item.close.is_some() && (selected || width >= DEFAULT_ROW_CLOSE_HIDE_BELOW);
            let mut cell = enz_document_tab(item.title.clone(), item.activate.clone())
                .selected(selected)
                .focused(selected)
                .embedded(true)
                .set_position(index + 1, self.items.len())
                .close_visible(show_close)
                .tab_min_width(width)
                .tab_max_width(width)
                .fill_width(true)
                .id_prefix(format!("{}-{}", self.id_prefix, item.key))
                .workspace_accent(item.workspace_accent);
            if let Some(background) = item.tab_background {
                cell = cell.tab_background(background);
            }
            if let Some(close) = item.close.clone() {
                cell = cell.on_close(close);
            }
            if !item.icon.is_empty() {
                cell = cell.icon(item.icon.clone());
            }
            cell = cell.modified(item.modified);
            let tab = described_titlebar_document_tab_fill(
                cell,
                item.description.as_ref(),
                item.tooltip_delay
                    .or(self.tooltip_delay)
                    .unwrap_or_else(crate::tooltip::default_tooltip_delay),
            );
            children.push(tab);
        }
        if show_list {
            children.push(self.list_trigger(plan.hidden()));
        }
        if show_trailing && let Some(trailing) = &self.trailing {
            children.push(
                container(trailing())
                    .width(Length::Fixed(DEFAULT_ROW_TRAILING_RESERVE))
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(Vertical::Center)
                    .into(),
            );
        }
        row(children)
            .spacing(DEFAULT_ROW_TAB_GAP)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Consume the builder and produce the responsive widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        if self.owned_policy().valid() {
            Element::new(owned::row(self))
        } else {
            responsive(move |size| self.row_at(size.width)).into()
        }
    }
}

impl<'a, Message: Clone + 'a> From<TitlebarTabRow<'a, Message>> for Element<'a, Message> {
    fn from(row: TitlebarTabRow<'a, Message>) -> Self {
        row.into_element()
    }
}

mod owned;
