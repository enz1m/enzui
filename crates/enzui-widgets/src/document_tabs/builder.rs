//! Document-tab builders and application callbacks.

use super::*;

pub(crate) type TabBuilder<'a, Message> =
    Box<dyn Fn(&DocumentItem, TabContext) -> Element<'a, Message> + 'a>;

pub(crate) type OnScroll<'a, Message> = Box<dyn Fn(Viewport) -> Message + 'a>;

pub(crate) type OnEvent<'a, Message> = Box<dyn Fn(TabEvent) -> Message + 'a>;

pub(crate) struct TabContextMenu<'a, Message> {
    pub(crate) model: &'a enzui_core::menu_model::MenuModel,
    pub(crate) state: &'a crate::context_menu_host::ContextMenuState,
    pub(crate) on_event: Rc<dyn Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a>,
}

/// Builds a keyed strip with every pinned tab and a capped visible session window.
///
/// Construction measures each tab through `width_of`; geometry-setting builder
/// methods can repeat that pass. `tab_builder` supplies the elements for the
/// realized tabs. Construct with [`enz_document_tabs`].
pub struct EnzDocumentTabs<'a, Message> {
    pub(crate) model: &'a DocumentTabs,
    pub(crate) metrics: StripMetrics,
    pub(crate) tab_builder: TabBuilder<'a, Message>,
    pub(crate) extent: f32,
    pub(crate) scroll_offset: f32,
    pub(crate) on_scroll: Option<OnScroll<'a, Message>>,
    pub(crate) hide_scrollbar: bool,
    pub(crate) on_event: Option<OnEvent<'a, Message>>,
    pub(crate) rtl: bool,
    pub(crate) context_menu: Option<TabContextMenu<'a, Message>>,
    pub(crate) overscan: usize,
    pub(crate) width: Length,
    pub(crate) height: Length,
    pub(crate) label: String,
    pub(crate) capped: Rc<Cell<bool>>,
    pub(crate) width_of: Rc<dyn Fn(&DocumentItem) -> f32 + 'a>,
    pub(crate) sizing: SizingPolicy,
    pub(crate) density: TabDensity,
    pub(crate) activation: ActivationMode,
    pub(crate) new_tab: Option<NewTab<'a, Message>>,
    pub(crate) pinned_columns: usize,
    pub(crate) pinned_column_width: Option<f32>,
}

pub(crate) struct NewTab<'a, Message> {
    pub(crate) element: Element<'a, Message>,
    // Shares the axis used by measurement and viewport filtering.
    pub(crate) extent: f32,
}

/// Construct a document-tab strip.
///
/// `width_of` supplies each tab’s extent along the strip axis, in pixels.
/// `label` names the tab list and identifies its scrollable; use a distinct label
/// for each strip. The cell builder supplies individual tab accessibility IDs;
/// context menus expect IDs derived from the document keys.
pub fn enz_document_tabs<'a, Message, W, B>(
    model: &'a DocumentTabs,
    label: impl Into<String>,
    width_of: W,
    tab_builder: B,
) -> EnzDocumentTabs<'a, Message>
where
    Message: 'a,
    W: Fn(&DocumentItem) -> f32 + 'a,
    B: Fn(&DocumentItem, TabContext) -> Element<'a, Message> + 'a,
{
    let width_of: Rc<dyn Fn(&DocumentItem) -> f32 + 'a> = Rc::new(width_of);
    let metrics = StripMetrics::new(model, width_of.as_ref());
    EnzDocumentTabs {
        model,
        metrics,
        width_of,
        sizing: SizingPolicy::Scroll,
        density: TabDensity::Full,
        activation: ActivationMode::Manual,
        new_tab: None,
        tab_builder: Box::new(tab_builder),
        extent: 1_200.0,
        scroll_offset: 0.0,
        on_scroll: None,
        hide_scrollbar: false,
        on_event: None,
        rtl: false,
        context_menu: None,
        overscan: 0,
        width: Length::Fill,
        height: Length::Shrink,
        label: label.into(),
        capped: Rc::new(Cell::new(false)),
        pinned_columns: 1,
        pinned_column_width: None,
    }
}

