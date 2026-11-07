//! Sidebar list rows for a [`sidebar_content`](crate::sidebar_content) column,
//! typically in a [`navigation_split_view`](crate::navigation_split_view)'s sidebar.
//!
//! A row has an optional leading icon, a label, an optional count pinned to
//! the trailing edge, and an optional trailing control. Build one with
//! [`enz_sidebar_row`], or with [`enz_sidebar_row_lazy`] when a rail should
//! build its activation message only when a row is activated.
//!
//! # Options
//! - `.count(text)` shows a trailing count, published to assistive technology
//!   as the row's description.
//! - `.icon(name)` adds a leading icon; `.leading_indent(px)` indents nested
//!   rows.
//! - `.selected(bool)` paints the selection fill and publishes the selected
//!   state. The label keeps one weight either way, so selecting a row never
//!   reflows or re-elides it. `.show_highlight(false)` drops the selection and
//!   hover fills but keeps the selected ink and state.
//! - `.trailing(element)` places an interactive control (remove button, menu,
//!   toggle) inside the row's hover and selection fill, after the count.
//!   `.trailing_visibility(TrailingVisibility::OnHoverOrFocus)` paints it only
//!   while the row is hovered or focused, keeping its slot and focus order.
//! - `.reduced_motion(bool)` makes the selection move between rows instantly.
//! - `.enabled(bool)` and `.tokens(SidebarRowTokens)`.
//!
//! # Semantics and interaction
//! Rows compose [`EnzListRowSurface`](crate::list_row_surface::EnzListRowSurface):
//! each is a selectable `Role::ListItem` with keyboard focus, a focus-visible
//! ring, and distinct hover and pressed overlays. Labels stay on one line and
//! elide at the end; the count keeps a minimum width and a gap from the label.
//! A trailing control receives its own clicks and publishes its own
//! accessible node beside the row's. Wrap a control with `.trailing(..)`
//! rather than `row![sidebar_row, control]`, so it sits inside the row's fill.
//!
//! For layouts beyond `[icon] [label] [count] [trailing]` (title and subtitle,
//! avatars, status columns), compose
//! [`list_row_surface`](crate::list_row_surface) directly.
//!
//! ## Example — with a trailing action

use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::widget::{Space, container, row, text};
use iced::{Alignment, Color, Element, Font, Length, Padding, font::Weight};

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colour + geometry tokens for the sidebar row.
#[derive(Debug, Clone, Copy)]
pub struct SidebarRowTokens {
    /// Idle row fill (default transparent — inherits the sidebar background).
    pub base_background: Color,
    /// Fill on hover — the composited overlay role
    /// ([`enzui_theme::SurfaceRole::Overlay`]): a translucent wash from
    /// [`enzui_theme::Palette::overlay_hover`], not an opaque surface.
    pub hover_background: Color,
    /// Fill when the row is the selected item: a translucent accent tint
    /// ([`selected_tint`]), so it reads on any sidebar surface.
    pub selected_background: Color,
    /// Label ink for an unselected row (default `text_muted`).
    pub text: Color,
    /// Label and icon ink for the selected row: the accent, deepened (light)
    /// or lifted (dark) to 4.5:1 on the tint ([`selected_ink`]).
    pub selected_text: Color,
    /// Ink for muted secondary text such as the count (default `text_muted`).
    pub muted: Color,
    /// Leading icon tint (default `text_muted`).
    pub icon: Color,
    /// Ink used when the row is disabled (default `disabled`).
    pub disabled: Color,
    /// Keyboard focus-ring colour (default `focus_ring`).
    pub focus_ring: Color,

    /// Row height in px (default 34, the [`SizingTokens::sidebar_row_height`]
    /// semantic token). Was hardcoded `32.0` earlier, disagreeing with both
    /// the specific `sidebar_row_height` and the generic `row_height` sizing
    /// tokens (both 34); routed to the token via the builder's `.sizing()`
    /// seam.
    ///
    /// [`SizingTokens::sidebar_row_height`]: enzui_theme::SizingTokens::sidebar_row_height
    pub row_height: f32,
    /// Row corner radius in px (default 8).
    pub row_radius: f32,
    /// Horizontal padding inside the row in px (default 8). This insets the
    /// content (icon, label, count) inside the painted pill; the pill
    /// itself is inset from the pane edge by [`Self::outer_gutter`]. The two
    /// compose into [`Self::label_inset_x`], the single number a section heading
    /// aligns its caption to.
    pub row_padding_x: f32,
    /// Horizontal inset in px of the row's fill and focus ring from the pane
    /// edge (default [`DEFAULT_OUTER_GUTTER`](crate::list_row_surface::DEFAULT_OUTER_GUTTER) = 4).
    /// Use `0.0` inside a container that already pads its rows.
    pub outer_gutter: f32,
    /// Vertical inset in px of the row's fill and focus ring from the top
    /// and bottom of the row (default 1). The row keeps its height; only the
    /// selected and hover fills are shorter, so a selected row and a hovered
    /// neighbour read as two pills with a hairline between them rather than
    /// one block.
    pub fill_inset_y: f32,
    /// Leading icon size in px (default 16).
    pub icon_size: f32,
    /// Gap between the icon and the label in px (default 8).
    pub icon_gap: f32,
    /// Label text size in px (default 13).
    pub label_size: f32,
    /// Minimum width reserved for the trailing count chip in px (default 32).
    /// Acts as a floor: short counts are padded out to this width so the
    /// sidebar's trailing column reads straight down; long counts (e.g.
    /// `"99999"`, `">99999"`) grow past it rather than being clipped. Pre-
    /// this was applied via `Length::Fixed` and acted as an exact pin.
    pub count_min_width: f32,
}

