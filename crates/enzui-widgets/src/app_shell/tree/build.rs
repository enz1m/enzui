//! Width-dependent shell construction and native window resize routing.

use super::*;

// Stable roles prevent optional notifications from inheriting the content’s retained tree.
pub(crate) const SHELL_SLOT_TITLEBAR: &str = "enz-app-shell.titlebar";
pub(crate) const SHELL_SLOT_NOTIFICATIONS: &str = "enz-app-shell.notifications";
pub(crate) const SHELL_SLOT_CONTENT: &str = "enz-app-shell.content";

impl<'a, Message: Clone + 'a> EnzAppShell<'a, Message> {
    pub(crate) fn build(
        &self,
        available: f32,
        motion: SidebarMotionFrame,
        custody: &crate::app_shell::seam::ContentTree,
        titlebar_custody: &crate::app_shell::seam::ContentTree,
        titlebar_content_custody: &crate::app_shell::seam::ContentTree,
    ) -> Element<'a, Message> {
        crate::app_shell::build_probe::note_build(available);
        self.built_width.set(available);

        if self.immersive_fullscreen() {
            return self.retained_content_region(self.geometry_for(available), custody);
        }

        debug_assert!(
            !(self.layout == ShellLayout::TitlebarSidebarContent
                && self.sidebar_titlebar.is_some()
                && !self.tokens.sidebar_header.uses_sidebar_titlebar_slot()),
            "{SPANNING_BAND_DROPS_SIDEBAR_TITLEBAR}"
        );

        let resolved_geometry = self.geometry_for(available);
        let resolved_geometry = if motion.toggle_mounted {
            resolved_geometry
        } else {
            ShellGeometry {
                shows_toggle: false,
                ..resolved_geometry
            }
        };
        // Keep inline ancestry throughout reveal; width is spent by the live Reserve clock.
        let geometry = if motion.inline_mounted {
            let tokens = self.tokens_normalized();
            ShellGeometry {
                sidebar: SidebarPresentation::Inline,
                split_titlebar: splits_the_band(self.layout, tokens, SidebarPresentation::Inline),
                sidebar_full_height: sidebar_spans_full_height(
                    self.layout,
                    tokens,
                    SidebarPresentation::Inline,
                ),
                ..resolved_geometry
            }
        } else {
            resolved_geometry
        };

        debug_assert!(
            !(sidebar_toggle_assert_armed()
                && self.would_render_toggle(geometry)
                && self.on_sidebar_state.is_none()
                && self.on_toggle.is_none()),
            "{SIDEBAR_TOGGLE_UNROUTED}"
        );

        debug_assert!(
            !(sidebar_inert_assert_armed()
                && self.would_render_toggle(geometry)
                && self.sidebar_toggle_is_inert(geometry, available)),
            "{SIDEBAR_TOGGLE_INERT}"
        );

        let titlebar_content_gap = self.tokens_normalized().effective_titlebar_content_gutter();
        let seam_gap = self
            .tokens_normalized()
            .effective_sidebar_content_gutter(self.layout);

        let inner: Element<'a, Message> = match self.layout {
            ShellLayout::ContentOnly => {
                self.with_floating_window_controls(self.with_full_width_footer(
                    self.retained_content_region(geometry, custody),
                    geometry,
                ))
            }

            ShellLayout::TitlebarSidebarContent if geometry.sidebar_full_height => {
                let content_column: Element<'a, Message> = match self.titlebar.as_ref() {
                    Some(bar) => {
                        let bar_region = self.titlebar_region(
                            self.titlebar_band_with_background(
                                self.with_stable_toggle_overlap_clearance(
                                    geometry,
                                    &motion,
                                    self.titlebar_slot(bar, geometry),
                                ),
                                Length::Fill,
                                true,
                                Some(geometry),
                            ),
                            geometry,
                        );
                        let mut items: Vec<(String, Element<'a, Message>)> = Vec::with_capacity(3);
                        items.push((SHELL_SLOT_TITLEBAR.to_owned(), bar_region));
                        if let Some(notif) = self.notifications_band(geometry) {
                            items.push((SHELL_SLOT_NOTIFICATIONS.to_owned(), notif));
                        }
                        items.push((
                            SHELL_SLOT_CONTENT.to_owned(),
                            self.retained_content_region(geometry, custody),
                        ));
                        crate::keyed_column::keyed_column(items)
                            .spacing(titlebar_content_gap)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into()
                    }
                    None => self.retained_content_region(geometry, custody),
                };
                let right = self.with_full_width_footer(content_column, geometry);

                match self.sidebar_pane_with_header(
                    geometry,
                    &motion,
                    SidebarPanePresentation {
                        width: Length::Fixed(geometry.sidebar_width),
                        carries_toggle: true,
                        progress: motion.inline_progress,
                        live_clock: Some(motion.inline_clock()),
                        mode: crate::motion::PanelRevealMode::Reserve,
                    },
                ) {
                    Some(sidebar) => {
                        let rtl = self.tokens_normalized().is_rtl();
                        let logical = match self.seam_rule(geometry, &motion) {
                            Some(seam) => vec![sidebar, seam, right],
                            None => vec![sidebar, right],
                        };
                        crate::app_shell::mirror_row::MirrorRow::new(logical, rtl)
                            .spacing(seam_gap)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into()
                    }
                    None => right,
                }
            }

            ShellLayout::TitlebarContent | ShellLayout::TitlebarSidebarContent => {
                let body = self.body_row(geometry, &motion, custody);
                let body_and_footer = self.with_full_width_footer(body, geometry);
                let (scrim_region, flush_to_titlebar) =
                    self.prepend_subtitlebar(body_and_footer, geometry);
                let window: Element<'a, Message> = match self.titlebar_row(geometry, &motion) {
                    Some(bar) => {
                        let mut items: Vec<(String, Element<'a, Message>)> = Vec::with_capacity(3);
                        items.push((SHELL_SLOT_TITLEBAR.to_owned(), bar));
                        let has_notif = if let Some(notif) = self.notifications_band(geometry) {
                            items.push((SHELL_SLOT_NOTIFICATIONS.to_owned(), notif));
                            true
                        } else {
                            false
                        };
                        items.push((SHELL_SLOT_CONTENT.to_owned(), scrim_region));
                        // Notifications interrupt the shared titlebar–subtitlebar edge and require a gutter.
                        let spacing = if flush_to_titlebar && !has_notif {
                            0.0
                        } else {
                            titlebar_content_gap
                        };
                        crate::keyed_column::keyed_column(items)
                            .spacing(spacing)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into()
                    }
                    None => scrim_region,
                };
                // As a drawer, the riding toggle belongs to the titlebar under
                // the scrim; the drawer brings its own over it.
                let window = match self
                    .toggle_button(resolved_geometry)
                    .filter(|_| self.rides_under_the_drawer(resolved_geometry))
                {
                    Some(toggle) => stack![window, self.resting_rider(toggle)]
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into(),
                    None => window,
                };
                // The drawer and its scrim cover the whole window, titlebar
                // included; the window controls stay live above them.
                let drawer_active = self.state.effective_overlay_open(resolved_geometry.sidebar);
                // The window's own rounding clips its square corners.
                self.drawer_over(
                    window,
                    self.overlay_drawer(resolved_geometry, &motion),
                    drawer_active,
                    [0.0; 4],
                )
            }

            ShellLayout::FloatingSidebar => {
                let base = self.with_full_width_footer(
                    self.retained_content_region(geometry, custody),
                    geometry,
                );
                // FloatingSidebar is nonmodal: a card over content without a
                // scrim, inset alike on both sides however narrow the window.
                let inset = self.tokens_normalized().effective_edge_inset();
                // One width open, closing and closed: the card takes none of
                // the content's, so the inline share never narrows it.
                let width = self
                    .tokens_normalized()
                    .sidebar_width
                    .min((available - 2.0 * inset).max(0.0));
                let card = if self.chrome_visible && motion.overlay_mounted {
                    self.sidebar_pane_with_header(
                        geometry,
                        &motion,
                        SidebarPanePresentation {
                            width: Length::Fixed(width),
                            carries_toggle: true,
                            progress: motion.overlay_progress,
                            live_clock: Some(motion.overlay_clock()),
                            mode: crate::motion::PanelRevealMode::Overlay,
                        },
                    )
                } else {
                    None
                };
                // Closed, the toggle waits on a chip where the card's header
                // shows it; a showing card would let it through its glass.
                let showing = resolved_geometry.sidebar == SidebarPresentation::Inline
                    || self.state.effective_overlay_open(resolved_geometry.sidebar);
                let chip = if showing {
                    None
                } else {
                    self.toggle_chip(geometry)
                };
                let align = if self.tokens_normalized().is_rtl() {
                    iced::alignment::Horizontal::Right
                } else {
                    iced::alignment::Horizontal::Left
                };
                let place = |layer: Element<'a, Message>| -> Element<'a, Message> {
                    container(layer)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .align_x(align)
                        .into()
                };
                let mut layers = vec![base];
                layers.extend(chip.map(place));
                layers.extend(card.map(place));
                self.with_floating_window_controls(
                    iced::widget::Stack::with_children(layers)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into(),
                )
            }

            ShellLayout::FloatingTitlebar => {
                let base = self.with_full_width_footer(
                    self.retained_content_region(geometry, custody),
                    geometry,
                );
                // The band fades with `chrome_visible` rather than vanishing.
                match self.titlebar.as_ref() {
                    Some(bar) => {
                        let band = self.titlebar_band_with_background(
                            super::super::seam::RetainedResponsiveWidth::floating_titlebar_content(
                                self.titlebar_slot(bar, geometry),
                                titlebar_content_custody,
                            )
                            .into(),
                            Length::Fill,
                            true,
                            Some(geometry),
                        );
                        let band = super::super::seam::RetainedResponsiveWidth::floating_titlebar(
                            band,
                            titlebar_custody,
                        )
                        .into();
                        // A lone floating titlebar stands in from the content's edges.
                        self.floating_over(
                            base,
                            self.faded_floating_chrome(band),
                            self.tokens_normalized().effective_edge_inset(),
                        )
                    }
                    None => base,
                }
            }

            ShellLayout::FloatingTitlebarSidebar => {
                use super::super::seam::RetainedResponsiveWidth;
                let content_base = self.with_full_width_footer(
                    self.retained_content_region(geometry, custody),
                    geometry,
                );

                let bar_needs_toggle = !geometry.sidebar_full_height;

                let content_side: Element<'a, Message> = match (
                    self.chrome_visible || !bar_needs_toggle,
                    self.titlebar.as_ref(),
                ) {
                    // Hidden chrome fades in place; only a collapsed
                    // sidebar's recovery toggle replaces it below.
                    (true, Some(bar)) => {
                        let slot = RetainedResponsiveWidth::floating_titlebar_content(
                            self.titlebar_slot(bar, geometry),
                            titlebar_content_custody,
                        )
                        .into();
                        let slot = if bar_needs_toggle {
                            self.with_toggle(geometry, &motion, slot, true)
                        } else if self.toggle_has_stable_parent() {
                            // The riding toggle passes over this band's leading
                            // end as the sidebar folds; the title gives way.
                            self.with_stable_toggle_overlap_clearance(geometry, &motion, slot)
                        } else {
                            slot
                        };
                        let band = self.titlebar_band_with_background(
                            slot,
                            Length::Fill,
                            true,
                            Some(geometry),
                        );
                        let band =
                            RetainedResponsiveWidth::floating_titlebar(band, titlebar_custody)
                                .into();
                        // Beside a sidebar the band shares the sidebar header's top.
                        self.floating_over(content_base, self.faded_floating_chrome(band), 0.0)
                    }
                    // Auto-hidden or absent titlebars still need recovery chrome for a collapsed sidebar.
                    _ => match (bar_needs_toggle, self.toggle_button(geometry)) {
                        (true, Some(_)) => {
                            let band = self.titlebar_band_with_background(
                                row![self.with_toggle(
                                    geometry,
                                    &motion,
                                    iced::widget::Space::new().into(),
                                    true,
                                )]
                                .width(Length::Fill)
                                .into(),
                                Length::Fill,
                                true,
                                Some(geometry),
                            );
                            let band =
                                RetainedResponsiveWidth::floating_titlebar(band, titlebar_custody)
                                    .into();
                            self.floating_over(content_base, band, 0.0)
                        }
                        _ => content_base,
                    },
                };
                // As a drawer, the riding toggle belongs to the titlebar under
                // the scrim; the drawer brings its own over it.
                let content_side = match self
                    .toggle_button(resolved_geometry)
                    .filter(|_| self.rides_under_the_drawer(resolved_geometry))
                {
                    Some(toggle) => stack![content_side, self.resting_rider(toggle)]
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into(),
                    None => content_side,
                };
                // The drawer and its scrim cover the floating titlebar too;
                // the window controls stay live above them.
                let drawer_active = self.state.effective_overlay_open(resolved_geometry.sidebar);
                let content_side = self.drawer_over(
                    content_side,
                    self.overlay_drawer(resolved_geometry, &motion),
                    drawer_active,
                    geometry.content_corner_radii,
                );

                let sidebar = if geometry.sidebar_full_height {
                    self.sidebar_pane_with_header(
                        geometry,
                        &motion,
                        SidebarPanePresentation {
                            width: Length::Fixed(geometry.sidebar_width),
                            carries_toggle: true,
                            progress: motion.inline_progress,
                            live_clock: Some(motion.inline_clock()),
                            mode: crate::motion::PanelRevealMode::Reserve,
                        },
                    )
                } else {
                    None
                };
                match sidebar {
                    Some(sidebar) => {
                        let rtl = self.tokens_normalized().is_rtl();
                        let logical = match self.seam_rule(geometry, &motion) {
                            Some(seam) => vec![sidebar, seam, content_side],
                            None => vec![sidebar, content_side],
                        };
                        crate::app_shell::mirror_row::MirrorRow::new(logical, rtl)
                            .spacing(seam_gap)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into()
                    }
                    _ => content_side,
                }
            }
        };

        // The docked sidebar and the content beside it follow the seam as one
        // group. Each frame's layout places them, and the group publishes the
        // travel so the render lane presents its frames, each at the instant
        // it was laid out for; a frame the UI thread is late with is held, not
        // moved. Present for every layout with a sidebar beside content,
        // docked or not, so the tree keeps its shape across that change.
        let inner: Element<'a, Message> = if self.layout.sidebar_beside_content() {
            let docked = geometry.sidebar.is_inline();
            let extent = geometry.sidebar_width;
            let mirror = if self.tokens_normalized().is_rtl() {
                -1.0
            } else {
                1.0
            };
            crate::app_shell::seam_rider::SeamRider::new(
                inner,
                motion.inline_lane_clock(),
                motion.seam_layout_fraction(),
                move |fraction: f32| {
                    if docked {
                        mirror * extent * fraction
                    } else {
                        0.0
                    }
                },
            )
            .extrapolating()
            .into()
        } else {
            inner
        };

        // Keep one toggle parent above the sidebar clip while the sidebar reveal moves underneath it.
        let inner =
            if self.toggle_has_stable_parent() && !self.rides_under_the_drawer(resolved_geometry) {
                match self.toggle_button(resolved_geometry) {
                    Some(toggle) => match Self::toggle_declared_width(&toggle) {
                        Some(toggle_width) => {
                            let sidebar_width = resolved_geometry.sidebar_width;
                            let trailing = self
                                .tokens_normalized()
                                .effective_toggle_anchor(self.layout)
                                .is_trailing();
                            let rtl = self.tokens_normalized().is_rtl();
                            let lead = self.tokens_normalized().effective_toggle_lead();
                            let policy = iced::advanced::layout::inset::Policy::ToggleTrailing {
                                sidebar: if trailing { sidebar_width } else { 0.0 },
                                toggle: toggle_width + lead,
                            };
                            let mirror = if rtl { -1.0 } else { 1.0 };
                            // Outside the seam group, so the toggle publishes its own travel.
                            let ridden: Element<'a, Message> = if geometry.sidebar.is_inline() {
                                crate::app_shell::seam_rider::SeamRider::new(
                                    toggle,
                                    motion.inline_lane_clock(),
                                    motion.seam_layout_fraction(),
                                    move |fraction| mirror * policy.amount(fraction),
                                )
                                .extrapolating()
                                .into()
                            } else {
                                toggle
                            };
                            let riding = crate::app_shell::seam::LiveSeamPad::driven(
                                ridden,
                                policy,
                                std::rc::Rc::clone(&motion.inline_live_progress),
                                motion.docked_driver,
                            )
                            .trailing_edge(rtl);
                            let rider: Element<'a, Message> = container(riding)
                                .width(Length::Fill)
                                .height(Length::Fixed(self.tokens_normalized().titlebar_height))
                                .align_y(iced::alignment::Vertical::Center)
                                .padding(if rtl {
                                    Padding::ZERO.right(lead)
                                } else {
                                    Padding::ZERO.left(lead)
                                })
                                .into();
                            stack![inner, rider]
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .into()
                        }
                        None => inner,
                    },
                    None => inner,
                }
            } else {
                inner
            };

        // A glass frame is the compositor's glass where it paints it, tinted
        // with the shell tone, and the shell tone everywhere else.
        let mut window_layer = enz_surface(inner);
        if let Some(material) = self.frame_material() {
            let tone = self.tokens_normalized().shell_tone;
            window_layer = window_layer
                .material(material)
                .material_tokens(enzui_theme::window_frame::frame_tokens(material))
                // The frame surrounds independently styled panes; their geometry owns padding.
                .material_ink(enzui_theme::MaterialInk::Strong)
                .fill_resolved(move |theme| {
                    crate::surface::SurfaceTokens::from_palette(&crate::theme_lookup::palette(
                        theme,
                    ))
                    .fill_for(tone)
                })
                .backdrop(crate::desktop_glass::Backdrop::Window)
                .window_frame(true);
        }
        let window_layer = window_layer
            .tone(self.tokens_normalized().shell_tone)
            .radius(self.tokens_normalized().window_radius)
            .padding(self.tokens_normalized().effective_edge_inset())
            .clip(true)
            .width(Length::Fill)
            .height(Length::Fill)
            .into_element();
        // A faint light line just inside the edge of a free-floating dark
        // window keeps it apart from a dark window behind; the dark line just
        // outside is part of the shadow the platform casts.
        let radius = self.tokens_normalized().window_radius;
        let highlight = enzui_theme::text_defaults::window_shadow().highlight;
        let edge = iced::widget::container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |theme: &iced::Theme| {
                let dark = crate::theme_lookup::palette(theme).is_dark();
                let alpha = if dark && radius > 0.0 { highlight } else { 0.0 };
                iced::widget::container::Style {
                    border: iced::Border {
                        color: iced::Color {
                            a: alpha,
                            ..iced::Color::WHITE
                        },
                        width: if alpha > 0.0 { 1.0 } else { 0.0 },
                        radius: radius.into(),
                    },
                    ..iced::widget::container::Style::default()
                }
            });
        let window_layer = iced::widget::stack![window_layer, edge]
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        self.with_window_resize_frame(window_layer)
    }

    /// Whether the riding toggle sits in the titlebar under a drawer rather
    /// than above everything: while the sidebar is a drawer.
    fn rides_under_the_drawer(&self, geometry: ShellGeometry) -> bool {
        self.toggle_has_stable_parent() && geometry.sidebar == SidebarPresentation::Overlay
    }

    /// The riding toggle at rest at the titlebar's leading end.
    fn resting_rider(&self, toggle: Element<'a, Message>) -> Element<'a, Message> {
        let tokens = self.tokens_normalized();
        let lead = tokens.effective_toggle_lead();
        container(toggle)
            .width(Length::Fill)
            .height(Length::Fixed(tokens.titlebar_height))
            .align_y(iced::alignment::Vertical::Center)
            .align_x(if tokens.is_rtl() {
                iced::alignment::Horizontal::Right
            } else {
                iced::alignment::Horizontal::Left
            })
            .padding(if tokens.is_rtl() {
                Padding::ZERO.right(lead)
            } else {
                Padding::ZERO.left(lead)
            })
            .into()
    }

    /// A titlebar-less layout's window controls, floating at the top trailing
    /// corner over `below`. The layer stays mounted while the controls change,
    /// so `below` keeps its place in the tree.
    fn with_floating_window_controls(&self, below: Element<'a, Message>) -> Element<'a, Message> {
        // The window layer already insets this layer from the window's edge.
        let controls = self
            .lifted_window_controls()
            .unwrap_or_else(|| Space::new().into());
        stack![below, controls]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Whether the window frame, controls and resize edges belong to someone
    /// else: the layout asks for server decorations, or the compositor drew
    /// them although the shell offered its own.
    pub(crate) fn draws_no_window_chrome(&self) -> bool {
        self.layout.window_decorations() == WindowDecorations::ServerSide
            || self.desktop.server_side
    }

    fn with_window_resize_frame(&self, window_layer: Element<'a, Message>) -> Element<'a, Message> {
        if self.draws_no_window_chrome() {
            return window_layer;
        }
        let enabled = self.resizable
            && !self.maximized
            && !self.fullscreen
            && !self.desktop.maximized
            && !self.desktop.fullscreen;
        // Flush chrome occupies edge hit boxes; its outer resize sliver must win over child controls.
        let flush_chrome = self.tokens_normalized().effective_edge_inset()
            < crate::window_resize_frame::DEFAULT_RESIZE_SLIVER_THICKNESS;
        let frame = crate::window_resize_frame::window_resize_frame(window_layer)
            .enabled(enabled)
            .tiled(self.desktop.tiled)
            .flush_chrome(flush_chrome);
        match self.on_window_gesture.as_ref() {
            Some(gesture) => {
                let gesture = std::rc::Rc::clone(gesture);
                frame.on_gesture(move |command| gesture(command)).into()
            }
            None => frame.into(),
        }
    }

    /// Build a width-responsive shell element.
    ///
    /// Slots are evaluated during layout at the assigned width. Hidden panes
    /// are omitted, and mounted content retains its widget state across rebuilds.
    ///
    /// # Panics
    ///
    /// In debug builds, supplying a sidebar titlebar to `TitlebarSidebarContent`
    /// with `SidebarHeader::SpanningBand` panics because that placement drops it.
    pub fn into_element(mut self) -> Element<'a, Message> {
        // The shell builds its slots during layout, after the view: read the
        // window's decorations now, while its view is being built. A window
        // with its own frame asks for no decorations; a compositor that
        // decorates it anyway gets no second set of window controls.
        self.desktop = iced::window::decorations();
        // Square corners wherever the window does not float free: framed by
        // the compositor, maximized, fullscreen or tiled.
        if self.draws_no_window_chrome()
            || self.maximized
            || self.fullscreen
            || self.desktop.maximized
            || self.desktop.fullscreen
            || self.desktop.tiled.any()
        {
            self.tokens.window_radius = 0.0;
        }
        crate::window_frame_clip::publish(
            self.tokens_normalized().window_radius,
            !self.draws_no_window_chrome(),
        );
        crate::window_frame_clip::publish_glass(self.frame_material().is_some());
        debug_assert!(
            !(self.layout == ShellLayout::TitlebarSidebarContent
                && self.sidebar_titlebar.is_some()
                && !self.tokens.sidebar_header.uses_sidebar_titlebar_slot()),
            "{SPANNING_BAND_DROPS_SIDEBAR_TITLEBAR}"
        );
        Element::new(AppShellWidget {
            // Diff is deferred until layout replaces this seed at the real width, avoiding a zero-width build.
            content: Element::new(iced::widget::Space::new()),
            shell: self,
            built_this_cycle: false,
        })
    }
}
