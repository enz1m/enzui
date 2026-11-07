//! Column-header labels with optional keyboard and pointer sort activation.
//!
//! Titles use single-line raw text, a scaled 12px size and the ambient Semibold
//! UI font. Active sorting adds a real chevron; nonstandard sorting uses a
//! bidirectional arrow. Title and icon ink follow the live palette unless pinned.
//! Defaults are left alignment, 32px height, 12px side padding and end ellipsis.
//!
//! Every cell publishes a ColumnHeader. Sortable cells also publish sort state;
//! an activation callback enables the default action. Sort direction is included
//! in the accessible description; genuinely sorted states also set the AccessKit
//! sort property. Unsorted cells clear that property.
//! Sort controls derive identity from namespace and title; same-named columns
//! need distinct namespaces when used together.

use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use enzui_core::semantics::{Role, Semantics, SortDirection};
use iced::widget::button::Style as ButtonStyle;
use iced::widget::text::{Ellipsis, Wrapping};
use iced::widget::{container, row, text};
use iced::{Border, Color, Element, Length, alignment::Vertical};

pub use crate::table_text_cell::EnzTableTextAlign as EnzTableHeaderCellAlign;
fn sort_glyph(sort_state: Option<SortDirection>) -> Option<&'static str> {
    match sort_state {
        Some(SortDirection::Ascending) => Some("chevron-up"),
        Some(SortDirection::Descending) => Some("chevron-down"),
        Some(SortDirection::Other) => Some("arrow-down-up"),
        Some(SortDirection::Unsorted) | None => None,
    }
}

/// Column-header builder with optional sort state and activation.
#[must_use]
pub struct EnzTableHeaderCell<'a, Message: Clone> {
    title: String,
    align: EnzTableHeaderCellAlign,
    sortable: bool,
    sort_active: bool,
    sort_ascending: bool,
    sort_nonstandard: bool,
    header_height: f32,
    cell_padding_x: f32,
    text_color: Option<Color>,
    accent_color: Option<Color>,
    ellipsis: Ellipsis,
    on_sort: Option<Box<dyn Fn() -> Message + 'a>>,
    id_prefix: String,
    id: Option<iced::advanced::widget::Id>,
    _phantom: std::marker::PhantomData<&'a ()>,
}

/// Default sort-control identity namespace: enz-table-header.
pub const DEFAULT_ID_PREFIX: &str = "enz-table-header";