/// Resolve disabled sidebar ink only when the row itself publishes an opaque
/// selected fill. Flat rows inherit an arbitrary parent surface, so retaining
/// the preferred rung there is deliberate: guessing `background` or `surface`
/// would resolve against a pixel the widget does not paint.
fn disabled_ink(
    p: &Palette,
    tokens: &SidebarRowTokens,
    selected: bool,
    show_highlight: bool,
    parent_backdrop: Option<Color>,
) -> Color {
    let own_fill = if selected && show_highlight {
        tokens.selected_background
    } else {
        tokens.base_background
    };
    let backdrop = if own_fill.a >= 0.999 {
        Some(own_fill)
    } else {
        parent_backdrop.map(|parent| crate::surface_interaction::composite(parent, own_fill))
    };
    backdrop.map_or(tokens.disabled, |backdrop| {
        p.disabled_on_with_preferred(tokens.disabled, backdrop)
    })
}

/// The label weight for a row. Selection never changes it, so selecting a
/// row cannot reshape the label or push it past its elision point; the fill
/// and ink show the selection instead.
#[inline]
pub(crate) fn label_weight_for(_selected: bool) -> Weight {
    // The single choice: one weight in every state. Medium, not Semibold:
    // a sidebar is a column of navigation labels, set as macOS and GNOME set
    // them, at a control weight; Semibold made every row read as a heading
    // and left the section captions nothing to stand out with. Every read
    // in the widget goes through this function; a future edit that branches
    // the weight on selection has to change it here, and would then trip
    // the gate in the tests module rather than silently returning to the
    // reflow behaviour.
    enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Medium)
}

/// The stock horizontal distance from a sidebar row's outer edge to the
/// left edge of its icon/label rail: [`SidebarRowTokens::outer_gutter`] +
/// [`SidebarRowTokens::row_padding_x`] (`4 + 8 = 12` at the stock tokens).
///
/// This is what a sidebar section caption must be inset by to sit on the
/// same vertical rail as the row labels below it — a pill that insets
/// while the caption does not is the same defect mirrored, so
/// [`crate::sidebar_content::EnzSidebarContent::push_section`] reads this
/// number rather than restating `12`. Geometry is palette-independent, so this
/// needs no theme.
pub fn default_label_inset_x() -> f32 {
    SidebarRowTokens::dark().label_inset_x()
}

/// The selected-row fill: the accent at a low opacity.
///
/// A translucent tint rather than an opaque grey step, as macOS source lists
/// and Fluent's navigation view mark the current place: the selection reads
/// as "here" in the app's accent on whatever surface the sidebar uses (grey,
/// white or material), and the label no longer has to jump to full-strength
/// black on grey to stand out.
pub fn selected_tint(p: &Palette) -> Color {
    let alpha = if p.is_dark() { 0.24 } else { 0.13 };
    Color {
        a: alpha,
        ..p.accent
    }
}

/// The selected-row label and icon ink: the accent, blended toward the
/// palette's text ink (darker in light mode, lighter in dark) until it clears
/// 4.5:1 (with a small margin) against the tint composited over the sidebar body
/// ([`Palette::surface_alt`]).
///
/// A saturated accent cannot be darkened along its own hue and chroma (it
/// already sits at the gamut edge), so the fixed-hue `raise_to_floor` search
/// returns it unchanged; mixing toward the text ink keeps the hue and reaches
/// the floor.
pub fn selected_ink(p: &Palette) -> Color {
    use crate::accessibility_metrics::{composite_over, contrast_ratio};
    let backdrop = composite_over(selected_tint(p), p.surface_alt);
    let mix = |t: f32| Color {
        r: p.accent.r + (p.text.r - p.accent.r) * t,
        g: p.accent.g + (p.text.g - p.accent.g) * t,
        b: p.accent.b + (p.text.b - p.accent.b) * t,
        a: 1.0,
    };
    (0..=50)
        .map(|step| mix(step as f32 / 50.0))
        // 4.6, not 4.5: the painted tint can land one sRGB step from the
        // composite computed here, which is worth a few hundredths of ratio.
        .find(|ink| contrast_ratio(*ink, backdrop) >= 4.6)
        .unwrap_or(p.text)
}

