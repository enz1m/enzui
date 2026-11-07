//! `focus_button`: builder.
//!
//! Builder state and methods are isolated here while the widget implementation
//! lives in the sibling `widget` module.

use super::*;

type FocusStyleFn<'a, Theme> = Box<dyn Fn(&Theme, FocusStatus) -> Style + 'a>;

type UnderlineFn<'a> = Box<dyn Fn(FocusStatus, bool) -> bool + 'a>;

/// A focusable, keyboard-activatable button that wraps arbitrary content.
#[must_use]
pub struct FocusButton<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    pub(crate) content: Element<'a, Message, Theme, Renderer>,
    pub(crate) content_status: Option<ContentStatus>,
    pub(crate) on_press: Option<Activation<'a, Message>>,
    pub(crate) width: Length,
    pub(crate) height: Length,
    pub(crate) padding: Padding,
    pub(crate) background_inset_y: f32,
    pub(crate) content_align_x: Option<iced::Alignment>,
    pub(crate) content_align_y: Option<iced::Alignment>,
    pub(crate) loose_content: bool,
    pub(crate) clip: bool,
    pub(crate) id: Option<iced::advanced::widget::Id>,
    pub(crate) semantics: Option<Semantics>,
    pub(crate) style_fn: Option<FocusStyleFn<'a, Theme>>,
    pub(crate) underline_fn: Option<UnderlineFn<'a>>,
    pub(crate) underline_line_height: Option<f32>,
    pub(crate) embedded: bool,
    pub(crate) on_enter: Option<Activation<'a, Message>>,
    pub(crate) on_space: Option<Activation<'a, Message>>,
    pub(crate) focus_visible: bool,
    pub(crate) inset_focus_ring: bool,
    pub(crate) outset_focus_ring: bool,
    pub(crate) disabled_backdrop: Option<Color>,
    pub(crate) focusable_without_activation: bool,
    pub(crate) semantics_disabled: Option<bool>,
    pub(crate) expand_alias: Option<iced::advanced::widget::Id>,
    pub(crate) content_text: Option<String>,
    pub(crate) reclaim_focus_on_return: Option<iced::advanced::widget::Id>,
    pub(crate) relinquish_focus_while_open: bool,
    pub(crate) reduced_motion: Option<bool>,
}

/// Create a [`FocusButton`] wrapping `content`, activated by `on_press`
/// (pointer click or Enter/Space while focused).
pub fn focus_button<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_press: Message,
) -> FocusButton<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    focus_button_maybe(content, Some(on_press))
}

/// Build a button with optional eager activation.
/// `None` disables activation and default focus registration;
/// [`FocusButton::focusable_without_activation`] can opt into read-only focus.
pub fn focus_button_maybe<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_press: Option<Message>,
) -> FocusButton<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    FocusButton {
        content: content.into(),
        content_status: None,
        on_press: on_press.map(Activation::Eager),
        width: Length::Shrink,
        height: Length::Shrink,
        padding: Padding::new(0.0),
        background_inset_y: 0.0,
        content_align_x: None,
        content_align_y: None,
        loose_content: false,
        clip: false,
        id: None,
        semantics: None,
        style_fn: None,
        underline_fn: None,
        underline_line_height: None,
        embedded: false,
        on_enter: None,
        on_space: None,
        focus_visible: false,
        inset_focus_ring: false,
        outset_focus_ring: false,
        disabled_backdrop: None,
        focusable_without_activation: false,
        semantics_disabled: None,
        expand_alias: None,
        content_text: None,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        reduced_motion: None,
    }
}

/// Build a button with an optional event-time message factory.
/// The factory runs on completed pointer, focused keyboard or routed activation,
/// never during construction. `None` disables activation.
pub fn focus_button_maybe_with<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_press: Option<Box<dyn Fn() -> Message + 'a>>,
) -> FocusButton<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    FocusButton {
        content: content.into(),
        content_status: None,
        on_press: on_press.map(Activation::Lazy),
        width: Length::Shrink,
        height: Length::Shrink,
        padding: Padding::new(0.0),
        background_inset_y: 0.0,
        content_align_x: None,
        content_align_y: None,
        loose_content: false,
        clip: false,
        id: None,
        semantics: None,
        style_fn: None,
        underline_fn: None,
        underline_line_height: None,
        embedded: false,
        on_enter: None,
        on_space: None,
        focus_visible: false,
        inset_focus_ring: false,
        outset_focus_ring: false,
        disabled_backdrop: None,
        focusable_without_activation: false,
        semantics_disabled: None,
        expand_alias: None,
        content_text: None,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        reduced_motion: None,
    }
}

