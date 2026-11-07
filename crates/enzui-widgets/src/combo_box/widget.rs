//! Selection-field trigger and popup routing.

use super::*;
use iced::widget::column;

pub(crate) struct ComboBoxWidget<'a, Message> {
    field: Element<'a, Message>,
    trigger_child: usize,
    options: Vec<String>,
    selected_index: Option<usize>,
    separators: Vec<usize>,
    on_select: Box<dyn Fn(String) -> Message + 'a>,
    on_select_indexed: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    size: TextFieldSize,
    radius: Option<f32>,
    available: bool,
    read_only: bool,
    palette: Option<Palette>,
    semantics: Semantics,
    listbox_semantics: Semantics,
    scroll_id: Id,
    node_id: Id,
    row_ids: Vec<Id>,
    row_keys: Vec<String>,
    direction: enzui_core::placement::Direction,
    scroll_cell: Option<crate::scroll_view::ScrollTranslation>,
    // `None` when no option has an icon: rows keep their plain layout.
    icons: Option<Vec<Option<crate::icon::IconSource>>>,
    /// The query and its edit callback, for a searchable field.
    search: Option<(String, QueryCallback<'a, Message>)>,
    /// Whether the list is open, for the chevron in the trigger.
    chevron_open: std::rc::Rc<std::cell::Cell<bool>>,
}

/// Receives a searchable field's edited query.
type QueryCallback<'a, Message> = std::rc::Rc<dyn Fn(String) -> Message + 'a>;

/// An option's leading icon at the field's text size, tinted like `tone`
/// when it is symbolic (a bundled name or a symbolic SVG).
fn option_icon_element<'a, Message: 'a>(
    source: crate::icon::IconSource,
    size: f32,
    tone: impl Fn(&iced::Theme) -> Color + 'static,
) -> Element<'a, Message> {
    source.into_slot_element(size, |icon| icon.tint_resolved(tone))
}

/// The gap between an option's icon and its label.
const ICON_GAP: f32 = 8.0;

pub(crate) fn resolve_node_id(
    id: Option<&Id>,
    id_prefix: &str,
    label: &str,
    placeholder: &str,
) -> Id {
    if let Some(id) = id {
        return id.clone();
    }
    let seed = if !label.is_empty() {
        label
    } else {
        placeholder
    };
    enzui_core::identity::derive_id(id_prefix, seed)
}

pub(crate) fn option_ids_and_keys(namespace: &Id, options: &[String]) -> (Vec<Id>, Vec<String>) {
    let prefix = format!("{namespace:?}-opt");
    let mut keyed = enzui_core::identity::KeyedIds::new(prefix.clone());
    let mut ids = Vec::with_capacity(options.len());
    let mut keys = Vec::with_capacity(options.len());
    for option in options {
        // Both identity helpers advance occurrences; derive the ID from this single key allocation.
        let key = keyed.key_for(option);
        ids.push(enzui_core::identity::derive_id(&prefix, &key));
        keys.push(key);
    }
    (ids, keys)
}