impl SidebarRowTokens {
    /// Distance from the row's outer edge to its content (icon/label) rail:
    /// [`Self::outer_gutter`] + [`Self::row_padding_x`]. See
    /// [`default_label_inset_x`].
    pub fn label_inset_x(&self) -> f32 {
        self.outer_gutter + self.row_padding_x
    }

    /// Derive tokens from a runtime palette (the normal path).
    pub fn from_palette(p: &Palette) -> Self {
        use enzui_theme::text_scale::ui_px;
        let ramp = enzui_theme::TypographyScale::ambient();
        Self {
            base_background: Color::TRANSPARENT,
            // A translucent wash, the same `overlay_hover` role ghost buttons
            // use, so it reads over any surface behind the row.
            hover_background: p.overlay_hover(),
            // The persistent-selection role (`SurfaceRole::Selected`), so
            // read `surface_selected`, not the raw `selected` alias. Identical
            // by default; distinct the moment a theme overrides one.
            selected_background: selected_tint(p),
            text: p.text_muted,
            selected_text: selected_ink(p),
            muted: p.text_muted,
            icon: p.text_muted,
            disabled: p.disabled,
            focus_ring: p.focus_ring,

            // The semantic token is `SizingTokens::sidebar_row_height`
            // (34.0), which the generic `row_height` (also 34.0) agrees with.
            // The theme's density, grown with the desktop text size.
            row_height: ui_px(enzui_theme::text_defaults::sizing().sidebar_row_height),
            row_radius: 8.0,
            row_padding_x: 8.0,
            // Adopt the shared row-chrome gutter default rather than
            // re-deriving a sidebar-local number, so a retune of
            // `DEFAULT_OUTER_GUTTER` moves every row surface in the framework
            // at once (pinned by the sidebar chrome suite's
            // `designed_row_chrome_matches_the_shipped_tokens`).
            outer_gutter: crate::list_row_surface::DEFAULT_OUTER_GUTTER,
            fill_inset_y: 1.0,
            icon_size: ui_px(16.0),
            icon_gap: 8.0,
            label_size: ramp.body_size,
            count_min_width: 32.0,
        }
    }
    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// The id namespace for rows built without [`id`](EnzSidebarRow::id) or
/// [`id_prefix`](EnzSidebarRow::id_prefix). Ids derive from the label, so an
/// anonymous row still exposes a click action to assistive technology.
pub const DEFAULT_ID_PREFIX: &str = "enz-sidebar-row";

/// Builder for a sidebar navigation row. See [`enz_sidebar_row`].
#[must_use]
pub struct EnzSidebarRow<'a, Message> {
    label: String,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    count: String,
    icon: String,
    /// App-owned icon slot. When set, wins over `icon` and renders
    /// untinted for `Svg`/`Image` sources so a workspace favicon or a
    /// vendor brand mark keeps its own colours. A bundled name in this
    /// field is treated as [`Self::icon`] would be (theme-muted tint,
    /// full theme reactivity).
    icon_source: Option<crate::icon::IconSource>,
    selected: bool,
    show_highlight: bool,
    /// Per-instance reduced-motion override forwarded to the composed row
    /// surface. `None` follows the theme.
    reduced_motion: Option<bool>,
    leading_indent: f32,
    enabled: bool,
    /// Eager activation message, from [`enz_sidebar_row`]. `None` from
    /// [`enz_sidebar_row_lazy`].
    on_activate: Option<Message>,
    /// Lazy activation factory. Wins over `on_activate` when set; the
    /// factory runs at activation time, not once per row per view pass.
    on_activate_with: Option<Box<dyn Fn() -> Message + 'a>>,
    tokens: Option<SidebarRowTokens>,
    /// Opaque parent fill supplied by [`Self::disabled_on`]. Flat rows have no
    /// other honest way to resolve their disabled ink.
    disabled_backdrop: Option<Color>,
    sizing: Option<enzui_theme::SizingTokens>,
    id: Option<iced::advanced::widget::Id>,
    /// Namespace for the label-derived default id. Defaults to
    /// [`DEFAULT_ID_PREFIX`]; a screen showing two sidebars that share a row
    /// label ("Inbox" in two accounts) gives each sidebar its own prefix so the
    /// two rows do not collide on one derived id (a collision would silently
    /// drop the second row from the published tree — the duplicate-id
    /// suppression in
    /// `enzui_core::collect::SemanticsCollector::to_tree_update_with_ids`,
    /// recorded by `enzui_core::collect::SemanticsCollector::dropped_duplicate_ids`).
    id_prefix: String,
    /// Optional trailing controls (a remove button, kebab menu or toggle),
    /// laid out after the count and inside the row's hover and selected
    /// pill. They publish their own accessible nodes beside the row's
    /// `Role::ListItem`, and receive their own clicks without the row
    /// swallowing them: `list_row_surface` bails on events a `focus_button`
    /// has captured. See the module-level examples.
    trailing: Option<Element<'a, Message>>,
    /// Paint policy for `trailing`; layout/focus/accessibility are unaffected.
    trailing_visibility: crate::list_row_surface::TrailingVisibility,
    /// Secondary-press (right-click) hook, forwarded to the composed
    /// `list_row_surface`. See [`EnzSidebarRow::on_context`].
    on_context: Option<Box<dyn Fn(iced::Point) -> Message + 'a>>,
    /// Accessible name for the context-menu affordance.
    context_label: Option<String>,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Construct a sidebar row.
pub fn enz_sidebar_row<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    on_activate: Message,
) -> EnzSidebarRow<'a, Message> {
    EnzSidebarRow {
        label: label.into(),
        count: String::new(),
        icon: String::new(),
        icon_source: None,
        direction: None,
        selected: false,
        show_highlight: true, // Fill is the default; opt OUT with `.show_highlight(false)`.
        leading_indent: 0.0,
        enabled: true,
        on_activate: Some(on_activate),
        on_activate_with: None,
        tokens: None,
        reduced_motion: None,
        disabled_backdrop: None,
        sizing: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        trailing: None,
        trailing_visibility: crate::list_row_surface::TrailingVisibility::Always,
        on_context: None,
        context_label: None,
        _lifetime: std::marker::PhantomData,
    }
}

