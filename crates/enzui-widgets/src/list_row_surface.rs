//! A pressable row with selection chrome, keyboard activation and list-item semantics.
//!
//! Defaults are a 34-pixel height, fill width, 4-pixel horizontal outer gutter,
//! 12-pixel content inset and vertically centered content. The gutter narrows
//! both chrome and child layout; presses in it do not activate the row.
//! Content is clipped to the rounded row outline. Disabled rows dim inherited
//! text and icon colours, leaving explicit-colour content unchanged.
//!
//! Hover and selection fills ease in both directions; pressed feedback is
//! immediate. Selection wins over hover and suppresses the focus ring, while
//! unselected focused rows show it. Reduced motion follows the theme unless
//! explicitly overridden. Adjacent selected rows can share squared inner edges.
//!
//! Interactive descendants retain captured gestures. Trailing actions reserve
//! their width even when hidden: visibility controls painting only, leaving
//! events, focus, accessibility and overlays active. Embedded rows delegate
//! keyboard navigation to their owning composite.
//!
//! Ids default to the accessible label and namespace; use explicit ids for
//! repeated labels or identities that must survive renaming. [`on_context`](EnzListRowSurface::on_context)
//! adds a row-sized context target and reports the window-space press point.
//! Its accessible affordance needs a derived or explicitly pinned context id.
//! [`crate::sidebar_row`] provides a constrained icon-and-label row builder.

use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::widget::button::Style;
use iced::widget::{Space, container, row};
use iced::{Alignment, Background, Border, Color, Element, Length, Theme};

use crate::focus_button::{FocusStatus, focus_button_maybe, focus_button_maybe_with};
use crate::row_motion::{RowActiveColor, RowMotion, RowMotionProgress, lerp_color};
use crate::surface_interaction::{DimContent, composite};
use crate::theme_lookup::resolve;
use std::cell::Cell;
use std::rc::Rc;
mod widget_impls;

/// Row chrome and layout tokens.
///
/// See the [module documentation](self) for defaults. Fixed token sets supply
/// layout metrics; resolved token sets require explicit layout setters.
#[derive(Debug, Clone, Copy)]
pub struct ListRowSurfaceTokens {
    /// Regular row surface background.
    pub surface_background: Color,
    /// Hover overlay, composited over the surface; defaults to [`Palette::overlay_hover`].
    pub hover_overlay: Color,
    /// Pressed overlay, composited over the surface; defaults to [`Palette::overlay_pressed`].
    pub pressed_overlay: Color,
    /// Selected background.
    pub selected_background: Color,
    /// Regular border colour.
    pub border_color: Color,
    /// Selected border colour.
    pub selected_border: Color,
    /// Focus-visible ring colour.
    pub focus_ring: Color,
    /// Focus-visible ring width in logical pixels (default 2).
    pub focus_ring_width: f32,
    /// Text/foreground colour.
    pub text_color: Color,
    /// Height of each row in pixels (`list-row-height` = 34).
    pub row_height: f32,
    /// Row corner radius in pixels (`radius-lg` = 10).
    pub row_radius: f32,
    /// Border width in logical pixels (default zero).
    pub border_width: f32,
    /// Symmetric horizontal inset outside the chrome, in pixels (default 4).
    pub outer_gutter: f32,
    /// Horizontal content inset inside the painted row, in pixels (default 12).
    pub content_padding: f32,
    /// Vertical content alignment (default center). Use start for top-aligned content.
    pub content_align_y: iced::Alignment,
    /// Opacity multiplier applied when disabled.
    pub disabled_alpha: f32,
}

/// Default horizontal outer gutter, in logical pixels.
pub const DEFAULT_OUTER_GUTTER: f32 = 4.0;

/// The vertical space between the painted fills of two adjacent rows, so a
/// selected row and a hovered one next to it read as two: taken from each
/// row's own height, half above and half below its fill, so a list keeps
/// its size. Rows joined in a [`selection_group`](EnzListRowSurface::selection_group)
/// keep no gap between them.
pub const DEFAULT_ROW_GAP: f32 = 2.0;

