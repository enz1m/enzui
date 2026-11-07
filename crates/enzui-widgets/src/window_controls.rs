//! Compose close, minimize and maximize controls for window chrome.
//!
//! [`crate::title_bar`] builds this cluster when its gesture callback is wired.
//! Use this module directly for other window-chrome compositions. By default only
//! Close is shown. [`EnzWindowControls::order`] sets left-to-right order and filters
//! omitted kinds; duplicates render once. The host translates desktop button-layout
//! settings, since this widget does not query them.
//!
//! Hidden controls are absent from the widget and accessibility trees. Disabled
//! controls remain visible and published as disabled without a Click action.
//! [`EnzWindowControls::fullscreen`] removes all controls. The application owns
//! visibility, capabilities and maximized presentation and applies callback messages.
//! Native fullscreen suppresses the cluster without a mirrored application flag.
//!
//! Supply a stable, instance-unique [`EnzWindowControls::id`] or
//! [`EnzWindowControls::id_prefix`] for routed accessibility actions. Otherwise the
//! nodes are anonymous. Control identity uses fixed action suffixes, so changing
//! the maximize/restore label or render order does not change the target.
//!
//! [`EnzWindowControls::on_drag_ended`] optionally reports pointer release over the
//! cluster. It does not observe compositor-owned drag completion; callers with
//! manual drag state decide how to interpret it. Leaving it unset adds no wrapper.

use crate::window_control_button::enz_window_control_button;
use iced::Element;
use iced::widget::{mouse_area, row};

/// A window action used in the caller-supplied control order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WindowControlKind {
    /// The minimize / iconify action.
    Minimize,
    /// The maximize action, which renders as *restore* when the window is
    /// already maximized.
    Maximize,
    /// The close action.
    Close,
}

impl WindowControlKind {
    /// The fixed logical suffix this action's derived widget id uses.
    ///
    /// Fixed on purpose: it is the action's role, never its glyph or aria
    /// label. The maximize control's label flips between "Maximize" and
    /// "Restore" with the window state and its position moves with the
    /// caller's [`order`](EnzWindowControls::order), but a routed AT Focus
    /// request must survive both.
    #[must_use]
    pub fn id_suffix(self) -> &'static str {
        match self {
            Self::Minimize => "minimize",
            Self::Maximize => "maximize",
            Self::Close => "close",
        }
    }
}

/// The pill's padding around its controls. Each control is this much
/// smaller on every side than a lone one, so the pill keeps a lone control's
/// height and the controls sit closer together.
const ISLAND_PADDING: f32 = 2.0;

/// How far inside its target a control in a shared pill draws its hover
/// fill, so the highlight sits within the pill's edge.
const ISLAND_HOVER_INSET: f32 = 1.0;

/// How much smaller the maximize and restore glyphs draw than the others:
/// their outlines span the whole glyph box, so at the same size they look
/// larger than the close cross.
const FRAME_GLYPH_SHRINK: f32 = 2.0;

/// How much further than a lone control's a control in a shared pill steps
/// its hover fill from the pill's: light palettes' one step barely reads.
const ISLAND_HOVER_EMPHASIS: f32 = 1.6;

/// The historical, and still default, render order: minimize → maximize →
/// close. Callers that set no [`order`](EnzWindowControls::order) get
/// exactly this, so the policy work cannot regress an existing caller.
pub const DEFAULT_ORDER: [WindowControlKind; 3] = [
    WindowControlKind::Minimize,
    WindowControlKind::Maximize,
    WindowControlKind::Close,
];

/// The control order the theme or the desktop's button layout asks for,
/// when either said one. A theme that names its buttons keeps
/// [`DEFAULT_ORDER`], reversed where the desktop leads with its controls.
fn desktop_order() -> Option<Vec<WindowControlKind>> {
    let layout = enzui_core::desktop_settings::current().button_layout;
    if enzui_theme::text_defaults::window_buttons() != enzui_theme::WindowButtons::Desktop {
        let mut order = DEFAULT_ORDER.to_vec();
        if layout
            .as_ref()
            .is_some_and(enzui_core::desktop_settings::ButtonLayout::is_leading)
        {
            order.reverse();
        }
        return Some(order);
    }
    layout.as_ref().map(layout_order)
}

