//! Shell layout presets, placement policy, and pane styling.

use super::*;

/// Placement of sidebar-side titlebar content in `TitlebarSidebarContent`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarHeader {
    /// Header inside the full-height sidebar pane; the default.
    #[default]
    Inside,
    /// Separate sidebar and content titlebar panes aligned at their seam.
    SplitBand,
    /// One titlebar above both panes.
    ///
    /// The `sidebar_titlebar` slot is omitted; put band content in
    /// [`EnzAppShell::titlebar`].
    SpanningBand,
}

impl SidebarHeader {
    /// All current header modes in gallery order.
    pub const ALL: [SidebarHeader; 3] = [
        SidebarHeader::Inside,
        SidebarHeader::SplitBand,
        SidebarHeader::SpanningBand,
    ];

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SidebarHeader::Inside => "header-inside",
            SidebarHeader::SplitBand => "header-split-band",
            SidebarHeader::SpanningBand => "header-spanning-band",
        }
    }

    /// Whether this header mode places its header inside the full-height sidebar.
    ///
    /// This predicate applies to `TitlebarSidebarContent`; other layouts may
    /// resolve full-height placement independently.
    #[must_use]
    pub fn sidebar_is_full_height(self) -> bool {
        matches!(self, SidebarHeader::Inside)
    }

    /// Whether this mode permits sidebar-side titlebar content.
    ///
    /// False for `SpanningBand`; actual mounting also depends on layout and slots.
    #[must_use]
    pub fn uses_sidebar_titlebar_slot(self) -> bool {
        !matches!(self, SidebarHeader::SpanningBand)
    }
}

/// Requested placement of the secondary content pane relative to the primary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitAxis {
    /// Secondary beside primary; the default, subject to narrow-split policy.
    #[default]
    Beside,
    /// Secondary below primary; does not compete for horizontal fit.
    Below,
}

impl SplitAxis {
    /// All current split axes in gallery order.
    pub const ALL: [SplitAxis; 2] = [SplitAxis::Beside, SplitAxis::Below];

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SplitAxis::Beside => "split-beside",
            SplitAxis::Below => "split-below",
        }
    }

    /// Whether the panes compete side by side for width.
    #[must_use]
    pub fn is_horizontal(self) -> bool {
        matches!(self, SplitAxis::Beside)
    }
}

/// Policy for a beside split whose fixed width or minimum does not fit.
///
/// A requested `SplitAxis::Below` bypasses this width policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NarrowSplit {
    /// Omit the secondary pane and report `secondary_content_fits` as false.
    ///
    /// The default policy.
    #[default]
    Drop,
    /// Place the secondary below the primary and report the effective axis as
    /// `Below`, with `secondary_content_fits` true.
    ///
    /// A fixed width requested for `Beside` is retained when restacked.
    Restack,
}

impl NarrowSplit {
    /// All current narrow-split policies in gallery order.
    pub const ALL: [NarrowSplit; 2] = [NarrowSplit::Drop, NarrowSplit::Restack];

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            NarrowSplit::Drop => "narrow-drop",
            NarrowSplit::Restack => "narrow-restack",
        }
    }

    /// Whether an insufficient horizontal fit selects a below split.
    #[must_use]
    pub fn restacks(self) -> bool {
        matches!(self, NarrowSplit::Restack)
    }
}

/// Shell compositions with stacked or floating titlebar and sidebar chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellLayout {
    /// Content without shell titlebar or sidebar chrome.
    ContentOnly,
    /// Titlebar above content; the default composition.
    #[default]
    TitlebarContent,
    /// Sidebar beside content, with titlebar placement selected by `SidebarHeader`.
    ///
    /// The default header is inside the full-height sidebar.
    TitlebarSidebarContent,
    /// Sidebar over content instead of reserving content-region width.
    FloatingSidebar,
    /// Titlebar over content; supports [`EnzAppShell::chrome_visible`].
    FloatingTitlebar,
    /// Full-height sidebar beside content, with a titlebar floating over content.
    ///
    /// While inline, the sidebar owns its header independently of `SidebarHeader`.
    /// Overlay drawers follow the header token.
    FloatingTitlebarSidebar,
}

