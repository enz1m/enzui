//! Document-tab configuration and element composition.

use super::*;

/// Builder for a document/browser-style tab. See [`enz_document_tab`].
#[must_use]
pub struct EnzDocumentTab<'a, Message: Clone> {
    pub(crate) title: String,
    pub(crate) icon: String,
    pub(crate) on_activate: Option<Message>,
    pub(crate) on_close: Option<Message>,
    pub(crate) on_context: Option<Box<dyn Fn(f32, f32) -> Message + 'a>>,
    pub(crate) loading: bool,
    pub(crate) modified: bool,
    pub(crate) selected: bool,
    pub(crate) focused: bool,
    pub(crate) close_visible: bool,
    pub(crate) fill_width: bool,
    pub(crate) tab_height: f32,
    pub(crate) tab_min_width: f32,
    pub(crate) tab_max_width: f32,
    pub(crate) tab_radius: f32,
    pub(crate) content_padding_x: f32,
    pub(crate) content_padding_trailing: f32,
    pub(crate) favicon_size: f32,
    pub(crate) title_size: f32,
    pub(crate) ellipsis: Ellipsis,
    pub(crate) selected_background: Option<Color>,
    pub(crate) hover_background: Option<Color>,
    pub(crate) tab_background: Option<Color>,
    pub(crate) workspace_accent: Option<Color>,
    pub(crate) title_color: Option<Color>,
    pub(crate) selected_title_color: Option<Color>,
    pub(crate) embedded: bool,
    pub(crate) set_position: Option<SetPosition>,
    pub(crate) id_prefix: String,
    pub(crate) id_override: Option<String>,
    pub(crate) _phantom: std::marker::PhantomData<&'a ()>,
}

/// Default namespace prefix for embedded tab and close identities.
pub const DEFAULT_ID_PREFIX: &str = "enz-document-tab";

/// Fixed suffix of the outer tab surface's derived id (embedded mode).
/// Fixed, never the title: the title is already folded into the namespace, and
/// a second copy there would make the suffix redundant rather than stable.
pub const TAB_ID_SUFFIX: &str = "tab";

/// Fixed suffix of the close affordance's derived id (embedded mode). This
/// is the id a routed AccessKit `Click` must target to close the document
/// without the close button being a Tab stop.
pub const CLOSE_ID_SUFFIX: &str = "close";

