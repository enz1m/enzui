//! Compose application content, chrome and adaptive navigation into rounded panes.
//!
//! Content is required; other slots depend on [`ShellLayout`]. Slot closures may
//! run more than once during layout. Geometry-aware slots receive the shell's
//! resolution, not their own assigned box; [`ShellGeometry::content_width`] reports
//! the content region before a secondary pane shares it.
//!
//! [`EnzAppShell::titlebar_background_with_geometry`] paints full-band secondary
//! and reader surfaces beneath foreground toggle clearance. Decorations retain
//! shell clipping and do not participate in input, focus or accessibility.
//! See the `reference_three_pane` example.
//!
//! [`ShellTokens`] selects pane styles, insets, headers and split policy. Seam
//! layouts resolve flush defaults unless explicitly opted out. Narrow beside
//! splits drop secondary content by default; [`NarrowSplit::Restack`] stacks it.
//!
//! Supply host-owned [`NavSplitState`](crate::nav_split::NavSplitState) and retain updates from
//! [`EnzAppShell::on_sidebar_state`]. Window controls and gestures also require
//! host callbacks. Match shell decoration with
//! [`enz_window_settings_for`](crate::app_shell::enz_window_settings_for);
//! [`DesktopApplicationIdentity`](crate::app_shell::DesktopApplicationIdentity) supplies
//! the installed desktop-entry basename.

use crate::exclusive_focus::exclusive_focus;
use crate::keyboard_capture::keyboard_capture_unscoped;
use crate::modal_host::scrim_color;
use crate::nav_split::{
    NavSplitGeometry, NavSplitState, SidebarPresentation, SidebarWidth, resolve_for_width,
};
use crate::scrim_click::ScrimClick;
use crate::semantics_container::SemanticsContainer;
use crate::surface::{BorderStyle, SurfaceTone, enz_surface};
use crate::title_bar_gesture::WindowCommand;
use crate::titlebar_drag_region::titlebar_drag_region;
use crate::toolbar_icon::{ToolbarIconVariant, enz_toolbar_icon};
use crate::window_controls::enz_window_controls;
use enzui_core::metrics::FiniteNonNeg;
use enzui_core::placement::{Direction, resolve_direction};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{BreakpointScale, SpaceToken, SpacingScale};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, overlay, renderer};
use iced::widget::{Space, container, row, stack};
use iced::{Element, Event, Length, Padding, Rectangle, Size, Vector, mouse};
/// Default window inset and inter-pane gap, in logical pixels.
pub const DEFAULT_SHELL_GAP: f32 = 8.0;
/// Recommended spacing token for the page-top inset.
pub const PAGE_INSET_TOKEN: SpaceToken = SpaceToken::Sm;
/// Default side inset for notifications in flush layouts, in logical pixels.
pub const DEFAULT_NOTIFICATIONS_INSET: f32 = 12.0;
/// The sidebar toggle's stand-off from the window's leading edge on a flush shell; see [`ShellTokens::effective_toggle_lead`].
pub const TOGGLE_EDGE_LEAD: f32 = 8.0;
/// Recommended page-top inset on the stock spacing scale (8 px).
pub const DEFAULT_PAGE_INSET: f32 = 8.0;
/// Logical pixels reserved between panes for [`SeamStyle::Hairline`].
pub const SEAM_RULE_THICKNESS: f32 = 1.0;
/// Default titlebar band height, in logical pixels: the same value as a
/// standalone [`crate::title_bar::enz_title_bar`], so a bar looks the same
/// inside and outside a shell.
pub const DEFAULT_SHELL_TITLEBAR_HEIGHT: f32 = crate::title_bar::DEFAULT_TITLEBAR_HEIGHT;
/// The default sidebar width, in logical px.
pub const DEFAULT_SHELL_SIDEBAR_WIDTH: f32 = 260.0;
/// Default outer window corner radius, in logical pixels, where no theme sets
/// `window.radius`.
pub const DEFAULT_WINDOW_RADIUS: f32 = enzui_theme::DEFAULT_WINDOW_RADIUS;
/// The default pane corner radius, in logical px: the window's, 2 px tighter.
pub const DEFAULT_PANE_RADIUS: f32 = DEFAULT_WINDOW_RADIUS - 2.0;
/// Default minimum content width beside an inline sidebar, in logical pixels.
pub const DEFAULT_CONTENT_MINIMUM: f32 = 320.0;
/// Fallback secondary-pane width for negative or non-finite inputs, in logical pixels.
pub const DEFAULT_SECONDARY_WIDTH: f32 = 280.0;
/// Bundled glyph for hiding the visible sidebar in left-to-right layouts.
pub const TOGGLE_ICON_COLLAPSE: &str = "panel-left-close";
/// The glyph the shell's collapse toggle draws while the sidebar is away — pressing it brings it back. Lucide `panel-left-open`.
pub const TOGGLE_ICON_EXPAND: &str = "panel-left-open";
/// Bundled glyph for hiding the visible sidebar in right-to-left layouts.
pub const TOGGLE_ICON_COLLAPSE_RTL: &str = "panel-right-close";
/// The expand glyph for a right-to-left layout.
pub const TOGGLE_ICON_EXPAND_RTL: &str = "panel-right-open";
/// English fallback action name for hiding the visible sidebar.
pub const TOGGLE_LABEL_COLLAPSE: &str = "Hide sidebar";
/// English fallback name of the toggle while the sidebar is away.
pub const TOGGLE_LABEL_EXPAND: &str = "Show sidebar";
#[doc(hidden)]
pub mod acceptance_probe;
#[doc(hidden)]
pub mod build_probe;
mod builder;
pub mod capture_hold;
mod clip_beyond;
#[doc(hidden)]
pub mod frame_cost;
mod geometry;
mod mirror_row;
mod paint_layer;
mod presets;
mod seam;
mod seam_rider;
mod toggle_assert;
mod token_builders;
mod tree;
mod widget;
mod window_settings;
pub mod window_state;