impl ShellLayout {
    /// All current layout presets in gallery order.
    pub const ALL: [ShellLayout; 6] = [
        ShellLayout::ContentOnly,
        ShellLayout::TitlebarContent,
        ShellLayout::TitlebarSidebarContent,
        ShellLayout::FloatingSidebar,
        ShellLayout::FloatingTitlebar,
        ShellLayout::FloatingTitlebarSidebar,
    ];

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ShellLayout::ContentOnly => "content-only",
            ShellLayout::TitlebarContent => "titlebar-content",
            ShellLayout::TitlebarSidebarContent => "titlebar-sidebar-content",
            ShellLayout::FloatingSidebar => "floating-sidebar",
            ShellLayout::FloatingTitlebar => "floating-titlebar",
            ShellLayout::FloatingTitlebarSidebar => "floating-titlebar-sidebar",
        }
    }

    /// Whether this composition permits a sidebar slot.
    #[must_use]
    pub fn has_sidebar(self) -> bool {
        matches!(
            self,
            ShellLayout::TitlebarSidebarContent
                | ShellLayout::FloatingSidebar
                | ShellLayout::FloatingTitlebarSidebar
        )
    }

    /// Whether an inline sidebar reserves width beside content.
    ///
    /// These layouts receive flush defaults when built unless explicitly opted out.
    #[must_use]
    pub fn sidebar_beside_content(self) -> bool {
        matches!(
            self,
            ShellLayout::TitlebarSidebarContent | ShellLayout::FloatingTitlebarSidebar
        )
    }

    /// Whether this composition permits a titlebar slot.
    #[must_use]
    pub fn has_titlebar(self) -> bool {
        matches!(
            self,
            ShellLayout::TitlebarContent
                | ShellLayout::TitlebarSidebarContent
                | ShellLayout::FloatingTitlebar
                | ShellLayout::FloatingTitlebarSidebar
        )
    }

    /// Whether the composition layers any chrome over content.
    ///
    /// This classification does not imply a positive resolved inset when chrome
    /// is hidden or dimensions are zero.
    #[must_use]
    pub fn is_floating(self) -> bool {
        matches!(
            self,
            ShellLayout::FloatingSidebar
                | ShellLayout::FloatingTitlebar
                | ShellLayout::FloatingTitlebarSidebar
        )
    }

    /// Whether the titlebar overlays content instead of reserving height above it.
    #[must_use]
    pub fn floats_titlebar(self) -> bool {
        matches!(
            self,
            ShellLayout::FloatingTitlebar | ShellLayout::FloatingTitlebarSidebar
        )
    }

    /// Layout's candidate toggle position before token overrides and flush defaults.
    #[must_use]
    pub fn toggle_anchor(self) -> ToggleAnchor {
        match self {
            ShellLayout::ContentOnly => ToggleAnchor::None,
            ShellLayout::FloatingSidebar => ToggleAnchor::FloatingOverContent,
            ShellLayout::TitlebarContent
            | ShellLayout::TitlebarSidebarContent
            | ShellLayout::FloatingTitlebar
            | ShellLayout::FloatingTitlebarSidebar => ToggleAnchor::LeadingBar,
        }
    }

    /// Every layout draws its own frame: titlebar layouts in their band,
    /// titlebar-less ones with window controls floating at the top trailing
    /// corner. A desktop frame is the explicit
    /// [`ShellFrame::Desktop`](crate::app_shell::ShellFrame::Desktop) opt-in.
    ///
    /// Use [`crate::app_shell::enz_window_settings`] to apply the request.
    #[must_use]
    pub fn window_decorations(self) -> WindowDecorations {
        WindowDecorations::ClientSide
    }
}

/// Candidate position for a sidebar collapse/reveal toggle.
///
/// An anchor alone does not enable a toggle; the layout, collapse policy, and
/// callback must permit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleAnchor {
    /// No candidate position.
    None,
    /// Leading end of the sidebar header while inline, or the titlebar when collapsed.
    LeadingBar,
    /// Trailing end of that bar.
    ///
    /// Flush defaults select this anchor for layouts with a sidebar beside content,
    /// unless an explicit anchor overrides it.
    TrailingBar,
    /// Top-leading corner over content, without a titlebar band.
    FloatingOverContent,
}