impl<'a, Message> EnzDocumentTabs<'a, Message> {
    /// The strip's pixel extent along its layout axis (width when horizontal,
    /// height when vertical). Drives the viewport filter.
    #[must_use]
    pub fn extent(mut self, extent: f32) -> Self {
        self.extent = extent;
        self.remeasure();
        self
    }

    /// Choose how session tabs are sized against the viewport.
    ///
    /// Defaults to [`SizingPolicy::Scroll`]. This and geometry-setting builders
    /// remeasure the strip, so the policy can be set before or after the extent.
    #[must_use]
    pub fn sizing(mut self, policy: SizingPolicy) -> Self {
        self.sizing = policy;
        self.remeasure();
        self
    }

    /// Choose the rendering density passed to the tab builder.
    ///
    /// Defaults to [`TabDensity::Full`]. A valid compact rail also limits every tab’s
    /// along-axis extent before layout and hit-testing; see [`TabDensity`].
    #[must_use]
    pub fn density(mut self, density: TabDensity) -> Self {
        self.density = density;
        self.remeasure();
        self
    }

    /// Pack pinned tabs row-major into a grid in a vertical strip.
    ///
    /// Defaults to one column; zero is normalized to one. Horizontal strips ignore
    /// this setting. Fewer pinned rows leave more height for the session viewport.
    #[must_use]
    pub fn pinned_columns(mut self, columns: usize) -> Self {
        self.pinned_columns = columns.max(1);
        self.remeasure();
        self
    }

    /// Set the pinned grid’s column width in pixels.
    ///
    /// Applies only to vertical strips with at least two pinned columns. By default,
    /// column widths reuse `width_of`, which also supplies row heights; set this
    /// separately when the tab content needs wider columns.
    ///
    /// Zero creates a zero-width column. Negative or non-finite values restore the
    /// default. The compact density rail does not clamp this explicit width.
    #[must_use]
    pub fn pinned_column_width(mut self, width: f32) -> Self {
        self.pinned_column_width = if width.is_finite() && width >= 0.0 {
            Some(width)
        } else {
            None
        };
        self.remeasure();
        self
    }

    /// Restore pinned column widths derived from `width_of`.
    ///
    /// This clears an explicit width, including a zero-width column.
    #[must_use]
    pub fn clear_pinned_column_width(mut self) -> Self {
        self.pinned_column_width = None;
        self.remeasure();
        self
    }

    /// Choose whether keyboard navigation activates its destination.
    ///
    /// Defaults to [`ActivationMode::Manual`]; see [`ActivationMode`].
    #[must_use]
    pub fn activation(mut self, activation: ActivationMode) -> Self {
        self.activation = activation;
        self
    }

    /// Add a fixed new-tab affordance after the session viewport.
    ///
    /// `reserve` subtracts pixels from the strip’s along-axis extent: width for a
    /// horizontal strip, height for a vertical strip. Negative or non-finite values
    /// reserve zero. The element sits outside the scrollable and supplies its own
    /// activation message, such as [`TabEvent::NewRequested`].
    #[must_use]
    pub fn new_tab(mut self, element: impl Into<Element<'a, Message>>, reserve: f32) -> Self {
        self.new_tab = Some(NewTab {
            element: element.into(),
            extent: if reserve.is_finite() && reserve > 0.0 {
                reserve
            } else {
                0.0
            },
        });
        self.remeasure();
        self
    }

    pub(crate) fn effective_extent(&self) -> f32 {
        let reserved = self.new_tab.as_ref().map_or(0.0, |n| n.extent);
        (self.extent - reserved).max(0.0)
    }