impl<'a, Message: Clone + 'a> ComboBoxWidget<'a, Message> {
    pub(crate) fn new(builder: EnzComboBox<'a, Message>) -> Self {
        let tokens = builder.size.combo_tokens();
        let has_error = builder.error_text.as_deref().is_some_and(|e| !e.is_empty());
        let available = builder.is_available();
        let read_only = builder.read_only;
        let variant = builder.variant;
        let pinned = builder.palette.clone();
        let semantics = builder.semantics(false);
        let listbox_semantics = builder.listbox_semantics();
        let node_id = resolve_node_id(
            builder.id.as_ref(),
            &builder.id_prefix,
            &builder.label,
            &builder.identity_placeholder,
        );
        // Keep the scroll ID stable across rebuilds to retain the popup offset.
        let scroll_id = enzui_core::identity::derive_id(&format!("{node_id:?}"), "popup-scroll");
        let (row_ids, row_keys) = option_ids_and_keys(&node_id, &builder.options);

        let resolve = {
            let pinned = pinned.clone();
            move |theme: &iced::Theme| -> Palette {
                pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme))
            }
        };

        let selected_index = builder.resolved_selected_index();
        let value = builder.resolved_value();
        let icons = builder.option_icon.as_ref().and_then(|icon| {
            let icons: Vec<_> = (0..builder.options.len()).map(icon).collect();
            icons.iter().any(Option::is_some).then_some(icons)
        });
        let icon_size = tokens.text_size.min(16.0);
        let placeholder = builder.placeholder;
        let label_resolve = resolve.clone();
        let search = builder.search.map(|search| (search.query, search.on_query));
        let query = search
            .as_ref()
            .map(|(query, _)| query.clone())
            .filter(|query| !query.is_empty());
        let has_value = value.is_some() || query.is_some();
        // A searching field shows what is typed.
        let shown = query
            .clone()
            .or(value)
            .unwrap_or_else(|| placeholder.into_owned());
        // Elision needs the remaining bounded width rather than the text's intrinsic width.
        let trigger_text = enz_text(shown)
            .size_px(tokens.text_size)
            .single_line_elided()
            .full_width()
            .color_resolved(move |theme: &iced::Theme| {
                let p = label_resolve(theme);
                if !available {
                    crate::color::with_opacity(
                        p.text,
                        crate::theme_lookup::opacity(theme).disabled(),
                    )
                } else if has_value {
                    p.text
                } else {
                    p.text_muted
                }
            })
            .into_element::<Message>();

        let chevron_resolve = resolve.clone();
        let chevron_open = std::rc::Rc::new(std::cell::Cell::new(false));
        let indicator = crate::icon::enz_icon(if read_only {
            "object.lock"
        } else {
            "navigation.down"
        })
        .size(tokens.text_size.min(14.0))
        .tint_resolved(move |theme: &iced::Theme| chevron_resolve(theme).text_muted);
        // The chevron points up while the list is open.
        let indicator = if read_only {
            indicator.into_element::<Message>()
        } else {
            indicator.half_turn_following(std::rc::Rc::clone(&chevron_open))
        };

        let mut trigger_row = row![].align_y(iced::Alignment::Center);
        if query.is_some() {
            let icon_resolve = resolve.clone();
            trigger_row = trigger_row
                .push(
                    crate::icon::enz_icon("search")
                        .size(icon_size)
                        .tint_resolved(move |theme: &iced::Theme| icon_resolve(theme).text_muted)
                        .into_element::<Message>(),
                )
                .push(Space::new().width(ICON_GAP));
        } else if let Some(source) =
            selected_index.and_then(|index| icons.as_ref()?.get(index).cloned().flatten())
        {
            let icon_resolve = resolve.clone();
            trigger_row = trigger_row
                .push(option_icon_element(source, icon_size, move |theme| {
                    let p = icon_resolve(theme);
                    if available {
                        p.text
                    } else {
                        crate::color::with_opacity(
                            p.text,
                            crate::theme_lookup::opacity(theme).disabled(),
                        )
                    }
                }))
                .push(Space::new().width(ICON_GAP));
        }
        let trigger_row = trigger_row
            .push(trigger_text)
            .push(Space::new().width(6.0))
            .push(indicator);

        let chrome_resolve = resolve.clone();
        let size = builder.size;
        let radius_pin = builder.radius;
        let trigger = container(trigger_row)
            .padding(Padding::ZERO.left(tokens.padding_x).right(tokens.padding_x))
            .height(tokens.height)
            .center_y(tokens.height)
            .width(Length::Fill)
            .style(move |theme: &iced::Theme| {
                let p = chrome_resolve(theme);
                let border_color = if has_error { p.error } else { p.field_border };
                let fill = crate::tokens::field_fill_for(&p, variant);
                container::Style {
                    background: Some(Background::Color(if available {
                        fill
                    } else {
                        crate::color::with_opacity(
                            fill,
                            crate::theme_lookup::opacity(theme).disabled(),
                        )
                    })),
                    border: Border {
                        color: border_color,
                        width: 1.0,
                        radius: size.resolve_radius(theme, radius_pin).into(),
                    },
                    ..container::Style::default()
                }
            });

        let mut col = column![].spacing(4.0).width(builder.width);
        let mut trigger_child = 0;
        if !builder.label.is_empty() {
            let label_palette = resolve.clone();
            col = col.push(
                text(builder.label.to_string())
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(label_palette(theme).text_muted),
                    }),
            );
            trigger_child = 1;
        }
        col = col.push(trigger);
        if let Some(err) = builder.error_text.filter(|e| !e.is_empty()) {
            let err_palette = resolve.clone();
            col = col.push(
                text(err.into_owned())
                    .size(enzui_theme::text_scale::ui_px(11.0))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(err_palette(theme).error),
                    }),
            );
        }

        Self {
            chevron_open,
            field: col.into(),
            trigger_child,
            options: builder.options,
            selected_index,
            separators: builder.separators,
            on_select: builder.on_select,
            on_select_indexed: builder.on_select_indexed,
            size: builder.size,
            radius: builder.radius,
            available,
            read_only,
            palette: pinned,
            semantics,
            listbox_semantics,
            scroll_id,
            node_id,
            row_ids,
            row_keys,
            direction: enzui_core::placement::resolve_direction(builder.direction),
            scroll_cell: builder.scroll_cell,
            icons,
            search,
        }
    }

    fn navigation(&self, active: Option<usize>) -> MenuNavigation {
        let mut nav = MenuNavigation::new(self.options.iter().map(|_| MenuEntry::item()).collect());
        if let Some(index) = active {
            nav.set_active(index);
        }
        nav
    }

    fn nav_target(&self, active: Option<usize>, movement: MenuMove) -> Option<usize> {
        self.navigation(active).apply(movement)
    }

    fn reconcile_active(&self, state: &mut ComboState) {
        let Some(old_key) = state.active_key.clone() else {
            return;
        };
        let keys: Vec<&str> = self.row_keys.iter().map(String::as_str).collect();
        let new_index =
            enzui_core::tree_nav::reconcile_cursor(Some(old_key.as_str()), state.active, &keys);
        state.active = new_index;
        state.active_key = new_index.and_then(|i| self.row_keys.get(i).cloned());
    }

    fn set_active(&self, state: &mut ComboState, idx: Option<usize>) {
        state.active = idx;
        state.active_key = idx.and_then(|i| self.row_keys.get(i).cloned());
    }

    fn trigger_bounds(&self, layout: iced::advanced::Layout, column: &[Tree]) -> iced::Rectangle {
        layout
            .children(column)
            .nth(self.trigger_child)
            .map_or_else(|| layout.bounds(), |l| l.bounds())
    }

    fn popup_window(&self, active: Option<usize>) -> PopupWindow {
        compute_popup_window(
            self.options.len(),
            self.size.combo_tokens().height,
            self.scroll_cell.as_ref().map(|c| c.get().y),
            active,
        )
    }

    pub(super) fn popup(&self, state: &ComboState) -> Option<Element<'a, Message>> {
        if !state.open || !self.available {
            return None;
        }

        let tokens = self.size.combo_tokens();
        let pinned = self.palette.clone();
        let resolve = move |theme: &iced::Theme| -> Palette {
            pinned
                .clone()
                .unwrap_or_else(|| crate::theme_lookup::palette(theme))
        };

        let selected_index = self.selected_index;
        let total = self.options.len();
        let window = self.popup_window(state.active);
        let mut list = column![].width(Length::Fill);

        if window.start > 0 {
            let leading_h = window.start as f32 * tokens.height;
            list = list.push(Space::new().width(Length::Fill).height(leading_h));
        }

        for index in window.start..window.start + window.len {
            let Some(option) = self.options.get(index) else {
                break;
            };
            let is_selected = selected_index == Some(index);
            let is_active = state.keyboard_navigation && state.active == Some(index);
            let is_hovered = !state.keyboard_navigation && state.hovered_row == Some(index);
            let row_resolve = resolve.clone();

            let check: Element<'a, Message> = if is_selected {
                container(
                    crate::icon::enz_icon(SELECTED_CHECK_ICON)
                        .size(tokens.text_size.min(14.0))
                        .tint_resolved(move |theme: &iced::Theme| row_resolve(theme).accent)
                        .into_element::<Message>(),
                )
                .width(CHECK_COLUMN)
                .into()
            } else {
                Space::new().width(CHECK_COLUMN).into()
            };

            let text_resolve = resolve.clone();
            let label =
                text(option.clone())
                    .size(tokens.text_size)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(text_resolve(theme).text),
                    });

            let mut content = row![].spacing(4.0).align_y(iced::Alignment::Center);
            if let Some(icons) = &self.icons {
                let icon_size = tokens.text_size.min(16.0);
                let leading: Element<'a, Message> = match icons.get(index).cloned().flatten() {
                    Some(source) => {
                        let icon_resolve = resolve.clone();
                        option_icon_element(source, icon_size, move |theme| {
                            icon_resolve(theme).text
                        })
                    }
                    None => Space::new().width(icon_size).into(),
                };
                content = content
                    .push(container(leading).width(icon_size))
                    .push(Space::new().width(ICON_GAP - 4.0));
            }
            let content = content.push(label.width(Length::Fill)).push(check);

            let fill_resolve = resolve.clone();
            let cell = container(content)
                .width(Length::Fill)
                .height((tokens.height - 4.0).max(0.0))
                .center_y((tokens.height - 4.0).max(0.0))
                .padding(Padding::ZERO.left(tokens.padding_x).right(tokens.padding_x))
                .style(move |theme: &iced::Theme| {
                    let p = fill_resolve(theme);
                    container::Style {
                        background: (is_active || is_hovered)
                            .then_some(Background::Color(p.surface_hover)),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: enzui_core::metrics::FiniteNonNeg::new_or(
                                crate::theme_lookup::shape_radius(theme, |s| s.row),
                                ROW_RADIUS_FALLBACK,
                            )
                            .get()
                            .into(),
                        },
                        ..container::Style::default()
                    }
                });

            // Inset paint inside fixed row targets preserves virtualized heights and pointer geometry.
            let cell: Element<'a, Message> = container(cell)
                .height(tokens.height)
                .center_y(tokens.height)
                .width(Length::Fill)
                .into();
            let cell: Element<'a, Message> = if self.separators.contains(&index) {
                iced::widget::stack![cell, crate::divider::enz_divider().into_element(),].into()
            } else {
                cell
            };

            // Focus stays on the trigger; active_descendant identifies the highlighted row.
            let row_semantics = Semantics::new(Role::ListBoxOption)
                .label(option.clone())
                .selected(is_selected)
                .set_position(SetPosition {
                    index: index + 1,
                    count: total,
                });

            let row_id = self.row_ids.get(index).cloned().unwrap_or_else(Id::unique);

            list = list.push(
                crate::semantics_container::SemanticsContainer::leaf(cell, row_semantics)
                    .id(row_id),
            );
        }

        if total == 0 && self.search.is_some() {
            let empty_resolve = resolve.clone();
            list = list.push(
                container(
                    text(enzui_core::localization::text("enzui-combo-box-no-matches"))
                        .size(tokens.text_size)
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(empty_resolve(theme).text_muted),
                        }),
                )
                .height(tokens.height)
                .center_y(tokens.height)
                .padding(Padding::ZERO.left(tokens.padding_x)),
            );
        }

        let after_window = window.total.saturating_sub(window.start + window.len);
        if after_window > 0 {
            let trailing_h = after_window as f32 * tokens.height;
            list = list.push(Space::new().width(Length::Fill).height(trailing_h));
        }

        let content = container(list).padding(POPUP_PADDING).width(Length::Fill);

        let scroller = crate::scroll_view::enz_scrollable(content)
            .height(Length::Shrink)
            .id(self.scroll_id.clone());
        let scroller_el: Element<'a, Message> = match &self.scroll_cell {
            Some(cell) => crate::scroll_view::publish_translation_of(scroller, cell.clone()),
            None => scroller.into(),
        };
        let scroller_el =
            crate::scroll_view::contain_wheel(scroller_el, crate::scroll_view::Axis::Vertical);
        let capped = crate::max_height::MaxHeight::new(scroller_el, popup_max_height(self.size));
        let fill_resolve = resolve.clone();
        // Wrap the viewport so rounded clipping does not scroll away with the document.
        let surface = crate::surface::enz_surface(capped)
            .width(Length::Fill)
            .border_width(1.0)
            .fill_resolved(move |theme| fill_resolve(theme).surface_elevated)
            .border_color_resolved(move |theme| resolve(theme).border)
            .radius_resolved(|theme| {
                enzui_core::metrics::FiniteNonNeg::new_or(
                    crate::theme_lookup::shape_radius(theme, |s| s.popup),
                    POPUP_RADIUS_FALLBACK,
                )
                .get()
                .into()
            });

        Some(
            crate::semantics_container::SemanticsContainer::new(
                surface,
                self.listbox_semantics.clone(),
            )
            .into(),
        )
    }

    // Bounds remain in document space; consumers apply the scroll translation.
    fn row_layer(&self, layout: iced::advanced::Layout, popup: &[Tree]) -> Option<Vec<Rectangle>> {
        let window = self.popup_window(None);
        row_rects_from_layout(layout, popup, window.len, self.size.combo_tokens().height)
    }

    // Reveal targets the active option because focus remains on the trigger.
    fn reveal_highlight(
        &self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        trigger: Rectangle,
        viewport: Rectangle,
        index: usize,
    ) {
        let state = tree.state.downcast_ref::<ComboState>().clone();
        let Some(mut popup) = self.popup(&state) else {
            return;
        };
        if tree.children.len() < 2 {
            return;
        }
        let popup_tree = &mut tree.children[1];
        popup_tree.diff(&mut popup);

        // Placement can shrink below the row cap; reveal must use the actual viewport.
        let node = crate::anchored_overlay::resolve_popup_layout(
            &mut popup,
            popup_tree,
            renderer,
            viewport.size(),
            trigger - Vector::new(viewport.x, viewport.y),
            &popup_placement(self.direction),
            Some(trigger.width),
        );
        let layout = iced::advanced::Layout::of(&node);

        let window = self.popup_window(Some(index));
        let row = match window.built_position_of(index).and_then(|bp| {
            self.row_layer(layout, &popup_tree.children)
                .and_then(|rects| rects.get(bp).copied())
        }) {
            Some(rect) => rect,
            None => {
                let tokens = self.size.combo_tokens();
                let row_h = tokens.height;
                let row_y = layout.bounds().y + POPUP_PADDING + (index as f32) * row_h;
                Rectangle {
                    x: 0.0,
                    y: row_y,
                    width: 0.0,
                    height: row_h,
                }
            }
        };

        let Some((view, current)) =
            popup_scroll_geometry(&mut popup, popup_tree, layout, renderer, &self.scroll_id)
        else {
            return;
        };

        // Offset geometry is relative to the scroll viewport, not the popup's screen origin.
        let new_y = reveal_axis_offset(current.y, view.height, row.y - view.y, row.height);

        if (new_y - current.y).abs() < 0.5 {
            return;
        }

        struct DoScroll {
            target: Id,
            offset: AbsoluteOffset<Option<f32>>,
        }
        impl<T> Operation<T> for DoScroll {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                content_bounds: iced::Size,
                _translation: Vector,
                state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    state.scroll_to(
                        self.offset,
                        iced::advanced::widget::operation::Animation::Instant,
                        bounds,
                        content_bounds,
                    );
                }
            }
        }
        let mut op = DoScroll {
            target: self.scroll_id.clone(),
            offset: AbsoluteOffset {
                x: None,
                y: Some(new_y),
            },
        };
        popup.as_widget_mut().operate(
            popup_tree,
            layout,
            &layout.bounds(),
            renderer,
            &mut op as &mut dyn Operation,
        );
    }
}