/// Whether a self-drawn frame shows `kind`: the theme file's
/// `window.buttons` first, then the desktop's button layout, and `fallback`
/// where neither says. Close always shows.
#[must_use]
pub fn frame_shows(kind: WindowControlKind, fallback: bool) -> bool {
    use enzui_core::desktop_settings::WindowButton;
    let theme = enzui_theme::text_defaults::window_buttons();
    let (from_theme, button) = match kind {
        WindowControlKind::Close => return true,
        WindowControlKind::Minimize => (theme.minimize(), WindowButton::Minimize),
        WindowControlKind::Maximize => (theme.maximize(), WindowButton::Maximize),
    };
    from_theme.unwrap_or_else(|| {
        enzui_core::desktop_settings::current()
            .button_layout
            .as_ref()
            .map_or(fallback, |layout| layout.shows(button))
    })
}

/// The control kinds a desktop button layout lists, in its order. Close is
/// always among them: a window that draws its own frame must stay closable,
/// so a layout without it (`appmenu:`, or an empty one from a desktop that
/// sets none) gets it at the end.
fn layout_order(layout: &enzui_core::desktop_settings::ButtonLayout) -> Vec<WindowControlKind> {
    use enzui_core::desktop_settings::WindowButton;
    let mut order: Vec<WindowControlKind> = layout
        .order()
        .into_iter()
        .map(|button| match button {
            WindowButton::Minimize => WindowControlKind::Minimize,
            WindowButton::Maximize => WindowControlKind::Maximize,
            WindowButton::Close => WindowControlKind::Close,
        })
        .collect();
    if !order.contains(&WindowControlKind::Close) {
        order.push(WindowControlKind::Close);
    }
    order
}

/// Whether the desktop puts window controls before the title, as a
/// `close,minimize,maximize:` button layout does. The app shell and title bar
/// place their controls at the start then.
#[must_use]
pub fn controls_lead() -> bool {
    enzui_core::desktop_settings::current()
        .button_layout
        .is_some_and(|layout| layout.is_leading())
}

/// Builder for a window control cluster (close / minimize / maximize). See
/// [`enz_window_controls`].
#[must_use]
pub struct EnzWindowControls<'a, Message> {
    on_close: Message,
    tooltip_delay: Option<std::time::Duration>,
    tooltips: bool,
    mirrored: bool,
    on_minimize: Option<Message>,
    on_maximize: Option<Message>,
    show_minimize: bool,
    show_maximize: bool,
    show_close: bool,
    minimize_enabled: bool,
    maximize_enabled: bool,
    close_enabled: bool,
    order: Option<Vec<WindowControlKind>>,
    fullscreen: bool,
    maximized: bool,
    control_size: f32,
    control_spacing: f32,
    icon_size: f32,
    ghost: bool,
    on_drag_ended: Option<Message>,
    id_prefix: Option<String>,
    id_override: Option<String>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Role namespace combined with caller identity for stable control IDs.
///
/// No instance prefix is assigned by default; the host must identify each cluster.
pub const DEFAULT_ID_PREFIX: &str = "enz-window-controls";

/// The default per-control hit-target edge length, in logical px.
///
/// Published because [`trailing_inset_for_square_corner`] — and therefore
/// every band that squares its own close-button corner — is a function of
/// this number and the band height. A literal `28` at those call sites would
/// create an independent value that could drift.
pub const DEFAULT_CONTROL_SIZE: f32 =
    crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.control_size;