pub use builder::*;
pub use geometry::*;
pub use presets::*;
pub use toggle_assert::*;
pub(crate) use widget::*;
#[doc(hidden)]
pub use widget::{MotionLayoutRoute, motion_layout_route};
pub use window_settings::*;
pub use window_state::{ShellWindowEvent, ShellWindowIdentity, ShellWindowState};
/// How the page meets titlebar and subtitlebar chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PagePresentation {
    /// Use the effective pane gutter between chrome and page content.
    #[default]
    Standard,
    /// Close chrome-to-content gutters; footer and sibling-pane spacing remain independent.
    Flush,
}
/// Shell pane styles, geometry and responsive policies.
/// Lengths are logical pixels. Defaults use the constants above; [`Self::from_theme`]
/// resolves gap and radii from theme scales. [`Self::normalized`] guards raw numeric values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShellTokens {
    /// Fallback edge inset and general pane gutter; defaults to [`DEFAULT_SHELL_GAP`].
    /// Unset overrides follow this value; layout-specific flush defaults may replace them.
    pub gap: f32,
    /// Window-edge inset override; unset falls back to gap before flush resolution.
    /// Seam layouts fill an unset inset with zero unless flush is disabled. Explicit values survive.
    pub edge_inset: Option<f32>,
    /// General inter-pane gutter override; unset follows gap.
    /// Flush sidebar/content seams use a separate zero gutter while vertical gutters remain.
    pub pane_gutter: Option<f32>,
    /// Chrome-to-content spacing policy; defaults to [`PagePresentation::Standard`].
    pub page_presentation: PagePresentation,
    /// Top padding inside primary and secondary content panes; defaults to zero.
    /// Use [`PAGE_INSET_TOKEN`] for opt-in spacing and remove any equivalent host padding.
    pub page_inset: f32,
    /// Notification-band side inset override.
    /// Unset uses [`DEFAULT_NOTIFICATIONS_INSET`] when the resolved edge inset is zero, otherwise zero.
    pub notifications_inset: Option<f32>,
    /// Outer window corner radius; defaults to the theme's `window.radius`
    /// ([`DEFAULT_WINDOW_RADIUS`] where the theme file sets none).
    pub window_radius: f32,
    /// Fallback pane corner radius; defaults to the window radius, 2 px
    /// tighter ([`DEFAULT_PANE_RADIUS`] where the theme file sets none).
    /// Pane-specific uniform or corner-array pins override it.
    pub pane_radius: f32,
    /// Titlebar-band height; defaults to [`DEFAULT_SHELL_TITLEBAR_HEIGHT`].
    pub titlebar_height: f32,
    /// Subtitlebar-band height; defaults to zero.
    /// A positive normalized height and a supplied subtitlebar slot are both required.
    pub subtitlebar_height: f32,
    /// Sidebar width; defaults to [`DEFAULT_SHELL_SIDEBAR_WIDTH`].
    /// The untouched default may narrow in inline presentation.
    pub sidebar_width: f32,
    /// Whether the default sidebar width may yield space to content.
    /// The setter pins any supplied width; non-default raw widths also disable adaptation.
    #[doc(hidden)]
    pub sidebar_width_adaptive: bool,
    /// Content width reserved beside an inline sidebar or secondary pane.
    /// Defaults to [`DEFAULT_CONTENT_MINIMUM`].
    pub content_minimum: f32,
    /// Secondary-pane minimum when no fixed secondary width is set.
    /// Unset follows content_minimum; sidebar collapse is unaffected.
    pub secondary_minimum: Option<f32>,
    /// Shell backdrop tone; defaults to [`SurfaceTone::Background`].
    pub shell_tone: SurfaceTone,
    /// The window frame's material: the backdrop layer around and between
    /// the panes as glass over the desktop and the windows behind, filtered
    /// by the compositor. `None`, the default, keeps it opaque. The theme's
    /// `window.frame` decides over it for every app; where the compositor
    /// paints no glass, the frame stays opaque. The panes stay filled: a
    /// blended pane takes [`Self::shell_tone`].
    pub frame_material: Option<enzui_theme::MaterialStyle>,
    /// Primary content pane style; defaults to [`PaneStyle::blended`].
    pub content: PaneStyle,
    /// Secondary content pane style; defaults to blended independently of the primary.
    /// Changing the primary style does not update this field.
    pub secondary_content: PaneStyle,
    /// Fixed secondary width for a requested beside split; unset shares the region.
    /// Narrow splits follow [`Self::narrow_split`]. Restacking retains a pinned width;
    /// a requested below split uses full width.
    pub secondary_width: Option<f32>,
    /// Requested secondary split axis; defaults to [`SplitAxis::Beside`].
    /// The resolved axis may change under [`NarrowSplit::Restack`].
    pub split_axis: SplitAxis,
    /// Narrow beside-split policy; defaults to [`NarrowSplit::Drop`].
    pub narrow_split: NarrowSplit,
    /// Whether secondary content precedes primary content; defaults to false.
    /// Width and style remain properties of the secondary pane.
    pub secondary_leading: bool,
    /// Sidebar pane style; defaults to [`PaneStyle::recessed`].
    pub sidebar: PaneStyle,
    /// Titlebar pane style; defaults to blended independently of the subtitlebar.
    pub titlebar: PaneStyle,
    /// Subtitlebar pane style; defaults to blended.
    /// Rendering sets its uniform radius to zero, but explicit corner arrays still win.
    pub subtitlebar: PaneStyle,
    /// Footer pane style; defaults to blended.
    pub footer: PaneStyle,
    /// Sidebar-header placement; defaults to [`SidebarHeader::Inside`].
    pub sidebar_header: SidebarHeader,
    /// Whether normal resolution may collapse the sidebar; defaults to true.
    /// False keeps it inline and removes the toggle.
    pub sidebar_collapsible: bool,
    /// Close-control visibility; defaults to true and requires a host action callback.
    pub show_close: bool,
    /// Minimize-control visibility; defaults to false.
    /// Route its action with [`EnzAppShell::on_minimize`]; the cluster also requires
    /// `show_close` true and [`EnzAppShell::on_close`].
    pub show_minimize: bool,
    /// Maximize-control visibility; defaults to false.
    /// Route its action with [`EnzAppShell::on_maximize`]; the cluster also requires
    /// `show_close` true and [`EnzAppShell::on_close`].
    pub show_maximize: bool,
    /// Whether an overlay drawer occupies the full shell width; defaults to false.
    pub full_width_drawer: bool,
    /// Sidebar-first narrow presentation preference; defaults to false.
    /// Initialize overlay state separately with [`NavSplitState::for_sidebar_first`].
    /// Inline presentation is unaffected.
    ///
    /// ```
    /// use enzui_widgets::app_shell::ShellTokens;
    /// use enzui_widgets::nav_split::NavSplitState;
    ///
    /// let tokens = ShellTokens::default().sidebar_first(true);
    /// let state = NavSplitState::for_sidebar_first(tokens.sidebar_first);
    /// assert!(state.overlay_open);
    /// ```
    pub sidebar_first: bool,
    /// Seam-style override; unset resolves to hairline on flush seam layouts.
    /// Explicit [`SeamStyle::None`] opts out of flush resolution as well as removing the rule.
    pub seam: Option<SeamStyle>,
    /// Seam-slot thickness; unset or invalid values use [`SEAM_RULE_THICKNESS`].
    /// A slot-consuming seam reserves this width from the content region; zero is accepted.
    pub seam_thickness: Option<f32>,
    /// Seam ink override; unset uses the runtime divider colour.
    pub seam_color: Option<iced::Color>,
    /// Sidebar-toggle anchor override.
    /// Non-bar structural anchors ignore it; unset bar anchors follow layout/flush resolution.
    pub toggle_anchor: Option<ToggleAnchor>,
    /// Sidebar-toggle colour overrides; unset uses runtime-theme tokens.
    /// For blended panes over a host canvas, choose colours for the painted surface.
    pub toggle_icon_tokens: Option<crate::toolbar_icon::ToolbarIconTokens>,
    /// Writing-direction override; unset uses the ambient locale.
    /// RTL mirrors sidebar placement, floating-sidebar inset, toggle glyphs and seam corners.
    pub direction: Option<Direction>,
}

