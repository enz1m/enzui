//! Shell token setters, preset resolution and effective-value accessors.

use super::*;

impl ShellTokens {
    /// Set the fallback window inset and pane gutter in logical pixels.
    /// Explicit overrides survive; negative or non-finite inputs use [`DEFAULT_SHELL_GAP`].
    #[must_use]
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = FiniteNonNeg::new_or(gap, DEFAULT_SHELL_GAP).get();
        self
    }
    /// Pin the window-edge inset in logical pixels.
    /// Negative or non-finite inputs use [`DEFAULT_SHELL_GAP`]; pane gutters are unchanged.
    #[must_use]
    pub fn edge_inset(mut self, edge_inset: f32) -> Self {
        self.edge_inset = Some(FiniteNonNeg::new_or(edge_inset, DEFAULT_SHELL_GAP).get());
        self
    }
    /// Pin general inter-pane spacing in logical pixels.
    /// Negative or non-finite inputs use [`DEFAULT_SHELL_GAP`]. Flush sidebar seams
    /// use [`Self::effective_sidebar_content_gutter`] independently.
    #[must_use]
    pub fn pane_gutter(mut self, pane_gutter: f32) -> Self {
        self.pane_gutter = Some(FiniteNonNeg::new_or(pane_gutter, DEFAULT_SHELL_GAP).get());
        self
    }
    /// Resolve the page-top inset from a spacing token.
    /// The default is zero; [`Self::normalized`] replaces invalid resolved values with zero.
    #[must_use]
    pub fn page_inset(mut self, inset: SpaceToken, spacing: &SpacingScale) -> Self {
        self.page_inset = inset.resolve(spacing);
        self
    }
    /// Apply [`PAGE_INSET_TOKEN`] using the supplied spacing scale.
    #[must_use]
    pub fn recommended_page_inset(self, spacing: &SpacingScale) -> Self {
        self.page_inset(PAGE_INSET_TOKEN, spacing)
    }
    /// Set the titlebar band height in logical pixels.
    /// Negative or non-finite inputs use [`DEFAULT_SHELL_TITLEBAR_HEIGHT`]; zero is accepted.
    #[must_use]
    pub fn titlebar_height(mut self, titlebar_height: f32) -> Self {
        self.titlebar_height =
            FiniteNonNeg::new_or(titlebar_height, DEFAULT_SHELL_TITLEBAR_HEIGHT).get();
        self
    }
    /// Pin the sidebar width in logical pixels and disable adaptive default narrowing.
    /// Negative or non-finite inputs use [`DEFAULT_SHELL_SIDEBAR_WIDTH`].
    #[must_use]
    pub fn sidebar_width(mut self, sidebar_width: f32) -> Self {
        self.sidebar_width = FiniteNonNeg::new_or(sidebar_width, DEFAULT_SHELL_SIDEBAR_WIDTH).get();
        self.sidebar_width_adaptive = false;
        self
    }
    /// Set the content width reserved beside an inline sidebar or secondary pane.
    /// Units are logical pixels; invalid inputs use [`DEFAULT_CONTENT_MINIMUM`].
    #[must_use]
    pub fn content_minimum(mut self, content_minimum: f32) -> Self {
        self.content_minimum = FiniteNonNeg::new_or(content_minimum, DEFAULT_CONTENT_MINIMUM).get();
        self
    }
    /// Normalize numeric tokens and all pane styles, including raw struct-literal values.
    /// Negative or non-finite numbers use their field-specific defaults; valid zero
    /// values and unset options survive. A non-default raw sidebar width is pinned.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            gap: FiniteNonNeg::new_or(self.gap, DEFAULT_SHELL_GAP).get(),
            // Preserve unset options so fallback links remain active after normalization.
            edge_inset: self
                .edge_inset
                .map(|v| FiniteNonNeg::new_or(v, DEFAULT_SHELL_GAP).get()),
            pane_gutter: self
                .pane_gutter
                .map(|v| FiniteNonNeg::new_or(v, DEFAULT_SHELL_GAP).get()),
            seam_thickness: self
                .seam_thickness
                .map(|v| FiniteNonNeg::new_or(v, SEAM_RULE_THICKNESS).get()),
            page_inset: FiniteNonNeg::new_or(self.page_inset, 0.0).get(),
            notifications_inset: self
                .notifications_inset
                .map(|v| FiniteNonNeg::new_or(v, DEFAULT_NOTIFICATIONS_INSET).get()),
            window_radius: FiniteNonNeg::new_or(self.window_radius, DEFAULT_WINDOW_RADIUS).get(),
            pane_radius: FiniteNonNeg::new_or(self.pane_radius, DEFAULT_PANE_RADIUS).get(),
            titlebar_height: FiniteNonNeg::new_or(
                self.titlebar_height,
                DEFAULT_SHELL_TITLEBAR_HEIGHT,
            )
            .get(),
            subtitlebar_height: FiniteNonNeg::new_or(self.subtitlebar_height, 0.0).get(),
            sidebar_width: FiniteNonNeg::new_or(self.sidebar_width, DEFAULT_SHELL_SIDEBAR_WIDTH)
                .get(),
            // Raw non-default widths are pinned even when they bypass the setter.
            sidebar_width_adaptive: self.sidebar_width_adaptive
                && self.sidebar_width == enzui_theme::text_defaults::sizing().sidebar_width,
            content_minimum: FiniteNonNeg::new_or(self.content_minimum, DEFAULT_CONTENT_MINIMUM)
                .get(),
            secondary_minimum: self
                .secondary_minimum
                .map(|m| FiniteNonNeg::new_or(m, DEFAULT_CONTENT_MINIMUM).get()),
            content: self.content.normalized(),
            secondary_content: self.secondary_content.normalized(),
            secondary_width: self
                .secondary_width
                .map(|w| FiniteNonNeg::new_or(w, DEFAULT_SECONDARY_WIDTH).get()),
            sidebar: self.sidebar.normalized(),
            titlebar: self.titlebar.normalized(),
            subtitlebar: self.subtitlebar.normalized(),
            footer: self.footer.normalized(),
            ..self
        }
    }
    /// Pin the secondary-pane minimum in logical pixels when no fixed width is set.
    /// Invalid inputs use [`DEFAULT_CONTENT_MINIMUM`]; sidebar collapse is unchanged.
    #[must_use]
    pub fn secondary_minimum(mut self, secondary_minimum: f32) -> Self {
        self.secondary_minimum =
            Some(FiniteNonNeg::new_or(secondary_minimum, DEFAULT_CONTENT_MINIMUM).get());
        self
    }
    /// Return the secondary minimum or, when unset, the content minimum.
    /// This accessor does not normalize raw values.
    #[must_use]
    pub fn effective_secondary_minimum(&self) -> f32 {
        self.secondary_minimum.unwrap_or(self.content_minimum)
    }
    /// Return the edge-inset override or the fallback gap.
    /// This accessor neither normalizes tokens nor applies layout-specific flush defaults;
    /// use [`Self::with_flush_defaults`] to resolve those defaults first.
    #[must_use]
    pub fn effective_edge_inset(&self) -> f32 {
        self.edge_inset.unwrap_or(self.gap)
    }
    /// Return the pane-gutter override or fallback gap without normalization.
    #[must_use]
    pub fn effective_pane_gutter(&self) -> f32 {
        self.pane_gutter.unwrap_or(self.gap)
    }
    /// Return zero with a positive edge inset, otherwise [`TOGGLE_EDGE_LEAD`].
    #[must_use]
    pub fn effective_toggle_lead(&self) -> f32 {
        if self.effective_edge_inset() > 0.0 {
            0.0
        } else {
            TOGGLE_EDGE_LEAD
        }
    }
    /// Return the notification inset override, or zero with a positive edge inset.
    /// Without a positive edge inset, the fallback is [`DEFAULT_NOTIFICATIONS_INSET`].
    #[must_use]
    pub fn effective_notifications_inset(&self) -> f32 {
        self.notifications_inset.unwrap_or_else(|| {
            if self.effective_edge_inset() > 0.0 {
                0.0
            } else {
                DEFAULT_NOTIFICATIONS_INSET
            }
        })
    }
    /// Return the page-top inset without normalization.
    #[must_use]
    pub fn effective_page_inset(&self) -> f32 {
        self.page_inset
    }
    /// Return zero for a flush page, otherwise the effective pane gutter.
    #[must_use]
    pub fn effective_titlebar_content_gutter(&self) -> f32 {
        match self.page_presentation {
            PagePresentation::Standard => self.effective_pane_gutter(),
            PagePresentation::Flush => 0.0,
        }
    }
    /// Return zero for a flush page, otherwise the effective pane gutter.
    #[must_use]
    pub fn effective_subtitlebar_content_gutter(&self) -> f32 {
        match self.page_presentation {
            PagePresentation::Standard => self.effective_pane_gutter(),
            PagePresentation::Flush => 0.0,
        }
    }
    /// Return the effective pane gutter, including for flush pages.
    #[must_use]
    pub fn effective_content_footer_gutter(&self) -> f32 {
        self.effective_pane_gutter()
    }
    /// Apply the override when the layout has a bar anchor.
    /// Layouts with [`ToggleAnchor::None`] or [`ToggleAnchor::FloatingOverContent`]
    /// keep their structural anchor. Overrides are returned as supplied.
    #[must_use]
    pub fn effective_toggle_anchor(&self, layout: ShellLayout) -> ToggleAnchor {
        let structural = layout.toggle_anchor();
        match structural {
            ToggleAnchor::LeadingBar | ToggleAnchor::TrailingBar => {
                self.toggle_anchor.unwrap_or(structural)
            }
            ToggleAnchor::None | ToggleAnchor::FloatingOverContent => structural,
        }
    }
    /// Return zero for a sidebar seam layout unless flush is explicitly disabled.
    /// Other arrangements use the effective pane gutter; vertical gutters are unchanged.
    #[must_use]
    pub fn effective_sidebar_content_gutter(&self, layout: ShellLayout) -> f32 {
        if layout.sidebar_beside_content() && !self.opts_out_of_flush() {
            0.0
        } else {
            self.effective_pane_gutter()
        }
    }
    /// Return the seam override, or [`SeamStyle::None`] when unset.
    /// [`Self::with_flush_defaults`] supplies the layout-specific hairline default.
    #[must_use]
    pub fn effective_seam(&self) -> SeamStyle {
        self.seam.unwrap_or(SeamStyle::None)
    }
    /// Return a finite nonnegative seam thickness, otherwise [`SEAM_RULE_THICKNESS`].
    /// Zero is accepted; geometry reserves this width only when the seam style needs a slot.
    #[must_use]
    pub fn effective_seam_thickness(&self) -> f32 {
        match self.seam_thickness {
            Some(px) if px.is_finite() && px >= 0.0 => px,
            _ => SEAM_RULE_THICKNESS,
        }
    }
    /// Return the explicit direction or the ambient locale direction.
    #[must_use]
    pub fn effective_direction(&self) -> Direction {
        resolve_direction(self.direction)
    }
    /// Whether the effective direction is right-to-left.
    #[must_use]
    pub fn is_rtl(&self) -> bool {
        self.effective_direction() == Direction::RightToLeft
    }
    /// Override the ambient writing direction.
    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }
    fn opts_out_of_flush(&self) -> bool {
        self.seam == Some(SeamStyle::None)
    }
    /// Fill unset flush defaults for layouts whose sidebar sits beside content.
    /// An explicit [`SeamStyle::None`] disables this resolution. Otherwise, unset
    /// edge inset, seam and toggle anchor become zero, hairline and trailing-bar.
    /// Unset sidebar/content corner arrays square the facing edges and mirror in RTL;
    /// explicit corner arrays survive. Uniform pane pins determine the outer
    /// radii before the shared pane radius. General pane gutters remain unchanged.
    #[must_use]
    pub fn with_flush_defaults(mut self, layout: ShellLayout) -> Self {
        if !layout.sidebar_beside_content() || self.opts_out_of_flush() {
            return self;
        }
        if self.edge_inset.is_none() {
            self.edge_inset = Some(0.0);
        }
        if self.seam.is_none() {
            self.seam = Some(SeamStyle::Hairline);
        }
        if self.toggle_anchor.is_none() {
            self.toggle_anchor = Some(ToggleAnchor::TrailingBar);
        }
        let rtl = self.is_rtl();
        if self.sidebar.radius_corners.is_none() {
            let r = self.sidebar.radius_or(self.pane_radius);
            self.sidebar.radius_corners = Some(if rtl {
                [0.0, r, 0.0, r]
            } else {
                [r, 0.0, r, 0.0]
            });
        }
        if self.content.radius_corners.is_none() {
            let r = self.content.radius_or(self.pane_radius);
            let stacked = layout == ShellLayout::TitlebarSidebarContent;
            self.content.radius_corners = Some(if rtl {
                let top_left = if stacked { 0.0 } else { r };
                [top_left, 0.0, r, 0.0]
            } else {
                let top_right = if stacked { 0.0 } else { r };
                [0.0, top_right, 0.0, r]
            });
        }

        self
    }
    /// Set the window radius and a pane radius two logical pixels smaller, floored at zero.
    /// Invalid inputs use [`DEFAULT_WINDOW_RADIUS`]; explicit pane corner arrays are unchanged.
    #[must_use]
    pub fn radius(mut self, window_radius: f32) -> Self {
        let r = FiniteNonNeg::new_or(window_radius, DEFAULT_WINDOW_RADIUS).get();
        self.window_radius = r;
        self.pane_radius = (r - 2.0).max(0.0);
        self
    }
    /// Blend the primary content, sidebar and titlebar pane styles into the shell.
    /// Secondary content, subtitlebar and footer styles are unchanged.
    #[must_use]
    pub fn flat(mut self) -> Self {
        self.content = PaneStyle::blended();
        self.sidebar = PaneStyle::blended();
        self.titlebar = PaneStyle::blended();
        self
    }
    /// Create flush page tokens with square, blended primary and secondary content.
    /// Titlebar/subtitlebar-to-content gutters close; footer spacing and window radius
    /// retain their defaults.
    #[must_use]
    pub fn flat_page() -> Self {
        Self {
            page_presentation: PagePresentation::Flush,
            content: PaneStyle::blended().radius_corners([0.0; 4]),
            secondary_content: PaneStyle::blended().radius_corners([0.0; 4]),
            ..Self::default()
        }
    }
    /// Alias for [`Self::flat_page`].
    #[must_use]
    pub fn flush_page() -> Self {
        Self::flat_page()
    }
    /// Create inset-pane tokens with raised primary and secondary content styles.
    /// The seam opt-out preserves fallback gap retuning and prevents flush corner resolution.
    #[must_use]
    pub fn floating_cards() -> Self {
        Self {
            content: PaneStyle::raised(),
            secondary_content: PaneStyle::raised(),
            ..Self::inset_panes()
        }
    }
    /// Create tokens that opt out of flush defaults while retaining default pane styles.
    /// The seam is explicitly absent and the toggle leads its bar. Insets and gutters
    /// remain unset, so later gap changes affect both.
    #[must_use]
    pub fn inset_panes() -> Self {
        Self {
            seam: Some(SeamStyle::None),
            toggle_anchor: Some(ToggleAnchor::LeadingBar),
            ..Self::default()
        }
    }
    /// Replace the primary content pane style.
    #[must_use]
    pub fn content_style(mut self, style: PaneStyle) -> Self {
        self.content = style;
        self
    }
    /// Replace the secondary content pane style independently of the primary.
    #[must_use]
    pub fn secondary_content_style(mut self, style: PaneStyle) -> Self {
        self.secondary_content = style;
        self
    }
    /// Replace the sidebar pane style.
    #[must_use]
    pub fn sidebar_style(mut self, style: PaneStyle) -> Self {
        self.sidebar = style;
        self
    }
    /// Replace the titlebar pane style independently of the subtitlebar.
    #[must_use]
    pub fn titlebar_style(mut self, style: PaneStyle) -> Self {
        self.titlebar = style;
        self
    }
    /// Replace the subtitlebar pane style.
    /// Rendering pins its uniform radius to zero; explicit per-corner arrays still take precedence.
    #[must_use]
    pub fn subtitlebar_style(mut self, style: PaneStyle) -> Self {
        self.subtitlebar = style;
        self
    }
    /// Set subtitlebar height in logical pixels; invalid values use zero.
    /// The band also requires a supplied subtitlebar slot.
    #[must_use]
    pub fn subtitlebar_height(mut self, height: f32) -> Self {
        self.subtitlebar_height = FiniteNonNeg::new_or(height, 0.0).get();
        self
    }
    /// Replace the footer pane style.
    #[must_use]
    pub fn footer_style(mut self, style: PaneStyle) -> Self {
        self.footer = style;
        self
    }
    /// Keep the sidebar inline and remove the collapse toggle.
    #[must_use]
    pub fn permanent_sidebar(mut self) -> Self {
        self.sidebar_collapsible = false;
        self
    }
    /// Choose where the sidebar header is composed.
    #[must_use]
    pub fn sidebar_header(mut self, placement: SidebarHeader) -> Self {
        self.sidebar_header = placement;
        self
    }
    /// Choose the requested secondary-content split axis.
    #[must_use]
    pub fn split_axis(mut self, axis: SplitAxis) -> Self {
        self.split_axis = axis;
        self
    }
    /// Choose whether a narrow beside split drops or restacks the secondary pane.
    /// The default is [`NarrowSplit::Drop`].
    #[must_use]
    pub fn narrow_split(mut self, policy: NarrowSplit) -> Self {
        self.narrow_split = policy;
        self
    }
    /// Select [`NarrowSplit::Restack`] for narrow beside splits.
    #[must_use]
    pub fn responsive_split(self) -> Self {
        self.narrow_split(NarrowSplit::Restack)
    }
    /// Place the secondary pane first in the resolved split order.
    /// The default is false; its width and style still belong to the secondary pane.
    #[must_use]
    pub fn secondary_leading(mut self, leading: bool) -> Self {
        self.secondary_leading = leading;
        self
    }
    /// Pin secondary width in logical pixels for a beside split.
    /// Invalid values use [`DEFAULT_SECONDARY_WIDTH`]. A requested below split uses full width;
    /// a restacked beside split retains the fixed width. [`Self::narrow_split`] selects
    /// dropping or restacking when it cannot fit beside.
    #[must_use]
    pub fn secondary_width(mut self, width: f32) -> Self {
        self.secondary_width = Some(FiniteNonNeg::new_or(width, DEFAULT_SECONDARY_WIDTH).get());
        self
    }
    /// Select [`SidebarHeader::SplitBand`].
    #[must_use]
    pub fn split_titlebar(self) -> Self {
        self.sidebar_header(SidebarHeader::SplitBand)
    }
    /// Select [`SidebarHeader::SpanningBand`], which ignores the sidebar-titlebar slot.
    #[must_use]
    pub fn spanning_titlebar(self) -> Self {
        self.sidebar_header(SidebarHeader::SpanningBand)
    }
    /// Set close-control visibility; the default is true.
    #[must_use]
    pub fn show_close(mut self, show: bool) -> Self {
        self.show_close = show;
        self
    }
    /// Set minimize-control visibility; the default is false.
    /// Supply [`EnzAppShell::on_minimize`] to route the action; the cluster also
    /// requires `show_close` true and [`EnzAppShell::on_close`].
    #[must_use]
    pub fn show_minimize(mut self, show: bool) -> Self {
        self.show_minimize = show;
        self
    }
    /// Set maximize-control visibility; the default is false.
    /// Supply [`EnzAppShell::on_maximize`] to route the action; the cluster also
    /// requires `show_close` true and [`EnzAppShell::on_close`].
    #[must_use]
    pub fn show_maximize(mut self, show: bool) -> Self {
        self.show_maximize = show;
        self
    }
    /// Set whether an overlay drawer uses the full shell width; the default is false.
    #[must_use]
    pub fn full_width_drawer(mut self, full: bool) -> Self {
        self.full_width_drawer = full;
        self
    }
    /// Declare sidebar-first narrow presentation; the default is false.
    /// Initialize drawer state separately with [`NavSplitState::for_sidebar_first`].
    #[must_use]
    pub fn sidebar_first(mut self, first: bool) -> Self {
        self.sidebar_first = first;
        self
    }
    /// Pin the seam style; [`SeamStyle::None`] also opts out of flush defaults.
    /// An unset seam is resolved according to the layout by [`Self::with_flush_defaults`].
    ///
    /// ```
    /// use enzui_widgets::app_shell::{SeamStyle, ShellTokens};
    ///
    /// assert_eq!(ShellTokens::default().seam, None);
    ///
    /// let flush = ShellTokens::default().pane_gutter(0.0).seam(SeamStyle::Hairline);
    /// assert_eq!(flush.seam, Some(SeamStyle::Hairline));
    /// assert_eq!(flush.effective_pane_gutter(), 0.0);
    /// assert_eq!(flush.effective_edge_inset(), ShellTokens::default().gap);
    /// ```
    #[must_use]
    pub fn seam(mut self, seam: SeamStyle) -> Self {
        self.seam = Some(seam);
        self
    }
    /// Store a seam thickness in logical pixels without immediate normalization.
    /// [`Self::effective_seam_thickness`] replaces negative or non-finite values with the default.
    #[must_use]
    pub fn seam_thickness(mut self, px: f32) -> Self {
        self.seam_thickness = Some(px);
        self
    }
    /// Pin seam ink independently of the runtime divider colour.
    #[must_use]
    pub fn seam_color(mut self, color: iced::Color) -> Self {
        self.seam_color = Some(color);
        self
    }
    /// Make the window frame glass of `style` over what lies behind the
    /// window; see [`Self::frame_material`].
    #[must_use]
    pub fn frame_material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.frame_material = Some(style);
        self
    }
    /// Store a sidebar-toggle anchor override.
    /// [`Self::effective_toggle_anchor`] keeps non-bar structural anchors unchanged.
    #[must_use]
    pub fn toggle_anchor(mut self, anchor: ToggleAnchor) -> Self {
        self.toggle_anchor = Some(anchor);
        self
    }
    /// Pin sidebar-toggle colours for the surface behind the control.
    /// Unset tokens use the runtime theme; other shell controls are unaffected.
    #[must_use]
    pub fn toggle_icon_tokens(mut self, tokens: crate::toolbar_icon::ToolbarIconTokens) -> Self {
        self.toggle_icon_tokens = Some(tokens);
        self
    }
}