impl ToggleAnchor {
    /// All current toggle anchors in gallery order.
    pub const ALL: [ToggleAnchor; 4] = [
        ToggleAnchor::None,
        ToggleAnchor::LeadingBar,
        ToggleAnchor::TrailingBar,
        ToggleAnchor::FloatingOverContent,
    ];

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            ToggleAnchor::None => "toggle-none",
            ToggleAnchor::LeadingBar => "toggle-leading-bar",
            ToggleAnchor::TrailingBar => "toggle-trailing-bar",
            ToggleAnchor::FloatingOverContent => "toggle-floating-over-content",
        }
    }

    /// Whether this anchor selects the trailing end of a bar.
    #[must_use]
    pub fn is_trailing(self) -> bool {
        matches!(self, ToggleAnchor::TrailingBar)
    }

    /// Whether the anchor names a position rather than `None`.
    #[must_use]
    pub fn is_placeable(self) -> bool {
        !matches!(self, ToggleAnchor::None)
    }
}

/// Requested window decoration ownership.
///
/// Pass the resulting flag to iced window settings; the compositor decides how
/// to honor the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowDecorations {
    /// Request no compositor frame; the shell supplies titlebar chrome.
    ClientSide,
    /// Request compositor decorations; what
    /// [`ShellFrame::Desktop`](crate::app_shell::ShellFrame::Desktop) asks for.
    ServerSide,
}

impl WindowDecorations {
    /// All current decoration requests in gallery order.
    pub const ALL: [WindowDecorations; 2] =
        [WindowDecorations::ClientSide, WindowDecorations::ServerSide];

    /// The decoration request for `iced::window::Settings::decorations`.
    #[must_use]
    pub fn iced_decorations_flag(self) -> bool {
        matches!(self, WindowDecorations::ServerSide)
    }

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            WindowDecorations::ClientSide => "csd",
            WindowDecorations::ServerSide => "ssd",
        }
    }
}

/// Sidebar/content seam slot while the sidebar is inline beside content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SeamStyle {
    /// No seam slot; an explicit selection opts out of flush defaults.
    ///
    /// This is the enum default. Flush shell defaults instead select `Hairline`.
    #[default]
    None,
    /// Reserve seam width without ink; thickness defaults to one logical pixel.
    Flat,
    /// Paint the seam in the theme divider color; thickness defaults to one
    /// logical pixel and can be configured with `seam_thickness`.
    Hairline,
}

impl SeamStyle {
    /// All current seam styles in gallery order.
    pub const ALL: [SeamStyle; 3] = [SeamStyle::None, SeamStyle::Flat, SeamStyle::Hairline];

    /// Stable caption and filename identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            SeamStyle::None => "seam-none",
            SeamStyle::Flat => "seam-flat",
            SeamStyle::Hairline => "seam-hairline",
        }
    }

    /// Whether this style reserves a seam slot.
    ///
    /// `Flat` reserves width without painting ink.
    #[must_use]
    pub fn draws_rule(self) -> bool {
        matches!(self, SeamStyle::Flat | SeamStyle::Hairline)
    }
}

/// Tone, outline, corner radii, and optional glass treatment for one shell pane.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneStyle {
    /// Toned fill, used when no glass material is selected.
    pub tone: SurfaceTone,
    /// Pane outline style.
    pub border: BorderStyle,
    /// Uniform radius in logical pixels, or `None` to inherit `pane_radius`.
    ///
    /// An explicit `radius_corners` array takes precedence.
    pub radius: Option<f32>,
    /// Optional radii as `[top_left, top_right, bottom_left, bottom_right]`.
    ///
    /// Overrides the uniform radius and shell fallback.
    pub radius_corners: Option<[f32; 4]>,
    /// Optional glass material in place of the toned fill; defaults to `None`.
    ///
    /// Rows and controls in a glass pane inherit glass treatment; see [`crate::glass`].
    pub material: Option<enzui_theme::MaterialStyle>,
    /// Glass sampling source; defaults to the app's drawing.
    pub backdrop: crate::desktop_glass::Backdrop,
    /// Grain on the pane, or none (the default): a grainy sidebar, titlebar
    /// or content pane. See [`crate::grain`].
    pub grain: enzui_theme::GrainSpec,
}

impl Default for PaneStyle {
    fn default() -> Self {
        Self {
            tone: SurfaceTone::Surface,
            border: BorderStyle::None,
            radius: None,
            radius_corners: None,
            material: None,
            backdrop: crate::desktop_glass::Backdrop::App,
            grain: enzui_theme::GrainSpec::None,
        }
    }
}