impl ListRowSurfaceTokens {
    /// Build row tokens from a palette, using the documented geometry defaults.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            surface_background: Color::TRANSPARENT,
            hover_overlay: p.overlay_hover(),
            pressed_overlay: p.overlay_pressed(),
            selected_background: p.surface_selected,
            border_color: Color::TRANSPARENT,
            selected_border: Color::TRANSPARENT,
            focus_ring: p.focus_ring,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            text_color: p.text,
            row_height: enzui_theme::text_defaults::sizing().row_height,
            row_radius: enzui_theme::text_defaults::shape_radius(|shape| shape.row),
            border_width: 0.0,
            outer_gutter: DEFAULT_OUTER_GUTTER,
            content_padding: enzui_theme::text_defaults::spacing().md(),
            content_align_y: iced::Alignment::Center,
            disabled_alpha: enzui_theme::text_defaults::opacity().disabled_surface(),
        }
    }
    /// Preset built from the built-in dark palette. Prefer
    /// [`ListRowSurfaceTokens::from_palette`] with the live palette when one is
    /// in scope.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Read the theme-resolved default outer gutter.
    ///
    /// This does not inspect a row's fixed tokens or instance overrides. Use
    /// [`EnzListRowSurface::resolved_outer_gutter`] for its applied layout value.
    #[must_use]
    pub fn outer_gutter_of(theme: &Theme) -> f32 {
        resolve(theme, Self::from_palette).outer_gutter
    }
    /// Preset built from the built-in light palette. Prefer
    /// [`ListRowSurfaceTokens::from_palette`] with the live palette when one is
    /// in scope.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replace negative or non-finite numeric fields with default values.
    ///
    /// Zero and other finite non-negative values are preserved, including
    /// `disabled_alpha` values above one. Colours and alignment pass through.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        let d = Self::dark();
        Self {
            focus_ring_width: FiniteNonNeg::new_or(self.focus_ring_width, d.focus_ring_width).get(),
            row_height: FiniteNonNeg::new_or(self.row_height, d.row_height).get(),
            row_radius: FiniteNonNeg::new_or(self.row_radius, d.row_radius).get(),
            border_width: FiniteNonNeg::new_or(self.border_width, d.border_width).get(),
            outer_gutter: FiniteNonNeg::new_or(self.outer_gutter, d.outer_gutter).get(),
            content_padding: FiniteNonNeg::new_or(self.content_padding, d.content_padding).get(),
            disabled_alpha: FiniteNonNeg::new_or(self.disabled_alpha, d.disabled_alpha).get(),
            ..*self
        }
    }
}

/// Choose when a trailing action paints.
///
/// Hidden actions retain layout, events, focus, accessibility and overlays.
/// Row hover or row/descendant focus reveals conditional actions; selection
/// also reveals [`Self::OnHoverFocusOrSelected`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum TrailingVisibility {
    /// Paint the trailing element in every interaction state.
    #[default]
    Always,
    /// Suppress only its draw pass until the row is hovered or the row/one of
    /// its descendants holds focus.
    OnHoverOrFocus,
    /// As [`Self::OnHoverOrFocus`], and also paint while the row is selected.
    /// Use this for contextual row commands that should remain visible on the
    /// active item after the pointer leaves.
    OnHoverFocusOrSelected,
}

pub(crate) type TrailingReveal = Rc<Cell<bool>>;

/// Namespace for label-derived row identities.
pub const DEFAULT_ID_PREFIX: &str = "enz-list-row";

const ROW_ROLE: &str = "row";

const ROW_CONTEXT_ROLE: &str = "row-ctx";

/// Derive a row id from its namespace and accessible label.
///
/// Reordering preserves the id; changing the label changes it. Repeated labels
/// in one namespace need explicit identities. The context affordance uses a
/// distinct component of the same owner via [`list_row_context_id`].
#[must_use]
pub fn list_row_id(id_prefix: &str, label: &str) -> iced::advanced::widget::Id {
    enzui_core::identity::OwnerId::of_kind(id_prefix, label).id(ROW_ROLE)
}

/// Derive a row context-affordance id from its namespace and accessible label.
///
/// Shares an owner with [`list_row_id`] under a distinct component.
#[must_use]
pub fn list_row_context_id(id_prefix: &str, label: &str) -> iced::advanced::widget::Id {
    enzui_core::identity::OwnerId::of_kind(id_prefix, label).id(ROW_CONTEXT_ROLE)
}

#[derive(Clone)]
enum TokenSource {
    Fixed(ListRowSurfaceTokens),
    Resolved(std::rc::Rc<dyn Fn(&Theme) -> ListRowSurfaceTokens>),
}

/// Rows built in the shell's sidebar select with the sidebar's tint, like
/// [`crate::sidebar_row`]; elsewhere the default (`surface_selected`) applies.
fn sidebar_default_tokens() -> Option<TokenSource> {
    crate::title_bar::in_sidebar_pane().then(|| {
        TokenSource::Resolved(std::rc::Rc::new(|theme: &Theme| {
            let mut t =
                crate::theme_lookup::resolve_live(theme, ListRowSurfaceTokens::from_palette);
            t.selected_background =
                crate::sidebar_row::selected_tint(&crate::theme_lookup::palette(theme));
            t
        }))
    })
}

impl TokenSource {
    fn resolve(&self, theme: &Theme) -> ListRowSurfaceTokens {
        match self {
            Self::Fixed(t) => *t,
            Self::Resolved(f) => f(theme),
        }
    }

    const fn fixed_row_height(&self) -> Option<f32> {
        match self {
            Self::Fixed(t) => Some(t.row_height),
            Self::Resolved(_) => None,
        }
    }

    const fn fixed_outer_gutter(&self) -> Option<f32> {
        match self {
            Self::Fixed(t) => Some(t.outer_gutter),
            Self::Resolved(_) => None,
        }
    }
}

fn content_alpha(
    enabled: bool,
    override_alpha: Option<f32>,
    tokens: Option<&TokenSource>,
    theme: &Theme,
) -> f32 {
    if enabled {
        1.0
    } else if let Some(alpha) = override_alpha {
        alpha
    } else {
        tokens
            .map_or_else(
                || crate::theme_lookup::resolve_live(theme, ListRowSurfaceTokens::from_palette),
                |source| source.resolve(theme),
            )
            .disabled_alpha
    }
}

// Explicit anonymity must not fall back to a label-derived context identity.
enum ContextIdentity {
    Derive,

    Pinned(Option<iced::advanced::widget::Id>),
}

