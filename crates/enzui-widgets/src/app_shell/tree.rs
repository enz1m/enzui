//! Shell pane composition, retained boundaries, and overlay input routing.

use super::*;
use iced::widget::column; // Explicit import disambiguates column! from the glob.

struct SidebarPanePresentation {
    width: Length,
    carries_toggle: bool,
    progress: f32,
    live_clock: Option<crate::motion::RevealClock>,
    mode: crate::motion::PanelRevealMode,
}

const ISLAND_INSET: f32 = 4.0;
/// The narrow drawer's lift off the dimmed window under it.
const DRAWER_ELEVATION: u8 = 4;
pub(crate) const ISLAND_HEIGHT: f32 =
    crate::window_controls::DEFAULT_CONTROL_SIZE + 2.0 * ISLAND_INSET;
pub(crate) const ISLAND_MARGIN: f32 = 8.0;

impl<'a, Message: Clone + 'a> EnzAppShell<'a, Message> {
    // Raw public token fields bypass setters; normalize before constructing layout.
    fn tokens_normalized(&self) -> ShellTokens {
        self.effective_tokens().normalized()
    }

    fn reveal_panel(
        &self,
        panel: Element<'a, Message>,
        progress: f32,
        live_clock: Option<crate::motion::RevealClock>,
        resting_width: Option<f32>,
        mode: crate::motion::PanelRevealMode,
    ) -> Element<'a, Message> {
        let edge = if self.tokens_normalized().is_rtl() {
            crate::motion::PanelRevealEdge::Trailing
        } else {
            crate::motion::PanelRevealEdge::Leading
        };
        // An inset pane slides out across the window's margin, not cut at its edge.
        let reveal = crate::motion::PanelReveal::new(panel, progress, resting_width, mode)
            .cache_layout_while_revealing()
            .enters_from(edge)
            .overhang(self.tokens_normalized().effective_edge_inset());
        match live_clock {
            Some(clock) => reveal.live_clock(clock).into(),
            None => reveal.into(),
        }
    }

    fn pane(
        &self,
        body: Element<'a, Message>,
        style: PaneStyle,
        width: Length,
        height: Length,
    ) -> Element<'a, Message> {
        self.pane_raised(body, style, width, height, 0)
    }

    /// A bar's pane (titlebar, subtitlebar, footer): over a glass frame a
    /// blended bar stays clear, showing the content it floats over or the
    /// glass it sits on.
    fn bar_pane(
        &self,
        body: Element<'a, Message>,
        style: PaneStyle,
        width: Length,
        height: Length,
    ) -> Element<'a, Message> {
        self.pane_on_frame(body, style, width, height, 0, false)
    }

    fn pane_raised(
        &self,
        body: Element<'a, Message>,
        style: PaneStyle,
        width: Length,
        height: Length,
        elevation: u8,
    ) -> Element<'a, Message> {
        self.pane_on_frame(body, style, width, height, elevation, true)
    }

    /// A pane; `filled` keeps a blended one filled over a glass frame.
    fn pane_on_frame(
        &self,
        body: Element<'a, Message>,
        style: PaneStyle,
        width: Length,
        height: Length,
        elevation: u8,
        filled: bool,
    ) -> Element<'a, Message> {
        let mut surface = enz_surface(body).elevation(elevation);
        if let Some(material) = style.material {
            surface = surface
                .material(material)
                .backdrop(style.backdrop)
                .material_ink(enzui_theme::MaterialInk::Strong);
        }
        let tokens = self.tokens_normalized();
        let corners = style.radius_corners_or(tokens.pane_radius);
        let pane = surface
            .grain(style.grain)
            .tone(style.tone)
            .border_style(style.border)
            .radius_corners(corners)
            // Full-bleed slot content must remain inside the pane’s rounded corners.
            .clip(true)
            .width(width)
            .height(height)
            .into_element();
        // Over a glass frame a filled blended pane keeps what an opaque frame
        // showed through it: the frame's tone, under the pane rather than as
        // its tone, so what floats over the pane still shows it.
        let blended = style.material.is_none() && style.tone == SurfaceTone::Transparent;
        let backed = filled && blended && self.frame_material().is_some();
        // The backing is always there, so a frame turning to glass never
        // re-parents the pane.
        let pane = enz_surface(pane)
            .tone(if backed {
                tokens.shell_tone
            } else {
                SurfaceTone::Transparent
            })
            .radius_corners(corners)
            .width(width)
            .height(height)
            .into_element();
        // An opaque pane hides the glass frame beneath it.
        let opaque = style.material.is_none() && (backed || !blended);
        crate::desktop_glass::cutout(pane, corners.into_iter().fold(0.0, f32::max), opaque)
    }

    /// The window frame's material: the theme's `window.frame` decides over
    /// the app's [`ShellTokens::frame_material`].
    pub(crate) fn frame_material(&self) -> Option<enzui_theme::MaterialStyle> {
        enzui_theme::text_defaults::window_frame().resolve(self.tokens.frame_material)
    }

    fn seam_rule(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
    ) -> Option<Element<'a, Message>> {
        let tokens = self.tokens_normalized();
        let seam = tokens.effective_seam();
        if !seam.draws_rule() || !geometry.sidebar.is_inline() {
            return None;
        }
        let rule = crate::divider::enz_divider()
            .vertical()
            .thickness(tokens.effective_seam_thickness());
        // Flat reserves a slot without ink, even when the caller supplies a color.
        let rule = match tokens.seam_color {
            Some(color) if seam == SeamStyle::Hairline => rule.color(color),
            _ => rule,
        };
        let rule: Element<'a, Message> = match seam {
            SeamStyle::Flat => rule.color(iced::Color::TRANSPARENT).into_element(),
            SeamStyle::Hairline => rule.into_element(),
            SeamStyle::None => unreachable!("None was rejected by draws_rule"),
        };
        Some(self.seam_rider(rule, geometry, motion, {
            let extent = geometry.sidebar_width;
            let mirror = if self.tokens_normalized().is_rtl() {
                -1.0
            } else {
                1.0
            };
            move |fraction: f32| mirror * extent * fraction
        }))
    }

    fn seam_rider(
        &self,
        chrome: Element<'a, Message>,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        offset_at: impl Fn(f32) -> f32 + 'a,
    ) -> Element<'a, Message> {
        if !geometry.sidebar.is_inline() {
            return chrome;
        }

        crate::app_shell::seam_rider::SeamRider::new(
            chrome,
            motion.inline_clock(),
            motion.seam_layout_fraction(),
            offset_at,
        )
        .into()
    }

    fn content_pane(&self, geometry: ShellGeometry) -> Element<'a, Message> {
        let tokens = self.tokens_normalized();
        let page: Element<'a, Message> = container(crate::layout_memo::LayoutMemo::new(
            self.content.build(&geometry),
        ))
        .padding(Padding::ZERO.top(if self.immersive_fullscreen() {
            0.0
        } else {
            tokens.effective_page_inset()
        }))
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
        // The main content is the window's main landmark.
        SemanticsContainer::new(
            self.pane(
                page,
                if self.fullscreen || self.desktop.fullscreen {
                    PaneStyle {
                        radius: Some(0.0),
                        radius_corners: Some([0.0; 4]),
                        border: BorderStyle::None,
                        ..tokens.content
                    }
                } else {
                    tokens.content
                },
                Length::Fill,
                Length::Fill,
            ),
            Semantics::new(Role::Main),
        )
        .into()
    }

    fn secondary_content_pane(&self, geometry: ShellGeometry) -> Option<Element<'a, Message>> {
        let secondary = self.secondary_content.as_ref()?;
        let tokens = self.tokens_normalized();
        // Width follows the requested axis; restacking keeps a requested beside width.
        let width = match (tokens.split_axis.is_horizontal(), tokens.secondary_width) {
            (true, Some(w)) => Length::Fixed(w),
            _ => Length::Fill,
        };
        let page: Element<'a, Message> = container(crate::layout_memo::LayoutMemo::new(
            secondary.build(&geometry),
        ))
        .padding(Padding::ZERO.top(tokens.effective_page_inset()))
        .width(Length::Fill)
        .height(Length::Fill)
        .into();
        Some(self.pane(page, tokens.secondary_content, width, Length::Fill))
    }

    pub(crate) fn activity_placement(&self, geometry: ShellGeometry) -> ActivityPlacement {
        if self.activity.is_none() {
            return ActivityPlacement::None;
        }
        if self.sidebar.is_none() {
            return ActivityPlacement::ContentBottomLeft;
        }
        match geometry.sidebar {
            SidebarPresentation::Inline => ActivityPlacement::SidebarBottom,
            SidebarPresentation::Overlay => {
                if self.state.effective_overlay_open(geometry.sidebar) {
                    ActivityPlacement::SidebarBottom
                } else {
                    ActivityPlacement::ContentBottomLeft
                }
            }
            SidebarPresentation::Hidden => ActivityPlacement::ContentBottomLeft,
        }
    }

    fn activity_surface(
        &self,
        geometry: ShellGeometry,
        site: ActivityPlacement,
    ) -> Option<Element<'a, Message>> {
        if self.immersive_fullscreen() || self.activity_placement(geometry) != site {
            return None;
        }
        let activity = self.activity.as_ref()?;
        // An `ActivityPresentation::Auto` surface built here reads the site:
        // compact at the sidebar's bottom, detailed when floating.
        crate::activity_surface::with_shell_placement(site, || activity(&geometry))
    }

    fn sidebar_slot_body(
        &self,
        sidebar: &ShellSlot<'a, Message>,
        geometry: ShellGeometry,
    ) -> Element<'a, Message> {
        let body: Element<'a, Message> =
            crate::layout_memo::LayoutMemo::new(crate::title_bar::with_sidebar_pane(|| {
                sidebar.build(&geometry)
            }))
            .into();
        if self.activity.is_none() {
            return body;
        }
        // Keep the bottom wrapper while idle so activity changes retain sidebar ancestry.
        let bottom: Element<'a, Message> =
            match self.activity_surface(geometry, ActivityPlacement::SidebarBottom) {
                Some(surface) => container(surface)
                    .padding(Padding::ZERO.top(self.tokens_normalized().effective_pane_gutter()))
                    .width(Length::Fill)
                    .into(),
                None => container(Space::new().width(0).height(0))
                    .width(Length::Fill)
                    .into(),
            };
        column![
            container(body).width(Length::Fill).height(Length::Fill),
            bottom,
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
    }

    fn activity_over(
        &self,
        region: Element<'a, Message>,
        geometry: ShellGeometry,
    ) -> Element<'a, Message> {
        if self.activity.is_none() {
            return region;
        }
        // Activity may disappear without moving the content subtree to a new parent.
        let mut layers: Vec<Element<'a, Message>> = vec![region];
        if let Some(surface) = self.activity_surface(geometry, ActivityPlacement::ContentBottomLeft)
        {
            let inset = self.tokens_normalized().effective_edge_inset();
            // Leave content exposed beside floating activity, including on narrow windows.
            let width = geometry
                .sidebar_width
                .min(
                    (geometry.content_width - 2.0 * inset).max(0.0)
                        * FLOATING_ACTIVITY_CONTENT_FRACTION,
                )
                .max(0.0);
            let bounded: Element<'a, Message> = container(surface)
                .width(Length::Fixed(width))
                .height(Length::Shrink)
                .into();
            let swallowing: Element<'a, Message> =
                crate::swallow_press::SwallowPress::new(bounded).into();
            let sidebar_align = if self.tokens_normalized().is_rtl() {
                iced::alignment::Horizontal::Right
            } else {
                iced::alignment::Horizontal::Left
            };
            layers.push(
                container(swallowing)
                    .padding(inset)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(sidebar_align)
                    .align_y(iced::alignment::Vertical::Bottom)
                    .into(),
            );
        }
        iced::widget::Stack::with_children(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn content_region(&self, geometry: ShellGeometry) -> Element<'a, Message> {
        self.activity_over(self.content_region_body(geometry), geometry)
    }

    fn content_region_body(&self, geometry: ShellGeometry) -> Element<'a, Message> {
        let primary = self.content_pane(geometry);
        if !geometry.secondary_content_fits {
            return primary;
        }
        let Some(secondary) = self.secondary_content_pane(geometry) else {
            return primary;
        };

        let tokens = self.tokens_normalized();
        let gap = tokens.effective_pane_gutter();
        let (first, second) = if tokens.secondary_leading {
            (secondary, primary)
        } else {
            (primary, secondary)
        };
        // Restacking resolves an axis independently of the requested token.
        if geometry.split_axis.is_horizontal() {
            row![first, second]
                .spacing(gap)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        } else {
            column![first, second]
                .spacing(gap)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        }
    }

    fn retained_content_region(
        &self,
        geometry: ShellGeometry,
        custody: &super::seam::ContentTree,
    ) -> Element<'a, Message> {
        let content = self.content_region(geometry);
        // Every layout variant must accept the transferred tree before diffing its child.
        crate::app_shell::seam::RetainedResponsiveWidth::new(content)
            .retaining(custody)
            .into()
    }

    fn footer_pane(&self, width: Length, geometry: ShellGeometry) -> Option<Element<'a, Message>> {
        let footer = self.footer.as_ref()?;
        Some(self.bar_pane(
            Self::centred_in_band(footer.build(&geometry)),
            self.tokens_normalized().footer,
            width,
            Length::Shrink,
        ))
    }

    fn with_full_width_footer(
        &self,
        above: Element<'a, Message>,
        geometry: ShellGeometry,
    ) -> Element<'a, Message> {
        match self.footer_pane(Length::Fill, geometry) {
            Some(footer) => column![above, footer]
                .spacing(self.tokens_normalized().effective_content_footer_gutter())
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
            None => above,
        }
    }

    fn sidebar_pane(
        &self,
        height: Length,
        geometry: ShellGeometry,
        progress: f32,
        live_clock: Option<crate::motion::RevealClock>,
        mode: crate::motion::PanelRevealMode,
    ) -> Option<Element<'a, Message>> {
        let sidebar = self.sidebar.as_ref()?;
        let pane = self.pane(
            self.sidebar_slot_body(sidebar, geometry),
            self.sidebar_pane_style(mode),
            Length::Fixed(geometry.sidebar_width),
            height,
        );
        let landmark: Element<'a, Message> = SemanticsContainer::new(
            pane,
            Semantics::new(Role::Navigation).label(self.sidebar_label.clone()),
        )
        .into();
        // Capture stays inside reveal gating so an exiting drawer cannot swallow base presses.
        let landmark: Element<'a, Message> =
            crate::swallow_press::SwallowPress::new(landmark).into();
        Some(self.reveal_panel(
            landmark,
            progress,
            live_clock,
            Some(geometry.sidebar_width),
            mode,
        ))
    }

    fn sidebar_pane_style(&self, mode: crate::motion::PanelRevealMode) -> PaneStyle {
        let tokens = self.tokens_normalized();
        let style = tokens.sidebar;
        if mode != crate::motion::PanelRevealMode::Overlay {
            return style;
        }
        let flush = tokens.effective_edge_inset() <= 0.0;
        let pane = style.radius_or(tokens.pane_radius);
        let (edge, free) = if flush {
            (tokens.window_radius, 0.0)
        } else {
            (pane, pane)
        };
        // Stacked titlebars meet the drawer directly; floating titlebars do not bound its top.
        let (edge_top, free_top) = if self.layout.floats_titlebar() {
            (edge, free)
        } else {
            (0.0, 0.0)
        };
        let corners = if tokens.is_rtl() {
            [free_top, edge_top, free, edge]
        } else {
            [edge_top, free_top, edge, free]
        };
        style.radius_corners(corners)
    }

    /// A closed floating sidebar's toggle on a chip of the sidebar's surface,
    /// a band-square placed where the open card's header shows the toggle.
    pub(crate) fn toggle_chip(&self, geometry: ShellGeometry) -> Option<Element<'a, Message>> {
        let toggle = self.toggle_button(geometry)?;
        let band = self.tokens_normalized().titlebar_height;
        let chip = container(toggle)
            .width(Length::Fixed(band))
            .height(Length::Fixed(band))
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center);
        Some(self.pane_raised(
            chip.into(),
            self.drawer_pane_style(),
            Length::Shrink,
            Length::Shrink,
            DRAWER_ELEVATION,
        ))
    }

    /// The narrow drawer's surface: it covers the titlebar, so it takes the
    /// band's tone and material and rounds the window corners it reaches.
    fn drawer_pane_style(&self) -> PaneStyle {
        let tokens = self.tokens_normalized();
        let band = tokens.titlebar;
        let tone = if band.tone == SurfaceTone::Transparent {
            tokens.shell_tone
        } else {
            band.tone
        };
        let flush = tokens.effective_edge_inset() <= 0.0;
        let pane = tokens.sidebar.radius_or(tokens.pane_radius);
        let (edge, free) = if flush {
            (tokens.window_radius, 0.0)
        } else {
            (pane, pane)
        };
        let corners = if tokens.is_rtl() {
            [free, edge, free, edge]
        } else {
            [edge, free, edge, free]
        };
        // The sidebar's own material wins; otherwise it shares the band's.
        let (material, backdrop) = match tokens.sidebar.material {
            Some(material) => (Some(material), tokens.sidebar.backdrop),
            None => (band.material, band.backdrop),
        };
        PaneStyle {
            tone,
            material,
            backdrop,
            ..tokens.sidebar
        }
        .radius_corners(corners)
    }

    fn sidebar_pane_with_header(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        presentation: SidebarPanePresentation,
    ) -> Option<Element<'a, Message>> {
        let SidebarPanePresentation {
            width,
            carries_toggle,
            progress,
            live_clock,
            mode,
        } = presentation;
        let sidebar = self.sidebar.as_ref()?;
        let drawer = mode == crate::motion::PanelRevealMode::Overlay;
        let style = if drawer {
            self.drawer_pane_style()
        } else {
            self.sidebar_pane_style(mode)
        };
        let band_height = self.tokens_normalized().titlebar_height;

        // The pane paints the header's tone with its own corners and
        // material; a band of its own would square them off.
        let band_tone = SurfaceTone::Transparent;
        // A drawer covers the titlebar, so a named sidebar titles it.
        let header: Element<'a, Message> = match self.sidebar_titlebar.as_ref() {
            Some(bar) => Self::in_band(band_tone, band_height, || {
                crate::title_bar::with_pane_header(
                    self.pane_header_toggle(geometry, drawer),
                    || bar.build(&geometry),
                )
            }),
            None if drawer && self.sidebar_label_named => {
                Self::in_band(band_tone, band_height, || {
                    crate::title_bar::with_pane_header(
                        self.pane_header_toggle(geometry, drawer),
                        || {
                            crate::title_bar::enz_title_bar()
                                .title(self.sidebar_label.clone())
                                .into_element()
                        },
                    )
                })
            }
            None => iced::widget::Space::new().into(),
        };
        let header = match self.toggle_button(geometry).filter(|_| drawer) {
            // A drawer brings its own toggle, over the titlebar's dimmed one
            // and exactly where that sits: so nothing overlaps while it slides.
            Some(toggle) => {
                let tokens = self.tokens_normalized();
                let lead = if self.toggle_has_stable_parent() {
                    tokens.effective_toggle_lead()
                } else if self.layout == ShellLayout::FloatingSidebar
                    && tokens.effective_edge_inset() > 0.0
                {
                    // Centred in a band-square at the card's corner, where the
                    // closed card's chip shows it.
                    Self::toggle_declared_width(&toggle)
                        .map_or(0.0, |width| ((band_height - width) / 2.0).max(0.0))
                } else {
                    0.0
                };
                let body: Element<'a, Message> = container(header).width(Length::Fill).into();
                let rtl = tokens.is_rtl();
                let composed = if rtl {
                    row![body, toggle]
                } else {
                    row![toggle, body]
                };
                container(
                    composed
                        .spacing(tokens.effective_pane_gutter())
                        .align_y(iced::alignment::Vertical::Center),
                )
                .padding(if rtl {
                    Padding::ZERO.right(lead)
                } else {
                    Padding::ZERO.left(lead)
                })
                .into()
            }
            None if carries_toggle => self.with_toggle(geometry, motion, header, false),
            // A header under the shell's riding toggle reserves its place.
            None => self.with_toggle_clearance(geometry, motion, header),
        };
        let centred_header = Self::centred_in_band(header);
        let header_with_drag: Element<'a, Message> = match self.on_window_gesture.as_ref() {
            Some(on_gesture) => {
                let on_gesture = std::rc::Rc::clone(on_gesture);
                titlebar_drag_region(centred_header)
                    .on_gesture(move |command| (on_gesture)(command))
                    .into()
            }
            None => centred_header,
        };
        let body = column![
            container(header_with_drag)
                .width(Length::Fill)
                .height(Length::Fixed(self.tokens_normalized().titlebar_height)),
            self.sidebar_slot_body(sidebar, geometry),
        ]
        .width(Length::Fill)
        .height(Length::Fill);

        let pane = self.pane_raised(
            body.into(),
            style,
            width,
            Length::Fill,
            if drawer { DRAWER_ELEVATION } else { 0 },
        );
        let landmark: Element<'a, Message> = SemanticsContainer::new(
            pane,
            Semantics::new(Role::Navigation).label(self.sidebar_label.clone()),
        )
        .into();
        // Capture stays inside reveal gating so an exiting drawer cannot swallow base presses.
        let landmark: Element<'a, Message> =
            crate::swallow_press::SwallowPress::new(landmark).into();
        let resting_width = match width {
            Length::Fixed(width) => Some(width),
            _ => None,
        };
        Some(self.reveal_panel(landmark, progress, live_clock, resting_width, mode))
    }

    // Fill-height slots have no slack to center and retain their own internal alignment.
    fn centred_in_band(body: Element<'a, Message>) -> Element<'a, Message> {
        container(body)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Center)
            .into()
    }

    pub(crate) fn window_controls_element(&self) -> Option<Element<'a, Message>> {
        // Right to left, the cluster's order mirrors, as a titlebar's does.
        let rtl = self.tokens_normalized().is_rtl();
        self.window_controls_with(|wc| wc.mirrored(rtl))
    }

    fn window_controls_with(
        &self,
        finish: impl FnOnce(
            crate::window_controls::EnzWindowControls<'a, Message>,
        ) -> crate::window_controls::EnzWindowControls<'a, Message>,
    ) -> Option<Element<'a, Message>> {
        let tokens = &self.tokens;
        // Explicit messages win; otherwise the window-gesture route supplies
        // all three, so `.on_window_gesture(..)` alone gives a window its
        // controls.
        let gesture = self.on_window_gesture.as_ref();
        let close_msg = if tokens.show_close {
            self.on_close
                .clone()
                .or_else(|| gesture.map(|g| g(WindowCommand::Close)))
        } else {
            None
        };
        // The controls builder requires close; minimize and maximize cannot mount independently.
        let close_msg = close_msg?;
        // The compositor drew the window's own frame, controls included.
        if self.desktop.server_side {
            return None;
        }

        use crate::window_controls::{WindowControlKind, frame_shows};
        // One shell per window, and accessibility routes per window, so a
        // fixed owner is unique where it is resolved.
        let mut wc = enz_window_controls(close_msg).id("app-shell");
        if let Some(delay) = self.tooltip_delay {
            wc = wc.tooltip_delay(delay);
        }

        // The theme file decides which buttons show, then the desktop's
        // button layout, then the shell tokens.
        if let Some(msg) = self
            .on_minimize
            .clone()
            .or_else(|| gesture.map(|g| g(WindowCommand::Minimize)))
            && frame_shows(WindowControlKind::Minimize, tokens.show_minimize)
        {
            wc = wc.show_minimize(true, msg);
        }
        if let Some(msg) = self
            .on_maximize
            .clone()
            .or_else(|| gesture.map(|g| g(WindowCommand::ToggleMaximize)))
            && frame_shows(WindowControlKind::Maximize, tokens.show_maximize)
        {
            wc = wc.show_maximize(true, msg, self.maximized || self.desktop.maximized);
        }

        if wc.control_count() == 0 {
            return None;
        }

        Some(finish(wc).into_element())
    }

    fn titlebar_content_with_controls(&self, body: Element<'a, Message>) -> Element<'a, Message> {
        if let Some(style) = self.island_material {
            return self.titlebar_content_with_island(body, style);
        }
        match self.window_controls_element() {
            Some(controls) => {
                let trailing = crate::window_controls::trailing_inset_for_square_corner(
                    self.tokens_normalized().titlebar_height,
                    crate::window_controls::DEFAULT_CONTROL_SIZE,
                );
                // The desktop's button layout decides the side, at the start
                // or the end of the writing direction, as in a titlebar.
                let lead =
                    crate::window_controls::controls_lead() != self.tokens_normalized().is_rtl();
                let ctrl_centred: Element<'a, Message> = container(controls)
                    .height(Length::Fill)
                    .align_y(iced::alignment::Vertical::Center)
                    .padding(if lead {
                        iced::Padding::ZERO.left(trailing)
                    } else {
                        iced::Padding::ZERO.right(trailing)
                    })
                    .into();
                let row = if lead {
                    row![ctrl_centred, body]
                } else {
                    row![body, ctrl_centred]
                };
                row.spacing(self.tokens_normalized().effective_pane_gutter())
                    .width(Length::Fill)
                    .align_y(iced::Alignment::Center)
                    .into()
            }
            // Without controls the band's far end stands off the window edge
            // by the lead a flush shell gives its near end.
            None => {
                let tokens = self.tokens_normalized();
                let lead = tokens.effective_toggle_lead();
                container(body)
                    .padding(if tokens.is_rtl() {
                        iced::Padding::ZERO.left(lead)
                    } else {
                        iced::Padding::ZERO.right(lead)
                    })
                    .width(Length::Fill)
                    .into()
            }
        }
    }

    fn titlebar_content_with_island(
        &self,
        body: Element<'a, Message>,
        style: enzui_theme::MaterialStyle,
    ) -> Element<'a, Message> {
        let height = ISLAND_HEIGHT;
        let mut cluster = row![]
            .spacing(ISLAND_INSET)
            .align_y(iced::Alignment::Center);
        if let Some(actions) = &self.island_actions {
            cluster = cluster.push(actions());
        }
        if let Some(controls) = self
            .window_controls_with(|wc| wc.ghost(true).mirrored(self.tokens_normalized().is_rtl()))
        {
            cluster = cluster.push(controls);
        }
        let mut island = crate::glass::enz_glass(
            container(cluster)
                .height(height)
                .align_y(iced::alignment::Vertical::Center)
                .padding([0.0, ISLAND_INSET]),
        )
        .style(style)
        .radius(height / 2.0);
        if let Some(ink) = self.island_ink {
            island = island.ink(ink);
        }
        let trailing = crate::window_controls::trailing_inset_for_square_corner(
            self.tokens_normalized().titlebar_height,
            height,
        )
        .max(ISLAND_MARGIN);
        // The island sits at the trailing edge: the left one right-to-left.
        let rtl = self.tokens_normalized().is_rtl();
        let inset = if rtl {
            iced::Padding::ZERO.left(trailing)
        } else {
            iced::Padding::ZERO.right(trailing)
        };
        let placed: Element<'a, Message> = container(island)
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Center)
            .padding(inset)
            .into();
        // Mirror placement without reversing logical focus and accessibility order.
        crate::app_shell::mirror_row::MirrorRow::new(vec![body, placed], rtl)
            .spacing(self.tokens_normalized().effective_pane_gutter())
            .width(Length::Fill)
            .into()
    }

    /// The side of a sidebar's header the toggle sits on, as `with_toggle`,
    /// `with_toggle_clearance` and a drawer place it, so the header can keep
    /// its own content one icon gap away from it.
    fn pane_header_toggle(
        &self,
        geometry: ShellGeometry,
        drawer: bool,
    ) -> crate::title_bar::PaneHeaderToggle {
        use crate::app_shell::ToggleAnchor;
        use crate::title_bar::PaneHeaderToggle;
        if drawer {
            return PaneHeaderToggle::Leading;
        }
        match self
            .tokens_normalized()
            .effective_toggle_anchor(self.layout)
        {
            ToggleAnchor::None | ToggleAnchor::FloatingOverContent => PaneHeaderToggle::None,
            anchor if anchor.is_trailing() && geometry.sidebar.is_inline() => {
                PaneHeaderToggle::Trailing
            }
            _ => PaneHeaderToggle::Leading,
        }
    }

    fn sidebar_titlebar_band(
        &self,
        body: Element<'a, Message>,
        width: Length,
    ) -> Element<'a, Message> {
        self.titlebar_band_with_background(body, width, false, None)
    }

    fn titlebar_band_with_background(
        &self,
        body: Element<'a, Message>,
        width: Length,
        with_controls: bool,
        geometry: Option<ShellGeometry>,
    ) -> Element<'a, Message> {
        let content = if with_controls {
            self.titlebar_content_with_controls(body)
        } else {
            body
        };
        // Drag routing delegates to child controls before claiming unhandled gestures.
        let centred = Self::centred_in_band(content);
        let with_drag: Element<'a, Message> = match self.on_window_gesture.as_ref() {
            Some(on_gesture) => {
                let on_gesture = std::rc::Rc::clone(on_gesture);
                titlebar_drag_region(centred)
                    .on_gesture(move |command| (on_gesture)(command))
                    .into()
            }
            None => centred,
        };
        let with_drag = match (self.titlebar_background.as_ref(), geometry) {
            (Some(background), Some(geometry)) => stack![
                Element::new(super::paint_layer::PaintLayer(background.build(&geometry))),
                with_drag,
            ]
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            _ => with_drag,
        };
        let is_full_width = matches!(width, Length::Fill);
        let base = self.tokens_normalized().titlebar;
        let style = if is_full_width && self.has_subtitlebar() {
            base.radius(0.0)
        } else {
            base
        };
        self.bar_pane(
            with_drag,
            style,
            width,
            Length::Fixed(self.tokens_normalized().titlebar_height),
        )
    }

    // Contained titlebars inherit the shell-owned tone and reserved height through ambient scope.
    fn in_band<T>(tone: SurfaceTone, height: f32, build: impl FnOnce() -> T) -> T {
        crate::title_bar::with_shell_band(tone, height, build)
    }

    fn titlebar_slot(
        &self,
        slot: &ShellSlot<'a, Message>,
        geometry: ShellGeometry,
    ) -> Element<'a, Message> {
        let tokens = self.tokens_normalized();
        // The shell owns the window controls when it draws them, and when the
        // compositor's frame carries them; a bar in the slot then draws none.
        let owns_controls = self.draws_no_window_chrome()
            || self.island_material.is_some()
            || self.window_controls_element().is_some();
        crate::title_bar::with_shell_window_controls(owns_controls, || {
            Self::in_band(tokens.titlebar.tone, tokens.titlebar_height, || {
                slot.build(&geometry)
            })
        })
    }

    fn has_subtitlebar(&self) -> bool {
        self.subtitlebar.is_some() && self.tokens_normalized().subtitlebar_height > 0.0
    }

    fn subtitlebar_band(&self, geometry: ShellGeometry) -> Option<Element<'a, Message>> {
        if !self.has_subtitlebar() {
            return None;
        }
        let bar = self.subtitlebar.as_ref()?;
        let height = self.tokens_normalized().subtitlebar_height;
        let style = self.tokens_normalized().subtitlebar.radius(0.0);
        let body = Self::in_band(style.tone, height, || bar.build(&geometry));
        Some(self.bar_pane(
            Self::centred_in_band(body),
            style,
            Length::Fill,
            Length::Fixed(height),
        ))
    }

    pub(crate) fn notifications_band(
        &self,
        geometry: ShellGeometry,
    ) -> Option<Element<'a, Message>> {
        let inset = self.tokens_normalized().effective_notifications_inset();
        self.notifications.as_ref()?(&geometry).map(|slot| {
            container(slot)
                .padding(iced::Padding {
                    left: inset,
                    right: inset,
                    ..iced::Padding::ZERO
                })
                .width(Length::Fill)
                .height(Length::Shrink)
                .into()
        })
    }

    fn titlebar_region(
        &self,
        bar: Element<'a, Message>,
        geometry: ShellGeometry,
    ) -> Element<'a, Message> {
        match self.subtitlebar_band(geometry) {
            Some(sub) => column![bar, sub].spacing(0.0).width(Length::Fill).into(),
            None => bar,
        }
    }

    fn prepend_subtitlebar(
        &self,
        region: Element<'a, Message>,
        geometry: ShellGeometry,
    ) -> (Element<'a, Message>, bool) {
        match self.subtitlebar_band(geometry) {
            Some(sub) => (
                column![sub, region]
                    .spacing(
                        self.tokens_normalized()
                            .effective_subtitlebar_content_gutter(),
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                true,
            ),
            None => (region, false),
        }
    }

    fn titlebar_row(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
    ) -> Option<Element<'a, Message>> {
        let Some(titlebar) = self.titlebar.as_ref() else {
            return self.toggle_button(geometry).map(|_| {
                self.titlebar_band_with_background(
                    row![self.with_toggle(
                        geometry,
                        motion,
                        iced::widget::Space::new().into(),
                        true,
                    )]
                    .width(Length::Fill)
                    .into(),
                    Length::Fill,
                    true,
                    Some(geometry),
                )
            });
        };

        if !geometry.split_titlebar {
            return Some(self.titlebar_band_with_background(
                self.with_toggle(
                    geometry,
                    motion,
                    self.titlebar_slot(titlebar, geometry),
                    true,
                ),
                Length::Fill,
                true,
                Some(geometry),
            ));
        }

        let sidebar_bar = match self.sidebar_titlebar.as_ref() {
            Some(bar) => self.sidebar_titlebar_band(
                self.with_toggle(
                    geometry,
                    motion,
                    crate::title_bar::with_pane_header(
                        self.pane_header_toggle(geometry, false),
                        || self.titlebar_slot(bar, geometry),
                    ),
                    false,
                ),
                Length::Fixed(geometry.sidebar_width),
            ),
            None => self.sidebar_titlebar_band(
                self.with_toggle(geometry, motion, iced::widget::Space::new().into(), false),
                Length::Fixed(geometry.sidebar_width),
            ),
        };

        let sidebar_bar = self.reveal_panel(
            sidebar_bar,
            motion.inline_progress,
            Some(motion.inline_clock()),
            Some(geometry.sidebar_width),
            crate::motion::PanelRevealMode::Reserve,
        );

        // The riding toggle passes over this band's leading end as the
        // sidebar folds; its title gives way as the toggle arrives.
        let content_slot = self.titlebar_slot(titlebar, geometry);
        let content_slot = if self.toggle_has_stable_parent() {
            self.with_stable_toggle_overlap_clearance(geometry, motion, content_slot)
        } else {
            content_slot
        };
        let content_bar =
            self.titlebar_band_with_background(content_slot, Length::Fill, true, Some(geometry));
        let bands = match self.seam_rule(geometry, motion) {
            Some(seam) => vec![sidebar_bar, seam, content_bar],
            None => vec![sidebar_bar, content_bar],
        };
        Some(
            // Mirror placement without reversing logical focus and accessibility order.
            crate::app_shell::mirror_row::MirrorRow::new(bands, self.tokens_normalized().is_rtl())
                .spacing(
                    self.tokens_normalized()
                        .effective_sidebar_content_gutter(self.layout),
                )
                .height(Length::Fixed(self.tokens_normalized().titlebar_height))
                .into(),
        )
    }

    // Drawer layering belongs above this row and the footer so both share modal input gating.
    fn body_row(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        custody: &super::seam::ContentTree,
    ) -> Element<'a, Message> {
        let content = self.retained_content_region(geometry, custody);
        match geometry.sidebar {
            SidebarPresentation::Inline => match self.sidebar_pane(
                Length::Fill,
                geometry,
                motion.inline_progress,
                Some(motion.inline_clock()),
                crate::motion::PanelRevealMode::Reserve,
            ) {
                Some(sidebar) => {
                    let spacing = self
                        .tokens_normalized()
                        .effective_sidebar_content_gutter(self.layout);
                    let rtl = self.tokens_normalized().is_rtl();
                    let logical = match self.seam_rule(geometry, motion) {
                        Some(seam) => vec![sidebar, seam, content],
                        None => vec![sidebar, content],
                    };
                    // Mirror placement without reversing logical focus and accessibility order.
                    crate::app_shell::mirror_row::MirrorRow::new(logical, rtl)
                        .spacing(spacing)
                        .height(Length::Fill)
                        .into()
                }
                None => content,
            },
            SidebarPresentation::Overlay => content,
            SidebarPresentation::Hidden => content,
        }
    }

    fn toggle_message(&self, geometry: ShellGeometry) -> Option<Message> {
        if let Some(route) = self.on_sidebar_state.as_ref() {
            let mut next = self.state;
            next.toggle_for_presentation(geometry.sidebar);
            return Some(route(next));
        }
        self.on_toggle.clone()
    }

    fn dismiss_message(&self) -> Option<Message> {
        if let Some(message) = self.on_dismiss_overlay.clone() {
            return Some(message);
        }
        if let Some(route) = self.on_sidebar_state.as_ref() {
            let mut next = self.state;
            next.dismiss_overlay();
            return Some(route(next));
        }
        self.on_toggle.clone()
    }

    pub(crate) fn would_render_toggle(&self, geometry: ShellGeometry) -> bool {
        geometry.shows_toggle
            && self.sidebar.is_some()
            && self.layout.toggle_anchor().is_placeable()
    }

    fn sidebar_toggle_is_inert(&self, geometry: ShellGeometry, available: f32) -> bool {
        // Opaque legacy messages cannot be simulated as state transitions.
        if self.on_sidebar_state.is_none() {
            return false;
        }
        // A stored presentation does not describe the host’s response to the next state.
        let Some(rule) = self.forced_by.as_ref() else {
            return false;
        };
        let mut next = self.state;
        next.toggle_for_presentation(geometry.sidebar);
        if next == self.state {
            return true;
        }
        let after = {
            let tokens = self.effective_tokens().normalized();
            let mut g =
                resolve_shell_geometry(self.layout, next, tokens, self.breakpoints, available);
            if self.layout.has_sidebar() {
                g.sidebar = rule(next);
            }
            g
        };
        after.sidebar == geometry.sidebar
            && next.effective_overlay_open(after.sidebar)
                == self.state.effective_overlay_open(geometry.sidebar)
    }

    fn toggle_button(&self, geometry: ShellGeometry) -> Option<Element<'a, Message>> {
        if !geometry.shows_toggle
            || self.sidebar.is_none()
            || !self.layout.toggle_anchor().is_placeable()
        {
            return None;
        }
        let message = self.toggle_message(geometry)?;

        let showing = geometry.sidebar == SidebarPresentation::Inline
            || self.state.effective_overlay_open(geometry.sidebar);

        let rtl = self.tokens_normalized().is_rtl();
        let (icon, label) = if showing {
            (
                if rtl {
                    TOGGLE_ICON_COLLAPSE_RTL
                } else {
                    TOGGLE_ICON_COLLAPSE
                },
                enzui_core::localization::text("enzui-shell-hide-sidebar"),
            )
        } else {
            (
                if rtl {
                    TOGGLE_ICON_EXPAND_RTL
                } else {
                    TOGGLE_ICON_EXPAND
                },
                enzui_core::localization::text("enzui-shell-show-sidebar"),
            )
        };

        let shell_tokens = self.tokens_normalized();
        let mut toggle = enz_toolbar_icon(icon, message)
            .variant(ToolbarIconVariant::InkOnly)
            .aria_label(label.clone())
            // Sidebar disclosure exposes expanded state rather than a pressed toggle state.
            .expanded(showing)
            .id(self.sidebar_toggle_id.clone());

        toggle = toggle.tooltip(if self.sidebar_toggle_tooltip {
            label
        } else {
            String::new()
        });
        if let Some(delay) = self.tooltip_delay {
            toggle = toggle.tooltip_delay(delay);
        }

        if let Some(tokens) = shell_tokens.toggle_icon_tokens {
            toggle = toggle.tokens(tokens);
        }

        let toggle = toggle.into_element();
        let Some(inset) = self.sidebar_toggle_side_inset else {
            return Some(toggle);
        };
        let inset = inset
            + if self.layout.floats_titlebar() && geometry.sidebar_full_height {
                shell_tokens.effective_edge_inset()
            } else {
                0.0
            };
        let size = toggle.as_widget().size();
        let width = Self::toggle_declared_width(&toggle)?;
        let trailing = shell_tokens
            .effective_toggle_anchor(self.layout)
            .is_trailing()
            && geometry.sidebar_full_height;
        Some(
            container(toggle)
                .padding(if rtl != trailing {
                    Padding::ZERO.right(inset)
                } else {
                    Padding::ZERO.left(inset)
                })
                .width(Length::Fixed(width + inset))
                .height(size.height)
                .into(),
        )
    }

    /// Whether one toggle rides above both the sidebar and the content
    /// titlebar, so it never hands off between bands mid-travel.
    fn toggle_has_stable_parent(&self) -> bool {
        match self.layout {
            ShellLayout::TitlebarSidebarContent => matches!(
                self.tokens_normalized().sidebar_header,
                SidebarHeader::Inside | SidebarHeader::SplitBand
            ),
            ShellLayout::FloatingTitlebarSidebar => true,
            _ => false,
        }
    }

    fn toggle_declared_width(toggle: &Element<'a, Message>) -> Option<f32> {
        match toggle.as_widget().size().width {
            Length::Fixed(width) => Some(width),
            _ => None,
        }
    }

    fn with_stable_toggle_overlap_clearance(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        body: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let tokens = self.tokens_normalized();
        let lead = tokens.effective_toggle_lead();
        let rtl = tokens.is_rtl();
        let body = match self
            .toggle_button(geometry)
            .as_ref()
            .and_then(Self::toggle_declared_width)
        {
            // Read clearance during layout so animation frames need no application view rebuild.
            Some(toggle_width) => crate::app_shell::seam::LiveSeamPad::driven(
                body,
                iced::advanced::layout::inset::Policy::OppositeClearance {
                    sidebar: geometry.sidebar_width,
                    toggle: toggle_width,
                    gutter: tokens.effective_pane_gutter(),
                },
                std::rc::Rc::clone(&motion.inline_live_progress),
                motion.docked_driver,
            )
            .trailing_edge(rtl)
            .into(),
            None => body,
        };
        // The lead stays while the sidebar is open, so the title stands off
        // its edge as a collapsed toggle stands off the window edge.
        container(body)
            .padding(if rtl {
                iced::Padding::ZERO.right(lead)
            } else {
                iced::Padding::ZERO.left(lead)
            })
            .width(Length::Fill)
            .into()
    }

    fn with_withheld_toggle_clearance(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        body: Element<'a, Message>,
        reserve_leading: bool,
    ) -> Element<'a, Message> {
        let lead = if reserve_leading {
            self.tokens_normalized().effective_toggle_lead()
        } else {
            0.0
        };
        self.with_withheld_toggle(geometry, motion, body, reserve_leading, lead, false)
    }

    /// Reserves the withheld toggle's place, `lead` from the band's edge, while
    /// it travels. With `draws_arriving`, a toggle arriving or leaving is also
    /// drawn there, inert, so it moves with its band instead of appearing when
    /// the travel ends or vanishing when it starts; it takes input once mounted.
    fn with_withheld_toggle(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        body: Element<'a, Message>,
        reserve_leading: bool,
        lead: f32,
        draws_arriving: bool,
    ) -> Element<'a, Message> {
        // An arriving control needs its reservation even on the initial zero-progress frame.
        if motion.toggle_clearance.get() <= f32::EPSILON && !motion.toggle_riding() {
            return body;
        }
        let probe = ShellGeometry {
            shows_toggle: true,
            ..geometry
        };
        let Some(toggle) = self.toggle_button(probe) else {
            return body;
        };
        let Some(toggle_width) = Self::toggle_declared_width(&toggle) else {
            return body;
        };
        let gutter = self.tokens_normalized().effective_pane_gutter();
        let progress = std::rc::Rc::clone(&motion.toggle_clearance);
        let rtl = self.tokens_normalized().is_rtl();
        let pad_trailing = if rtl {
            reserve_leading
        } else {
            !reserve_leading
        };
        // Read clearance during layout so animation frames need no application view rebuild.
        let padded: Element<'a, Message> = crate::app_shell::seam::LiveSeamPad::driven(
            body,
            iced::advanced::layout::inset::Policy::WithheldClearance {
                toggle: toggle_width + lead,
                gutter,
            },
            progress,
            motion.toggle_driver,
        )
        .trailing_edge(pad_trailing)
        .into();
        if !(draws_arriving && motion.toggle_riding()) {
            return padded;
        }
        // Where the mounted toggle will sit: at the reserved edge, after its lead.
        let arriving = container(crate::inert::Inert::new(toggle, true))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(if pad_trailing {
                iced::alignment::Horizontal::Right
            } else {
                iced::alignment::Horizontal::Left
            })
            .align_y(iced::alignment::Vertical::Center)
            .padding(if pad_trailing {
                Padding::ZERO.right(lead)
            } else {
                Padding::ZERO.left(lead)
            });
        iced::widget::stack![padded, arriving].into()
    }

    fn with_toggle_clearance(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        body: Element<'a, Message>,
    ) -> Element<'a, Message> {
        let Some(toggle) = self.toggle_button(geometry) else {
            return self.with_withheld_toggle_clearance(geometry, motion, body, true);
        };
        // The riding toggle stands off the window edge by its lead; so does the clearance.
        let size = toggle.as_widget().size();
        let lead = self.tokens_normalized().effective_toggle_lead();
        let width = match size.width {
            Length::Fixed(width) => Length::Fixed(width + lead),
            other => other,
        };
        let clearance = iced::widget::Space::new().width(width).height(size.height);
        let composed = if self.tokens_normalized().is_rtl() {
            row![body, clearance]
        } else {
            row![clearance, body]
        };
        composed
            .spacing(self.tokens_normalized().effective_pane_gutter())
            .align_y(iced::alignment::Vertical::Center)
            .into()
    }

    fn with_toggle(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
        body: Element<'a, Message>,
        band_carries_controls: bool,
    ) -> Element<'a, Message> {
        if self.toggle_has_stable_parent() {
            let reserve_leading_when_withheld = band_carries_controls
                || !self
                    .tokens_normalized()
                    .effective_toggle_anchor(self.layout)
                    .is_trailing();
            let Some(toggle) = self.toggle_button(geometry) else {
                // The band shows a toggle that is arriving or leaving with the
                // sidebar itself, so it travels, and is clipped, with the band.
                let lead = if reserve_leading_when_withheld {
                    self.tokens_normalized().effective_toggle_lead()
                } else {
                    0.0
                };
                return self.with_withheld_toggle(
                    geometry,
                    motion,
                    body,
                    reserve_leading_when_withheld,
                    lead,
                    !band_carries_controls,
                );
            };
            let Some(toggle_width) = Self::toggle_declared_width(&toggle) else {
                return body;
            };
            let tokens = self.tokens_normalized();
            let reserve_leading_end =
                band_carries_controls || !tokens.effective_toggle_anchor(self.layout).is_trailing();
            let lead = if reserve_leading_end {
                tokens.effective_toggle_lead()
            } else {
                0.0
            };
            // Beside a sidebar header the trailing toggle is one more of the
            // header's icons: the icon gap, not the pane gutter, separates them.
            let gap = if !band_carries_controls && !reserve_leading_end {
                crate::title_bar::PANE_HEADER_ICON_GAP
            } else {
                tokens.effective_pane_gutter()
            };
            let extent = toggle_width + lead + gap;
            let reserve_leading =
                band_carries_controls || !tokens.effective_toggle_anchor(self.layout).is_trailing();
            let pad_left = if tokens.is_rtl() {
                !reserve_leading
            } else {
                reserve_leading
            };
            // A band sliding beneath the riding toggle hides past its edge
            // rather than passing under its icon.
            let body: Element<'a, Message> = if reserve_leading {
                let reach = tokens.effective_edge_inset() + lead + toggle_width;
                if tokens.is_rtl() {
                    crate::app_shell::clip_beyond::ClipBeyond::new(
                        body,
                        self.built_width.get() - reach,
                        false,
                    )
                    .into()
                } else {
                    crate::app_shell::clip_beyond::ClipBeyond::new(body, reach, true).into()
                }
            } else {
                body
            };
            return container(body)
                .padding(if pad_left {
                    iced::Padding::ZERO.left(extent)
                } else {
                    iced::Padding::ZERO.right(extent)
                })
                .into();
        }

        let spacing = self.tokens_normalized().effective_pane_gutter();
        let anchor = self
            .tokens_normalized()
            .effective_toggle_anchor(self.layout);

        let shell_owns_trailing_controls = band_carries_controls
            && (self.window_controls_element().is_some() || self.island_material.is_some());

        let has_trailing_seam_to_ride = geometry.sidebar.is_inline();
        let ride_trailing =
            anchor.is_trailing() && !shell_owns_trailing_controls && has_trailing_seam_to_ride;

        // A withheld toggle reserves, and arrives at, the side it will mount on.
        let Some(toggle) = self.toggle_button(geometry) else {
            return self.with_withheld_toggle(geometry, motion, body, !ride_trailing, 0.0, true);
        };

        let rtl = self.tokens_normalized().is_rtl();
        let toggle_first = if rtl { ride_trailing } else { !ride_trailing };
        // A sidebar header's trailing toggle sits one icon gap from the
        // header's own icons (see the stable-parent branch above).
        let spacing = if !band_carries_controls && ride_trailing {
            crate::title_bar::PANE_HEADER_ICON_GAP
        } else {
            spacing
        };
        let composed = if toggle_first {
            row![toggle, body]
        } else {
            // A separate Fill spacer would split slack with filling slot content and halve its width.
            let filling_body: Element<'a, Message> = container(body).width(Length::Fill).into();
            row![filling_body, toggle]
        };
        composed
            .spacing(spacing)
            .align_y(iced::alignment::Vertical::Center)
            .into()
    }

    fn overlay_drawer(
        &self,
        geometry: ShellGeometry,
        motion: &SidebarMotionFrame,
    ) -> Option<Element<'a, Message>> {
        if !motion.overlay_mounted {
            return None;
        }
        // The drawer covers the titlebar, so its header carries the toggle
        // unless the shell's riding toggle already sits above it.
        let pane = self.sidebar_pane_with_header(
            geometry,
            motion,
            SidebarPanePresentation {
                width: if self.tokens.full_width_drawer {
                    Length::Fill
                } else {
                    Length::Fixed(self.tokens_normalized().sidebar_width)
                },
                carries_toggle: !self.toggle_has_stable_parent(),
                progress: motion.overlay_progress,
                live_clock: Some(motion.overlay_clock()),
                mode: crate::motion::PanelRevealMode::Overlay,
            },
        )?;
        let drawer_align = if self.tokens_normalized().is_rtl() {
            iced::alignment::Horizontal::Right
        } else {
            iced::alignment::Horizontal::Left
        };
        Some(
            // The window layer already insets the drawer from the window's edges.
            container(exclusive_focus(pane))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(drawer_align)
                .into(),
        )
    }

    /// The window controls alone, where a titlebar band draws them, for a
    /// layer above content: above the drawer's scrim, so closing the window
    /// stays one click away while the drawer covers the titlebar, and over a
    /// titlebar-less layout's content. `None` when the window draws none, or
    /// draws them in an island that carries app actions too.
    pub(crate) fn lifted_window_controls(&self) -> Option<Element<'a, Message>> {
        if self.island_material.is_some() {
            return None;
        }
        self.window_controls_element()?;
        let tokens = self.tokens_normalized();
        Some(
            container(self.titlebar_content_with_controls(Space::new().width(Length::Fill).into()))
                .width(Length::Fill)
                .height(Length::Fixed(tokens.titlebar_height))
                .align_y(iced::alignment::Vertical::Center)
                .into(),
        )
    }

    /// The drawer and its scrim over `base`, whose corners are `corners`
    /// (`[top-left, top-right, bottom-left, bottom-right]`): the scrim keeps
    /// to them, so a rounded pane stays rounded under it.
    fn drawer_over(
        &self,
        base: Element<'a, Message>,
        drawer: Option<Element<'a, Message>>,
        drawer_active: bool,
        corners: [f32; 4],
    ) -> Element<'a, Message> {
        // Keep the base parent chain stable while optional drawer layers enter and leave.
        let drawer_mounted = drawer.is_some();
        let dismiss = drawer_active.then(|| self.dismiss_message()).flatten();

        let mut layers: Vec<Element<'a, Message>> = Vec::with_capacity(4);
        // Modal input follows state, not exit animation mounting; dismissals release the base immediately.
        layers.push(crate::inert::Inert::new(base, drawer_active).into());

        // The scrim fades with the drawer: in as it opens, out while it slides
        // away, taking no input once dismissed.
        if drawer_active || drawer_mounted {
            let body = container(Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |theme: &iced::Theme| container::Style {
                    background: Some(scrim_color(&crate::theme_lookup::palette(theme)).into()),
                    border: iced::Border {
                        radius: iced::border::Radius {
                            top_left: corners[0],
                            top_right: corners[1],
                            bottom_left: corners[2],
                            bottom_right: corners[3],
                        },
                        ..iced::Border::default()
                    },
                    ..container::Style::default()
                });

            let scrim: Element<'a, Message> = match (drawer_active, dismiss.clone()) {
                // Pointer chrome only: assistive technology closes the drawer
                // through its toggle or Escape, as platform drawers do.
                (true, Some(message)) => ScrimClick::new(body, move || message.clone())
                    .block_scroll()
                    .into(),
                (true, None) => ScrimClick::shield(body).into(),
                (false, _) => crate::inert::Inert::new(body, true).into(),
            };
            let scrim = crate::motion::animated(scrim)
                .enters_from(iced::Vector::ZERO, 0.0)
                .opacity(if drawer_active { 1.0 } else { 0.0 });
            layers.push(scrim.into());
        }

        if let Some(drawer) = drawer {
            // Dismissal can leave progress at one, so reveal gating alone cannot disable the drawer.
            let drawer = if drawer_active {
                drawer
            } else {
                crate::inert::Inert::new(drawer, true).into()
            };
            layers.push(drawer);
            if let Some(controls) = self.lifted_window_controls() {
                // A dismissed drawer still slides away under these; the
                // titlebar's own controls are live again, and two live sets
                // would publish the same control ids.
                layers.push(crate::inert::Inert::new(controls, !drawer_active).into());
            }
        }

        debug_assert!(
            !drawer_active || layers.len() >= 2,
            "drawer_active must place at least the base + scrim in the stack",
        );
        debug_assert!(
            !drawer_mounted || layers.len() > 1,
            "a mounted drawer must appear as a layer above the base",
        );
        let layered: Element<'a, Message> = iced::widget::Stack::with_children(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        keyboard_capture_unscoped(layered, move |key, _modifiers| {
            dismiss.as_ref().and_then(|message| {
                matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape)
                )
                .then(|| message.clone())
            })
        })
    }

    /// `chrome` floating at the top of `base`, `inset` in from the window
    /// layer's edges.
    fn floating_over(
        &self,
        base: Element<'a, Message>,
        chrome: Element<'a, Message>,
        inset: f32,
    ) -> Element<'a, Message> {
        let layer = container(chrome)
            .padding(inset)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(iced::alignment::Vertical::Top);
        let layers: Vec<Element<'a, Message>> = match self.floating_scrim() {
            Some(scrim) => vec![base, scrim, layer.into()],
            None => vec![base, layer.into()],
        };
        iced::widget::Stack::with_children(layers)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// A floating titlebar band that fades with [`EnzAppShell::chrome_visible`].
    fn faded_floating_chrome(&self, band: Element<'a, Message>) -> Element<'a, Message> {
        crate::motion::motion(band, self.chrome_visible, crate::motion::MotionEffect::Fade).into()
    }

    /// The legibility shade under a floating titlebar, reaching the window's
    /// edges and fading with the chrome; `None` unless
    /// [`EnzAppShell::titlebar_scrim`] asked for it.
    fn floating_scrim(&self) -> Option<Element<'a, Message>> {
        if !self.titlebar_scrim {
            return None;
        }
        let tokens = self.tokens_normalized();
        let depth = tokens.titlebar_height + 2.0 * tokens.effective_edge_inset() + SCRIM_TAIL;
        let tone = tokens.shell_tone;
        let scrim = container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fixed(depth))
            .style(move |theme: &iced::Theme| {
                let color = crate::surface::SurfaceTokens::from_palette(
                    &crate::theme_lookup::palette(theme),
                )
                .fill_for(tone);
                let at = |a: f32| iced::Color {
                    a: color.a * a,
                    ..color
                };
                iced::widget::container::Style {
                    background: Some(iced::Background::Gradient(iced::Gradient::Linear(
                        iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
                            .add_stop(0.0, at(0.9))
                            .add_stop(0.5, at(0.55))
                            .add_stop(1.0, at(0.0)),
                    ))),
                    ..Default::default()
                }
            });
        Some(
            crate::motion::motion(
                scrim,
                self.chrome_visible,
                crate::motion::MotionEffect::Fade,
            )
            .into(),
        )
    }
}

/// How far a floating titlebar's scrim reaches below its band, in px.
const SCRIM_TAIL: f32 = 48.0;

mod build;