impl Default for ShellTokens {
    fn default() -> Self {
        Self {
            gap: enzui_theme::text_defaults::spacing().sm(),
            edge_inset: None,
            pane_gutter: None,
            page_presentation: PagePresentation::Standard,
            page_inset: 0.0,
            notifications_inset: None,
            // The theme's window radius; panes sit 2 px tighter inside it.
            window_radius: enzui_theme::text_defaults::window_radius(),
            pane_radius: (enzui_theme::text_defaults::window_radius() - 2.0).max(0.0),
            titlebar_height: enzui_theme::text_defaults::sizing().titlebar_height,
            subtitlebar_height: 0.0,
            sidebar_width: enzui_theme::text_defaults::sizing().sidebar_width,
            sidebar_width_adaptive: true,
            content_minimum: DEFAULT_CONTENT_MINIMUM,
            secondary_minimum: None,
            shell_tone: SurfaceTone::Background,
            frame_material: None,
            content: PaneStyle::blended(),
            secondary_content: PaneStyle::blended(),
            secondary_width: None,
            split_axis: SplitAxis::default(),
            narrow_split: NarrowSplit::default(),
            secondary_leading: false,
            sidebar: PaneStyle::recessed(),
            titlebar: PaneStyle::blended(),
            subtitlebar: PaneStyle::blended(),
            footer: PaneStyle::blended(),
            sidebar_header: SidebarHeader::default(),
            sidebar_collapsible: true,
            show_close: true,
            show_minimize: false,
            show_maximize: false,
            full_width_drawer: false,
            sidebar_first: false,
            seam: None,
            seam_thickness: None,
            seam_color: None,
            toggle_anchor: None,
            toggle_icon_tokens: None,
            direction: None,
        }
    }
}
/// Alias for [`ShellTokens`] with the module-based token name.
pub type AppShellTokens = ShellTokens;

impl ShellTokens {
    /// Resolve gap from spacing.sm and radii from radius.xl and radius.lg.
    /// Other tokens retain defaults, and the resulting numeric values are normalized.
    #[must_use]
    pub fn from_theme(theme: &enzui_theme::EnzTheme) -> Self {
        Self {
            gap: theme.spacing.sm(),
            window_radius: theme.radius.xl,
            pane_radius: theme.radius.lg,
            ..Self::default()
        }
        .normalized()
    }
}