/// Convenience constructor: `enz_list_row_surface(child, on_activate)`.
pub fn enz_list_row_surface<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    on_activate: Message,
) -> EnzListRowSurface<'a, Message>
where
    Message: Clone + 'a,
{
    EnzListRowSurface {
        child: child.into(),
        on_activate: Some(on_activate),
        selected: false,
        selectable: false,
        enabled: true,
        disabled_content_alpha: None,
        label: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        tokens: sidebar_default_tokens(),
        width: Length::Fill,
        row_height_override: None,
        outer_gutter_override: None,
        content_padding_override: None,
        content_align_y_override: None,
        outer_vertical_gap: None,
        content_height: false,
        embedded: false,
        on_enter: None,
        on_space: None,
        focus_visible: false,
        semantics_override: None,
        on_activate_with: None,
        on_activate_with_modifiers: None,
        on_activate_with_point: None,
        on_context: None,
        context_label: None,
        context_identity: ContextIdentity::Derive,
        inert: false,
        navigable_when_inert: false,
        reduced_motion: None,
        content_text: None,
        content_align_x: None,
        trailing: None,
        trailing_visibility: TrailingVisibility::Always,
        trailing_reveal: None,
        trailing_reveal_parented: false,
        group_selected_above: false,
        group_selected_below: false,
    }
}

/// Build a row without an eager activation message.
///
/// It starts inert; supply activation through a lazy factory. Layout and chrome
/// defaults match [`enz_list_row_surface`].
pub fn enz_list_row_surface_lazy<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> EnzListRowSurface<'a, Message>
where
    Message: Clone + 'a,
{
    EnzListRowSurface {
        child: child.into(),
        on_activate: None,
        selected: false,
        selectable: false,
        enabled: true,
        disabled_content_alpha: None,
        label: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        tokens: sidebar_default_tokens(),
        width: Length::Fill,
        row_height_override: None,
        outer_gutter_override: None,
        content_padding_override: None,
        content_align_y_override: None,
        outer_vertical_gap: None,
        content_height: false,
        embedded: false,
        on_enter: None,
        on_space: None,
        focus_visible: false,
        semantics_override: None,
        on_activate_with: None,
        on_activate_with_modifiers: None,
        on_activate_with_point: None,
        on_context: None,
        context_label: None,
        context_identity: ContextIdentity::Derive,
        inert: true,
        navigable_when_inert: false,
        reduced_motion: None,
        content_text: None,
        content_align_x: None,
        trailing: None,
        trailing_visibility: TrailingVisibility::Always,
        trailing_reveal: None,
        trailing_reveal_parented: false,
        group_selected_above: false,
        group_selected_below: false,
    }
}

/// Builder returned by [`enz_list_row_surface`].
#[must_use]
pub struct EnzListRowSurface<'a, Message> {
    child: Element<'a, Message>,
    on_activate: Option<Message>,
    selected: bool,
    selectable: bool,
    enabled: bool,

    disabled_content_alpha: Option<f32>,
    label: Option<String>,
    id: Option<iced::advanced::widget::Id>,

    id_prefix: String,
    tokens: Option<TokenSource>,
    width: Length,
    row_height_override: Option<f32>,

    outer_gutter_override: Option<f32>,
    content_padding_override: Option<f32>,
    content_align_y_override: Option<iced::Alignment>,

    /// `None`: the default [`DEFAULT_ROW_GAP`], taken from the row height.
    /// `Some`: a gap the caller already took from the height it passes.
    outer_vertical_gap: Option<f32>,

    content_height: bool,

    embedded: bool,

    on_enter: Option<Box<dyn Fn() -> Message + 'a>>,

    on_space: Option<Box<dyn Fn() -> Message + 'a>>,

    focus_visible: bool,

    semantics_override: Option<Semantics>,

    on_activate_with: Option<Box<dyn Fn() -> Message + 'a>>,

    on_activate_with_modifiers: Option<Box<dyn Fn(iced::keyboard::Modifiers) -> Message + 'a>>,

    on_activate_with_point: Option<Box<dyn Fn(Option<iced::Point>) -> Message + 'a>>,

    on_context: Option<Box<dyn Fn(iced::Point) -> Message + 'a>>,

    context_label: Option<String>,

    context_identity: ContextIdentity,

    inert: bool,

    navigable_when_inert: bool,

    reduced_motion: Option<bool>,

    content_text: Option<String>,

    content_align_x: Option<iced::Alignment>,

    trailing: Option<Element<'a, Message>>,
    trailing_visibility: TrailingVisibility,

    trailing_reveal: Option<TrailingReveal>,
    trailing_reveal_parented: bool,

    group_selected_above: bool,

    group_selected_below: bool,
}