/// A sidebar row with no activation yet: pair it with
/// [`on_activate_maybe_with`](EnzSidebarRow::on_activate_maybe_with), whose
/// closure builds the message only when the row is activated. A rail of N
/// rows then builds no messages per view pass. Without an activation the row
/// is inert but not announced as disabled.
pub fn enz_sidebar_row_lazy<'a, Message: Clone + 'a>(
    label: impl Into<String>,
) -> EnzSidebarRow<'a, Message> {
    EnzSidebarRow {
        label: label.into(),
        count: String::new(),
        icon: String::new(),
        icon_source: None,
        direction: None,
        selected: false,
        show_highlight: true, // Fill is the default; opt OUT with `.show_highlight(false)`.
        leading_indent: 0.0,
        enabled: true,
        on_activate: None,
        on_activate_with: None,
        tokens: None,
        reduced_motion: None,
        disabled_backdrop: None,
        sizing: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        trailing: None,
        trailing_visibility: crate::list_row_surface::TrailingVisibility::Always,
        on_context: None,
        context_label: None,
        _lifetime: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzSidebarRow<'a, Message> {
    /// Override the writing direction. By default the row follows the
    /// locale; right-to-left puts the icon on the right and the count on
    /// the left.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Trailing count/badge text (e.g. unread count). Empty by default (no chip).
    pub fn count(mut self, c: impl Into<String>) -> Self {
        self.count = c.into();
        self
    }
    /// Leading icon by Lucide/semantic name. Empty by default (no icon).
    pub fn icon(mut self, i: impl Into<String>) -> Self {
        self.icon = i.into();
        self
    }
    /// A leading icon from an app-owned source, which takes precedence over
    /// [`icon`](Self::icon): a bundled Lucide name, an SVG handle, or an image
    /// handle (`.ico`, `.png`), through [`IconSource`](crate::icon::IconSource).
    ///
    /// Bundled names are tinted with the row's icon colour; SVG and image
    /// handles keep their own colours so brand marks stay recognisable. The
    /// icon is decorative: the row's label is its accessible name.
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.icon_source = Some(source.into());
        self
    }
    /// Mark the row as the currently selected item. Defaults to `false`.
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
    /// Whether the row paints its selection fill and hover wash (default
    /// `true`). With `false` a selected row still uses the selected ink and
    /// publishes its selected state; use it on surfaces that draw their own
    /// selection chrome.
    pub fn show_highlight(mut self, s: bool) -> Self {
        self.show_highlight = s;
        self
    }
    /// Left indent in px for nested rows. Non-finite or negative input falls
    /// back to 0. Defaults to 0.
    pub fn leading_indent(mut self, i: f32) -> Self {
        self.leading_indent = enzui_core::metrics::FiniteNonNeg::new_or(i, 0.0).get();
        self
    }
    /// Enable or disable the row. Defaults to `true`.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Override the colour/geometry tokens. Unset by default (resolved from the
    /// runtime theme so live theme switches apply).
    pub fn tokens(mut self, t: SidebarRowTokens) -> Self {
        self.tokens = Some(t);
        self
    }

    /// The opaque colour behind a disabled row whose own fill is transparent,
    /// so its disabled ink is resolved against what is actually painted there.
    /// A translucent pinned fill is composited over it first.
    pub fn disabled_on(mut self, backdrop: Color) -> Self {
        self.disabled_backdrop = Some(backdrop);
        self
    }

    /// The theme's sizing tokens, so the row height follows
    /// [`sidebar_row_height`](enzui_theme::SizingTokens::sidebar_row_height)
    /// (layout runs before the theme reaches the widget). A `row_height` pinned
    /// through `.tokens(..)` wins; invalid values fall back to the 34 px
    /// default.
    pub fn sizing(mut self, sizing: enzui_theme::SizingTokens) -> Self {
        self.sizing = Some(sizing);
        self
    }

    /// Assign a stable widget identity, forwarded to the composed
    /// [`crate::focus_button::FocusButton`] so programmatic `focus(id)` and
    /// AccessKit action routing can target this row.
    ///
    /// Overrides the label-derived default id entirely. Prefer this when the
    /// caller owns a real route key (a mailbox id, a settings-page id) that is
    /// more stable than the visible label — the label moves when the copy is
    /// retranslated, and a label-derived id would then re-identify the row.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the label-derived default id.
    ///
    /// Two sidebars on one screen that each carry a row labelled "Inbox" would
    /// otherwise derive the same id under [`DEFAULT_ID_PREFIX`] and collide;
    /// release keeps the first and drops the second from the tree. A
    /// distinct prefix per sidebar keeps every row routable. Ignored when an
    /// explicit [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Lazy activation that may be absent. Wins over the eager message
    /// from [`enz_sidebar_row`] when set.
    ///
    /// `None` leaves the row inert without marking it *disabled*: an
    /// unactivatable row is not a greyed-out one. Forwards to
    /// [`crate::list_row_surface::EnzListRowSurface::on_activate_maybe_with`],
    /// which owns that distinction.
    pub fn on_activate_maybe_with(mut self, f: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_activate_with = f;
        self
    }

    /// A trailing control (remove button, menu, toggle) placed inside the
    /// row's hover and selection fill, after the count. It keeps its own
    /// accessible node and receives its own clicks.
    pub fn trailing(mut self, control: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(control.into());
        self
    }

    /// Force or forbid reduced motion for this row's hover/selection chrome.
    /// Unset follows the active theme policy.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Choose when the trailing control is painted.
    ///
    /// [`crate::list_row_surface::TrailingVisibility::OnHoverOrFocus`] keeps
    /// the control mounted in its stable slot and in the accessibility/Tab
    /// order at rest; row hover or keyboard focus on either the row or control
    /// reveals it. `OnHoverFocusOrSelected` also reveals the selected row's
    /// action. The default is `Always`.
    pub fn trailing_visibility(
        mut self,
        visibility: crate::list_row_surface::TrailingVisibility,
    ) -> Self {
        self.trailing_visibility = visibility;
        self
    }

    /// The secondary-press (right-click) hook, given the press position in
    /// window space; the app builds its context menu from it.
    pub fn on_context(mut self, f: impl Fn(iced::Point) -> Message + 'a) -> Self {
        self.on_context = Some(Box::new(f));
        self
    }

    /// The accessible name announced for the row's context-menu affordance
    /// (for example, `"Mailbox actions"`). Only meaningful alongside
    /// [`Self::on_context`].
    pub fn context_label(mut self, label: impl Into<String>) -> Self {
        self.context_label = Some(label.into());
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // Geometry is palette-independent, so it can be read eagerly from
        // whichever tokens resolve. Colours must not be: an unpinned row
        // resolves them per render below so `Theme::Light` cannot inherit
        // near-white ink and dark selection fills from the dark fallback.
        // Moved out before the partial moves below (`self.trailing`,
        // `self.on_activate_with`) borrow the builder apart.
        let on_context = self.on_context;
        let context_label = self.context_label;
        let trailing_visibility = self.trailing_visibility;
        let reduced_motion = self.reduced_motion;
        let selected_for_trailing = self.selected;
        let trailing_reveal = if self.trailing.is_some()
            && !matches!(
                trailing_visibility,
                crate::list_row_surface::TrailingVisibility::Always
            ) {
            Some(std::rc::Rc::new(std::cell::Cell::new(
                selected_for_trailing
                    && matches!(
                        trailing_visibility,
                        crate::list_row_surface::TrailingVisibility::OnHoverFocusOrSelected
                    ),
            )))
        } else {
            None
        };
        let pinned = self.tokens;
        let t = pinned.unwrap_or_else(SidebarRowTokens::dark);
        // The row height feeds `Length::Fixed` in `layout()`, which has no
        // theme, so it is resolved here at construction: pinned `.tokens(..)`
        // win, then the forwarded runtime scale's `sidebar_row_height`, then
        // the 34 px default. `FiniteNonNeg` keeps a bad hand-built scale
        // value out of layout.
        let row_height = if pinned.is_some() {
            t.row_height
        } else if let Some(sizing) = self.sizing {
            enzui_core::metrics::FiniteNonNeg::new_or(sizing.sidebar_row_height, t.row_height).get()
        } else {
            t.row_height
        };
        // The row-chrome gutter is geometry too — the composed
        // `list_row_surface` consumes it in `layout()`, where iced supplies no
        // `&Theme` — so it resolves eagerly beside `row_height`. A pinned
        // `.tokens(…)` can retune it (or zero it for a full-bleed row);
        // otherwise it is the shared `DEFAULT_OUTER_GUTTER`. A junk value in a
        // hand-built token struct falls back rather than reaching layout.
        let outer_gutter = enzui_core::metrics::FiniteNonNeg::new_or(
            t.outer_gutter,
            crate::list_row_surface::DEFAULT_OUTER_GUTTER,
        )
        .get();
        // The fill's vertical inset comes out of the row's own height, so the
        // row, and every row's position below it, does not move.
        let fill_inset_y = enzui_core::metrics::FiniteNonNeg::new_or(t.fill_inset_y, 1.0)
            .get()
            .min(row_height / 4.0);
        let fill_height = row_height - 2.0 * fill_inset_y;
        let selected = self.selected;
        let show_highlight = self.show_highlight;
        let enabled = self.enabled;
        let disabled_backdrop = self.disabled_backdrop;
        let a11y_name = self.label.clone();
        // The row's stable logical key for its derived id. Captured here
        // because `self.label` is moved into the visible `text(...)` below.
        let id_key = self.label.clone();

        // Per-render colour resolution, shared by every child below.
        let resolve = move |theme: &iced::Theme| -> SidebarRowTokens {
            pinned.unwrap_or_else(|| {
                crate::theme_lookup::resolve(theme, SidebarRowTokens::from_palette)
            })
        };
        let label_color = move |theme: &iced::Theme| {
            let t = resolve(theme);
            if !enabled {
                disabled_ink(
                    &crate::theme_lookup::palette(theme),
                    &t,
                    selected,
                    show_highlight,
                    disabled_backdrop,
                )
            } else if selected {
                t.selected_text
            } else {
                t.text
            }
        };
        let icon_color = move |theme: &iced::Theme| {
            let t = resolve(theme);
            if !enabled {
                disabled_ink(
                    &crate::theme_lookup::palette(theme),
                    &t,
                    selected,
                    show_highlight,
                    disabled_backdrop,
                )
            } else if selected {
                t.selected_text
            } else {
                t.icon
            }
        };
        let count_color = move |theme: &iced::Theme| {
            let t = resolve(theme);
            if !enabled {
                disabled_ink(
                    &crate::theme_lookup::palette(theme),
                    &t,
                    selected,
                    show_highlight,
                    disabled_backdrop,
                )
            } else {
                t.muted
            }
        };
        // Selection never changes the label weight; see `label_weight_for`.
        let label_weight = label_weight_for(selected);

        // [indent] [icon] [gap] [label: Fill, one line, elided] [count].
        // The label is the only `Fill` child, so the count stays pinned to the
        // trailing edge; the clip keeps a partly elided glyph out of its lane.
        use iced::widget::text::{Ellipsis, Wrapping};
        // Captured before `self.count` is moved into the visible `text(...)`.
        let count_annotation = if self.count.is_empty() {
            None
        } else {
            Some(enzui_core::localized!("enzui-sidebar-item-count", "count" => self.count.as_str()))
        };
        let mut children: Vec<Element<'a, Message>> = Vec::new();
        if self.leading_indent > 0.0 {
            children.push(
                Space::new()
                    .width(Length::Fixed(self.leading_indent))
                    .into(),
            );
        }
        // `icon_source` Wins over `icon` when set, so a workspace
        // favicon or vendor brand mark can occupy the leading slot without
        // hand-rolling the row. The bundled-name branch keeps the row's
        // `.tint_resolved(icon_color)`, so a Lucide fallback beside an
        // app-owned favicon still re-tints on hover / theme change.
        let leading_icon: Option<Element<'a, Message>> = match self.icon_source {
            Some(source) if !source.is_empty() => {
                Some(source.into_slot_element(t.icon_size, |icon| icon.tint_resolved(icon_color)))
            }
            // A `Named("")` from `icon_source("")` is treated as no icon,
            // matching the legacy `icon("")` shape; fall through to the
            // legacy `.icon(name)` field.
            Some(_) | None => {
                if !self.icon.is_empty() {
                    Some(
                        crate::icon::enz_icon(&self.icon)
                            .size(t.icon_size)
                            .tint_resolved(icon_color)
                            .into(),
                    )
                } else {
                    None
                }
            }
        };
        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;
        if let Some(icon_el) = leading_icon {
            children.push(icon_el);
            children.push(Space::new().width(Length::Fixed(t.icon_gap)).into());
        }
        children.push(
            container(
                text(self.label)
                    .size(t.label_size)
                    .wrapping(Wrapping::None)
                    .ellipsis(Ellipsis::End)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(label_color(theme)),
                    })
                    .font(Font {
                        weight: label_weight,
                        ..enzui_theme::text_defaults::font(
                            enzui_theme::FontRole::Ui,
                            enzui_theme::WeightRung::Regular,
                        )
                    }),
            )
            // The one `Fill` element: takes the leftover, right-pinning the
            // count. `clip(true)` hard-stops an elided partial glyph.
            .width(Length::Fill)
            .align_x(if rtl {
                iced::alignment::Horizontal::Right
            } else {
                iced::alignment::Horizontal::Left
            })
            .clip(true)
            .into(),
        );
        // If a count is present the label’s `Length::Fill` right-pins the
        // count. When neither count nor trailing control exists the label is
        // still the sole `Fill` element, so its container consumes the row.
        // When only a trailing element is present, the label still fills and
        // the trailing control sits at the pinned right edge — there is one
        // `Fill` regardless (`sidebar_row`’s established right-pin pattern).
        if !self.count.is_empty() {
            // A protected gap between an elided label and the count, so the two
            // never touch even at the narrowest width the label is clipped to.
            children.push(Space::new().width(Length::Fixed(t.icon_gap)).into());
            // `count_min_width` Is a floor, not a pin. The prior
            // `Length::Fixed(t.count_min_width)` bound the width in both
            // directions, so any count that shapes wider than 32 px at
            // `label_size = 13` (`"99999"`, `">99999"`, a long localised
            // string) was clipped to the "minimum" rather than growing.
            // `Length::Fit.min(w)` raises the
            // layout limit's `min_width` to `w` while letting the resolver
            // fall through to `intrinsic.min(max).max(min)`: short counts are
            // padded out to the floor (keeping the sidebar's trailing
            // column aligned), long counts grow past it, and a parent
            // narrower than the floor clamps it to the parent
            // (`Limits::width` at `min.min(self.max.width)`). Same fix as
            // `EnzButton::min_width` in `72abf2a`; pinned by
            // `tests/enz_sidebar_row_count_min_width_floor.rs`.
            children.push(
                container(
                    text(self.count)
                        .size(t.label_size)
                        .wrapping(Wrapping::None)
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(count_color(theme)),
                        }),
                )
                // Right-align the count inside its lane so short counts pin to
                // the trailing edge (bullet 3).
                .align_x(Alignment::End)
                .width(Length::Fit.min(t.count_min_width))
                .into(),
            );
        }

        // [label] [gap] [count] [gap] [trailing], inside the row's fill.
        if let Some(trailing) = self.trailing {
            children.push(Space::new().width(Length::Fixed(t.icon_gap)).into());
            let trailing = match trailing_reveal.as_ref() {
                Some(reveal) => crate::list_row_surface::trailing_paint(
                    trailing,
                    trailing_visibility,
                    std::rc::Rc::clone(reveal),
                ),
                None => trailing,
            };
            children.push(trailing);
        }

        let row_inner = row(children)
            .mirrored(rtl)
            .align_y(Alignment::Center)
            .spacing(0);
        // Full row height, so the icon and label centre on the row's midline.
        let content = container(row_inner)
            .height(Length::Fill)
            .align_y(Alignment::Center)
            .padding(Padding::from([0.0, t.row_padding_x]));

        // The row is a selectable `Role::ListItem`; the count is published as
        // its description rather than as a separate text node.
        let mut semantics = Semantics::new(Role::ListItem)
            .label(a11y_name)
            .selected(selected)
            .disabled(!enabled);
        // Only an enabled row exposes a default (activate) action. A disabled
        // row must not *claim* `supports_default_action`: the `disabled` gate
        // masks it at the AccessKit hop, but the stored flag is a public field
        // non-AccessKit consumers read directly, so "this inert row has a
        // default action" would be a false statement in the model
        // (`activatable_widgets_advertise_click::disabled_sidebar_row_does_not_advertise_click`).
        if enabled {
            semantics = semantics.default_action();
        }
        if let Some(annotation) = count_annotation {
            semantics = semantics.description(annotation);
        }

        // Bullet 1 (already satisfied at HEAD, re-pinned): identity is
        // derived from the label — the row's stable logical key — never its
        // index, so it follows a row across reorder/filter
        // (`enzui_core::collect::SemanticsCollector::to_tree_update_with_ids`
        // strips an anonymous node's `Click`). An explicit `.id()` wins.
        let widget_id = self
            .id
            .unwrap_or_else(|| enzui_core::identity::derive_id(&self.id_prefix, &id_key));

        // The row is activated either eagerly (a `Message` handed to
        // `enz_sidebar_row`) or lazily (a factory from
        // `on_activate_maybe_with`). The lazy factory wins when both are set,
        // matching `list_row_surface`'s own precedence. A rail built from a
        // caller's `Fn(String) -> Message` uses the lazy door so N rows do not
        // construct N messages per view pass.
        let lazy_activation: Option<Box<dyn Fn() -> Message + 'a>> = match self.on_activate_with {
            Some(f) => Some(f),
            None => self
                .on_activate
                .map(|m| Box::new(move || m.clone()) as Box<dyn Fn() -> Message + 'a>),
        };
        // Forward the id prefix so the row and its context-menu surface derive
        // distinct ids, and identically labelled rows under different prefixes
        // do not collide.
        let id_prefix = self.id_prefix.clone();
        let mut surface = crate::list_row_surface::enz_list_row_surface_lazy(content)
            .content_padding(0.0)
            .on_activate_maybe_with(lazy_activation)
            .width(Length::Fill)
            // The row owns its gutter so the fill stays inset from the pane
            // edge even when the column has no padding.
            .outer_gutter(outer_gutter)
            .row_height(fill_height)
            .outer_vertical_gap(2.0 * fill_inset_y)
            .selectable(true)
            .selected(selected)
            .enabled(enabled)
            .semantics(semantics)
            .id_prefix(id_prefix)
            .id(widget_id)
            .trailing_reveal_handle(trailing_reveal)
            // Even though SidebarRow composes its trailing child itself, pass
            // the policy into the shared surface so RowMotion can keep a
            // selected-aware reveal live after the pointer leaves.
            .trailing_visibility(trailing_visibility)
            .tokens_resolved(move |theme: &iced::Theme| {
                let t = resolve(theme);
                let mut lt = crate::theme_lookup::resolve_live(
                    theme,
                    crate::list_row_surface::ListRowSurfaceTokens::from_palette,
                );
                // Sidebar geometry: 8 px corner (vs the list-row default 10)
                // and the resolved row height.
                lt.row_radius = t.row_radius;
                lt.row_height = fill_height;
                lt.focus_ring = t.focus_ring;
                // `show_highlight == false` is the explicit opt-out: flat, with
                // no hover/selected wash. The pressed overlay is
                // intentionally left on so a press is still felt even in the
                // flat mode (bullet 4).
                if !show_highlight {
                    lt.hover_overlay = Color::TRANSPARENT;
                    lt.selected_background = t.base_background;
                } else {
                    lt.hover_overlay = t.hover_background;
                    lt.selected_background = t.selected_background;
                }
                lt
            });
        // The selected/highlighted branch already supplies a complete
        // backdrop-resolved disabled ink. Do not alpha-dim it a second time;
        // flat/inherited rows keep the historical alpha treatment; their owned
        // label/icon/count inks are explicit and are not inherited through it.
        if !enabled && selected && show_highlight {
            surface = surface.disabled_content_alpha(1.0);
        }
        if let Some(reduced) = reduced_motion {
            surface = surface.reduced_motion(reduced);
        }
        // When the row is disabled it has no activation; `ListRowSurface`
        // encodes "no on_press" as disabled, which is exactly the intent.
        if !enabled {
            surface = surface.enabled(false);
        }
        // Forward the secondary-press hook to the row chrome, which owns
        // the capture surface, the row-hugging height and the routable AT node.
        // Forwarding rather than re-wrapping is the point: an app's sidebar
        // hand-wrapped `enz_context_target` around a finished `enz_sidebar_row`
        // and had to rediscover the `Length::Fill` trap for itself.
        surface = surface.on_context_maybe(on_context);
        if let Some(label) = context_label {
            surface = surface.context_label(label);
        }
        surface.into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzSidebarRow<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzSidebarRow<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