/// Build an activatable tab with no icon, loading, selection or embedded mode.
/// The close control is visible but disabled until [`EnzDocumentTab::on_close`] is
/// set. Colours resolve from the live theme; dimensions use the published defaults.
pub fn enz_document_tab<Message: Clone>(
    title: impl Into<String>,
    on_activate: Message,
) -> EnzDocumentTab<'static, Message> {
    EnzDocumentTab {
        title: title.into(),
        icon: String::new(),
        on_activate: Some(on_activate),
        on_close: None,
        on_context: None,
        loading: false,
        modified: false,
        selected: false,
        focused: false,
        close_visible: true,
        fill_width: false,
        tab_height: DEFAULT_TAB_HEIGHT,
        tab_min_width: DEFAULT_TAB_MIN_WIDTH,
        tab_max_width: DEFAULT_TAB_MAX_WIDTH,
        tab_radius: enzui_theme::text_defaults::shape_radius(|s| s.tab),
        content_padding_x: DEFAULT_CONTENT_PADDING_X,
        content_padding_trailing: DEFAULT_CONTENT_PADDING_TRAILING,
        favicon_size: DEFAULT_FAVICON_SIZE,
        title_size: enzui_theme::text_scale::ui_px(DEFAULT_TITLE_SIZE),
        ellipsis: Ellipsis::End,
        selected_background: None,
        hover_background: None,
        tab_background: None,
        workspace_accent: None,
        title_color: None,
        selected_title_color: None,
        embedded: false,
        set_position: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        id_override: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzDocumentTab<'a, Message> {
    /// Leading favicon/icon by Lucide/semantic name. Empty by default (no icon).
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }
    /// Show the loading spinner in place of the favicon. Defaults to `false`.
    pub fn loading(mut self, l: bool) -> Self {
        self.loading = l;
        self
    }
    /// Mark the document as having unsaved changes: a filled dot in the
    /// title's ink takes the favicon slot (the loading spinner still wins),
    /// and assistive technology hears "modified". Defaults to `false`.
    pub fn modified(mut self, modified: bool) -> Self {
        self.modified = modified;
        self
    }
    /// Mark the tab as the active/selected one. Defaults to `false`.
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    /// Set the strip's roving cursor flag; defaults to false.
    /// An unselected cursored tab paints a model-focus border independently of actual
    /// keyboard focus. In embedded mode this flag keeps the surface in the Tab ring;
    /// the host must select exactly one cursored cell for a single roving stop.
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    /// Show the trailing close (×) button. Defaults to `true`; pair with
    /// [`Self::on_close`] to make it functional.
    pub fn close_visible(mut self, v: bool) -> Self {
        self.close_visible = v;
        self
    }
    /// Fill the available bounded width and place the close control at the trailing edge.
    /// Defaults to false, preserving content-sized tabs. Supply a bounded slot; this
    /// option fills its available width rather than choosing a document-tab width.
    pub fn fill_width(mut self, v: bool) -> Self {
        self.fill_width = v;
        self
    }
    /// Set the close-control message and the tab's middle-click message.
    /// Middle clicks require press and release inside this tab, even if the close
    /// control is hidden. Child capture prevents the tab from handling the event.
    pub fn on_close(mut self, msg: Message) -> Self {
        self.on_close = Some(msg);
        self
    }

    /// Handle completed right clicks with logical-pixel coordinates local to the tab.
    /// Press and release must both land inside this tab. Child capture takes priority.
    pub fn on_context<F>(mut self, f: F) -> Self
    where
        F: Fn(f32, f32) -> Message + 'a,
    {
        self.on_context = Some(Box::new(f));
        self
    }
    /// Set unbounded height fallback and close-size reference in logical pixels.
    /// Defaults to 32; non-finite or nonpositive inputs reset to 32. Bounded parent
    /// height overrides the painted height without rescaling the close or icon.
    pub fn tab_height(mut self, h: f32) -> Self {
        self.tab_height = FinitePositive::new_or(h, DEFAULT_TAB_HEIGHT).get();
        self
    }
    /// Set the width floor in logical pixels; defaults to 96.
    /// Zero removes the floor. Negative or non-finite inputs reset to 96. The min/max
    /// pair is sorted before layout, so reversed settings exchange their roles.
    pub fn tab_min_width(mut self, w: f32) -> Self {
        self.tab_min_width = FiniteNonNeg::new_or(w, DEFAULT_TAB_MIN_WIDTH).get();
        self
    }
    /// Set the width ceiling in logical pixels; defaults to 240.
    /// Non-finite or nonpositive inputs reset to 240; unlike the minimum, zero is
    /// not retained. The min/max pair is sorted before layout.
    pub fn tab_max_width(mut self, w: f32) -> Self {
        self.tab_max_width = FinitePositive::new_or(w, DEFAULT_TAB_MAX_WIDTH).get();
        self
    }
    /// Corner radius in px. Normalized to finite non-negative (0 = square).
    pub fn tab_radius(mut self, r: f32) -> Self {
        self.tab_radius =
            FiniteNonNeg::new_or(r, enzui_theme::text_defaults::shape_radius(|s| s.tab)).get();
        self
    }
    /// Favicon box size in px, normalized to finite positive.
    pub fn favicon_size(mut self, s: f32) -> Self {
        self.favicon_size = FinitePositive::new_or(s, DEFAULT_FAVICON_SIZE).get();
        self
    }
    /// Set title size in logical pixels. Construction uses the scaled 13 px UI size;
    /// non-finite or nonpositive setter inputs reset to the unscaled 13 px base.
    pub fn title_size(mut self, s: f32) -> Self {
        self.title_size = FinitePositive::new_or(s, DEFAULT_TITLE_SIZE).get();
        self
    }

    /// Ellipsis strategy for a title that overflows the tab width. Defaults to
    /// [`Ellipsis::End`] (right for a filename). Use [`Ellipsis::Middle`] when
    /// the tail carries information (e.g. an extension or path segment);
    /// [`Ellipsis::None`] disables the marker; overflow is clipped at the
    /// title lane before the close button.
    ///
    /// The title always lays out single-line ([`Wrapping::None`]); the mode
    /// only chooses *where* the `…` goes, never whether the run wraps.
    pub fn ellipsis(mut self, e: Ellipsis) -> Self {
        self.ellipsis = e;
        self
    }

    fn run_config(&self) -> (Wrapping, Ellipsis) {
        (Wrapping::None, self.ellipsis)
    }

    /// Return the configured title marker; defaults to [`Ellipsis::End`].
    pub fn ellipsis_mode(&self) -> Ellipsis {
        self.run_config().1
    }

    /// Return [`Wrapping::None`]; document titles stay on one line.
    pub fn wrapping_mode(&self) -> Wrapping {
        self.run_config().0
    }

    /// Return whether a close handler is installed, independently of its visibility.
    pub fn close_is_enabled(&self) -> bool {
        self.on_close.is_some()
    }

    pub(crate) fn width_range(&self) -> OrderedRangeF32 {
        OrderedRangeF32::new_sorted(self.tab_min_width, self.tab_max_width).unwrap_or_else(|| {
            OrderedRangeF32::new(DEFAULT_TAB_MIN_WIDTH, DEFAULT_TAB_MAX_WIDTH)
                .expect("default tab width range is finite and ordered")
        })
    }

    /// Pin the selected fill before the optional workspace-accent wash.
    /// Defaults to the live palette's selected surface.
    pub fn selected_background(mut self, c: Color) -> Self {
        self.selected_background = Some(c);
        self
    }

    /// Pin the hover fill before any workspace-accent wash.
    /// By default a live-theme hover overlay composites over the resting fill;
    /// selection takes priority over hover.
    pub fn hover_background(mut self, c: Color) -> Self {
        self.hover_background = Some(c);
        self
    }

    /// Override the resting tab background (`tab-background`, default
    /// transparent).
    pub fn tab_background(mut self, c: Color) -> Self {
        self.tab_background = Some(c);
        self
    }

    /// Apply an optional workspace-accent wash over the resolved state fill.
    /// Defaults to no wash; selected tabs use stronger alpha. Resolve accents with
    /// [`WorkspaceAccentPalette::for_mode`](enzui_theme::WorkspaceAccentPalette::for_mode)
    /// for the live palette. Arbitrary colours are not validated here.
    pub fn workspace_accent(mut self, c: Option<Color>) -> Self {
        self.workspace_accent = c;
        self
    }

    /// Pin title ink for both states unless [`Self::selected_title_color`] overrides it.
    /// Defaults to live-theme text ink.
    pub fn title_color(mut self, c: Color) -> Self {
        self.title_color = Some(c);
        self
    }

    /// Override the selected title colour.
    /// Default resolves `text` from the runtime theme.
    pub fn selected_title_color(mut self, c: Color) -> Self {
        self.selected_title_color = Some(c);
        self
    }

    /// Configure a cell for a strip with host-owned roving focus; defaults to false.
    /// The close control leaves the Tab ring, and surfaces leave it unless
    /// [`Self::focused`] is true. Completed pointer clicks and routed AT activation
    /// remain available. Embedded cells publish IDs; standalone cells stay anonymous.
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Publish a one-based position and full strip count for accessibility.
    /// Use the total sequence count, including unrealized cells; do not pre-subtract
    /// for AccessKit's zero-based mapping. Unset by default.
    pub fn set_position(mut self, position: usize, count: usize) -> Self {
        self.set_position = Some(SetPosition {
            index: position,
            count,
        });
        self
    }

    /// Set the embedded identity namespace prefix; defaults to [`DEFAULT_ID_PREFIX`].
    /// The namespace combines this prefix and title, with distinct tab/close suffixes.
    /// Same-title cells need distinct prefixes or [`Self::id`] overrides.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Override the embedded tab/close namespace, excluding the title.
    /// Use a stable document key to preserve routed identity across retitles and reorder.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id_override = Some(id.into());
        self
    }

    fn id_namespace(&self) -> String {
        self.id_override
            .clone()
            .unwrap_or_else(|| format!("{}-{}", self.id_prefix, self.title))
    }

    /// Derive the outer surface ID that embedded mode publishes.
    /// Calling this does not attach an ID to a standalone tab.
    #[must_use]
    pub fn tab_id(&self) -> iced::advanced::widget::Id {
        enzui_core::identity::derive_id(&self.id_namespace(), TAB_ID_SUFFIX)
    }

    /// Derive the close-control ID that embedded mode publishes for routed activation.
    /// Calling this does not attach an ID to a standalone tab.
    #[must_use]
    pub fn close_id(&self) -> iced::advanced::widget::Id {
        enzui_core::identity::derive_id(&self.id_namespace(), CLOSE_ID_SUFFIX)
    }

    /// Consume the builder and produce the widget tree.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a present, idle favicon always resolves a \
                  handle"
    )]
    pub fn into_element(mut self) -> Element<'a, Message> {
        let embedded = self.embedded;
        let cursored = embedded && self.focused;
        let (tab_id, close_id) = if embedded {
            (Some(self.tab_id()), Some(self.close_id()))
        } else {
            (None, None)
        };
        let selected = self.selected;
        let model_focused = self.focused;
        let radius = self.tab_radius;
        let height = self.tab_height;
        let favicon_size = self.favicon_size;
        let width_range = self.width_range();
        let a11y_title = self.title.clone();
        let a11y_title_for_pin = self.title.clone();
        let middle_close_msg = self.on_close.clone();
        let close_enabled = self.close_is_enabled();
        let context_cb = self.on_context.take();
        let selected_background = self.selected_background;
        let hover_background = self.hover_background;
        let base_bg = self.tab_background.unwrap_or(Color::TRANSPARENT);
        let workspace_accent = self.workspace_accent;
        let title_override = if selected {
            self.selected_title_color.or(self.title_color)
        } else {
            self.title_color
        };

        let mut row = row![]
            .spacing(DEFAULT_ROW_SPACING)
            .align_y(Alignment::Center);
        let modified = self.modified;
        if self.loading || modified || !self.icon.is_empty() {
            let spinner_label = if a11y_title.trim().is_empty() {
                enzui_core::localization::text("enzui-document-tab-loading")
            } else {
                enzui_core::localized!("enzui-document-tab-loading-named", "title" => a11y_title)
            };
            let slot: Element<'a, Message> = if self.loading {
                container(enz_spinner().size(favicon_size).label(spinner_label))
                    .width(Length::Fixed(favicon_size))
                    .height(Length::Fixed(favicon_size))
                    .center_x(Length::Fixed(favicon_size))
                    .center_y(Length::Fixed(favicon_size))
                    .into()
            } else if modified {
                // Half the favicon box: reads as a mark beside the title, not
                // as a glyph competing with it.
                let dot = (favicon_size * 0.5).round().max(4.0);
                container(
                    container(iced::widget::Space::new())
                        .width(Length::Fixed(dot))
                        .height(Length::Fixed(dot))
                        .style(move |theme: &iced::Theme| container::Style {
                            background: Some(
                                title_override
                                    .unwrap_or_else(|| theme_lookup::palette(theme).text)
                                    .into(),
                            ),
                            border: iced::Border::default().rounded(dot / 2.0),
                            ..container::Style::default()
                        }),
                )
                .width(Length::Fixed(favicon_size))
                .height(Length::Fixed(favicon_size))
                .center_x(Length::Fixed(favicon_size))
                .center_y(Length::Fixed(favicon_size))
                .into()
            } else {
                let handle = favicon_handle(&self.icon, self.loading)
                    .expect("non-empty, non-loading favicon must resolve a handle");
                let icon = crate::icon::enz_icon_from_handle(handle)
                    .size(favicon_size)
                    .tint_resolved(move |theme: &iced::Theme| {
                        title_override.unwrap_or_else(|| theme_lookup::palette(theme).text)
                    });
                container(icon)
                    .width(Length::Fixed(favicon_size))
                    .height(Length::Fixed(favicon_size))
                    .center_x(Length::Fixed(favicon_size))
                    .center_y(Length::Fixed(favicon_size))
                    .into()
            };
            row = row.push(slot);
        }
        let (wrapping, ellipsis) = self.run_config();
        let title_font = title_font_for(selected);
        // Derive both title reservation and outer clamps from the sorted width ceiling.
        let title_cap = title_max_width(
            width_range.hi(),
            self.content_padding_x,
            self.content_padding_trailing,
            self.close_visible,
            (self.tab_height - DEFAULT_CLOSE_HEIGHT_INSET).max(DEFAULT_CLOSE_MIN_SIZE),
            self.loading || modified || !self.icon.is_empty(),
            self.favicon_size,
        );
        let close_reserve = if self.close_visible {
            (self.tab_height - DEFAULT_CLOSE_HEIGHT_INSET).max(DEFAULT_CLOSE_MIN_SIZE)
        } else {
            0.0
        };
        let title_inner: Element<'a, Message> = container(
            text(self.title)
                .size(self.title_size)
                .font(title_font)
                .wrapping(wrapping)
                .ellipsis(ellipsis)
                .style(move |theme: &iced::Theme| text::Style {
                    color: Some(
                        title_override.unwrap_or_else(|| theme_lookup::palette(theme).text),
                    ),
                }),
        )
        .width(Length::Shrink)
        .clip(true)
        .align_x(iced::alignment::Horizontal::Left)
        .align_y(iced::alignment::Vertical::Center)
        .into();
        // Reserve trailing chrome against the actual parent offer, not only the build-time cap.
        let title: Element<'a, Message> = TitleReserveCap {
            child: title_inner,
            close_reserve,
            ceiling_cap: title_cap,
        }
        .into();
        row = row.push(title);
        if self.fill_width {
            row = row.push(iced::widget::Space::new().width(Length::Fill));
        }
        if self.close_visible {
            let close_size =
                (self.tab_height - DEFAULT_CLOSE_HEIGHT_INSET).max(DEFAULT_CLOSE_MIN_SIZE);
            let close_btn: Element<'a, Message> = {
                let close_status_cell = std::rc::Rc::new(std::cell::Cell::new(FocusStatus::Active));
                let close_glyph = crate::icon::enz_icon("action.close")
                    .size((close_size - 8.0).max(9.0))
                    .tint_with_status(
                        std::rc::Rc::clone(&close_status_cell),
                        move |theme: &iced::Theme, status: FocusStatus| {
                            let p = theme_lookup::palette(theme);
                            let state_fill = document_tab_state_bg(
                                &p,
                                status,
                                selected,
                                base_bg,
                                selected_background,
                                hover_background,
                                workspace_accent,
                            );
                            let painted_fill = document_tab_composited_fill(&p, state_fill);
                            document_tab_close_icon_ink(&p, painted_fill, status, close_enabled)
                        },
                    );
                let close_radius = close_size / 2.0;
                let close_label =
                    enzui_core::localized!("enzui-document-tab-close", "title" => a11y_title);
                let mut close = focus_button_maybe(
                    container(close_glyph)
                        .center_x(Length::Fixed(close_size))
                        .center_y(Length::Fixed(close_size)),
                    self.on_close.clone(),
                )
                .width(Length::Fixed(close_size))
                .height(Length::Fixed(close_size))
                .padding(0)
                .semantics(
                    Semantics::new(Role::Button)
                        .default_action()
                        .label(close_label)
                        .disabled(self.on_close.is_none()),
                );
                if embedded {
                    close = close.embedded(true);
                    if let Some(id) = close_id {
                        close = close.id(id);
                    }
                }
                close
                    .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                        close_status_cell.set(status);
                        close_button_style(theme, status, close_radius)
                    })
                    .into()
            };
            row = row.push(close_btn);
        }
        let inner_width = if self.fill_width {
            Length::Fill
        } else {
            Length::Shrink
        };
        let mut outer = focus_button_maybe(
            container(row)
                .width(inner_width)
                .height(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                .clip(true)
                .padding(iced::Padding {
                    top: 0.0,
                    right: self.content_padding_trailing,
                    bottom: 0.0,
                    left: self.content_padding_x,
                }),
            self.on_activate.clone(),
        )
        .width(inner_width)
        .content_align_x(Alignment::Start)
        .height(Length::Fill)
        .padding(0)
        .semantics({
            let sem = Semantics::new(Role::Tab)
                .default_action()
                .label(a11y_title)
                .selected(selected)
                .disabled(self.on_activate.is_none());
            let sem = if modified {
                sem.description(enzui_core::localization::text(
                    "enzui-document-tab-modified",
                ))
            } else {
                sem
            };
            match self.set_position {
                Some(p) => sem.set_position(p),
                None => sem,
            }
        });
        if embedded {
            if !cursored {
                outer = outer.embedded(true);
            }
            if let Some(id) = tab_id {
                outer = outer.id(id);
            }
        }
        let outer = outer.style_focus(move |theme: &iced::Theme, status: FocusStatus| {
            let p = theme_lookup::palette(theme);
            let bg = document_tab_state_bg(
                &p,
                status,
                selected,
                base_bg,
                selected_background,
                hover_background,
                workspace_accent,
            );
            let border = tab_surface_border(&p, status, model_focused, selected, radius);
            ButtonStyle {
                background: Some(Background::Color(bg)),
                text_color: p.text,
                border,
                ..ButtonStyle::default()
            }
        });
        let title_pin_font = title_font;
        let title_pin_string = a11y_title_for_pin;
        let title_pin_size = self.title_size;
        let title_pin_cap = title_cap;
        let title_pin_ceiling = width_range.hi();
        // Elided glyph width can underfill the reserved slot; pin only overflowing titles.
        let pinned: Element<'a, Message> = if self.fill_width {
            outer.into()
        } else {
            TitleWidthPin {
                child: outer.into(),
                title: title_pin_string,
                title_size: title_pin_size,
                title_font: title_pin_font,
                title_cap: title_pin_cap,
                pinned_width: title_pin_ceiling,
            }
            .into()
        };
        let clamped: Element<'a, Message> = crate::min_width::min_width(
            crate::max_width::max_width(pinned, width_range.hi()),
            width_range.lo(),
        )
        .into();
        let captured: Element<'a, Message> = TabMouseCapture {
            child: clamped,
            on_close: middle_close_msg,
            on_context: context_cb,
        }
        .into();
        TabHeightFill {
            child: captured,
            floor: height,
        }
        .into()
    }
}