impl PaneStyle {
    /// Surface fill, no border, and inherited corner radii.
    ///
    /// This is `PaneStyle::default`; shell content uses a blended style by default.
    #[must_use]
    pub fn raised() -> Self {
        Self::default()
    }

    /// Alternate surface fill with no border and inherited radii.
    #[must_use]
    pub fn recessed() -> Self {
        Self {
            tone: SurfaceTone::SurfaceAlt,
            ..Self::default()
        }
    }

    /// Surface fill with a thin border and inherited radii.
    #[must_use]
    pub fn outlined() -> Self {
        Self {
            border: BorderStyle::Thin,
            ..Self::default()
        }
    }

    /// Transparent tone without border or glass material.
    #[must_use]
    pub fn blended() -> Self {
        Self {
            tone: SurfaceTone::Transparent,
            border: BorderStyle::None,
            radius: None,
            radius_corners: None,
            material: None,
            backdrop: crate::desktop_glass::Backdrop::App,
            grain: enzui_theme::GrainSpec::None,
        }
    }

    /// Elevated surface fill with a thin border and inherited radii.
    #[must_use]
    pub fn floating() -> Self {
        Self {
            tone: SurfaceTone::Elevated,
            border: BorderStyle::Thin,
            radius: None,
            radius_corners: None,
            material: None,
            backdrop: crate::desktop_glass::Backdrop::App,
            grain: enzui_theme::GrainSpec::None,
        }
    }

    /// Select the toned fill without clearing any glass material.
    #[must_use]
    pub fn tone(mut self, tone: SurfaceTone) -> Self {
        self.tone = tone;
        self
    }

    /// Select glass treatment in place of the toned fill.
    #[must_use]
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Lay grain on this pane: a [`Grain`](enzui_theme::Grain), or
    /// [`GrainSpec::Theme`](enzui_theme::GrainSpec::Theme) for the theme's.
    #[must_use]
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Request desktop sampling for this pane's glass material.
    ///
    /// See [`crate::desktop_glass`] for window setup and fallback behavior.
    #[must_use]
    pub fn behind_window(mut self) -> Self {
        self.backdrop = crate::desktop_glass::Backdrop::Window;
        self
    }

    /// Select the pane outline.
    #[must_use]
    pub fn border(mut self, border: BorderStyle) -> Self {
        self.border = border;
        self
    }

    /// Pin the uniform radius in logical pixels.
    ///
    /// Negative or non-finite values become zero. Existing per-corner pins retain
    /// precedence.
    #[must_use]
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }

    /// Pin `[top_left, top_right, bottom_left, bottom_right]` radii in logical pixels.
    ///
    /// Each negative or non-finite value becomes zero; this array takes precedence
    /// over the uniform radius.
    #[must_use]
    pub fn radius_corners(mut self, corners: [f32; 4]) -> Self {
        self.radius_corners = Some(corners.map(|c| FiniteNonNeg::new_or(c, 0.0).get()));
        self
    }

    /// Return the uniform pin, or the supplied fallback, without normalization.
    #[must_use]
    pub fn radius_or(&self, fallback: f32) -> f32 {
        self.radius.unwrap_or(fallback)
    }

    /// Return the corner array, uniform pin, or supplied fallback in that order.
    ///
    /// Values are returned without normalization.
    #[must_use]
    pub fn radius_corners_or(&self, fallback: f32) -> [f32; 4] {
        self.radius_corners
            .unwrap_or_else(|| [self.radius_or(fallback); 4])
    }

    /// Normalize radius pins from public fields, preserving absent overrides.
    ///
    /// Negative and non-finite radii become zero.
    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            radius: self.radius.map(|r| FiniteNonNeg::new_or(r, 0.0).get()),
            radius_corners: self
                .radius_corners
                .map(|c| c.map(|r| FiniteNonNeg::new_or(r, 0.0).get())),
            ..self
        }
    }

    /// Whether the tone is nontransparent or the border is enabled.
    ///
    /// This predicate does not inspect glass material; it is not a paint observation.
    #[must_use]
    pub fn paints_chrome(&self) -> bool {
        self.tone != SurfaceTone::Transparent || self.border != BorderStyle::None
    }
}