impl<'a, Message> EnzListRowSurface<'a, Message>
where
    Message: Clone + 'a,
{
    /// Mark the row as selected (persistent highlight, distinct from transient
    /// hover). Exposed to AT as `item-selected`.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Join the selected row's upper or lower edge to adjacent selected rows.
    ///
    /// The corresponding inner corners become square. Unselected and standalone
    /// rows keep rounded corners; the caller supplies adjacency flags.
    pub fn selection_group(mut self, above: bool, below: bool) -> Self {
        self.group_selected_above = above;
        self.group_selected_below = below;
        self
    }

    /// Force (or forbid) reduced motion for this row's background transition,
    /// overriding the theme's `motion.reduced_motion` policy for this instance.
    ///
    /// `true` snaps hover/selection instantly; `false` runs the full
    /// `motion-fast` ease even under a reduced-motion theme. Unset follows
    /// the theme.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Expose selected and unselected states to accessibility.
    ///
    /// Defaults to `false`; a selected row exposes its selected state regardless.
    pub fn selectable(mut self, selectable: bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Enable row focus and activation (default `true`).
    ///
    /// Disabling removes row activation and dims inherited content ink. Nested
    /// controls retain their own event handling and focus behavior.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub(crate) fn disabled_content_alpha(mut self, alpha: f32) -> Self {
        self.disabled_content_alpha = Some(alpha.clamp(0.0, 1.0));
        self
    }

    /// Set the row's accessible name and label-derived identity key.
    ///
    /// Without an explicit name, accessibility may infer one from child text.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Stable widget id so programmatic `focus(id)` can target this row.
    /// Explicit identity wins over the label-derived default.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the default identity derived from the accessible label.
    ///
    /// Uses [`Self::label`] or the semantics override's label. Without either
    /// label or an explicit id, the row stays anonymous. Repeated names in a
    /// namespace require explicit identities.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Use a fixed, sanitized token set.
    ///
    /// Colours remain fixed across theme changes; layout metrics come from these
    /// tokens unless an instance setter overrides them.
    pub fn tokens(mut self, tokens: ListRowSurfaceTokens) -> Self {
        self.tokens = Some(TokenSource::Fixed(tokens.sanitized()));
        self
    }

    /// Resolve colors and paint metrics from the runtime theme on each draw.
    /// Layout metrics require explicit setters: row height, outer gutter,
    /// content padding, and content alignment cannot be read from a resolver
    /// before a theme is available.
    pub fn tokens_resolved(
        mut self,
        resolver: impl Fn(&Theme) -> ListRowSurfaceTokens + 'static,
    ) -> Self {
        self.tokens = Some(TokenSource::Resolved(std::rc::Rc::new(move |theme| {
            resolver(theme).sanitized()
        })));
        self
    }

    /// Set the fixed row height in logical pixels (default 34).
    ///
    /// Negative or non-finite values use 34; zero is preserved.
    /// [`Self::content_height`] bypasses the fixed height.
    pub fn row_height(mut self, height: f32) -> Self {
        self.row_height_override =
            Some(enzui_core::metrics::FiniteNonNeg::new_or(height, 34.0).get());
        self
    }

    /// Set symmetric horizontal space outside the row chrome, in logical pixels.
    ///
    /// Defaults to 4 or the fixed tokens' gutter. Invalid values use 4; zero
    /// removes it. It narrows child layout as well as the painted/hit-tested row.
    pub fn outer_gutter(mut self, px: f32) -> Self {
        self.outer_gutter_override =
            Some(enzui_core::metrics::FiniteNonNeg::new_or(px, DEFAULT_OUTER_GUTTER).get());
        self
    }

    /// Leave `px` of vertical space around the painted row, outside the
    /// [`row_height`](Self::row_height) the caller passes (which it has
    /// already reduced to make room). Without it the row leaves
    /// [`DEFAULT_ROW_GAP`] inside its height.
    pub(crate) fn outer_vertical_gap(mut self, px: f32) -> Self {
        self.outer_vertical_gap = Some(enzui_core::metrics::FiniteNonNeg::new_or(px, 0.0).get());
        self
    }

    /// Override the outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Horizontal inset inside the painted row, in pixels (default 12).
    /// Zero allows full-bleed content or a composite with its own padding.
    pub fn content_padding(mut self, px: f32) -> Self {
        self.content_padding_override =
            Some(enzui_core::metrics::FiniteNonNeg::new_or(px, 12.0).get());
        self
    }

    /// Vertical content alignment (default center), independent of row height.
    pub fn content_align_y(mut self, align: impl Into<iced::Alignment>) -> Self {
        self.content_align_y_override = Some(align.into());
        self
    }

    /// Align the whole leading content group horizontally (default start).
    pub fn content_align_x(mut self, align: impl Into<iced::Alignment>) -> Self {
        self.content_align_x = Some(align.into());
        self
    }

    /// Add a trailing action after an 8-pixel gap.
    ///
    /// Its layout width is reserved even when [`Self::trailing_visibility`] hides
    /// its paint. Events, focus, accessibility and overlays remain active.
    pub fn trailing(mut self, trailing: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// Choose when the trailing action is painted.
    ///
    /// [`OnHoverOrFocus`](TrailingVisibility::OnHoverOrFocus) holds layout,
    /// focus and a11y trees while visually quiet; row/action focus or hover
    /// reveals. [`OnHoverFocusOrSelected`](TrailingVisibility::OnHoverFocusOrSelected)
    /// additionally paints on active model selection. Default
    /// [`Always`](TrailingVisibility::Always).
    pub fn trailing_visibility(mut self, visibility: TrailingVisibility) -> Self {
        self.trailing_visibility = visibility;
        self
    }

    pub(crate) fn trailing_reveal_handle(mut self, reveal: Option<TrailingReveal>) -> Self {
        self.trailing_reveal = reveal;
        self.trailing_reveal_parented = false;
        self
    }

    pub(crate) fn parented_trailing_reveal_handle(mut self, reveal: TrailingReveal) -> Self {
        self.trailing_reveal = Some(reveal);
        self.trailing_reveal_parented = true;
        self
    }

    /// Let the row height follow its content (for multi-line rows such as
    /// `EnzPreviewRow`) instead of snapping to the fixed `row-height`. The
    /// content then supplies its own vertical rhythm; hover/selected/focus
    /// chrome still wraps the full content box.
    pub fn content_height(mut self, content_height: bool) -> Self {
        self.content_height = content_height;
        self
    }

    /// Delegate Tab and keyboard activation to a roving-focus composite.
    ///
    /// Pointer activation and accessibility remain available. The composite
    /// can request focus chrome through [`Self::focus_visible`]. Defaults to
    /// `false`.
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Override Enter and the routed accessibility default action.
    ///
    /// The callback runs on activation; it does not enable an activationless row.
    /// Pointer clicks retain the primary action. Embedded rows leave keyboard
    /// Enter to the composite, while routed default actions still use this hook.
    pub fn on_enter(mut self, on_enter: impl Fn() -> Message + 'a) -> Self {
        self.on_enter = Some(Box::new(on_enter));
        self
    }

    /// Set or clear the Enter/default-action override.
    ///
    /// `None` restores primary activation routing.
    pub fn on_enter_maybe(mut self, on_enter: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_enter = on_enter;
        self
    }

    /// Override keyboard Space activation without changing pointer activation.
    ///
    /// The callback does not enable an activationless row. Embedded rows leave
    /// Space to their owning composite.
    pub fn on_space(mut self, on_space: impl Fn() -> Message + 'a) -> Self {
        self.on_space = Some(Box::new(on_space));
        self
    }

    /// Set or clear the Space override; `None` restores primary activation routing.
    pub fn on_space_maybe(mut self, on_space: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_space = on_space;
        self
    }

    /// Request focus chrome for the owning composite's active row.
    ///
    /// Meaningful in embedded mode; selected rows still suppress the ring.
    pub fn focus_visible(mut self, focus_visible: bool) -> Self {
        self.focus_visible = focus_visible;
        self
    }

    /// Create the primary message lazily on activation.
    ///
    /// Wins over the eager message; point- and modifier-aware factories take
    /// precedence when set.
    pub fn on_activate_with(mut self, f: impl Fn() -> Message + 'a) -> Self {
        self.on_activate_with = Some(Box::new(f));
        self
    }

    /// Set or clear the lazy primary activation factory.
    ///
    /// `None` suppresses the eager fallback without dimming content. Point- and
    /// modifier-aware factories still take precedence. Accessibility reports the
    /// activationless row disabled unless [`Self::navigable_when_inert`] is set.
    pub fn on_activate_maybe_with(mut self, f: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.inert = f.is_none();
        self.on_activate_with = f;
        self
    }

    /// Activate with modifiers latched at pointer press.
    ///
    /// Overrides eager and simple lazy factories; point-aware activation wins
    /// over this hook. Keyboard activation reports no modifiers. See
    /// [`FocusButton::on_press_with_modifiers`](crate::focus_button::FocusButton::on_press_with_modifiers).
    pub fn on_activate_with_modifiers(
        mut self,
        f: impl Fn(iced::keyboard::Modifiers) -> Message + 'a,
    ) -> Self {
        self.inert = false;
        self.on_activate_with_modifiers = Some(Box::new(f));
        self
    }

    /// Activate with the latched window-space pointer press point.
    ///
    /// Keyboard and routed activation report `None`. This factory takes
    /// precedence over eager, simple lazy and modifier-aware activation.
    pub fn on_activate_with_point(
        mut self,
        f: impl Fn(Option<iced::Point>) -> Message + 'a,
    ) -> Self {
        self.inert = false;
        self.on_activate_with_point = Some(Box::new(f));
        self
    }

    /// Publish a context-menu gesture at the window-space press point.
    ///
    /// Cursorless accessibility requests use the row center. The row supplies
    /// a height-constrained context target; menu content and routing belong to
    /// the caller. Unset composes no context target.
    pub fn on_context(mut self, f: impl Fn(iced::Point) -> Message + 'a) -> Self {
        self.on_context = Some(Box::new(f));
        self
    }

    /// Set or clear the context-menu callback; `None` composes no context target.
    pub fn on_context_maybe(mut self, f: Option<Box<dyn Fn(iced::Point) -> Message + 'a>>) -> Self {
        self.on_context = f;
        self
    }

    /// Set an explicit context-affordance id, replacing label-derived identity.
    ///
    /// This also permits an accessible context affordance on an unlabelled row.
    /// Has an effect only with [`Self::on_context`].
    pub fn context_id(self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.context_id_maybe(Some(id.into()))
    }

    /// Set the context-affordance identity; `None` explicitly makes it anonymous.
    ///
    /// An anonymous target publishes no accessibility node but still accepts
    /// pointer context gestures. `None` does not fall back to the row label.
    pub fn context_id_maybe(mut self, id: Option<iced::advanced::widget::Id>) -> Self {
        self.context_identity = ContextIdentity::Pinned(id);
        self
    }

    /// Set the accessible context-affordance name.
    ///
    /// Requires [`Self::on_context`] and a derived or explicitly pinned context id.
    pub fn context_label(mut self, label: impl Into<String>) -> Self {
        self.context_label = Some(label.into());
        self
    }

    /// Expose an inert row as enabled to accessibility.
    ///
    /// Requires the inert flag set by the lazy constructor or
    /// [`Self::on_activate_maybe_with`]. It does not add activation or standalone
    /// keyboard focus; composites must own navigation.
    pub fn navigable_when_inert(mut self, navigable: bool) -> Self {
        self.navigable_when_inert = navigable;
        self
    }

    /// Declare drawn content text to suppress its duplicate accessibility label.
    ///
    /// Use only for content whose text is known; unrelated descendant prose
    /// should retain its own announcement.
    pub fn content_text(mut self, text: impl Into<String>) -> Self {
        self.content_text = Some(text.into());
        self
    }

    /// Set or clear known drawn text for duplicate-label suppression.
    ///
    /// `None` leaves descendant text to the normal name-equality rule.
    pub fn content_text_maybe(mut self, text: Option<String>) -> Self {
        self.content_text = text;
        self
    }

    /// Replace the default list-item accessibility descriptor.
    ///
    /// Focus and disabled state are still supplied by the underlying focus button.
    pub fn semantics(mut self, semantics: Semantics) -> Self {
        self.semantics_override = Some(semantics);
        self
    }

    fn resolved_row_height(&self) -> f32 {
        self.row_height_override
            .or_else(|| self.tokens.as_ref().and_then(TokenSource::fixed_row_height))
            .unwrap_or_else(|| {
                enzui_core::metrics::FinitePositive::new_or(
                    enzui_theme::text_defaults::sizing().row_height,
                    34.0,
                )
                .get()
            })
    }

    /// Read the gutter applied to layout.
    ///
    /// Precedence is the instance override, fixed tokens, then the default 4.
    /// A token resolver supplies no layout gutter without an explicit setter.
    #[must_use]
    pub fn resolved_outer_gutter(&self) -> f32 {
        self.outer_gutter_override
            .or_else(|| {
                self.tokens
                    .as_ref()
                    .and_then(TokenSource::fixed_outer_gutter)
            })
            .unwrap_or(DEFAULT_OUTER_GUTTER)
    }
}

impl<'a, Message> EnzListRowSurface<'a, Message>
where
    Message: Clone + 'a,
{
    /// Convert the builder into an [`Element`] without relying on type inference.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzListRowSurface<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(w: EnzListRowSurface<'a, Message>) -> Self {
        let captured = w.tokens.clone();

        let owner = w
            .label
            .as_deref()
            .or_else(|| {
                w.semantics_override
                    .as_ref()
                    .and_then(|semantics| semantics.label.as_deref())
            })
            .map(|label| enzui_core::identity::OwnerId::of_kind(&w.id_prefix, label));
        let widget_id =
            w.id.clone()
                .or_else(|| owner.as_ref().map(|o| o.id(ROW_ROLE)));
        let selected = w.selected;

        let group_above = w.group_selected_above;
        let group_below = w.group_selected_below;
        let selectable = w.selectable;
        let enabled = w.enabled;
        let reduced_motion = w.reduced_motion;

        let motion_progress: RowMotionProgress = Rc::new(Cell::new(0.0));

        use crate::row_motion::RowGroupHandle;
        let group_above_handle: RowGroupHandle =
            Rc::new(Cell::new(if selected && group_above { 1.0 } else { 0.0 }));
        let group_below_handle: RowGroupHandle =
            Rc::new(Cell::new(if selected && group_below { 1.0 } else { 0.0 }));

        // RowMotion retains the last active endpoint across view rebuilds for fade-out.
        let active_bg: RowActiveColor = Rc::new(Cell::new(Color::TRANSPARENT));
        let active_border: RowActiveColor = Rc::new(Cell::new(Color::TRANSPARENT));

        let (gap_top, gap_bottom, take_from_row) = match w.outer_vertical_gap {
            Some(gap) => (gap / 2.0, gap / 2.0, false),
            None => {
                let half = DEFAULT_ROW_GAP / 2.0;
                (
                    if selected && group_above { 0.0 } else { half },
                    if selected && group_below { 0.0 } else { half },
                    true,
                )
            }
        };
        let row_height = if take_from_row && !w.content_height {
            (w.resolved_row_height() - gap_top - gap_bottom).max(0.0)
        } else {
            w.resolved_row_height()
        };
        let fixed_tokens = match w.tokens.as_ref() {
            Some(TokenSource::Fixed(tokens)) => Some(tokens),
            _ => None,
        };
        let content_padding = w
            .content_padding_override
            .or_else(|| fixed_tokens.map(|t| t.content_padding))
            .unwrap_or(12.0);
        let content_align_y = w
            .content_align_y_override
            .or_else(|| fixed_tokens.map(|t| t.content_align_y))
            .unwrap_or(Alignment::Center);

        let outer_gutter = w.resolved_outer_gutter();
        let outer_width = w.width;

        let semantics = w.semantics_override.clone().unwrap_or_else(|| {
            let mut semantics = Semantics::new(Role::ListItem).default_action();
            if let Some(label) = &w.label {
                semantics = semantics.label(label.clone());
            }
            if selectable || selected {
                semantics = semantics.selected(selected);
            }
            semantics
        });

        let align_clip = w.trailing.is_none() && (!w.content_height || w.content_align_x.is_some());
        let (child, trailing_reveal): (Element<'a, Message>, Option<TrailingReveal>) =
            if let Some(trailing) = w.trailing {
                let reveal = w.trailing_reveal.unwrap_or_else(|| {
                    Rc::new(Cell::new(
                        matches!(w.trailing_visibility, TrailingVisibility::Always)
                            || (selected
                                && matches!(
                                    w.trailing_visibility,
                                    TrailingVisibility::OnHoverFocusOrSelected
                                )),
                    ))
                });
                let trailing = trailing_paint(trailing, w.trailing_visibility, Rc::clone(&reveal));
                let leading = container(w.child).width(Length::Fill);
                let leading = if w.content_height && w.content_align_x.is_none() {
                    leading
                } else {
                    leading
                        .height(Length::Fill)
                        .align_x(w.content_align_x.unwrap_or(Alignment::Start))
                        .align_y(content_align_y)
                };
                (
                    // The trailing action sits at the end edge: the left one
                    // in right-to-left text.
                    row![leading, Space::new().width(Length::Fixed(8.0)), trailing]
                        .mirrored(
                            enzui_core::placement::ambient_direction()
                                == enzui_core::placement::Direction::RightToLeft,
                        )
                        .align_y(Alignment::Center)
                        .into(),
                    Some(reveal),
                )
            } else {
                (w.child, w.trailing_reveal)
            };

        let alpha_override = w.disabled_content_alpha;
        let alpha_source = captured.clone();
        let content = DimContent::themed(child, move |theme| {
            content_alpha(enabled, alpha_override, alpha_source.as_ref(), theme)
        });

        let radius_source = captured.clone();

        let default_radius = ListRowSurfaceTokens::dark().row_radius;
        let baseline_radius = radius_source.as_ref().map_or(default_radius, |source| {
            match source {
                TokenSource::Fixed(t) => t.row_radius,

                // Before the first draw, event hit masking has only the default radius.
                TokenSource::Resolved(_) => default_radius,
            }
        });
        // Effective group progress keeps the mask and chrome coherent through selection fade-out.
        let ga_clip = Rc::clone(&group_above_handle);
        let gb_clip = Rc::clone(&group_below_handle);
        let content = crate::rounded_clip::rounded_clip(
            content,
            group_radius_interpolated(
                baseline_radius,
                group_above_handle.get(),
                group_below_handle.get(),
            ),
        )
        .radius_resolved(move |theme: &Theme| {
            let tokens = radius_source.as_ref().map_or_else(
                || crate::theme_lookup::resolve_live(theme, ListRowSurfaceTokens::from_palette),
                |source| source.resolve(theme),
            );

            group_radius_interpolated(tokens.row_radius, ga_clip.get(), gb_clip.get())
        });
        let content: Element<'a, Message> = if align_clip {
            content
                .align_content([
                    w.content_align_x.unwrap_or(Alignment::Start),
                    content_align_y,
                ])
                .into()
        } else {
            content.into()
        };

        let height = if w.content_height {
            Length::Shrink
        } else {
            Length::Fixed(row_height)
        };

        let mut btn = match (
            enabled,
            w.on_activate_with_point,
            w.on_activate_with_modifiers,
        ) {
            (true, Some(f), _) => {
                focus_button_maybe(content, None).on_press_with_point_maybe(Some(f))
            }
            (true, None, Some(f)) => {
                focus_button_maybe(content, None).on_press_with_modifiers_maybe(Some(f))
            }
            (true, None, None) => match (w.inert, w.on_activate_with) {
                (_, Some(f)) => focus_button_maybe_with(content, Some(f)),
                (false, None) => focus_button_maybe(content, w.on_activate),
                (true, None) => focus_button_maybe(content, None),
            },
            (false, _, _) => focus_button_maybe(content, None),
        }
        .width(w.width)
        .height(height)
        .padding([0.0, content_padding])
        // FocusButton defaults Fill content to centered; rows need leading alignment.
        .content_align_x(w.content_align_x.unwrap_or(iced::Alignment::Start))
        .content_align_y(content_align_y)
        // Rectangular clipping complements the rounded child mask.
        .clip(true)
        .embedded(w.embedded)
        .on_enter_with_maybe(w.on_enter)
        .on_space_with_maybe(w.on_space)
        .focus_visible(w.focus_visible)
        .semantics(semantics)
        .content_text_maybe(w.content_text)
        // Row motion animates this style itself.
        .reduced_motion(true)
        .style_focus({
            let motion_progress = Rc::clone(&motion_progress);
            let active_bg = Rc::clone(&active_bg);
            let active_border = Rc::clone(&active_border);
            let ga_style = Rc::clone(&group_above_handle);
            let gb_style = Rc::clone(&group_below_handle);
            move |theme: &Theme, status: FocusStatus| {
                let tokens = captured.as_ref().map_or_else(
                    || crate::theme_lookup::resolve_live(theme, ListRowSurfaceTokens::from_palette),
                    |source| source.resolve(theme),
                );

                row_style_at(
                    status,
                    selected,
                    ga_style.get(),
                    gb_style.get(),
                    tokens,
                    motion_progress.get(),
                    active_bg.as_ref(),
                    active_border.as_ref(),
                )
            }
        });

        // Accessible navigation actions must survive the missing primary activation callback.
        if w.navigable_when_inert && w.inert {
            btn = btn.disabled_semantics(false);
        }

        if let Some(id) = widget_id {
            btn = btn.id(id);
        }

        let row: Element<'a, Message> =
            RowMotion::new(btn.into(), selected, reduced_motion, motion_progress)
                .track_hover(true)
                .active_colors(active_bg, active_border)
                .group(
                    group_above,
                    group_below,
                    group_above_handle,
                    group_below_handle,
                )
                .trailing_reveal(
                    trailing_reveal,
                    w.focus_visible
                        || (selected
                            && matches!(
                                w.trailing_visibility,
                                TrailingVisibility::OnHoverFocusOrSelected
                            )),
                    w.trailing_reveal_parented,
                )
                .into();

        let inner: Element<'a, Message> = match w.on_context {
            None => row,
            Some(on_context) => {
                let mut target = crate::context_target::enz_context_target(row, on_context)
                    .width(w.width)
                    // The target defaults to Fill, which would capture the pane below this row.
                    .height(Length::Shrink);

                let context_id = match w.context_identity {
                    ContextIdentity::Pinned(id) => id,
                    ContextIdentity::Derive => owner.as_ref().map(|o| o.id(ROW_CONTEXT_ROLE)),
                };
                if let Some(id) = context_id {
                    target = target.id(id);
                    if let Some(label) = w.context_label {
                        target = target.label(label);
                    }
                }
                target.into()
            }
        };

        // Padded layout preserves min-height; stock Container loosens the child limits.
        outer_gutter_widget(inner, outer_width, outer_gutter, gap_top, gap_bottom)
    }
}