/// Matches the trailing inset to the vertical inset of centred window controls.
///
/// Both arguments are measured logical-pixel sizes, not requested builder sizes.
/// Negative or non-finite slack yields zero; content padding is independent.
#[must_use]
pub fn trailing_inset_for_square_corner(band_height: f32, control_size: f32) -> f32 {
    iced::advanced::layout::priority_band::trailing_inset(band_height, control_size)
}

/// Construct the controls with a required close message. Defaults: only the
/// close button shown (minimize/maximize hidden until enabled), 28px controls,
/// 8px spacing, 16px icons.
pub fn enz_window_controls<'a, Message: Clone + 'a>(
    on_close: Message,
) -> EnzWindowControls<'a, Message> {
    EnzWindowControls {
        on_close,
        tooltip_delay: None,
        tooltips: true,
        mirrored: false,
        on_minimize: None,
        on_maximize: None,
        show_minimize: false,
        show_maximize: false,
        show_close: true,
        minimize_enabled: true,
        maximize_enabled: true,
        close_enabled: true,
        order: None,
        fullscreen: false,
        maximized: false,
        control_size: crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT
            .effective_control_size(),
        control_spacing: 8.0, // spacing-sm
        icon_size: crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.glyph_size,
        ghost: false,
        on_drag_ended: None,
        id_prefix: None,
        id_override: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzWindowControls<'a, Message> {
    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Show named control hints; enabled by default without changing accessible names.
    pub fn tooltips(mut self, enabled: bool) -> Self {
        self.tooltips = enabled;
        self
    }

    /// Show the minimize button and set its message. Hidden by default.
    pub fn show_minimize(mut self, show: bool, on_minimize: Message) -> Self {
        self.show_minimize = show;
        self.on_minimize = Some(on_minimize);
        self
    }
    /// Show the maximize button, set its message, and its current maximized
    /// state (which selects the restore vs maximize glyph). Hidden by default.
    pub fn show_maximize(mut self, show: bool, on_maximize: Message, maximized: bool) -> Self {
        self.show_maximize = show;
        self.on_maximize = Some(on_maximize);
        self.maximized = maximized;
        self
    }

    /// Show or remove the close control. Defaults to shown.
    ///
    /// Hidden controls leave the widget and accessibility trees; use
    /// [`Self::close_enabled`] to keep a visible, disabled control instead.
    pub fn show_close(mut self, show: bool) -> Self {
        self.show_close = show;
        self
    }

    /// Enable the close control. Defaults to true.
    ///
    /// A disabled control remains rendered and published without a Click action.
    pub fn close_enabled(mut self, enabled: bool) -> Self {
        self.close_enabled = enabled;
        self
    }

    /// Enable or disable the minimize control. Enabled by default.
    /// See [`close_enabled`](Self::close_enabled) for the visibility-vs-enabled
    /// distinction.
    pub fn minimize_enabled(mut self, enabled: bool) -> Self {
        self.minimize_enabled = enabled;
        self
    }

    /// Enable or disable the maximize/restore control. Enabled by
    /// default. A window whose application capability says it may not be
    /// maximized sets this `false` rather than hiding the control, so the
    /// cluster's geometry does not jump when the capability changes.
    pub fn maximize_enabled(mut self, enabled: bool) -> Self {
        self.maximize_enabled = enabled;
        self
    }

    /// Lay the controls out from right to left, as a right-to-left locale
    /// does: the first control in [`order`](Self::order) sits rightmost.
    /// Off by default; [`EnzTitleBar`](crate::title_bar::EnzTitleBar) sets
    /// it from its writing direction.
    pub fn mirrored(mut self, mirrored: bool) -> Self {
        self.mirrored = mirrored;
        self
    }

    /// Set left-to-right control order and filter omitted kinds.
    ///
    /// Defaults to the desktop's button layout
    /// ([`enzui_core::desktop_settings::button_layout`]: only the buttons it
    /// lists, in its order), or [`DEFAULT_ORDER`] when the desktop says
    /// nothing. Each kind renders at most once.
    pub fn order(mut self, order: impl IntoIterator<Item = WindowControlKind>) -> Self {
        self.order = Some(order.into_iter().collect());
        self
    }

    /// Remove all controls when fullscreen is true.
    ///
    /// Native fullscreen also suppresses the cluster automatically.
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }
    /// Per-button hit-target edge length in px. Non-finite or non-positive
    /// input falls back to 28.
    pub fn control_size(mut self, s: f32) -> Self {
        self.control_size =
            enzui_core::metrics::FinitePositive::new_or(s, DEFAULT_CONTROL_SIZE).get();
        self
    }
    /// Set the gap between controls in logical pixels. Defaults to eight.
    ///
    /// Zero is allowed; negative or non-finite values restore the default.
    pub fn control_spacing(mut self, s: f32) -> Self {
        self.control_spacing = enzui_core::metrics::FiniteNonNeg::new_or(s, 8.0).get();
        self
    }

    /// Publish `msg` on pointer release over the control cluster.
    ///
    /// This optional wrapper does not signal completion of compositor-owned window
    /// movement. When unset, no mouse-area wrapper is added.
    pub fn on_drag_ended(mut self, msg: Message) -> Self {
        self.on_drag_ended = Some(msg);
        self
    }
    /// Draw the buttons without a fill at rest, like toolbar icons; the fill
    /// still shows on hover and press. For controls that sit with icons on a
    /// shared glass island, where a filled disc stands out from its row.
    pub fn ghost(mut self, ghost: bool) -> Self {
        self.ghost = ghost;
        self
    }
    /// Glyph size within each button in px. Non-finite or non-positive input
    /// falls back to 16.
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = enzui_core::metrics::FinitePositive::new_or(
            s,
            crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.glyph_size,
        )
        .get();
        self
    }

    /// Set the namespace prefix for this cluster's three per-action ids.
    ///
    /// A caller-supplied prefix is host-owned identity — the host typed it
    /// and chose it to differ per instance. Supplying it makes the three
    /// buttons routable. Prefer [`Self::id`] when the host has a real window
    /// key. See [`Self::owner`] for the full table.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// Set this cluster's host-owned stable per-window key.
    ///
    /// The preferred way to make a titlebar routable. Pass the host's own
    /// window identity — an `iced::window::Id` rendered to a string, a route
    /// name, a document key. It must be unique among the clusters mounted in
    /// one AT tree and stable across the host's view rebuilds.
    ///
    /// The three role suffixes (`minimize` / `maximize` / `close`) are fixed
    /// and are not derived from the glyph or the aria label: the maximize
    /// label flips between "Maximize" and "Restore" with the window state, and
    /// an id that moved with it would drift out from under a routed AT request
    /// exactly when the user maximizes the window.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id_override = Some(id.into());
        self
    }

    /// Whether a given control kind will render: it must be shown, must have a
    /// message where one is required, and the cluster must not be fullscreen.
    fn renders(&self, kind: WindowControlKind) -> bool {
        if self.fullscreen || iced::window::decorations().fullscreen {
            return false;
        }
        match kind {
            WindowControlKind::Minimize => self.show_minimize && self.on_minimize.is_some(),
            WindowControlKind::Maximize => self.show_maximize && self.on_maximize.is_some(),
            WindowControlKind::Close => self.show_close,
        }
    }

    /// The control kinds that will render, in render order.
    ///
    /// Deduplicated: a kind listed twice in [`order`](Self::order) renders
    /// once, because two buttons for one action would publish the same derived
    /// widget id and collide.
    pub fn rendered_order(&self) -> Vec<WindowControlKind> {
        self.rendered_order_in(desktop_order())
    }

    /// [`Self::rendered_order`] with the desktop's layout given.
    fn rendered_order_in(&self, desktop: Option<Vec<WindowControlKind>>) -> Vec<WindowControlKind> {
        let order: &[WindowControlKind] = match (&self.order, &desktop) {
            (Some(o), _) => o,
            (None, Some(desktop)) => desktop,
            (None, None) => &DEFAULT_ORDER,
        };
        let mut seen = [false; 3];
        let mut out = Vec::with_capacity(order.len().min(3));
        for kind in order {
            let slot = match kind {
                WindowControlKind::Minimize => 0,
                WindowControlKind::Maximize => 1,
                WindowControlKind::Close => 2,
            };
            if seen[slot] || !self.renders(*kind) {
                continue;
            }
            seen[slot] = true;
            out.push(*kind);
        }
        out
    }

    /// Number of buttons that will render (close when shown, plus
    /// minimize/maximize when shown; zero when
    /// [`fullscreen`](Self::fullscreen)).
    pub fn control_count(&self) -> u32 {
        self.rendered_order().len() as u32
    }

    /// The cluster's per-control edge length, in logical px.
    ///
    /// Exposed so a band that hosts this cluster can derive the trailing inset
    /// that squares the close button's corner
    /// ([`trailing_inset_for_square_corner`]) from the cluster's actual
    /// size rather than from a literal `28` — a caller who set
    /// [`control_size`](Self::control_size) must move the corner with it.
    #[must_use]
    pub fn resolved_control_size(&self) -> f32 {
        self.control_size
    }

    /// The maximize-button glyph name for the current state: `window.restore`
    /// when maximized, else `window.maximize`.
    pub fn maximize_icon(&self) -> &'static str {
        if self.maximized {
            "window.restore"
        } else {
            "window.maximize"
        }
    }

    /// This cluster's owner namespace, or `None` when the caller supplied
    /// neither [`Self::id`] nor [`Self::id_prefix`].
    ///
    /// | caller supplied | node ids |
    /// |---|---|
    /// | `.id("window-7")` | `enz-window-controls#window-7/minimize`, `…/maximize`, `…/close` |
    /// | `.id_prefix("titlebar-main")` | `titlebar-main#/minimize`, … |
    /// | neither | anonymous — no ids, actions stripped |
    ///
    /// The app shell names the cluster it draws `.id("app-shell")`: one
    /// shell per window, and accessibility actions resolve per window.
    ///
    /// # Why the default is `None` rather than [`DEFAULT_ID_PREFIX`]
    ///
    /// A fixed default prefix would make cross-instance collisions merely
    /// *detectable* via `identity::duplicates`. Detection alone is insufficient because
    /// detectable is not the same as handled: the detector only runs where a
    /// test calls it, so a release tree could still carry duplicate ids and
    /// misroute a titlebar action — and a misrouted `close` closes the wrong
    /// window, which is the worst outcome in this cluster.
    ///
    /// The analysis that a cluster has no intrinsic per-instance key
    /// remains correct and is the reason this is `None` rather than derived
    /// from something. The window state that drives the Maximize↔Restore glyph
    /// is *mutable*, so folding it in would make the maximize id move when the
    /// window is maximized. The host knows the window id; the widget cannot.
    /// So the widget asks, and publishes nothing when it is not told.
    ///
    /// The role suffixes are preserved exactly by design
    /// (`minimize` / `maximize` / `close`), which is the property that keeps
    /// the maximize id stable across the window-state flip.
    pub fn owner(&self) -> Option<enzui_core::identity::OwnerId> {
        enzui_core::identity::OwnerId::resolve(
            DEFAULT_ID_PREFIX,
            self.id_prefix.as_deref(),
            self.id_override.as_deref(),
        )
    }

    /// Consume the builder and produce the widget tree.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a button renders only when it has a message"
    )]
    pub fn into_element(self) -> Element<'a, Message> {
        // Derived, stable per-action identity for the routable control
        // nodes. An anonymous node is collected and announced but absent from
        // `NodeIdMap`, so an AT Focus/Click request against it resolves to
        // nothing. Each button's id is the namespace plus a *fixed* suffix
        // (the action role, never the glyph/label — the maximize label flips
        // with the window state), matching `banner.rs`'s convention.
        let owner = self.owner();

        // The sequence is caller-driven, so the buttons are emitted by
        // walking `rendered_order()` rather than by three hard-coded pushes.
        // The id suffix stays keyed to the *kind*, never to the position, so
        // reordering the cluster does not repoint a routed AT request.
        let order = self.rendered_order();
        let maximize_icon = self.maximize_icon().to_owned();

        // Two or more controls share one pill of a single control's height:
        // the pill carries the resting fill, and each control highlights a
        // circle inset within it on hover.
        let island = order.len() > 1;
        let spacing = if island { 0.0 } else { self.control_spacing };
        let mut r = row![].mirrored(self.mirrored).spacing(spacing);
        for kind in order {
            let id = owner.as_ref().map(|o| o.id(kind.id_suffix()));
            let (icon, aria, msg, enabled) = match kind {
                WindowControlKind::Minimize => (
                    "window.minimize".to_owned(),
                    enzui_core::localization::text("enzui-window-controls-minimize"),
                    self.on_minimize.clone().expect("renders() checked the msg"),
                    self.minimize_enabled,
                ),
                WindowControlKind::Maximize => (
                    maximize_icon.clone(),
                    if self.maximized {
                        enzui_core::localization::text("enzui-window-controls-restore")
                    } else {
                        enzui_core::localization::text("enzui-window-controls-maximize")
                    },
                    self.on_maximize.clone().expect("renders() checked the msg"),
                    self.maximize_enabled,
                ),
                WindowControlKind::Close => (
                    "window.close".to_owned(),
                    enzui_core::localization::text("enzui-window-controls-close"),
                    self.on_close.clone(),
                    self.close_enabled,
                ),
            };
            let control_size = if island {
                (self.control_size - 2.0 * ISLAND_PADDING).max(0.0)
            } else {
                self.control_size
            };
            let icon_size = if kind == WindowControlKind::Maximize {
                (self.icon_size - FRAME_GLYPH_SHRINK).max(0.0)
            } else {
                self.icon_size
            };
            let mut button = enz_window_control_button(icon, msg)
                .control_size(control_size)
                .icon_size(icon_size)
                .aria_label(aria)
                // A disabled control still renders and is still published,
                // marked `disabled` with no Click action.
                .enabled(enabled);
            if let Some(delay) = self.tooltip_delay {
                button = button.tooltip_delay(delay);
            }
            if !self.tooltips {
                button = button.tooltip("");
            }
            if self.ghost || island {
                button = button.button_background(iced::Color::TRANSPARENT);
            }
            if island {
                button = button
                    .fill_inset(ISLAND_HOVER_INSET)
                    .hover_emphasis(ISLAND_HOVER_EMPHASIS);
            }
            // No host key -> anonymous, not aliased. `collect.rs` strips
            // an anonymous node's actions, so the AT announces "Close, button,
            // not operable" rather than offering a Click that closes some
            // other window's titlebar.
            if let Some(id) = id {
                button = button.id(id);
            }
            r = r.push(button);
        }
        let cluster: Element<'a, Message> = if island {
            let radius = self.control_size / 2.0;
            let ghost = self.ghost;
            iced::widget::container(r)
                .padding(ISLAND_PADDING)
                .style(move |theme: &iced::Theme| iced::widget::container::Style {
                    // A ghost cluster keeps the pill's geometry without its fill.
                    background: (!ghost)
                        .then(|| crate::theme_lookup::palette(theme).surface_hover.into()),
                    border: iced::Border {
                        radius: radius.into(),
                        ..iced::Border::default()
                    },
                    ..iced::widget::container::Style::default()
                })
                .into()
        } else {
            r.into()
        };
        // Only pay for a MouseArea when a drag-ended shield was requested;
        // an unused feature must not alter the group's layout/geometry.
        match self.on_drag_ended {
            Some(msg) => mouse_area(cluster).on_release(msg).into(),
            None => cluster,
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzWindowControls<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzWindowControls<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
