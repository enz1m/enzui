//! Document-tab viewport rendering.

use super::*;

impl<'a, Message: Clone + 'a> EnzDocumentTabs<'a, Message> {
    /// Build the pinned cluster and capped visible session window.
    ///
    /// The strip publishes one [`Role::TabList`] node with its orientation. The
    /// `tab_builder` must supply each cell’s [`Role::Tab`] semantics and focus behavior,
    /// for example through [`EnzDocumentTab`](crate::document_tab::EnzDocumentTab).
    pub fn into_element(self) -> Element<'a, Message> {
        // Filtering and spacers share the button-reserved extent used by measurement.
        let extent = self.effective_extent();
        let EnzDocumentTabs {
            model,
            metrics,
            width_of: _,
            sizing: _,
            extent: _,
            density,
            activation,
            new_tab,
            tab_builder,
            scroll_offset,
            on_scroll,
            hide_scrollbar,
            on_event,
            rtl,
            context_menu,
            overscan,
            width,
            height,
            label,
            capped,
            pinned_columns: _,
            pinned_column_width: _,
        } = self;

        let orientation = model.orientation();
        let count = model.len();
        // The shared overlay scrollbar, or none for a chromeless strip.
        let scrollbar = || {
            if hide_scrollbar {
                scrollable::Scrollbar::hidden()
            } else {
                crate::scroll_view::overlay_scrollbar()
            }
        };

        // Per-tab hosts anchor to actual bounds; keyed IDs preserve routing across reorder.
        let wrap_in_host = |cell: Element<'a, Message>,
                            key: &DocumentKey|
         -> Element<'a, Message> {
            let Some(cm) = context_menu.as_ref() else {
                return cell;
            };
            let invoker = iced::advanced::widget::Id::from(format!("{}-tab", key.as_str()));
            let on_event = Rc::clone(&cm.on_event);
            crate::context_menu_host::enz_context_menu_host(cell, cm.model, cm.state, move |e| {
                (on_event)(e)
            })
            .invoker(invoker.clone())
            .at_id(crate::context_menu_host::derive_context_menu_id(&invoker))
            .label(enzui_core::localized!("enzui-document-tabs-actions", "label" => key.as_str()))
            .into_element()
        };

        let window = enzui_core::virtual_sequence::RealizationWindow::new(overscan);
        let visible = metrics.realize(scroll_offset, extent, window);

        let raw_start = visible.session.start;
        let raw_end = visible.session.end;
        let raw_len = raw_end.saturating_sub(raw_start);
        let capped_len = raw_len.min(MAX_REALIZED_SESSION);
        capped.set(capped_len < raw_len);
        let sess_start = raw_start;
        let sess_end = raw_start + capped_len;
        let session_keys = metrics.session_keys();

        let selected = model.selected().cloned();
        let focused = model.focused().cloned();

        let pinned_keys = metrics.pinned_keys();

        // Stable partition keys preserve row state on reorder; pinning rebuilds widget state.
        let mut pinned_children: Vec<(String, Element<'a, Message>)> =
            Vec::with_capacity(pinned_keys.len() + 1);
        for (pos, key) in pinned_keys.iter().enumerate() {
            if let Some(item) = model.item(key) {
                let ctx = TabContext {
                    selected: selected.as_ref() == Some(key),
                    focused: focused.as_ref() == Some(key),
                    pinned: true,
                    position: pos + 1,
                    count,
                    density,
                };
                let cell = wrap_in_host((tab_builder)(item, ctx), key);
                // Declared metrics must also constrain layout for hit-test agreement.
                let (along, _cross) = metrics.pinned_cell_box(pos);
                let sized: Element<'a, Message> = match orientation {
                    Orientation::Horizontal => container(cell).width(along).into(),
                    Orientation::Vertical => container(cell).height(along).into(),
                };
                pinned_children.push((tab_key(key, true), sized));
            }
        }

        let mut session_children: Vec<(String, Element<'a, Message>)> =
            Vec::with_capacity(capped_len);
        let session_base_pos = pinned_keys.len();
        for (offset, key) in session_keys[sess_start..sess_end].iter().enumerate() {
            let strip_index = sess_start + offset;
            let position = session_base_pos + strip_index + 1;
            if let Some(item) = model.item(key) {
                let ctx = TabContext {
                    selected: selected.as_ref() == Some(key),
                    focused: focused.as_ref() == Some(key),
                    pinned: false,
                    position,
                    count,
                    density,
                };
                let cell = (tab_builder)(item, ctx);
                // Declared cell extents keep hit-testing and spacer geometry aligned.
                let declared = metrics.session_width_of(strip_index);
                let cell = wrap_in_host(cell, key);
                let sized: Element<'a, Message> = match orientation {
                    Orientation::Horizontal => container(cell).width(declared).into(),
                    Orientation::Vertical => container(cell).height(declared).into(),
                };
                session_children.push((tab_key(key, false), sized));
            }
        }

        // Spacers preserve the full scroll range even when most cells are unrealized.
        let leading = metrics.session_offset_of(sess_start).max(0.0);
        let realized_extent =
            (metrics.session_offset_of(sess_end) - metrics.session_offset_of(sess_start)).max(0.0);
        let trailing = (metrics.session_extent() - leading - realized_extent).max(0.0);

        let strip: Element<'a, Message> = match orientation {
            Orientation::Horizontal => {
                let mut content: Vec<(String, Element<'a, Message>)> =
                    Vec::with_capacity(session_children.len() + 2);
                if leading > 0.0 {
                    content.push((
                        LEADING_SPACER_KEY.to_owned(),
                        Space::new().width(leading).height(Length::Shrink).into(),
                    ));
                }
                content.extend(session_children);
                if trailing > 0.0 {
                    content.push((
                        TRAILING_SPACER_KEY.to_owned(),
                        Space::new().width(trailing).height(Length::Shrink).into(),
                    ));
                }
                let session_row = keyed_row(content);
                let mut session_scroll = crate::scroll_view::enz_scrollable(session_row)
                    .id(session_scrollable_id(&label))
                    .direction(scrollable::Direction::Horizontal(scrollbar()))
                    .width(Length::Fill);
                if let Some(f) = on_scroll {
                    session_scroll = session_scroll.on_scroll(
                        move |scroll: iced::widget::scrollable::Scroll| f(scroll.viewport),
                    );
                }
                pinned_children.push((
                    SESSION_STRIP_KEY.to_owned(),
                    crate::scroll_view::contain_wheel(
                        session_scroll,
                        crate::scroll_view::Axis::Horizontal,
                    ),
                ));
                if let Some(nt) = new_tab {
                    pinned_children.push((
                        NEW_TAB_KEY.to_owned(),
                        container(nt.element).width(nt.extent).into(),
                    ));
                }
                container(keyed_row(pinned_children))
                    .width(width)
                    .height(height)
                    .into()
            }
            Orientation::Vertical => {
                let mut content: Vec<(String, Element<'a, Message>)> =
                    Vec::with_capacity(session_children.len() + 2);
                if leading > 0.0 {
                    content.push((
                        LEADING_SPACER_KEY.to_owned(),
                        Space::new().width(Length::Shrink).height(leading).into(),
                    ));
                }
                content.extend(session_children);
                if trailing > 0.0 {
                    content.push((
                        TRAILING_SPACER_KEY.to_owned(),
                        Space::new().width(Length::Shrink).height(trailing).into(),
                    ));
                }
                let session_col = keyed_column(content);
                let mut session_scroll = crate::scroll_view::enz_scrollable(session_col)
                    .id(session_scrollable_id(&label))
                    .direction(scrollable::Direction::Vertical(scrollbar()))
                    .height(Length::Fill);
                if let Some(f) = on_scroll {
                    session_scroll = session_scroll.on_scroll(
                        move |scroll: iced::widget::scrollable::Scroll| f(scroll.viewport),
                    );
                }

                let columns = metrics.pinned_columns();
                let pinned_cluster: Element<'a, Message> = if columns <= 1 {
                    keyed_column(std::mem::take(&mut pinned_children)).into()
                } else {
                    let mut rows: Vec<(String, Element<'a, Message>)> = Vec::new();
                    let mut row_cells: Vec<(String, Element<'a, Message>)> = Vec::new();
                    for (idx, (cell_key, cell)) in
                        std::mem::take(&mut pinned_children).into_iter().enumerate()
                    {
                        let (_along, cross) = metrics.pinned_cell_box(idx);
                        let sized = container(cell).width(cross);
                        row_cells.push((cell_key, sized.into()));
                        if row_cells.len() == columns {
                            let row_key = format!("\u{0}pinrow:{}", rows.len());
                            rows.push((row_key, keyed_row(std::mem::take(&mut row_cells)).into()));
                        }
                    }
                    if !row_cells.is_empty() {
                        let row_key = format!("\u{0}pinrow:{}", rows.len());
                        rows.push((row_key, keyed_row(row_cells).into()));
                    }
                    keyed_column(rows).into()
                };

                let mut outer: Vec<(String, Element<'a, Message>)> = Vec::with_capacity(3);
                outer.push((PINNED_CLUSTER_KEY.to_owned(), pinned_cluster));
                outer.push((
                    SESSION_STRIP_KEY.to_owned(),
                    crate::scroll_view::contain_wheel(
                        session_scroll,
                        crate::scroll_view::Axis::Vertical,
                    ),
                ));
                if let Some(nt) = new_tab {
                    outer.push((
                        NEW_TAB_KEY.to_owned(),
                        container(nt.element).height(nt.extent).into(),
                    ));
                }
                container(keyed_column(outer))
                    .width(width)
                    .height(height)
                    .into()
            }
        };

        let strip: Element<'a, Message> = match on_event {
            Some(sink) => {
                // The fallback anchor uses model geometry and has no cross-axis extent.
                let anchor = cursored_bounds(&metrics, model, scroll_offset, extent, orientation);
                let model_for_keys = model;
                // Scoped capture leaves sibling controls’ keys alone; child handlers run first.
                crate::keyboard_capture::keyboard_capture(strip, move |key, modifiers| {
                    event_for_key_with(model_for_keys, key, modifiers, rtl, anchor, activation)
                        .map(&sink)
                })
            }
            None => strip,
        };

        let semantics = Semantics::new(Role::TabList)
            .label(label.clone())
            .orientation(orientation);
        crate::semantics_container::SemanticsContainer::new(strip, semantics)
            .id(iced::advanced::widget::Id::from(format!(
                "enz-document-tabs::{label}"
            )))
            .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzDocumentTabs<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzDocumentTabs<'a, Message>) -> Self {
        w.into_element()
    }
}