    fn remeasure(&mut self) {
        let raw = Rc::clone(&self.width_of);
        let available = f64::from(self.metrics.available_extent(self.effective_extent()));
        let mut natural = 0.0_f64;
        let mut count = 0_usize;
        for item in self.model.items() {
            if !item.pinned {
                natural += f64::from(raw(item)).max(0.0);
                count += 1;
            }
        }

        // Density can clamp pinned tabs too; sizing applies only to session tabs.
        let rail = self.density.rail_width().map(f64::from);
        let session_each = self
            .sizing
            .resolve(natural, count, available)
            .map(f64::from);

        let columns = match self.model.orientation() {
            Orientation::Vertical => self.pinned_columns.max(1),
            Orientation::Horizontal => 1,
        };

        let grid_active = matches!(self.model.orientation(), Orientation::Vertical) && columns >= 2;
        let cross_override = if grid_active {
            self.pinned_column_width
        } else {
            None
        };

        if rail.is_none() && session_each.is_none() && cross_override.is_none() {
            self.metrics = StripMetrics::new_gridded(self.model, raw.as_ref(), columns);
            return;
        }

        let raw_along = Rc::clone(&raw);
        let along = move |item: &DocumentItem| -> f32 {
            let base = if item.pinned {
                f64::from(raw_along(item))
            } else {
                session_each.unwrap_or_else(|| f64::from(raw_along(item)))
            };
            let clamped = match rail {
                Some(r) => base.min(r),
                None => base,
            };
            clamped as f32
        };

        if let Some(cross_w) = cross_override {
            // Explicit grid widths bypass the density rail.
            self.metrics = StripMetrics::new_gridded_with_cross(
                self.model,
                along,
                move |_item: &DocumentItem| cross_w,
                columns,
            );
            return;
        }

        self.metrics = StripMetrics::new_gridded(self.model, along, columns);
    }

    /// The session viewport's scroll offset, in px from its leading edge.
    #[must_use]
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Receive session scrolling updates for the viewport filter.
    ///
    /// Store `viewport.absolute_offset().x` for a horizontal strip, or `.y` for a
    /// vertical strip, and feed it back through [`scroll_offset`](Self::scroll_offset)
    /// on the next frame. Without this feedback, the realized window stays at the
    /// old offset while the scrollable moves.
    #[must_use]
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Scroll the session strip without showing a scrollbar (default
    /// `false`), for a chromeless strip such as a titlebar's that scrolls by
    /// wheel and shows edge fades. Scrolling, its state, reveal and the
    /// realized window are unchanged.
    #[must_use]
    pub fn hide_scrollbar(mut self, hide: bool) -> Self {
        self.hide_scrollbar = hide;
        self
    }

    /// Receive the strip’s keyed keyboard and tab events.
    ///
    /// The application applies these requests to its [`DocumentTabs`] model.
    /// Keyboard actions require this handler even when cells emit their own messages.
    #[must_use]
    pub fn on_event(mut self, f: impl Fn(TabEvent) -> Message + 'a) -> Self {
        self.on_event = Some(Box::new(f));
        self
    }

    /// Mirror horizontal arrow-key navigation for right-to-left layout.
    ///
    /// Defaults to false. Vertical navigation still runs top to bottom.
    #[must_use]
    pub fn rtl(mut self, rtl: bool) -> Self {
        self.rtl = rtl;
        self
    }

    /// Host a context menu on each realized tab.
    ///
    /// Supports pointer, Menu key, Shift+F10 and routed accessibility invocation.
    /// Each host uses its tab’s laid-out bounds and focus. The cell must publish the
    /// stable accessibility ID `{document_key}-tab` for routing and focus restoration.
    ///
    /// The application owns `model` and `state` and applies callback events with
    /// [`apply_event`](crate::context_menu_host::apply_event).
    #[must_use]
    pub fn context_menu(
        mut self,
        model: &'a enzui_core::menu_model::MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> Self {
        self.context_menu = Some(TabContextMenu {
            model,
            state,
            on_event: Rc::new(on_event),
        });
        self
    }

    /// Extra session tabs realised on each side of the exact visible window.
    /// `0` is honoured literally.
    #[must_use]
    pub fn overscan(mut self, n: usize) -> Self {
        self.overscan = n;
        self
    }

    /// Container width.
    #[must_use]
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Container height.
    #[must_use]
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Returns a shared cap flag updated when [`into_element`](Self::into_element) runs.
    ///
    /// Read the handle after building the element. A true flag means the session
    /// range, including overscan, was truncated to [`MAX_REALIZED_SESSION`].
    #[must_use]
    pub fn realized_was_capped(&self) -> Rc<Cell<bool>> {
        Rc::clone(&self.capped)
    }
}
