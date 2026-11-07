//! Application shell construction, content slots, and host event routing.

use super::*;

/// Layout-time application shell with caller-owned navigation and window state.
///
/// Slot closures must build fresh elements on repeated calls. Defaults to
/// `TitlebarContent`; choose a sidebar layout before supplying navigation.
#[must_use]
pub struct EnzAppShell<'a, Message> {
    pub(crate) layout: ShellLayout,
    pub(crate) tokens: ShellTokens,
    // Shell-level axis choices survive token replacement.
    pub(crate) split_axis_override: Option<SplitAxis>,
    pub(crate) breakpoints: BreakpointScale,
    pub(crate) state: NavSplitState,
    pub(crate) content: ContentSlot<'a, Message>,
    pub(crate) secondary_content: Option<ShellSlot<'a, Message>>,
    pub(crate) sidebar: Option<ShellSlot<'a, Message>>,
    pub(crate) titlebar: Option<ShellSlot<'a, Message>>,
    pub(crate) titlebar_background: Option<ShellSlot<'a, Message>>,
    pub(crate) subtitlebar: Option<ShellSlot<'a, Message>>,
    pub(crate) sidebar_titlebar: Option<ShellSlot<'a, Message>>,
    pub(crate) footer: Option<ShellSlot<'a, Message>>,
    #[allow(
        clippy::type_complexity,
        reason = "one boxed slot callback; an alias would only hide it"
    )]
    pub(crate) activity: Option<Box<dyn Fn(&ShellGeometry) -> Option<Element<'a, Message>> + 'a>>,
    #[allow(
        clippy::type_complexity,
        reason = "one boxed slot callback; an alias would only hide it"
    )]
    pub(crate) notifications:
        Option<Box<dyn Fn(&ShellGeometry) -> Option<Element<'a, Message>> + 'a>>,
    pub(crate) chrome_visible: bool,
    pub(crate) titlebar_scrim: bool,
    pub(crate) sidebar_available: bool,
    pub(crate) forced: Option<SidebarPresentation>,
    pub(crate) forced_by: Option<Box<dyn Fn(NavSplitState) -> SidebarPresentation + 'a>>,
    pub(crate) on_close: Option<Message>,
    pub(crate) island_actions: Option<Box<dyn Fn() -> Element<'a, Message> + 'a>>,
    pub(crate) island_material: Option<enzui_theme::MaterialStyle>,
    pub(crate) island_ink: Option<enzui_theme::GlassInk>,
    pub(crate) on_minimize: Option<Message>,
    pub(crate) on_maximize: Option<Message>,
    pub(crate) maximized: bool,
    pub(crate) fullscreen: bool,
    pub(crate) fullscreen_chrome_visible: bool,
    /// How the desktop decorates and arranges the window, read when the
    /// shell becomes an element.
    pub(crate) desktop: iced::window::Decorations,
    pub(crate) resizable: bool,
    pub(crate) on_window_gesture: Option<std::rc::Rc<dyn Fn(WindowCommand) -> Message + 'a>>,
    pub(crate) on_toggle: Option<Message>,
    pub(crate) on_sidebar_state: Option<Box<dyn Fn(NavSplitState) -> Message + 'a>>,
    pub(crate) on_dismiss_overlay: Option<Message>,
    pub(crate) sidebar_label: String,
    /// Whether the app named the sidebar; a named sidebar titles its drawer.
    pub(crate) sidebar_label_named: bool,
    pub(crate) sidebar_toggle_id: iced::advanced::widget::Id,
    pub(crate) sidebar_toggle_tooltip: bool,
    pub(crate) tooltip_delay: Option<std::time::Duration>,
    pub(crate) sidebar_toggle_side_inset: Option<f32>,
    /// The width the current build lays the shell out at.
    pub(crate) built_width: std::cell::Cell<f32>,
}

/// Build a shell with a content closure and default tokens.
///
/// Slots rebuild during layout and must produce a fresh element each time.
/// Layouts omit titlebar or sidebar slots for which they have no placement.
pub fn enz_app_shell<'a, Message: Clone + 'a>(
    content: impl Fn() -> Element<'a, Message> + 'a,
) -> EnzAppShell<'a, Message> {
    app_shell_with_content(ContentSlot::Unsized(Box::new(content)))
}