impl<'a, Message> ComboBoxWidget<'a, Message> {
    fn is_focusable(&self) -> bool {
        self.available || self.read_only
    }

    fn is_operable(&self) -> bool {
        self.available && !self.read_only
    }

    fn publish_selection(&self, index: usize, shell: &mut iced::advanced::Shell<'_, Message>)
    where
        Message: Clone,
    {
        if let Some(on_index) = &self.on_select_indexed {
            shell.publish(on_index(index));
        } else if let Some(option) = self.options.get(index) {
            shell.publish((self.on_select)(option.clone()));
        }
        if let Some((query, on_query)) = &self.search
            && !query.is_empty()
        {
            shell.publish(on_query(String::new()));
        }
    }
}

impl<'a, Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for ComboBoxWidget<'a, Message>
where
    Message: Clone + 'a,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<ComboState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(ComboState::default())
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        {
            let state = tree.state.downcast_mut::<ComboState>();
            // Release stale focus before draw; accessibility operations may not run that frame.
            let _ =
                enzui_core::focus::release_focus_if_inert(&mut state.focus, self.is_focusable());
        }

        // A new query re-filters the list: highlight its first option.
        if let Some((query, _)) = &self.search {
            let first = (!self.options.is_empty()).then_some(0);
            let state = tree.state.downcast_mut::<ComboState>();
            if state.query != *query {
                state.query.clone_from(query);
                if state.open {
                    self.set_active(state, first);
                }
            }
        }