/// Build a button with an optional press-anchor callback.
/// See [`FocusButton::on_press_with_point`] for coordinates and non-pointer fallback.
/// `None` disables activation.
pub fn focus_button_maybe_with_point<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_press: Option<Box<dyn Fn(Option<iced::Point>) -> Message + 'a>>,
) -> FocusButton<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    FocusButton {
        content: content.into(),
        content_status: None,
        on_press: on_press.map(Activation::Positioned),
        width: Length::Shrink,
        height: Length::Shrink,
        padding: Padding::new(0.0),
        background_inset_y: 0.0,
        content_align_x: None,
        content_align_y: None,
        loose_content: false,
        clip: false,
        id: None,
        semantics: None,
        style_fn: None,
        underline_fn: None,
        underline_line_height: None,
        embedded: false,
        on_enter: None,
        on_space: None,
        focus_visible: false,
        inset_focus_ring: false,
        outset_focus_ring: false,
        disabled_backdrop: None,
        focusable_without_activation: false,
        semantics_disabled: None,
        expand_alias: None,
        content_text: None,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        reduced_motion: None,
    }
}

impl<'a, Message, Theme, Renderer> FocusButton<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    /// Width of the button.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Height of the button.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Inner padding.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    pub(crate) fn background_inset_y(mut self, inset: f32) -> Self {
        self.background_inset_y = enzui_core::metrics::FiniteNonNeg::new_or(inset, 0.0).get();
        self
    }

    /// Supplies live paint state to a child's style or draw callback.
    ///
    /// This does not rebuild the child or change its geometry. Use a distinct
    /// handle per button and read it during painting, rather than view building.
    pub fn content_status(mut self, status: ContentStatus) -> Self {
        self.content_status = Some(status);
        self
    }

    /// Set content alignment independently of button width.
    /// Unset by default: Shrink/Fit use Start, and other widths use Center.
    pub fn content_align_x(mut self, align: impl Into<iced::Alignment>) -> Self {
        self.content_align_x = Some(align.into());
        self
    }

    /// Set content alignment independently of button height.
    /// Unset by default: Shrink/Fit use Start, and other heights use Center.
    pub fn content_align_y(mut self, align: impl Into<iced::Alignment>) -> Self {
        self.content_align_y = Some(align.into());
        self
    }

    pub(crate) fn centered_loose_content(mut self) -> Self {
        self.loose_content = true;
        self.content_align_x = Some(iced::Alignment::Center);
        self.content_align_y = Some(iced::Alignment::Center);
        self
    }
    pub(crate) fn copy_text(mut self, text: String) -> Self {
        self.on_press = Some(Activation::Clipboard(text));
        self
    }

    /// Replace eager activation; `None` removes it and disables activation.
    /// Re-enabling requires supplying a message again; this does not retain the old one.
    pub fn on_press_maybe(mut self, on_press: Option<Message>) -> Self {
        self.on_press = on_press.map(Activation::Eager);
        self
    }

    /// Set an event-time factory for primary activation.
    /// Runs on completed pointer, focused keyboard or routed activation, never
    /// during view construction. [`Self::on_enter`] and [`Self::on_space`] may override it.
    pub fn on_press_with(mut self, on_press: impl Fn() -> Message + 'a) -> Self {
        self.on_press = Some(Activation::Lazy(Box::new(on_press)));
        self
    }

    /// Lazy activation callback that may be absent (disabled). See
    /// [`Self::on_press_with`].
    pub fn on_press_with_maybe(mut self, on_press: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_press = on_press.map(Activation::Lazy);
        self
    }

    /// Set a callback receiving the mouse chord latched at press time.
    /// Releasing modifiers before the mouse does not change the reported chord.
    /// Touch, keyboard and routed activation report empty modifiers.
    pub fn on_press_with_modifiers(
        mut self,
        on_press: impl Fn(keyboard::Modifiers) -> Message + 'a,
    ) -> Self {
        self.on_press = Some(Activation::Modified(Box::new(on_press)));
        self
    }

    /// Modifier-aware activation callback that may be absent (disabled). See
    /// [`Self::on_press_with_modifiers`].
    pub fn on_press_with_modifiers_maybe(
        mut self,
        on_press: Option<Box<dyn Fn(keyboard::Modifiers) -> Message + 'a>>,
    ) -> Self {
        self.on_press = on_press.map(Activation::Modified);
        self
    }

    /// Set a callback receiving the window-space logical-pixel press anchor.
    /// Mouse and touch retain the press point through release. Keyboard and routed
    /// activation report `None`; the caller supplies a suitable fallback anchor.
    pub fn on_press_with_point(
        mut self,
        on_press: impl Fn(Option<iced::Point>) -> Message + 'a,
    ) -> Self {
        self.on_press = Some(Activation::Positioned(Box::new(on_press)));
        self
    }

    /// Point-aware activation callback that may be absent (disabled). See
    /// [`Self::on_press_with_point`].
    pub fn on_press_with_point_maybe(
        mut self,
        on_press: Option<Box<dyn Fn(Option<iced::Point>) -> Message + 'a>>,
    ) -> Self {
        self.on_press = on_press.map(Activation::Positioned);
        self
    }

    /// Clip content to the button bounds.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Assign a stable widget id so programmatic `focus(id)` can target it.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Answer expand/collapse operations under an additional ID.
    /// The alias publishes no semantic node, joins no focus ring and does not
    /// answer ordinary activation under that ID. Primary activation must be enabled.
    /// Expansion shares the routed dispatcher, including [`Self::on_enter`] preference.
    pub fn expand_alias(mut self, alias: iced::advanced::widget::Id) -> Self {
        self.expand_alias = Some(alias);
        self
    }

    /// Set the target for opt-in keyboard focus return; defaults to `None`.
    /// Use the button's own ID and enable this only while its popup is closed.
    /// A retained, previously updated tree claims a fresh matching record for
    /// its window when keyboard modality permits; measurement trees cannot claim it.
    pub fn reclaim_focus_on_return(mut self, id: Option<iced::advanced::widget::Id>) -> Self {
        self.reclaim_focus_on_return = id;
        self
    }

    /// Release keyboard focus during reconciliation while `open` is true.
    /// Defaults to false. Pair with [`Self::reclaim_focus_on_return`] enabled only
    /// while the popup is closed, so release and reclaim do not compete.
    pub fn relinquish_focus_while(mut self, open: bool) -> Self {
        self.relinquish_focus_while_open = open;
        self
    }

    /// record the drawn text so the collector suppresses it when it
    /// differs from the accessible name (an `aria_label` override).
    pub fn content_text(mut self, text: impl Into<String>) -> Self {
        self.content_text = Some(text.into());
        self
    }

    /// Replace the visible text declaration used for accessibility label suppression.
    /// `None` clears it, meaning the caller does not know the content text.
    pub fn content_text_maybe(mut self, text: Option<String>) -> Self {
        self.content_text = text;
        self
    }

    /// Attach a descriptor published with live focus and disabled state.
    /// Primary activation adds the default-action flag; [`Self::disabled_semantics`]
    /// can override the derived disabled flag. Caller-supplied descriptor actions
    /// are retained. A routed activation also requires [`Self::id`].
    pub fn semantics(mut self, semantics: Semantics) -> Self {
        self.semantics = Some(semantics);
        self
    }

    /// Delegate focus registration, Enter/Space and pointer self-focus to a composite.
    /// Defaults to false. Completed pointer and routed activation remain available.
    /// Attached semantics use [`Self::focus_visible`] for the focused flag; the host
    /// also supplies that flag for ring visibility independently of window modality.
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Override focused Enter and routed default activation with a distinct message.
    /// Requires primary activation; it does not enable a disabled control. Embedded
    /// cells leave Enter to their owner while routed activation still uses this override.
    /// Space uses [`Self::on_space`] or primary activation independently.
    pub fn on_enter(mut self, on_enter: Message) -> Self {
        self.on_enter = Some(Activation::Eager(on_enter));
        self
    }

    /// Lazy form of [`Self::on_enter`]. Runs the factory once per real
    /// activation (Enter, AT default action) — never at view-build time —
    /// mirroring [`Self::on_press_with`].
    pub fn on_enter_with(mut self, on_enter: impl Fn() -> Message + 'a) -> Self {
        self.on_enter = Some(Activation::Lazy(Box::new(on_enter)));
        self
    }

    /// Optional lazy form of [`Self::on_enter`]: pass `None` to explicitly
    /// leave Enter bound to `on_press`. Symmetric with
    /// [`Self::on_press_with_maybe`].
    pub fn on_enter_with_maybe(mut self, on_enter: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_enter = on_enter.map(Activation::Lazy);
        self
    }

    /// Override focused Space release with a distinct message.
    /// Requires primary activation when Space is pressed and focus when released.
    /// Embedded cells leave Space to their owner. Enter and routed activation use
    /// [`Self::on_enter`] or primary activation independently.
    pub fn on_space(mut self, on_space: Message) -> Self {
        self.on_space = Some(Activation::Eager(on_space));
        self
    }

    /// Lazy form of [`Self::on_space`]. Runs the factory once per real Space
    /// release, never at view-build time; mirrors [`Self::on_press_with`].
    pub fn on_space_with(mut self, on_space: impl Fn() -> Message + 'a) -> Self {
        self.on_space = Some(Activation::Lazy(Box::new(on_space)));
        self
    }

    /// Optional lazy form of [`Self::on_space`]: `None` leaves Space bound to
    /// `on_press`. Symmetric with [`Self::on_enter_with_maybe`].
    pub fn on_space_with_maybe(mut self, on_space: Option<Box<dyn Fn() -> Message + 'a>>) -> Self {
        self.on_space = on_space.map(Activation::Lazy);
        self
    }

    /// Allow standalone focus registration without an activation message.
    /// Defaults to false. This does not install activation or change the disabled
    /// style/semantics; use [`Self::disabled_semantics`] for a read-only enabled node.
    pub fn focusable_without_activation(mut self, focusable: bool) -> Self {
        self.focusable_without_activation = focusable;
        self
    }

    /// Override the published disabled flag; unset by default.
    /// Changes accessibility advertisement without adding activation, focus
    /// registration or a routed activation carrier.
    pub fn disabled_semantics(mut self, disabled: bool) -> Self {
        self.semantics_disabled = Some(disabled);
        self
    }

    /// Supply embedded focus visibility and the published focused flag.
    /// Defaults to false and is ignored for standalone focus policy. Hover/press
    /// retain their fill status while the ring remains independently visible.
    pub fn focus_visible(mut self, focus_visible: bool) -> Self {
        self.focus_visible = focus_visible;
        self
    }

    /// Override the theme's reduced-motion policy for this button.
    ///
    /// By default a change of fill, ink, border or shadow (hover, release,
    /// a style closure that now paints the button selected) fades over the
    /// theme's `motion-fast` duration and curve; a press lands at once.
    /// `true` makes every change land at once, as the theme's
    /// `reduced_motion` does for the whole app; `false` fades even under a
    /// reduced-motion theme. A style closure that animates on its own
    /// (reading a progress cell each frame) sets `true`, so the fade does
    /// not trail it.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Style the button from a closure that receives the [`FocusStatus`],
    /// including the `Focused` variant for the focus-visible ring.
    pub fn style_focus(mut self, f: impl Fn(&Theme, FocusStatus) -> Style + 'a) -> Self {
        self.style_fn = Some(Box::new(f));
        self
    }

    /// Paint a separate inset focus ring while retaining the fill border.
    /// Defaults to false. Styled callers provide the ring in the Focused arm and
    /// selection chrome in the Active/Hovered/Pressed arms; the fill resolves
    /// Focused as Active in this mode.
    pub fn inset_focus_ring(mut self, inset: bool) -> Self {
        self.inset_focus_ring = inset;
        self
    }

    /// Use the theme's default ring painter even with a custom style closure.
    /// Defaults to false. The ring paints after content; the closure still resolves
    /// fill and foreground, including its Focused arm. Ring ink uses the policy
    /// in [`FocusRingTheme`], which does not sample ancestor paint.
    pub fn outset_focus_ring(mut self, outset: bool) -> Self {
        self.outset_focus_ring = outset;
        self
    }

    /// Resolve disabled foreground against a caller-supplied opaque backdrop.
    /// Unset by default. A solid disabled fill composites over this backdrop first;
    /// gradient or absent fill uses the supplied backdrop directly.
    pub fn disabled_on(mut self, backdrop: Color) -> Self {
        self.disabled_backdrop = Some(backdrop);
        self
    }

    /// Draw a status-dependent 1 px rule under the content's first child, or bare content.
    /// The predicate also receives actual pointer-over; pressed status alone can
    /// include dragging away or keyboard Space. Rules use resolved text ink and
    /// span the full label width; [`Self::underline_line_height`] enables multiple rows.
    pub fn underline(mut self, f: impl Fn(FocusStatus, bool) -> bool + 'a) -> Self {
        self.underline_fn = Some(Box::new(f));
        self
    }

    /// Supply the label line height in logical pixels for wrapped underline rows.
    /// Unset means one rule under the label block. Finite positive inputs replace
    /// the height; invalid inputs preserve any existing setting. Row count comes
    /// from label bounds, without per-line paragraph or ragged-width metrics.
    pub fn underline_line_height(mut self, line_height: f32) -> Self {
        if line_height.is_finite() && line_height > 0.0 {
            self.underline_line_height = Some(line_height);
        }
        self
    }

    pub(crate) fn is_focus_visible(&self, state: &State) -> bool {
        // The composite owns modality policy; do not re-gate its supplied ring flag.
        if self.embedded {
            self.focus_visible
        } else {
            enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key)
        }
    }

    pub(crate) fn status(
        &self,
        state: &State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> FocusStatus {
        if self.on_press.is_none() {
            FocusStatus::Disabled
        } else if cursor.is_over(bounds) {
            if state.is_pressed {
                FocusStatus::Pressed
            } else {
                FocusStatus::Hovered
            }
        } else if state.is_pressed {
            FocusStatus::Pressed
        } else if self.is_focus_visible(state) {
            FocusStatus::Focused
        } else {
            FocusStatus::Active
        }
    }

    /// The style the state asks for, before any fade: the focused fill
    /// paints as rest under an inset ring.
    pub(crate) fn fill_style(&self, theme: &Theme, status: FocusStatus) -> Style
    where
        Theme: Catalog<Class<'a> = StyleFn<'a, Theme>> + FocusRingTheme,
    {
        let fill_status = if self.inset_focus_ring && status == FocusStatus::Focused {
            FocusStatus::Active
        } else {
            status
        };
        self.resolve_style(theme, fill_status)
    }

    pub(crate) fn uses_default_ring(&self) -> bool {
        self.style_fn.is_none() || self.outset_focus_ring
    }

    pub(crate) fn resolve_style(&self, theme: &Theme, status: FocusStatus) -> Style
    where
        Theme: Catalog<Class<'a> = StyleFn<'a, Theme>> + FocusRingTheme,
    {
        let mut style = if let Some(f) = &self.style_fn {
            f(theme, status)
        } else {
            theme.enz_neutral_button_style(status)
        };

        if matches!(status, FocusStatus::Disabled)
            && let Some(parent) = self.disabled_backdrop
        {
            let backdrop = match style.background {
                Some(Background::Color(fill)) => {
                    crate::surface_interaction::composite(parent, fill)
                }
                _ => parent,
            };
            style.text_color = theme.enz_disabled_on(style.text_color, backdrop);
        }
        style
    }

    pub(crate) fn default_ring_border(
        &self,
        theme: &Theme,
        fill_style: &Style,
    ) -> Option<iced::Border>
    where
        Theme: Catalog<Class<'a> = StyleFn<'a, Theme>> + FocusRingTheme,
    {
        if !self.uses_default_ring() {
            return None;
        }
        let width = theme.enz_focus_ring_width();
        if !(width.is_finite() && width > 0.0) {
            return None;
        }
        let fill = match fill_style.background {
            Some(Background::Color(c)) => Some(c),
            _ => None,
        };
        Some(iced::Border {
            color: theme.enz_focus_ring_color(fill),
            width,
            radius: fill_style.border.radius,
        })
    }
}