/// Build a shell whose content closure receives the resolved content-region width.
///
/// The logical-pixel width excludes outer insets, an inline sidebar, its gutter,
/// and its seam, but precedes sharing the region with a secondary pane. It is
/// [`ShellGeometry::content_width`], rather than the child slot's final bounds.
pub fn enz_app_shell_sized<'a, Message: Clone + 'a>(
    content: impl Fn(f32) -> Element<'a, Message> + 'a,
) -> EnzAppShell<'a, Message> {
    app_shell_with_content(ContentSlot::WidthAware(Box::new(content)))
}

/// Build a shell whose content closure receives its subtree's shell-wide geometry.
///
/// The same resolution shapes the subtree. Fields do not describe the receiving
/// slot's own bounds; see [`ShellGeometry`].
pub fn enz_app_shell_with_geometry<'a, Message: Clone + 'a>(
    content: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
) -> EnzAppShell<'a, Message> {
    app_shell_with_content(ContentSlot::GeometryAware(Box::new(content)))
}

fn app_shell_with_content<'a, Message: Clone + 'a>(
    content: ContentSlot<'a, Message>,
) -> EnzAppShell<'a, Message> {
    EnzAppShell {
        layout: ShellLayout::default(),
        tokens: ShellTokens::default(),
        split_axis_override: None,
        breakpoints: BreakpointScale::default(),
        state: NavSplitState::new(),
        content,
        secondary_content: None,
        footer: None,
        activity: None,
        notifications: None,
        sidebar: None,
        titlebar: None,
        titlebar_background: None,
        subtitlebar: None,
        sidebar_titlebar: None,
        chrome_visible: true,
        titlebar_scrim: false,
        sidebar_available: true,
        forced: None,
        forced_by: None,
        on_close: None,
        island_actions: None,
        island_material: None,
        island_ink: None,
        on_minimize: None,
        on_maximize: None,
        maximized: false,
        desktop: iced::window::Decorations::default(),
        fullscreen: false,
        fullscreen_chrome_visible: false,
        resizable: true,
        on_window_gesture: None,
        on_toggle: None,
        on_sidebar_state: None,
        on_dismiss_overlay: None,
        sidebar_label: enzui_core::localization::text("enzui-navigation-sidebar"),
        sidebar_label_named: false,
        sidebar_toggle_id: iced::advanced::widget::Id::from(SIDEBAR_TOGGLE_ID),
        sidebar_toggle_tooltip: true,
        tooltip_delay: None,
        sidebar_toggle_side_inset: None,
        built_width: std::cell::Cell::new(0.0),
    }
}