        // Retain the popup slot across closing and reopening to preserve scroll state.
        if tree.children.len() != 2 {
            tree.children = vec![
                iced::advanced::widget::Tree::new(&self.field),
                iced::advanced::widget::Tree::empty(),
            ];
        }
        tree.children[0].diff(&mut self.field);
        let state = tree.state.downcast_ref::<ComboState>().clone();
        if let Some(mut popup) = self.popup(&state) {
            tree.children[1].diff(&mut popup);
        }
    }

    fn size(&self) -> iced::Size<Length> {
        self.field.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.field
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let (open, active, focused) = {
            let state = tree.state.downcast_mut::<ComboState>();
            if self.is_focusable() {
                operation.focusable(Some(&self.node_id), layout.bounds(), state);
            } else if state.focus.is_focused() {
                state.focus.unfocus();
            }
            self.reconcile_active(state);
            (state.open, state.active, state.focus.is_focused())
        };

        let mut semantics = self
            .semantics
            .clone()
            .expanded(open && self.is_operable())
            .focused(focused && self.is_focusable());
        if open
            && self.is_operable()
            && let Some(row_id) = active.and_then(|i| self.row_ids.get(i))
        {
            semantics = semantics.active_descendant(row_id.clone());
        }
        let mut node = enzui_core::collect::SemanticsNode::new(semantics);
        operation.custom(Some(&self.node_id), layout.bounds(), &mut node);

        if self.is_operable() {
            let mut activation = crate::activation::ActivationRequest::new(self.node_id.clone());
            operation.custom(Some(&self.node_id), layout.bounds(), &mut activation);

            let mut expand = crate::activation::ExpandRequest::new(self.node_id.clone());
            operation.custom(Some(&self.node_id), layout.bounds(), &mut expand);

            // Apply routed requests during operate; waiting for update would depend on new input.
            if activation.armed() || expand.armed() {
                let state = tree.state.downcast_mut::<ComboState>();
                state.open = !state.open;
                let idx = if state.open {
                    self.selected_index
                } else {
                    None
                };
                self.set_active(state, idx);
                state.pressed_row = None;
                state.hovered_row = None;
            }
        }

        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.field.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        let window_key = enzui_core::focus::window_key(shell.window());
        {
            let state = tree.state.downcast_mut::<ComboState>();
            state.window_key = window_key;
            let open = state.open;
            state.reveal.track(event, open, shell);
            self.chevron_open.set(open);
        }
        enzui_core::focus::note_input_event_for(event, window_key);

        self.field.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        let trigger = self.trigger_bounds(layout, &tree.children[0].children);

        if let Some(hit) = enzui_core::focus::press_focus_from_cursor(event, cursor, trigger) {
            use enzui_core::focus::PressFocus;
            let mut opened_at: Option<usize> = None;
            let state = tree.state.downcast_mut::<ComboState>();
            // While the popup is open, a press it captured or one that reached the layer
            // above is its own; any other press moves focus on.
            let consumed_by_popup =
                state.open && (shell.is_event_captured() || cursor.is_levitating());
            let was_focused = state.focus.is_focused();
            let was_open = state.open;

            match hit {
                PressFocus::Take if self.is_focusable() => state.focus.focus(),
                PressFocus::Drop if consumed_by_popup => {}
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }

            match hit {
                PressFocus::Take if self.is_operable() => {
                    state.open = !state.open;
                    state.keyboard_navigation = false;
                    let idx = if state.open {
                        self.selected_index
                    } else {
                        None
                    };
                    self.set_active(state, idx);
                    state.pressed_row = None;
                    state.hovered_row = None;
                    opened_at = state.active.filter(|_| state.open);
                }
                // Outside dismissal passes the press through to the control it hit.
                PressFocus::Drop if !consumed_by_popup => {
                    state.open = false;
                    self.set_active(state, None);
                    state.pressed_row = None;
                    state.hovered_row = None;
                }
                _ => {}
            }

            if state.focus.is_focused() != was_focused || state.open != was_open {
                shell.request_redraw();
            }
            if state.open != was_open {
                shell.invalidate_layout();
            }
            if let Some(index) = opened_at {
                self.reveal_highlight(tree, renderer, trigger, *viewport, index);
            }
        }

        if shell.is_event_captured() {
            return;
        }

        let iced::Event::Keyboard(kb) = event else {
            return;
        };
        let (focused, open) = {
            let state = tree.state.downcast_ref::<ComboState>();
            (state.focus.is_focused(), state.open)
        };
        if !focused || !self.is_operable() {
            return;
        }
        if let Some((query, on_query)) = &self.search
            && let Some(edited) = search_edit(query, kb)
        {
            shell.publish(on_query(edited));
            let state = tree.state.downcast_mut::<ComboState>();
            state.keyboard_navigation = true;
            if !state.open {
                state.open = true;
                shell.invalidate_layout();
            }
            shell.capture_event();
            shell.request_redraw();
            return;
        }
        let Some(action) = key_for_combo(open, kb) else {
            return;
        };

        {
            let state = tree.state.downcast_mut::<ComboState>();
            self.reconcile_active(state);
        }

        match action {
            ComboKey::Open { last } => {
                let state = tree.state.downcast_mut::<ComboState>();
                state.open = true;
                state.keyboard_navigation = true;
                let idx = self.selected_index.or_else(|| {
                    if last {
                        self.options.len().checked_sub(1)
                    } else {
                        (!self.options.is_empty()).then_some(0)
                    }
                });
                self.set_active(state, idx);
                let opened_at = state.active;
                if let Some(index) = opened_at {
                    self.reveal_highlight(tree, renderer, trigger, *viewport, index);
                }
                shell.capture_event();
                shell.request_redraw();
                shell.invalidate_layout();
            }
            ComboKey::Move(movement) => {
                tree.state.downcast_mut::<ComboState>().keyboard_navigation = true;
                let active = tree.state.downcast_ref::<ComboState>().active;
                if let Some(target) = self.nav_target(active, movement) {
                    self.set_active(tree.state.downcast_mut::<ComboState>(), Some(target));
                    self.reveal_highlight(tree, renderer, trigger, *viewport, target);
                    shell.request_redraw();
                }
                shell.capture_event();
            }
            ComboKey::Commit => {
                let active = tree.state.downcast_ref::<ComboState>().active;
                if let Some(i) = active.filter(|&i| i < self.options.len()) {
                    self.publish_selection(i, shell);
                    let state = tree.state.downcast_mut::<ComboState>();
                    state.open = false;
                    self.set_active(state, None);
                    state.pressed_row = None;
                    state.hovered_row = None;
                    shell.request_redraw();
                    shell.invalidate_layout();
                }
                shell.capture_event();
            }
            ComboKey::Dismiss => {
                let state = tree.state.downcast_mut::<ComboState>();
                state.open = false;
                self.set_active(state, None);
                state.pressed_row = None;
                state.hovered_row = None;
                shell.capture_event();
                shell.request_redraw();
                shell.invalidate_layout();
            }
            ComboKey::Swallow => shell.capture_event(),
        }
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.field.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        {
            let state = tree.state.downcast_ref::<ComboState>();
            if !enzui_core::focus::focus_ring_visible_for(
                state.focus.is_focused(),
                state.window_key,
            ) {
                return;
            }
        }

        let p = self
            .palette
            .clone()
            .unwrap_or_else(|| crate::theme_lookup::palette(theme));
        crate::focus_button::paint_focus_ring(
            renderer,
            self.trigger_bounds(layout, &tree.children[0].children),
            crate::focus_button::FocusRingQuad {
                ink: p.focus_ring,
                width: 0.75 * crate::theme_lookup::focus_ring_width(theme),
                radius: self.size.resolve_radius(theme, self.radius).into(),
                outset: 0.0,
                snap: true,
            },
        );
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        if self.is_operable()
            && cursor.is_over(self.trigger_bounds(layout, &tree.children[0].children))
        {
            return iced::advanced::mouse::Interaction::Pointer;
        }
        self.field.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let state_snapshot = tree.state.downcast_ref::<ComboState>().clone();
        let anchor = self.trigger_bounds(layout, &tree.children[0].children) + translation;
        let popup_window = self.popup_window(state_snapshot.active);
        let popup = self.popup(&state_snapshot);

        let iced::advanced::widget::Tree {
            state: tree_state,
            children,
            ..
        } = tree;
        let combo_state = tree_state.downcast_mut::<ComboState>();
        let (field_tree, rest) = children.split_at_mut(1);

        let field_overlay = self.field.as_widget_mut().overlay(
            &mut field_tree[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        );

        let popup_overlay = match (popup, rest.first_mut()) {
            (Some(mut popup), Some(popup_tree)) => {
                // Overlay layout can run without a preceding widget diff.
                popup_tree.diff(&mut popup);
                Some(crate::overlay_adapter::element(
                    ComboPopupOverlay {
                        popup,
                        tree: popup_tree,
                        scroll_id: &self.scroll_id,
                        anchor,
                        direction: self.direction,
                        state: combo_state,
                        options: &self.options,
                        row_keys: &self.row_keys,
                        on_select: self.on_select.as_ref(),
                        on_select_indexed: self.on_select_indexed.as_deref(),
                        row_height: self.size.combo_tokens().height,
                        window: popup_window,
                    },
                    renderer,
                    window,
                ))
            }
            _ => None,
        };

        let mut overlays = field_overlay;
        overlays.extend(popup_overlay);
        overlays
    }
}

/// The query a key press makes of `query`: typed text is appended (a space
/// only once something is typed, since Space alone opens the list),
/// Backspace drops the last character and Escape clears it; `None` for
/// other keys and for edits of an empty query.
fn search_edit(query: &str, event: &iced::keyboard::Event) -> Option<String> {
    use iced::keyboard::{Event, Key, key::Named};
    let Event::KeyPressed {
        key,
        modifiers,
        text,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Named(Named::Backspace) if !query.is_empty() => {
            let mut edited = query.to_owned();
            let _ = edited.pop();
            Some(edited)
        }
        Key::Named(Named::Escape) if !query.is_empty() => Some(String::new()),
        _ => {
            let typed = text.as_deref()?;
            let printable = !typed.is_empty() && typed.chars().all(|c| !c.is_control());
            let space_first = typed.trim().is_empty() && query.is_empty();
            (printable && !space_first).then(|| format!("{query}{typed}"))
        }
    }
}