pub(crate) fn trailing_paint<'a, Message: 'a>(
    child: Element<'a, Message>,
    visibility: TrailingVisibility,
    reveal: TrailingReveal,
) -> Element<'a, Message> {
    if matches!(visibility, TrailingVisibility::Always) {
        child
    } else {
        Element::new(TrailingPaint { child, reveal })
    }
}

struct TrailingPaint<'a, Message> {
    child: Element<'a, Message>,
    reveal: TrailingReveal,
}

struct OuterGutter<'a, Message> {
    child: iced::Element<'a, Message>,
    width: Length,
    gutter: f32,
    gap_top: f32,
    gap_bottom: f32,
}

fn outer_gutter_widget<'a, Message: 'a>(
    child: iced::Element<'a, Message>,
    width: Length,
    gutter: f32,
    gap_top: f32,
    gap_bottom: f32,
) -> iced::Element<'a, Message> {
    Element::new(OuterGutter {
        child,
        width,
        gutter,
        gap_top,
        gap_bottom,
    })
}

impl crate::theme_lookup::FollowsThemeOpacity for ListRowSurfaceTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled_surface(),
            ..self
        }
    }
}

fn group_radius_interpolated(row_radius: f32, above_t: f32, below_t: f32) -> iced::border::Radius {
    let top = row_radius * (1.0 - above_t.clamp(0.0, 1.0));
    let bottom = row_radius * (1.0 - below_t.clamp(0.0, 1.0));
    iced::border::Radius {
        top_left: top,
        top_right: top,
        bottom_right: bottom,
        bottom_left: bottom,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the style probe keeps independently varied motion inputs explicit"
)]
fn row_style_at(
    status: FocusStatus,
    selected: bool,
    group_above_t: f32,
    group_below_t: f32,
    tokens: ListRowSurfaceTokens,
    progress: f32,
    active_bg: &std::cell::Cell<Color>,
    active_border: &std::cell::Cell<Color>,
) -> Style {
    let p = progress.clamp(0.0, 1.0);
    let resting = tokens.surface_background;

    let radius = group_radius_interpolated(tokens.row_radius, group_above_t, group_below_t);

    if matches!(status, FocusStatus::Pressed) {
        return Style {
            background: Some(Background::Color(composite(
                resting,
                tokens.pressed_overlay,
            ))),
            text_color: tokens.text_color,
            border: Border {
                color: tokens.border_color,
                width: tokens.border_width,
                radius,
            },
            ..Default::default()
        };
    }

    let endpoint: Option<(Color, Color)> = if selected {
        Some((tokens.selected_background, tokens.selected_border))
    } else if matches!(status, FocusStatus::Hovered) {
        Some((
            composite(resting, tokens.hover_overlay),
            tokens.border_color,
        ))
    } else {
        None
    };

    // Inactive frames retain the last active endpoint so reverse motion fades from it.
    if let Some((bg, brd)) = endpoint {
        active_bg.set(bg);
        active_border.set(brd);
    }

    let bg = lerp_color(resting, active_bg.get(), p);
    let border_color = lerp_color(tokens.border_color, active_border.get(), p);

    let border = if matches!(status, FocusStatus::Focused) && !selected {
        Border {
            color: tokens.focus_ring,
            width: tokens.focus_ring_width,
            radius,
        }
    } else {
        Border {
            color: border_color,
            width: tokens.border_width,
            radius,
        }
    };

    Style {
        background: Some(Background::Color(bg)),
        text_color: tokens.text_color,
        border,
        ..Default::default()
    }
}