impl<'a, Message: Clone + 'a> EnzAppShell<'a, Message> {
    /// Choose the composition; defaults to `ShellLayout::TitlebarContent`.
    pub fn layout(mut self, layout: ShellLayout) -> Self {
        self.layout = layout;
        self
    }

    /// Replace the raw token set without clearing a shell-level split-axis override.
    ///
    /// Numeric values normalize when geometry and panes consume them.
    /// [`Self::split_axis`] takes precedence regardless of setter order.
    pub fn tokens(mut self, tokens: ShellTokens) -> Self {
        self.tokens = tokens;
        self
    }

    /// Supply the breakpoint scale used during layout.
    ///
    /// Pass the live theme's scale when it differs from the default; layout has no
    /// theme reference to resolve it automatically.
    pub fn breakpoints(mut self, breakpoints: BreakpointScale) -> Self {
        self.breakpoints = breakpoints;
        self
    }

    /// Supply the host's navigation state for this view.
    ///
    /// Store states emitted by [`Self::on_sidebar_state`] and pass them back here.
    /// If using [`Self::forced_by`], set this state before configuring that rule.
    pub fn state(mut self, state: NavSplitState) -> Self {
        self.state = state;
        self
    }

    /// Supply a second content pane, omitted when unset.
    ///
    /// The region splits along the requested axis with the general pane gutter.
    /// A beside split that cannot satisfy its fixed width or secondary minimum
    /// is dropped by default; `NarrowSplit::Restack` places it below instead.
    /// A requested below split is never dropped for horizontal fit.
    pub fn secondary_content(mut self, secondary: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.secondary_content = Some(ShellSlot::blind(secondary));
        self
    }

    /// Supply a secondary pane that receives shell-wide geometry.
    ///
    /// Read `split_axis` for the effective axis after narrow-split policy.
    /// The geometry does not report this pane's final bounds.
    pub fn secondary_content_with_geometry(
        mut self,
        secondary: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.secondary_content = Some(ShellSlot::geometry_aware(secondary));
        self
    }

    /// Override the requested content split axis independently of token replacement.
    ///
    /// The last shell-level override takes precedence over `ShellTokens::split_axis`.
    /// Without this call, the token's axis is used.
    pub fn split_axis(mut self, axis: SplitAxis) -> Self {
        self.split_axis_override = Some(axis);
        self
    }

    /// Supply navigation content, ignored by layouts without a sidebar.
    pub fn sidebar(mut self, sidebar: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.sidebar = Some(ShellSlot::blind(sidebar));
        self
    }

    /// Supply navigation content that receives its subtree's shell-wide geometry.
    ///
    /// During docked motion, the inline subtree receives its inline geometry; an
    /// open drawer receives overlay geometry. No field reports this slot's final box.
    pub fn sidebar_with_geometry(
        mut self,
        sidebar: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.sidebar = Some(ShellSlot::geometry_aware(sidebar));
        self
    }

    /// Supply titlebar content, ignored by layouts without a titlebar.
    ///
    /// Floating sidebar drawers enable [`crate::title_bar::EnzTitleBar::modal_navigation`]
    /// for native titlebars until the drawer leaves. Arbitrary slot elements must
    /// supply their own modal input and paint policy; the shell cannot distinguish
    /// window exits from custom controls inside an opaque element.
    pub fn titlebar(mut self, titlebar: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.titlebar = Some(ShellSlot::blind(titlebar));
        self
    }

    /// Supply titlebar content that receives shell-wide geometry.
    ///
    /// The closure fills the content-side band, including split or floating bands.
    /// For centered titlebar content, prefer [`crate::title_bar::EnzTitleBar::center`].
    pub fn titlebar_with_geometry(
        mut self,
        titlebar: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.titlebar = Some(ShellSlot::geometry_aware(titlebar));
        self
    }

    /// Paint the content titlebar beneath foreground clearance and window controls.
    ///
    /// The decoration fills the band, including the sidebar-toggle recovery area.
    /// Supports contained, split and floating titlebars; the sidebar's own header
    /// is excluded. The shell retains clipping, drag routing and control ownership.
    /// Decoration children receive no input, focus, accessibility operations or
    /// overlays; put commands in
    /// [`Self::titlebar`]. No band is created when titlebar chrome is absent.
    pub fn titlebar_background(
        mut self,
        background: impl Fn() -> Element<'a, Message> + 'a,
    ) -> Self {
        self.titlebar_background = Some(ShellSlot::blind(background));
        self
    }

    /// Paint a full-band decoration using the resolved shell geometry.
    ///
    /// Use `secondary_content_fits` and `split_axis` to omit a list header when
    /// its body is dropped or restacked. Background widths use the body pane
    /// widths without subtracting foreground toggle or window-control clearance.
    /// Input and clipping follow [`Self::titlebar_background`].
    pub fn titlebar_background_with_geometry(
        mut self,
        background: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.titlebar_background = Some(ShellSlot::geometry_aware(background));
        self
    }

    /// Supply a second band flush beneath a stacked titlebar.
    ///
    /// Requires positive `subtitlebar_height`; an unset slot or zero height consumes
    /// no band. `TitlebarContent` and `TitlebarSidebarContent` render it; floating
    /// and content-only layouts omit it. The shell sets uniform band radii to zero,
    /// but explicit per-corner arrays retain precedence. This band has no shell
    /// window-drag region.
    pub fn subtitlebar(mut self, bar: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.subtitlebar = Some(ShellSlot::blind(bar));
        self
    }

    /// Supply a subtitlebar that receives shell-wide geometry.
    ///
    /// Mounting and height follow [`Self::subtitlebar`].
    pub fn subtitlebar_with_geometry(
        mut self,
        bar: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.subtitlebar = Some(ShellSlot::geometry_aware(bar));
        self
    }

    /// Supply the sidebar header for inline navigation and open drawers.
    ///
    /// A closed drawer omits the header with its body. In `TitlebarSidebarContent`,
    /// `SidebarHeader::SpanningBand` drops this slot; supplying it there triggers a
    /// debug assertion when built. `FloatingTitlebarSidebar` always uses an inside
    /// header while inline; its drawer still follows the header token.
    pub fn sidebar_titlebar(mut self, bar: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.sidebar_titlebar = Some(ShellSlot::blind(bar));
        self
    }

    /// Supply a sidebar header that receives inline or overlay subtree geometry.
    ///
    /// Mounting follows [`Self::sidebar_titlebar`].
    pub fn sidebar_titlebar_with_geometry(
        mut self,
        bar: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.sidebar_titlebar = Some(ShellSlot::geometry_aware(bar));
        self
    }

    /// Supply a status strip beneath the whole content region, including both split panes.
    ///
    /// An unset slot consumes no strip. With a full-height inline sidebar the footer
    /// runs beside it; with split or spanning bands it runs beneath both panes.
    /// Floating sidebars overlay the content region rather than reserving a footer
    /// column. Height follows the slot content; `content_footer_gutter` controls
    /// the separation above it.
    pub fn footer(mut self, footer: impl Fn() -> Element<'a, Message> + 'a) -> Self {
        self.footer = Some(ShellSlot::blind(footer));
        self
    }

    /// Supply an optional activity surface placed by the shell.
    ///
    /// An inline sidebar or open drawer hosts it at the sidebar bottom. Otherwise,
    /// including when no sidebar slot is supplied, it floats at the content’s bottom
    /// leading corner (left in LTR, right in RTL).
    /// Returning `None` mounts no surface, while supplied-slot wrappers remain to
    /// preserve hosted pane state.
    ///
    /// An [`crate::activity_surface::EnzActivitySurface`] built inside this
    /// closure learns its site: with the default
    /// [`ActivityPresentation::Auto`](crate::activity_surface::ActivityPresentation::Auto)
    /// it is the compact status row at the sidebar's bottom and the detailed
    /// panel when floating.
    ///
    /// For [`enzui_core::operation_activity::ActivityCenter`] rows, the host
    /// must drive `prune` and synchronize [`enzui_core::operation_activity::ActivitySchedule`]
    /// after changes so completed rows disappear without another user action.
    pub fn activity(
        mut self,
        activity: impl Fn(&ShellGeometry) -> Option<Element<'a, Message>> + 'a,
    ) -> Self {
        self.activity = Some(Box::new(activity));
        self
    }

    /// Supply an optional status band between stacked titlebar chrome and content.
    ///
    /// Return `None` to omit the band. Keyed band stacking preserves content state
    /// when it appears or disappears. Horizontal padding follows
    /// `notifications_inset`; floating and content-only layouts omit this slot.
    pub fn notifications(
        mut self,
        notifications: impl Fn(&ShellGeometry) -> Option<Element<'a, Message>> + 'a,
    ) -> Self {
        self.notifications = Some(Box::new(notifications));
        self
    }

    /// Supply a footer that receives shell-wide geometry.
    ///
    /// Mounting follows [`Self::footer`]; the geometry does not report the footer's
    /// final width.
    pub fn footer_with_geometry(
        mut self,
        footer: impl Fn(&ShellGeometry) -> Element<'a, Message> + 'a,
    ) -> Self {
        self.footer = Some(ShellSlot::geometry_aware(footer));
        self
    }

    /// Show or hide floating chrome; defaults to true.
    ///
    /// A floating titlebar fades out while false and takes no input. Reveal
    /// toggles may remain as recovery chrome.
    /// This flag does not hide stacked titlebars or the inline sidebar in
    /// `FloatingTitlebarSidebar`. Pair it with
    /// [`crate::activity_pulse`] to hide the chrome while the pointer rests.
    pub fn chrome_visible(mut self, visible: bool) -> Self {
        self.chrome_visible = visible;
        self
    }

    /// Shade the content under a floating titlebar so its title and buttons
    /// stay legible over photos, video or scrolling pages; defaults to false.
    ///
    /// The shade runs from the shell's background colour at the top edge to
    /// transparent below the band, and fades with the chrome.
    pub fn titlebar_scrim(mut self, scrim: bool) -> Self {
        self.titlebar_scrim = scrim;
        self
    }

    /// Declare whether this route permits navigation; defaults to true.
    ///
    /// False overrides natural and forced presentation with `Hidden` and withholds
    /// the reveal toggle. Keep the sidebar slot supplied across routes to allow
    /// docked departure motion; removing the slot cuts directly to the new layout.
    pub fn sidebar_available(mut self, available: bool) -> Self {
        self.sidebar_available = available;
        self
    }

    /// Override sidebar presentation for layouts that permit navigation.
    ///
    /// [`Self::sidebar_available`] false still takes precedence. This sets a value
    /// without clearing any liveness-probe rule stored by [`Self::forced_by`].
    pub fn forced(mut self, presentation: SidebarPresentation) -> Self {
        self.forced = Some(presentation);
        self
    }

    /// Evaluate a sidebar presentation rule against the currently supplied state.
    ///
    /// Set [`Self::state`] before this call: it stores the rule's current result.
    /// The shell also retains the rule for toggle liveness probes, which may call
    /// it with alternative states under `ENZUI_ASSERT_SIDEBAR_TOGGLE_LIVE`.
    /// Use a pure rule without ambient mutable-state reads.
    pub fn forced_by(mut self, rule: impl Fn(NavSplitState) -> SidebarPresentation + 'a) -> Self {
        self.forced = Some(rule(self.state));
        self.forced_by = Some(Box::new(rule));
        self
    }

    /// Supply the close message; the control also requires `show_close` true.
    pub fn on_close(mut self, msg: Message) -> Self {
        self.on_close = Some(msg);
        self
    }

    /// Group supplied actions and window controls on a trailing glass island.
    ///
    /// Controls still require their tokens and messages. The island establishes a
    /// glass palette and increases titlebar height and window radius when needed
    /// for its fixed geometry.
    pub fn titlebar_island(
        mut self,
        style: enzui_theme::MaterialStyle,
        actions: impl Fn() -> Element<'a, Message> + 'a,
    ) -> Self {
        self.island_material = Some(style);
        self.island_actions = Some(Box::new(actions));
        self
    }

    /// Override titlebar island ink while preserving its material optics.
    /// Defaults to the selected material's ink policy.
    pub fn titlebar_island_ink(mut self, ink: enzui_theme::GlassInk) -> Self {
        self.island_ink = Some(ink);
        self
    }

    /// Supply the minimize message; the control also requires `show_minimize` true.
    /// The cluster requires `show_close` true and an [`Self::on_close`] message.
    pub fn on_minimize(mut self, msg: Message) -> Self {
        self.on_minimize = Some(msg);
        self
    }

    /// Supply the maximize message and state used to select its restore glyph.
    ///
    /// The control also requires `show_maximize` true. The cluster requires
    /// `show_close` true and an [`Self::on_close`] message. The state gates widget
    /// resize regions independently of whether the control is enabled.
    pub fn on_maximize(mut self, msg: Message, maximized: bool) -> Self {
        self.on_maximize = Some(msg);
        self.maximized = maximized;
        self
    }

    /// Set maximized presentation and disable widget resize regions while true.
    ///
    /// Defaults to false. Native window state also gates automatic resize support.
    pub fn maximized(mut self, maximized: bool) -> Self {
        self.maximized = maximized;
        self
    }

    /// Supply fullscreen presentation when native window state is unavailable.
    ///
    /// Native fullscreen is detected automatically. Fullscreen shows only the primary
    /// content, without shell chrome or pane insets, unless [`Self::fullscreen_chrome_visible`]
    /// is enabled. This builder does not request a native window transition; use
    /// [`ShellWindowState::set_fullscreen`](super::ShellWindowState::set_fullscreen).
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Retain application chrome during native fullscreen; disabled by default.
    ///
    /// Useful for workspace-style F11 modes. Immersive media keeps the default:
    /// primary content fills the output, and shell-owned sidebars, controls,
    /// notifications and footers are omitted. Supplied content owns its own controls.
    pub fn fullscreen_chrome_visible(mut self, visible: bool) -> Self {
        self.fullscreen_chrome_visible = visible;
        self
    }

    pub(crate) fn immersive_fullscreen(&self) -> bool {
        (self.fullscreen || self.desktop.fullscreen) && !self.fullscreen_chrome_visible
    }

    /// Permit widget resize regions; defaults to true.
    ///
    /// Native `iced::window::Settings::resizable` is authoritative independently of
    /// this additional widget policy gate.
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Route titlebar drag, double-click maximize, and system-menu intents.
    ///
    /// Without this callback, the shell adds no titlebar drag region. The host can
    /// convert [`WindowCommand`] into per-window tasks with `into_task`. Nested
    /// drag regions delegate to children before interpreting an uncaptured press.
    pub fn on_window_gesture(mut self, f: impl Fn(WindowCommand) -> Message + 'a) -> Self {
        self.on_window_gesture = Some(std::rc::Rc::new(f));
        self
    }

    /// Supply a bare sidebar-toggle message.
    ///
    /// Prefer [`Self::on_sidebar_state`], which publishes the already-toggled state
    /// and takes precedence. With a bare message, apply
    /// [`NavSplitState::toggle_for_presentation`] in the host using the shell's
    /// resolved presentation.
    pub fn on_toggle_sidebar(mut self, message: Message) -> Self {
        self.on_toggle = Some(message);
        self
    }

    /// Publish navigation state after applying a toggle for the resolved presentation.
    ///
    /// Store it and pass it back through [`Self::state`]. This handler precedes
    /// [`Self::on_toggle_sidebar`] and supplies drawer dismissal when an explicit
    /// [`Self::on_dismiss_overlay`] message is unset.
    pub fn on_sidebar_state(mut self, route: impl Fn(NavSplitState) -> Message + 'a) -> Self {
        self.on_sidebar_state = Some(Box::new(route));
        self
    }

    /// Publish a message when the drawer scrim is clicked or Escape is pressed.
    ///
    /// Apply [`NavSplitState::dismiss_overlay`] in the host. This overrides the
    /// state callback's dismissal; without it, the state callback supplies the
    /// dismissed state, followed by the bare toggle-message fallback.
    pub fn on_dismiss_overlay(mut self, message: Message) -> Self {
        self.on_dismiss_overlay = Some(message);
        self
    }

    /// Name the sidebar's navigation landmark, including its overlay drawer.
    ///
    /// Defaults to `SIDEBAR_LABEL` (`Navigation`). A named sidebar without a
    /// [`sidebar_titlebar`](Self::sidebar_titlebar) shows the name as its
    /// drawer's title.
    pub fn sidebar_label(mut self, label: impl Into<String>) -> Self {
        self.sidebar_label = label.into();
        self.sidebar_label_named = true;
        self
    }

    /// Override the app's hover delay for shell-owned controls; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Show the sidebar disclosure's delayed hover tooltip; enabled by default.
    ///
    /// Disabling the tooltip preserves the accessible name and expanded state.
    pub fn sidebar_toggle_tooltip(mut self, enabled: bool) -> Self {
        self.sidebar_toggle_tooltip = enabled;
        self
    }

    /// Inset the disclosure from its anchored chrome edge in pixels.
    ///
    /// Floating titlebar/sidebar layouts share the window edge inset in open
    /// and closed placements. Use [`ShellTokens::toggle_anchor`] with
    /// [`ToggleAnchor::LeadingBar`] for a fixed leading placement.
    /// Unset preserves the layout's default placement.
    pub fn sidebar_toggle_side_inset(mut self, inset: f32) -> Self {
        self.sidebar_toggle_side_inset = Some(FiniteNonNeg::new_or(inset, 0.0).get());
        self
    }

    /// Set the toggle's widget ID for accessibility action routing.
    ///
    /// Defaults to `SIDEBAR_TOGGLE_ID`. Use distinct IDs for multiple shell toggles
    /// in the same frame to avoid ambiguous targets.
    pub fn sidebar_toggle_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.sidebar_toggle_id = id;
        self
    }

    pub(crate) fn effective_tokens(&self) -> ShellTokens {
        let base = match self.split_axis_override {
            Some(axis) => ShellTokens {
                split_axis: axis,
                ..self.tokens
            },
            None => self.tokens,
        };
        let mut tokens = base.with_flush_defaults(self.layout);
        if self.island_material.is_some() {
            use super::tree::{ISLAND_HEIGHT, ISLAND_MARGIN};
            tokens.titlebar_height = tokens
                .titlebar_height
                .max(ISLAND_HEIGHT + 2.0 * ISLAND_MARGIN);
            tokens.window_radius = tokens
                .window_radius
                .max(ISLAND_HEIGHT / 2.0 + ISLAND_MARGIN);
        }
        tokens
    }

    /// Resolve this shell's policy for an assigned logical-pixel width.
    ///
    /// Applies split-axis overrides, flush defaults, island geometry, numeric
    /// normalization, forced presentation, and route availability. Reports policy
    /// even for unsupplied slots; it does not observe final layout bounds or paint.
    #[must_use]
    pub fn geometry_for(&self, available: f32) -> ShellGeometry {
        // Forced paths call geometry helpers directly, bypassing the public resolver guard.
        let tokens = self.effective_tokens().normalized();
        let mut geometry =
            resolve_shell_geometry(self.layout, self.state, tokens, self.breakpoints, available);
        let mut resolved_tokens = tokens;
        resolved_tokens.sidebar_width = geometry.sidebar_width;
        // The width is already resolved and must not be adapted a second time.
        resolved_tokens.sidebar_width_adaptive = false;
        if let Some(forced) = self.forced
            && self.layout.has_sidebar()
        {
            geometry.sidebar = forced;
            geometry.split_titlebar = splits_the_band(self.layout, resolved_tokens, forced);
            geometry.sidebar_full_height =
                sidebar_spans_full_height(self.layout, resolved_tokens, forced);
            geometry.content_inset = floating_content_inset(
                self.layout,
                resolved_tokens,
                forced == SidebarPresentation::Inline,
            );
            geometry.content_width =
                content_region_width(self.layout, resolved_tokens, forced, available);
            (geometry.secondary_content_fits, geometry.split_axis) = resolve_split(
                self.layout,
                resolved_tokens,
                self.breakpoints,
                forced,
                available,
            );
        }
        // Route availability outranks both natural and forced placement.
        if !self.sidebar_available && self.layout.has_sidebar() {
            let hidden = SidebarPresentation::Hidden;
            geometry.sidebar = hidden;
            geometry.split_titlebar = splits_the_band(self.layout, resolved_tokens, hidden);
            geometry.sidebar_full_height =
                sidebar_spans_full_height(self.layout, resolved_tokens, hidden);
            geometry.content_inset = floating_content_inset(self.layout, resolved_tokens, false);
            geometry.content_width =
                content_region_width(self.layout, resolved_tokens, hidden, available);
            (geometry.secondary_content_fits, geometry.split_axis) = resolve_split(
                self.layout,
                resolved_tokens,
                self.breakpoints,
                hidden,
                available,
            );
            geometry.shows_toggle = false;
        }
        if self.immersive_fullscreen() {
            geometry.sidebar = SidebarPresentation::Hidden;
            geometry.sidebar_width = 0.0;
            geometry.split_titlebar = false;
            geometry.sidebar_full_height = false;
            geometry.shows_toggle = false;
            geometry.content_inset = [0.0; 4];
            geometry.content_corner_radii = [0.0; 4];
            geometry.secondary_content_corner_radii = [0.0; 4];
            geometry.content_width = available.max(0.0);
            geometry.secondary_content_fits = false;
        }
        geometry
    }
}