/// Own a title with left alignment, 32px height, 12px side padding and live ink.
/// Sorting is disabled by default; titles use end ellipsis without wrapping.
pub fn enz_table_header_cell<Message: Clone>(
    title: impl Into<String>,
) -> EnzTableHeaderCell<'static, Message> {
    EnzTableHeaderCell {
        title: title.into(),
        align: EnzTableHeaderCellAlign::Left,
        sortable: false,
        sort_active: false,
        sort_ascending: false,
        sort_nonstandard: false,
        header_height: 32.0,
        cell_padding_x: 12.0,
        text_color: None,
        accent_color: None,
        ellipsis: Ellipsis::End,
        on_sort: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        id: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzTableHeaderCell<'a, Message> {
    fn sort_state(&self) -> Option<SortDirection> {
        if !self.sortable {
            None
        } else if self.sort_nonstandard {
            Some(SortDirection::Other)
        } else if !self.sort_active {
            Some(SortDirection::Unsorted)
        } else if self.sort_ascending {
            Some(SortDirection::Ascending)
        } else {
            Some(SortDirection::Descending)
        }
    }

    /// Set title alignment; defaults to Left.
    pub fn align(mut self, a: EnzTableHeaderCellAlign) -> Self {
        self.align = a;
        self
    }

    /// Enable sort semantics and the focus-button wrapper; defaults to false.
    /// Activation also requires a callback. Unsorted cells show no arrow.
    pub fn sortable(mut self, v: bool) -> Self {
        self.sortable = v;
        self
    }

    /// Mark this as the active sort column; defaults to false.
    /// Nonstandard sorting takes precedence over this flag.
    pub fn sort_active(mut self, v: bool) -> Self {
        self.sort_active = v;
        self
    }

    /// Select ascending instead of descending for an active standard sort.
    /// Defaults to false; has no effect on inactive or nonstandard sorts.
    pub fn sort_ascending(mut self, v: bool) -> Self {
        self.sort_ascending = v;
        self
    }

    /// Publish Other and show a bidirectional arrow when sortable.
    /// Defaults to false; true takes precedence over the active and direction flags.
    pub fn sort_nonstandard(mut self, v: bool) -> Self {
        self.sort_nonstandard = v;
        self
    }

    /// Set cell height in logical pixels; non-finite or nonpositive values reset it to 32.
    pub fn header_height(mut self, h: f32) -> Self {
        self.header_height = enzui_core::metrics::FinitePositive::new_or(h, 32.0).get();
        self
    }

    /// Set side padding in logical pixels; negative or non-finite values reset it to 12.
    pub fn cell_padding_x(mut self, p: f32) -> Self {
        self.cell_padding_x = enzui_core::metrics::FiniteNonNeg::new_or(p, 12.0).get();
        self
    }

    /// Pin title ink instead of resolving live palette text ink.
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }

    /// Pin arrow ink instead of resolving live palette accent ink.
    pub fn accent_color(mut self, c: Color) -> Self {
        self.accent_color = Some(c);
        self
    }

    /// Select the overflow marker; defaults to End. None disables the marker.
    /// Titles remain single-line in every mode.
    pub fn ellipsis(mut self, e: Ellipsis) -> Self {
        self.ellipsis = e;
        self
    }
    fn run_config(&self) -> (Wrapping, Ellipsis) {
        (Wrapping::None, self.ellipsis)
    }

    /// Return the configured overflow marker; defaults to End.
    pub fn ellipsis_mode(&self) -> Ellipsis {
        self.run_config().1
    }

    /// Return None: header titles do not wrap.
    pub fn wrapping_mode(&self) -> Wrapping {
        self.run_config().0
    }

    /// Emit a cloned message on sortable-cell activation. No callback is installed by default.
    pub fn on_sort(mut self, msg: Message) -> Self {
        self.on_sort = Some(Box::new(move || msg.clone()));
        self
    }

    /// Run the factory on sortable-cell activation.
    pub fn on_sort_with(mut self, factory: impl Fn() -> Message + 'a) -> Self {
        self.on_sort = Some(Box::new(factory));
        self
    }

    /// Set the sort-control identity namespace.
    /// Use distinct prefixes for same-named columns that appear together.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Sets a sort-control ID that remains independent of translated column text.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Compose the label and optional sort control with ColumnHeader semantics.
    pub fn into_element(self) -> Element<'a, Message> {
        let pinned_text = self.text_color;
        let pinned_accent = self.accent_color;
        let sortable = self.sortable;
        let aria_title = self.title.clone();
        let (wrapping, ellipsis) = self.run_config();
        let sort_state = self.sort_state();

        let title_h = self.align.to_iced();
        // Raw iced text bypasses the framework text fallback setup.
        crate::sans_serif_fallback::ensure_sans_serif_fallback();
        let label = text(self.title)
            .size(enzui_theme::text_scale::ui_px(12.0))
            .font(enzui_theme::text_defaults::font(
                enzui_theme::FontRole::Ui,
                enzui_theme::WeightRung::Semibold,
            ))
            .wrapping(wrapping)
            .ellipsis(ellipsis)
            .style(move |theme: &iced::Theme| text::Style {
                color: Some(
                    pinned_text.unwrap_or_else(|| crate::theme_lookup::palette(theme).text),
                ),
            })
            .width(Length::Shrink)
            .align_x(title_h)
            .align_y(iced::alignment::Vertical::Center);
        // The sort chevron follows the label, as in Finder and GNOME Files; a
        // Fill label pushed it to the far edge of a wide column (Files' Name
        // column put it 1,050 px from the word it sorts). A filling empty text
        // carries the column's alignment.
        // An empty *text* fills the rest of the cell: column fitting reads a
        // header's natural width from the slack of a filling paragraph (see
        // `table::column_resize::text_slack`), which a plain `Space` lacks.
        let filler = || text("").width(Length::Fill);
        let mut inner = row![].spacing(6).align_y(iced::alignment::Vertical::Center);
        if !matches!(title_h, iced::alignment::Horizontal::Left) {
            inner = inner.push(filler());
        }
        inner = inner.push(label);
        let arrow_name = sort_glyph(sort_state);
        if let Some(arrow_name) = arrow_name {
            inner = inner.push(
                crate::icon::enz_icon(arrow_name)
                    .size(enzui_theme::text_scale::ui_px(11.0))
                    .tint_resolved(move |theme| {
                        pinned_accent.unwrap_or_else(|| crate::theme_lookup::palette(theme).accent)
                    }),
            );
        }

        if !matches!(title_h, iced::alignment::Horizontal::Right) {
            inner = inner.push(filler());
        }
        // Align vertically without center_y(Fill), which would replace the fixed height.
        let padded = container(inner)
            .width(Length::Fill)
            .height(Length::Fixed(self.header_height))
            .padding(
                iced::Padding::default()
                    .left(self.cell_padding_x)
                    .right(self.cell_padding_x),
            )
            .align_y(Vertical::Center);

        if sortable {
            let sort_id = self
                .id
                .unwrap_or_else(|| enzui_core::identity::derive_id(&self.id_prefix, &aria_title));
            focus_button_maybe_with(padded, self.on_sort)
                .id(sort_id)
                .width(Length::Fill)
                .padding(0)
                .semantics({
                    let mut sem = Semantics::new(Role::ColumnHeader).label(aria_title);
                    if let Some(dir) = sort_state {
                        sem = sem.sort(dir).selected(dir != SortDirection::Unsorted);
                    }
                    sem
                })
                .style_focus(move |t: &iced::Theme, status: FocusStatus| {
                    let p = crate::theme_lookup::palette(t);
                    let border = if matches!(status, FocusStatus::Focused) {
                        Border {
                            color: p.focus_ring,
                            width: 1.5,
                            radius: 4.0.into(),
                        }
                    } else {
                        Border::default()
                    };
                    ButtonStyle {
                        background: None,
                        border,
                        text_color: pinned_text.unwrap_or(p.text),
                        shadow: iced::Shadow::default(),
                        snap: false,
                    }
                })
                .into()
        } else {
            // A leaf suppresses the raw label node that would duplicate the header name.
            crate::semantics_container::SemanticsContainer::leaf(
                padded,
                Semantics::new(Role::ColumnHeader).label(aria_title),
            )
            .into()
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzTableHeaderCell<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTableHeaderCell<'a, Message>) -> Self {
        w.into_element()
    }
}
