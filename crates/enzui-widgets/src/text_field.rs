//! EnzTextField: a styled text input composed on iced's `TextInput`, with
//! a label, helper text, error state and enzUI design tokens.
//!
//! # Inline value editors
//!
//! For a number beside a slider or an editable value in a settings row, use
//! [`quiet`](EnzTextField::quiet) to drop the resting outline,
//! [`trailing`](EnzTextField::trailing) for in-field commit and dismiss
//! actions, and [`adjustable_value`](EnzTextField::adjustable_value) when the
//! text stands for a number. A quiet [`read_only`](EnzTextField::read_only)
//! field is a plain readout.
//!
//! # Hazards
//!
//! A field in a `column![]` whose siblings appear conditionally loses its
//! retained state (focus included) to the wrong child when one is inserted,
//! because `Column` reconciles by position. Use
//! [`keyed_column`](crate::keyed_column) there.

use std::borrow::Cow;

use crate::field_chrome::{ChromePaint, ChromeStatus, FieldChrome};
use crate::tokens::FieldVariant;
use enzui_core::semantics::{NumericValue, Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::text_input::{Status, Style};
use iced::widget::{text, text_input};
use iced::{Background, Border, Element, Length, Padding};

// Slotted text fields, text areas and autocomplete editors share one accessory gap.
pub(crate) const SLOT_SPACING: f32 = 6.0;

/// Text field size presets matching the enzUI 3-step scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextFieldSize {
    /// 26px height.
    Sm,
    /// 32px height (default).
    #[default]
    Md,
    /// 40px height.
    Lg,
}

impl TextFieldSize {
    /// The size tier → [`FieldSizeTokens`](crate::tokens::FieldSizeTokens)
    /// resolver for the whole inline-control family. Fields, pickers and
    /// segmented controls asked for the same size paint the same height by
    /// construction, not because two `match` arms happen to name the same
    /// constant. Per-widget matches drifted before: a 32 px `Md`
    /// `enz_combo_box` beside a 26 px `Sm` `enz_autocomplete_field` in the
    /// calendar toolbar. `tests/rendering/inline_control_height_inventory.rs`
    /// measures every member's painted height, not its builder field.
    #[must_use]
    pub fn tokens(self) -> crate::tokens::FieldSizeTokens {
        let tokens = match self {
            Self::Sm => crate::tokens::FieldSizeTokens::SM,
            Self::Md => crate::tokens::FieldSizeTokens::MD,
            Self::Lg => crate::tokens::FieldSizeTokens::LG,
        };
        // The text and the slot holding it follow the desktop text size.
        let ui = enzui_theme::text_scale::ui_px;
        crate::tokens::FieldSizeTokens {
            height: ui(tokens.height),
            text_size: ui(tokens.text_size),
            ..tokens
        }
    }

    pub(crate) fn resolve_radius(self, theme: &iced::Theme, pin: Option<f32>) -> f32 {
        // The theme's `shape.field` step for a medium field; a small field
        // takes the step below it and a large one the step above.
        let radius = pin.unwrap_or_else(|| {
            crate::theme_lookup::shape_radius(
                theme,
                match self {
                    Self::Sm => |shape: &enzui_theme::ShapeScale| shape.field.squarer(),
                    Self::Md => |shape: &enzui_theme::ShapeScale| shape.field,
                    Self::Lg => |shape: &enzui_theme::ShapeScale| shape.field.rounder(),
                },
            )
        });
        enzui_core::metrics::FiniteNonNeg::new_or(radius, self.tokens().radius).get()
    }
}

/// Builder for an enzUI-styled text field.
///
/// Labels, placeholders and captions accept borrowed or owned text. The
/// controlled value stays borrowed with [`enz_text_field`]; use
/// [`enz_text_field_owned`] for owned values in retained subtrees.
#[must_use]
pub struct EnzTextField<'a, Message> {
    /// See [`EnzTextField::material`].
    material: Option<enzui_theme::MaterialStyle>,
    label: Cow<'a, str>,
    /// An accessibility-only name, published to the AT tree without
    /// rendering a visible label column. For fields whose visible name sits
    /// beside them (e.g. an `EnzPreferencesRow` that paints its own title
    /// column): a second visible caption would be wrong, but the field still
    /// needs a spoken name and a title-derived id. A set `label` wins, so a
    /// control never has two conflicting names; `aria_label` is consulted
    /// only when `label` is empty. The routed identity from [`effective_id`]
    /// follows the announced name, never the index.
    aria_label: Option<String>,
    placeholder: Cow<'a, str>,
    value: Cow<'a, str>,
    on_input: Box<dyn Fn(String) -> Message + 'a>,
    on_submit: Option<Message>,
    /// A submit factory run when Enter is pressed, rather than a message built
    /// on every view. [`on_submit`](Self::on_submit) wins if both are set.
    on_submit_lazy: Option<Box<dyn Fn() -> Message + 'a>>,
    size: TextFieldSize,
    /// A text size overriding the size tier's, in logical pixels before the
    /// desktop text scale.
    text_px: Option<f32>,
    radius: Option<f32>,
    error_text: Option<Cow<'a, str>>,
    description: Option<Cow<'a, str>>,
    enabled: bool,
    read_only: bool,
    secure: bool,
    purpose: iced::advanced::input_method::Purpose,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    edit_menu: bool,
    /// A numeric contract this field's text stands for, set by a control whose
    /// value can also be stepped. Changes the published role to
    /// `Role::SpinButton` and withdraws this field's own routed `SetValue`
    /// carrier, because the owner of the numeric contract owns that action.
    adjustable: Option<NumericValue>,
    /// optional in-frame affordance to the left of the text run.
    leading: Option<Element<'a, Message>>,
    /// optional in-frame affordance to the right of the text run.
    trailing: Option<Element<'a, Message>>,
    id: Option<iced::advanced::widget::Id>,
    /// Namespace for the label-derived default id. Defaults to
    /// [`DEFAULT_ID_PREFIX`].
    id_prefix: String,
    /// `None` → derive from runtime `iced::Theme` at style time.
    /// `Some(_)` → caller-pinned palette.
    palette: Option<Palette>,
    variant: FieldVariant,
    border_variant: Option<FieldVariant>,
    /// the inline value-editor presentation: no resting outline, every other
    /// painted state unchanged. Honoured only for a field that is both named
    /// and paints its own value or placeholder ([`EnzTextField::quiet`]).
    quiet: bool,
    /// Focus shows as a faint tint instead of the focus ring.
    subtle_focus: bool,
    /// The click that focuses the field selects its whole value.
    select_all_on_focus: bool,
}

/// The namespace text-field ids are derived under when no [`EnzTextField::id`]
/// or [`EnzTextField::id_prefix`] is supplied.
///
/// The key is the field's label, falling back to the placeholder — the
/// same two-step the accessible name uses, so the routable identity and the
/// announced name are derived from one string and cannot disagree.
pub const DEFAULT_ID_PREFIX: &str = "enz-text-field";

/// Creates a new enzUI-styled text field.
///
/// - `label`: Label text above the field (empty string hides label).
/// - `placeholder`: Placeholder text shown when empty.
/// - `value`: Current text value.
/// - `on_input`: Message factory for text changes.
pub fn enz_text_field<'a, Message: Clone + 'a>(
    label: impl Into<Cow<'a, str>>,
    placeholder: impl Into<Cow<'a, str>>,
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> EnzTextField<'a, Message> {
    enz_text_field_owned(label, placeholder, value, on_input)
}

/// Creates a text field whose controlled value can also be owned.
///
/// Owned strings and `'static` callbacks allow retained subtrees to return
/// `EnzTextField<'static, Message>`. String literals stay borrowed.
pub fn enz_text_field_owned<'a, Message: Clone + 'a>(
    label: impl Into<Cow<'a, str>>,
    placeholder: impl Into<Cow<'a, str>>,
    value: impl Into<Cow<'a, str>>,
    on_input: impl Fn(String) -> Message + 'a,
) -> EnzTextField<'a, Message> {
    EnzTextField {
        material: None,
        label: label.into(),
        aria_label: None,
        placeholder: placeholder.into(),
        value: value.into(),
        on_input: Box::new(on_input),
        on_submit: None,
        on_submit_lazy: None,
        size: TextFieldSize::Md,
        text_px: None,
        radius: None,
        error_text: None,
        description: None,
        enabled: true,
        read_only: false,
        secure: false,
        purpose: iced::advanced::input_method::Purpose::Normal,
        direction: None,
        edit_menu: true,
        adjustable: None,
        leading: None,
        trailing: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        palette: None,
        variant: FieldVariant::default(),
        border_variant: None,
        quiet: false,
        subtle_focus: false,
        select_all_on_focus: false,
    }
}

/// Produces an `iced::advanced::widget::Operation` that selects
/// Produces an `iced::advanced::widget::Operation` that selects
/// all content in the [`EnzTextField`] identified by `id`. A thin wrapper
/// over `operation::text_input::select_all`: the inner `text_input` carries
/// the same `Id`, and the operation traverses the `TextFieldSemantics`
/// wrapper to reach its state.
///
/// # Disabled fields
///
/// A disabled field has no live `text_input`, so the operation matches
/// nothing and does nothing: a disabled field is fully inert.
pub fn select_all<T>(id: iced::advanced::widget::Id) -> impl iced::advanced::widget::Operation<T> {
    iced::advanced::widget::operation::text_input::select_all(id)
}

impl<'a, Message: Clone + 'a> EnzTextField<'a, Message> {
    /// Sets the size variant.
    pub fn size(mut self, size: TextFieldSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the text size in logical pixels, independently of the size tier
    /// (a large numeric readout). It follows the desktop text scale like the
    /// tier's own size, and the field grows taller to keep the tier's
    /// vertical padding around the larger line; it never shrinks below the
    /// tier's height. Non-finite or non-positive sizes are ignored.
    pub fn text_size(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.text_px = Some(px);
        }
        self
    }

    /// The size tokens this field lays out with: the tier's, with an
    /// independent text size and the height that fits it.
    fn size_tokens(&self) -> crate::tokens::FieldSizeTokens {
        let tier = self.size.tokens();
        let Some(px) = self.text_px else {
            return tier;
        };
        let text_size = enzui_theme::text_scale::ui_px(px);
        let height = tier
            .height
            .max((1.3 * text_size + 2.0 * tier.padding_y()).ceil());
        crate::tokens::FieldSizeTokens {
            height,
            text_size,
            ..tier
        }
    }

    /// Pin the corner radius in logical pixels, overriding the live theme.
    ///
    /// Without a pin, the theme's `shape.field` step sets it (by default
    /// Sm/Md/Lg use `radius.sm`/`md`/`lg`), including when colours are
    /// pinned with [`Self::palette`]. Negative or non-finite pins
    /// become zero (square corners).
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }

    /// Sets the fill treatment. Defaults to [`FieldVariant::Filled`].
    ///
    /// `Filled` gives the field a body (`surface_alt`) distinct from the pane
    /// behind it; `Outlined` restores the compatibility page-coloured fill
    /// (`surface`) for dense/inline placements. Filled uses field-border roles;
    /// Outlined uses stronger outline roles unless [`Self::border_variant`]
    /// selects an independent border treatment.
    pub fn variant(mut self, variant: FieldVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Select border roles independently of the fill; unset follows [`Self::variant`].
    ///
    /// Filled uses `field_border` / `field_border_strong`; Outlined uses
    /// `border_strong` / `control_border`. Focus and error borders keep precedence.
    pub fn border_variant(mut self, variant: FieldVariant) -> Self {
        self.border_variant = Some(variant);
        self
    }

    /// Focus as a faint tint of the text colour over the field, with no focus
    /// ring: chrome that is always one click from editing, such as a browser
    /// address bar. An error still shows its edge.
    pub fn subtle_focus(mut self, subtle: bool) -> Self {
        self.subtle_focus = subtle;
        self
    }

    /// Whether the click that focuses the field selects its whole value (an
    /// address bar), instead of placing the caret; later clicks and drags
    /// edit as usual. Keyboard focus is unchanged.
    pub fn select_all_on_focus(mut self, select_all: bool) -> Self {
        self.select_all_on_focus = select_all;
        self
    }

    /// Omits the resting outline, for an inline value editor whose visible
    /// value identifies the control (a number beside a slider, an editable
    /// value cell in a settings row). Hover, focus and error borders still
    /// paint, and editing, accessibility and the [`variant`](Self::variant)
    /// fill are unchanged.
    ///
    /// It applies only when the field has an accessible name (a label or
    /// [`aria_label`](Self::aria_label)) and paints its own text (a value, or
    /// a placeholder when empty); otherwise the outline stays, as WCAG 1.4.11
    /// requires. Leading and trailing slots do not count as identifying text.
    ///
    /// Pair it with [`trailing`](Self::trailing) commit and dismiss actions,
    /// and [`adjustable_value`](Self::adjustable_value) for numbers.
    pub fn quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    // The `quiet` opt-in plus the identification it requires; every
    // resting-edge decision reads this, never the flag.
    fn omits_resting_outline(&self) -> bool {
        if !self.quiet {
            return false;
        }
        let named = !self.label.is_empty()
            || self
                .aria_label
                .as_deref()
                .is_some_and(|name| !name.is_empty());
        // Text the field itself paints, which is what WCAG's boundary waiver
        // turns on. A caption above the box is naming, not content; a slot
        // identifies its own affordance, so neither qualifies an empty input.
        let shows_its_own_text = !self.value.is_empty() || !self.placeholder.is_empty();
        named && shows_its_own_text
    }

    /// Sets the error text (non-empty enables error state).
    ///
    /// Accepts anything convertible to [`Cow<'a, str>`]: a `&'a str`
    /// (borrowed, unchanged for existing callers) or an owned `String` for the
    /// owned-value path.
    pub fn error(mut self, error: impl Into<Cow<'a, str>>) -> Self {
        let error = error.into();
        self.error_text = if error.is_empty() { None } else { Some(error) };
        self
    }

    /// Sets the description/helper text.
    ///
    /// Accepts anything convertible to [`Cow<'a, str>`]: a `&'a str`
    /// (borrowed, unchanged for existing callers) or an owned `String`.
    pub fn description(mut self, desc: impl Into<Cow<'a, str>>) -> Self {
        let desc = desc.into();
        self.description = if desc.is_empty() { None } else { Some(desc) };
        self
    }

    /// Sets whether the field is enabled.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Pin the whole [`Palette`], bypassing runtime-theme derivation.
    ///
    /// # This is a GLOBAL role override, not a per-widget styling knob
    ///
    /// It is not per-field styling: it replaces whole colour *roles*
    /// (`surface`, `field_border`, `accent`, …) that dozens of other widgets
    /// read. Even the field-scoped `field_border` spans all eight field-family
    /// widgets, so retuning it here retunes every composed control and any
    /// child the palette is forwarded to (slot buttons, popups). The blast
    /// radius is the whole subtree. Use it to pin a known theme (render tests
    /// with exact RGB expectations), not to make one field look different.
    /// For field fill treatments use [`variant`](Self::variant)
    /// ([`FieldVariant`]), a real per-widget knob that does not leak.
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Sets the message to emit on Enter/Return.
    pub fn on_submit(mut self, message: Message) -> Self {
        self.on_submit = Some(message);
        self
    }

    /// A submit factory run only when Enter is pressed in an editable field,
    /// so a form does not rebuild its submit message on every keystroke.
    /// [`on_submit`](Self::on_submit) wins if both are set.
    pub fn on_submit_with(mut self, on_submit: impl Fn() -> Message + 'a) -> Self {
        self.on_submit_lazy = Some(Box::new(on_submit));
        self
    }

    /// Publishes this field as an adjustable spin button carrying `numeric`
    /// beside its text, for a control whose value an assistive technology can
    /// also step (`enz_time_field`).
    ///
    /// One node either way: the field stays a single editable focus target and
    /// gains a numeric contract, rather than growing a second one. The field's
    /// own routed `SetValue` carrier is withdrawn, because a caller that
    /// declares a numeric contract owns how a routed set is interpreted.
    pub fn adjustable_value(mut self, numeric: NumericValue) -> Self {
        self.adjustable = Some(numeric);
        self
    }

    /// Sets secure (password) mode.
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    /// Override the writing direction. By default the field follows the
    /// locale: right-to-left aligns the text to the right and swaps the
    /// leading and trailing slots.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Tells the input method what the text is for, so an on-screen keyboard
    /// can show a number pad, an `@` key for email, and so on. A secure field
    /// always reports `Secure`.
    pub fn input_purpose(mut self, purpose: iced::advanced::input_method::Purpose) -> Self {
        self.purpose = purpose;
        self
    }

    /// Places an arbitrary element in the field's leading (left) slot,
    /// inside the field border, vertically centred against the text run.
    ///
    /// # Why `Element` and not an icon name/enum
    ///
    /// Slots serve decorative glyphs (search magnifiers) and interactive
    /// message-emitting affordances (clear-×/reveal toggles): icon-typed slots
    /// express only firsts sans parallel clickable/sending/called axes.
    /// `Element`s express both (`enz_icon(name).into()` decorative
    /// compositions).
    ///
    /// # Accessibility rule for slot content
    ///
    /// Slots publish exactly element-published — fields add nothing.
    /// Decorative (`enz_icon(...)`) `svg`s implement zero `operate` hooks:
    /// NO AT nodes (label-value-between announced "image" noise avoided; label-
    /// unduplicated glyph information). Interactive
    /// ([`focus_button`](crate::focus_button::focus_button) clear-×/reveal toggles) publish own
    /// labelled `Role::Button` nodes in `operate()`: reachable + named.
    /// Element-CARRIED (never field-configured) distinctions: interactive slots
    /// demand semantics-publishing builders (`focus_button`), never mouse-
    /// handler-wrapped bare `svg`s (silent AT-invisible controls). Slots shrink
    /// input widths, never heights: full 26/32/40px hit regions kept
    /// (commit-`2709187`-established, `input_fills_slot_height_with_a_leading_icon`-
    /// pinned).
    pub fn leading(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.leading = Some(element.into());
        self
    }

    /// Places an arbitrary element in the field's trailing (right) slot.
    /// Same rules as [`Self::leading`] — see there for why the slot is an
    /// `Element` and how AT presence is decided.
    pub fn trailing(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(element.into());
        self
    }

    /// Read-only: a live, focusable input that refuses edits. The value can be
    /// focused, selected and copied (Ctrl+A, Ctrl+C) and is not dimmed; typing,
    /// Backspace, Delete, Ctrl+X, Ctrl+V and Enter do nothing. Compare
    /// [`enabled`](Self::enabled)`(false)`, which renders static, unfocusable,
    /// dimmed text.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Whether a right click (or the Menu key) opens the built-in edit menu:
    /// Undo, Redo, Cut, Copy, Paste, Delete and Select All, or Copy and
    /// Select All when read-only. On by default; turn it off when the
    /// application supplies its own context menu.
    pub fn edit_menu(mut self, enabled: bool) -> Self {
        self.edit_menu = enabled;
        self
    }

    /// Assign a stable `iced::advanced::widget::Id` for programmatic focus
    /// (`text_input::focus(id)`) and for AT action routing. Label-derived-
    /// default-winning explicits: same-label two-field views (otherwise twice-
    /// derived ids). Label (or placeholder)-derived defaults, never index:
    /// reordered-form-shifting index ids route `SetValue`s into moved-in slots
    /// (strictly worse than idless).
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Namespace the label-derived default id.
    ///
    /// Two panels each offering a "Name" field on one screen would derive the
    /// same id; a distinct prefix per panel keeps both individually routable.
    /// Ignored when [`Self::id`] is set.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// An accessible name published without a visible label, for a field
    /// labelled by its surroundings (such as an
    /// [`EnzPreferencesRow`](crate::preferences_row::EnzPreferencesRow)'s
    /// title). A visible label wins; with neither, the placeholder is used.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// The field's routable identity: an explicit [`Self::id`], else one derived
    /// from the label, else from [`Self::aria_label`], else from the
    /// placeholder.
    ///
    /// The key is the accessible name, so id and name cannot disagree. It is
    /// stable across frames, which matters because assistive technology sends
    /// requests against the previous frame's tree; the value is never used,
    /// since it changes with every keystroke. Without a label or placeholder
    /// the field is anonymous and advertises no `SetValue`.
    fn effective_id(&self) -> Option<iced::advanced::widget::Id> {
        self.id.clone().or_else(|| {
            // The identity key must be exactly the string [`Self::semantics`]
            // uses for the accessible name, so the routable id and the
            // announced name track one another: visible label wins;
            // otherwise the aria-only name; otherwise the placeholder as last
            // resort. Anonymous field iff all three are empty.
            let key: String = if !self.label.is_empty() {
                self.label.to_string()
            } else if let Some(aria) = self.aria_label.as_deref()
                && !aria.is_empty()
            {
                aria.to_owned()
            } else if !self.placeholder.is_empty() {
                self.placeholder.to_string()
            } else {
                return None;
            };
            Some(enzui_core::identity::derive_id(&self.id_prefix, &key))
        })
    }

    /// Converts this builder into an iced `Element`. Palette follows the
    /// runtime `iced::Theme` unless `.palette()` was called.
    /// The accessibility semantics this field publishes into the AT tree
    /// a TextInput (or PasswordInput when secure) node with
    /// the label, current value (omitted when secure), disabled state, and the
    /// error/description text as the node description.
    fn semantics(&self) -> Semantics {
        let role = if self.secure {
            SemRole::PasswordInput
        } else if self.adjustable.is_some() {
            SemRole::SpinButton
        } else {
            SemRole::TextInput
        };
        // Text fields are focused and edited, not clicked, so they advertise
        // no default action. Editability is declared unconditionally; read-only
        // and disabled withdraw it when the node is built.
        let mut s = Semantics::new(role)
            .disabled(!self.enabled)
            .read_only(self.read_only)
            .editable(true)
            .invalid(self.error_text.is_some());
        // Accessible name: the visual label, else the caller-supplied
        // `aria_label` (accessibility-only, no visible column), else
        // the placeholder as a last resort (a labelless field still
        // needs a name for AT; the placeholder is the best available text,
        // matching how a search field names itself).
        if !self.label.is_empty() {
            s = s.label(self.label.to_string());
        } else if let Some(aria) = self.aria_label.as_deref()
            && !aria.is_empty()
        {
            s = s.label(aria.to_owned());
        } else if !self.placeholder.is_empty() {
            s = s.label(self.placeholder.to_string());
        }
        if !self.value.is_empty() && !self.secure {
            s = s.value(self.value.to_string());
        }
        if let Some(err) = &self.error_text {
            s = s.description(err.to_string());
        } else if let Some(desc) = &self.description {
            s = s.description(desc.to_string());
        }
        if let Some(numeric) = self.adjustable {
            s = s.adjustable(true).numeric(numeric);
        }
        s
    }

    /// Put the field on liquid glass of `style` (see [`crate::glass`]).
    /// Controls inside default to glass.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let material = self.material;
        let (size, pin) = (self.size, self.radius);
        crate::glass::wrap_resolved(self.build_element(), material, move |theme| {
            size.resolve_radius(theme, pin)
        })
    }

    fn build_element(self) -> Element<'a, Message> {
        let size_tokens = self.size_tokens();
        let size = self.size;
        let radius_pin = self.radius;
        let paint_tokens = move |theme: &iced::Theme| crate::tokens::FieldSizeTokens {
            radius: size.resolve_radius(theme, radius_pin),
            ..size_tokens
        };
        let has_error = self.error_text.is_some();
        let variant = self.variant;
        let border_variant = self.border_variant.unwrap_or(variant);
        let quiet_resting = self.omits_resting_outline();
        let subtle_focus = self.subtle_focus;
        let select_all_on_focus = self.select_all_on_focus;

        // Only the input element (not the label or helper) is wrapped in the
        // TextInput (or secure PasswordInput) semantics node: label, value
        // and disabled state, with bounds matching the input. The node
        // carries the field's own [`Id`], so AT `SetValue`/`Focus` resolve
        // through `NodeIdMap` and AT focus matches iced's live focus.
        // Computed before `self` is partially moved.
        let field_semantics = self.semantics();
        // A field with a stable id names its error line, so a screen reader
        // reads the error with the invalid field.
        let error_id = self
            .effective_id()
            .filter(|_| self.error_text.is_some())
            .map(|id| enzui_core::identity::part_id(&id, "error"));
        let field_semantics = match &error_id {
            Some(id) => field_semantics.error_message(id.clone()),
            None => field_semantics,
        };

        // The wrapper carries the same id as the inner `text_input`, so a
        // routed `SetValue` reaches `on_input`. A minted id for an anonymous
        // field is not published as a routing target. When an adjustable
        // value owns the numeric contract, it answers routed sets instead.
        let owns_routed_set = self.adjustable.is_none();
        let user_id = self.effective_id();
        let probe_id = user_id
            .clone()
            .unwrap_or_else(iced::advanced::widget::Id::unique);

        // the commit line is needed in two places now -- the
        // `text_input` (typing) and the `TextFieldSemantics` carrier (a routed
        // `SetValue`) -- and it must be the same closure in both, or the two
        // paths could diverge. An `Rc` is what lets one `Box<dyn Fn>` reach both;
        // nothing here is `Clone`.
        let on_input: std::rc::Rc<dyn Fn(String) -> Message + 'a> =
            std::rc::Rc::from(self.on_input);

        let pinned_palette = self.palette;

        // Helper: pick between caller-pinned palette and theme-derived.
        let pick = move |theme: &iced::Theme, pinned: Option<Palette>| -> Palette {
            pinned.unwrap_or_else(|| crate::theme_lookup::palette(theme))
        };

        // Label — 12 px, medium weight, text-muted colour. Optional: an empty
        // label string omits the slot. It is composed into the keyed stack
        // below under a stable `"label"` key so that toggling it empty↔non-empty
        // never shifts the stateful input's retained state.
        let label_el: Option<Element<'a, Message>> = if !self.label.is_empty() {
            let label_pinned = pinned_palette.clone();
            Some(
                text(self.label)
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(pick(theme, label_pinned.clone()).text_muted),
                    })
                    .font(enzui_theme::text_defaults::font(
                        enzui_theme::FontRole::Ui,
                        enzui_theme::WeightRung::Medium,
                    ))
                    .into(),
            )
        } else {
            None
        };

        // Vertical padding sized so the input's bounds fill the height slot
        // (full-height chrome plus hit region).
        // An inline field's text starts where the label it replaces did.
        let pad_x = if variant == FieldVariant::Inline {
            0.0
        } else {
            size_tokens.padding_x
        };
        let pad = Padding::from([size_tokens.padding_y(), pad_x]);
        // Two flags, not one `interactive`, so read-only does not become
        // static: `renders_input` is false only when disabled (a read-only
        // field stays focusable, selectable and copyable), and `editable`
        // decides whether `on_input`/`on_submit` are wired, which is how iced
        // expresses read-only.
        let renders_input = self.enabled;
        let editable = self.enabled && !self.read_only;
        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;
        let text_align = if rtl {
            iced::alignment::Horizontal::Right
        } else {
            iced::alignment::Horizontal::Left
        };
        // an event-time submit factory, if the caller opted in. Extracted
        // once here because it must reach whichever editable path runs (slotted
        // chrome or plain), and only there. When present it wins over the eager
        // `on_submit`, and the wrapped `text_input` is built with no eager
        // submit at all — the `TextFieldSemantics` wrapper produces the message
        // at the moment Enter is pressed instead.
        let on_submit_lazy = self.on_submit_lazy;
        let has_slots = self.leading.is_some() || self.trailing.is_some();
        // A read-only input always reports `Status::Disabled` to its own style
        // closure (iced derives that status from `on_input.is_none()`,
        // in iced's `text_input`), so it can never paint its own hover border or
        // focus ring. `FieldChrome` resolves both from the live widget state
        // instead, so the read-only path borrows the slotted path's wrapper.
        // An editable secure field takes the chrome path too: its Caps Lock
        // warning sits inside the frame, beside the input.
        let use_chrome = has_slots || (renders_input && !editable) || (self.secure && editable);

        // The one stateful child — the live input row (or its inert display).
        // Whichever branch runs, it is keyed `"field"` in the stack below, so
        // its focus/cursor/selection follow the key when a label or caption
        // appears or disappears.
        let field_el: Element<'a, Message> = if use_chrome {
            // With slots, the chrome moves out of `text_input` onto a
            // `FieldChrome` wrapper, so in-frame icons are not the input's
            // children. `FieldChrome` owns `draw`, which keeps the hover and
            // focus borders working; a container style closure only gets
            // `&Theme` and would silently drop both. The chrome has a fixed
            // height and owns `padding_x`; the input keeps `padding_y` with
            // zero horizontal padding, so `padding_y * 2 + 1.3 * text_size ==
            // height` still holds and only the input's width shrinks, keeping
            // the full-height hit region.
            let dimmed = !self.enabled;
            let middle: Element<'a, Message> = if renders_input {
                let mut input = text_input(self.placeholder.as_ref(), self.value.as_ref())
                    .padding(Padding::from([size_tokens.padding_y(), 0.0]))
                    .size(size_tokens.text_size)
                    .width(Length::Fill)
                    .id(probe_id.clone())
                    .purpose(self.purpose)
                    .align_x(text_align)
                    .select_all_on_focus(select_all_on_focus);
                if self.secure {
                    input = input.secure(true);
                }
                // Read-only == no `on_input` and no `on_submit`. See
                // [`Self::read_only`] for why that is the iced-native spelling.
                if editable {
                    let typed = std::rc::Rc::clone(&on_input);
                    input = input.on_input(move |s| typed(s));
                    // the eager `on_submit` is withheld whenever a lazy
                    // factory is present — the wrapper produces the submit
                    // message at event time in that case, so compatibility baking one here
                    // would both double-fire and defeat the deferral.
                    if on_submit_lazy.is_none()
                        && let Some(msg) = self.on_submit
                    {
                        input = input.on_submit(msg);
                    }
                }
                let style_pinned = pinned_palette.clone();
                input = input.style(move |theme: &iced::Theme, status: Status| {
                    let palette = pick(theme, style_pinned.clone());
                    let size_tokens = paint_tokens(theme);
                    // The wrapper paints the surface/border; strip them here so
                    // the field does not draw two nested borders.
                    Style {
                        background: Background::Color(iced::Color::TRANSPARENT),
                        border: Border::default(),
                        ..text_field_style_with(
                            subtle_focus,
                            &palette,
                            status,
                            has_error,
                            &size_tokens,
                            variant,
                            border_variant,
                            quiet_resting,
                        )
                    }
                });
                // A read-only input reports `Status::Disabled` to the closure
                // above, but `text_field_style` only varies background/border
                // by status -- both stripped here -- so the value/placeholder
                // colours stay the undimmed read-only ones.
                let input: Element<'a, Message> = input.into();
                let input = if self.edit_menu {
                    crate::edit_menu::edit_menu(
                        input,
                        crate::edit_menu::EditMenuOptions {
                            editable,
                            secure: self.secure,
                        },
                    )
                } else {
                    input
                };
                if self.secure && editable {
                    crate::caps_lock_hint::caps_lock_hint(input)
                } else {
                    input
                }
            } else {
                // Inert display (disabled only, now): static text, no
                // focusable input — same rule as the unslotted path below.
                let shown = if self.value.is_empty() {
                    self.placeholder.to_string()
                } else if self.secure {
                    "•".repeat(self.value.chars().count())
                } else {
                    self.value.to_string()
                };
                let is_placeholder = self.value.is_empty();
                let disp_pinned = pinned_palette.clone();
                text(shown)
                    .size(size_tokens.text_size)
                    .width(Length::Fill)
                    .style(move |theme: &iced::Theme| {
                        let p = pick(theme, disp_pinned.clone());
                        iced::widget::text::Style {
                            color: Some(if dimmed {
                                // Disabled: use the audited disabled-text
                                // ink for muted channels, alpha-dim for text.
                                if is_placeholder {
                                    crate::color::disabled_text_ink(&p, p.surface_alt)
                                } else {
                                    crate::color::with_opacity(
                                        p.text,
                                        crate::theme_lookup::opacity(theme).disabled(),
                                    )
                                }
                            } else if is_placeholder {
                                p.text_muted
                            } else {
                                p.text
                            }),
                        }
                    })
                    .into()
            };

            let mut row = iced::widget::Row::new()
                .mirrored(rtl)
                .spacing(SLOT_SPACING)
                .align_y(iced::alignment::Vertical::Center)
                .width(Length::Fill)
                .height(Length::Fill);
            if let Some(leading) = self.leading {
                row = row.push(leading);
            }
            row = row.push(middle);
            if let Some(trailing) = self.trailing {
                row = row.push(trailing);
            }

            let chrome_pinned = pinned_palette.clone();
            let chrome_input: Element<'a, Message> = FieldChrome::new(
                row,
                size_tokens.height,
                Padding::from([0.0, size_tokens.padding_x]),
                move |theme: &iced::Theme, status: ChromeStatus| {
                    let p = pick(theme, chrome_pinned.clone());
                    let size_tokens = paint_tokens(theme);
                    let mapped = match status {
                        ChromeStatus::Active => Status::Active,
                        ChromeStatus::Hovered => Status::Hovered,
                        ChromeStatus::Focused => Status::Focused { is_hovered: false },
                        ChromeStatus::Inert if dimmed => Status::Disabled,
                        ChromeStatus::Inert => Status::Active,
                    };
                    let s = text_field_style_with(
                        subtle_focus,
                        &p,
                        mapped,
                        has_error,
                        &size_tokens,
                        variant,
                        border_variant,
                        quiet_resting,
                    );
                    ChromePaint {
                        background: match s.background {
                            Background::Color(c) => c,
                            _ => p.surface,
                        },
                        border_color: s.border.color,
                        border_width: s.border.width,
                        radius: size_tokens.radius,
                    }
                },
            )
            .interactive(renders_input)
            .into();
            // service a routed `SetValue` through the same commit line
            // typing uses. `TextFieldSemantics` re-checks `editable` itself (via
            // the semantics it was handed), so a read-only slotted field is
            // handed the closure and still offers no carrier.
            let routed = std::rc::Rc::clone(&on_input);
            // only an editable field submits; a read-only/disabled slotted
            // field is handed no factory, matching the eager-submit gate above
            // and `FormField::enter_submits`.
            let lazy_submit = editable.then_some(on_submit_lazy).flatten();
            crate::text_field_semantics::TextFieldSemantics::new(
                chrome_input,
                probe_id,
                user_id,
                field_semantics,
            )
            .on_input_when(owns_routed_set, move |s| routed(s))
            .on_submit_with_maybe(lazy_submit)
            .into()
        } else if editable {
            // Editable path: a real, focusable TextInput.
            let mut input = text_input(self.placeholder.as_ref(), self.value.as_ref())
                .padding(pad)
                .size(size_tokens.text_size)
                .width(Length::Fill)
                .id(probe_id.clone())
                .purpose(self.purpose)
                .align_x(text_align)
                .select_all_on_focus(select_all_on_focus);
            if self.secure {
                input = input.secure(true);
            }
            let typed = std::rc::Rc::clone(&on_input);
            input = input.on_input(move |s| typed(s));
            // withhold the eager `on_submit` when a lazy factory is set;
            // the wrapper below fires the submit at event time instead.
            if on_submit_lazy.is_none()
                && let Some(msg) = self.on_submit
            {
                input = input.on_submit(msg);
            }
            let style_pinned = pinned_palette.clone();
            input = input.style(move |theme: &iced::Theme, status: Status| {
                let palette = pick(theme, style_pinned.clone());
                let size_tokens = paint_tokens(theme);
                text_field_style_with(
                    subtle_focus,
                    &palette,
                    status,
                    has_error,
                    &size_tokens,
                    variant,
                    border_variant,
                    quiet_resting,
                )
            });
            let editable_input: Element<'a, Message> = if self.edit_menu {
                crate::edit_menu::edit_menu(
                    input.into(),
                    crate::edit_menu::EditMenuOptions {
                        editable: true,
                        secure: self.secure,
                    },
                )
            } else {
                input.into()
            };
            // the routed-`SetValue` carrier, committing through the same
            // `on_input` a keystroke commits through.
            let routed = std::rc::Rc::clone(&on_input);
            crate::text_field_semantics::TextFieldSemantics::new(
                editable_input,
                probe_id,
                user_id,
                field_semantics,
            )
            .on_input_when(owns_routed_set, move |s| routed(s))
            .on_submit_with_maybe(on_submit_lazy)
            .into()
        } else {
            // Disabled path: a NON-interactive display. Rendering the
            // value/placeholder as static text inside a styled container means
            // a *disabled* field cannot pointer-focus, select, or copy — the
            // stock `TextInput` still does all three when merely given no
            // `on_input`, which is precisely why read-only takes the
            // chrome path above and disabled does not.
            let disabled = !self.enabled;
            let shown = if self.value.is_empty() {
                self.placeholder.to_string()
            } else if self.secure {
                "•".repeat(self.value.chars().count())
            } else {
                self.value.to_string()
            };
            let is_placeholder = self.value.is_empty();
            let disp_pinned = pinned_palette.clone();
            let display =
                text(shown)
                    .size(size_tokens.text_size)
                    .style(move |theme: &iced::Theme| {
                        let p = pick(theme, disp_pinned.clone());
                        iced::widget::text::Style {
                            color: Some(if disabled {
                                if is_placeholder {
                                    crate::color::disabled_text_ink(&p, p.surface_alt)
                                } else {
                                    crate::color::with_opacity(
                                        p.text,
                                        crate::theme_lookup::opacity(theme).disabled(),
                                    )
                                }
                            } else if is_placeholder {
                                p.text_muted
                            } else {
                                p.text
                            }),
                        }
                    });
            let box_pinned = pinned_palette.clone();
            let disabled_input: Element<'a, Message> = iced::widget::container(display)
                .width(Length::Fill)
                .height(Length::Fixed(size_tokens.height))
                .padding(Padding::from([0.0, size_tokens.padding_x]))
                .align_y(iced::alignment::Vertical::Center)
                .style(move |theme: &iced::Theme| {
                    let p = pick(theme, box_pinned.clone());
                    let size_tokens = paint_tokens(theme);
                    let border_color = if has_error {
                        p.error
                    } else {
                        crate::tokens::field_edge(&p, border_variant, false)
                    };
                    let border_width = if quiet_resting && !has_error {
                        0.0
                    } else {
                        1.0
                    };
                    iced::widget::container::Style {
                        background: Some(Background::Color(crate::tokens::field_fill_for(
                            &p, variant,
                        ))),
                        border: Border {
                            color: border_color,
                            width: border_width,
                            radius: size_tokens.radius.into(),
                        },
                        ..Default::default()
                    }
                })
                .into();
            // Live-`text_input`-less branches make focus probes always-false-
            // reading `focused`s: correct (inert unfocusable disabled fields;
            // `text_field_interaction`-matched `Focusable`/`Tab` counts).
            // Deliberate here-absent `.on_input(..)`s: `SetValue`-unadvertising
            // disabled nodes (`to_accesskit_node` withdrawals) must service
            // none. Own-`editable`-gate-refusing `TextFieldSemantics` anyway:
            // closure-withholding makes two agree BY CONSTRUCTION (never one-
            // guard).
            crate::text_field_semantics::TextFieldSemantics::new(
                disabled_input,
                probe_id,
                user_id,
                field_semantics,
            )
            .into()
        };

        // Error text (supersedes description). Both use the theme-derived
        // palette so a light-mode swap re-tints the caption too. Optional: the
        // trailing caption is composed under a stable `"caption"` key so it can
        // appear, disappear, or change error↔description type without moving
        // the field's retained state.
        let caption_el: Option<Element<'a, Message>> = if let Some(err) = self.error_text {
            let err_pinned = pinned_palette;
            let message: Element<'a, Message> = text(err.clone())
                .size(enzui_theme::text_scale::ui_px(12.0))
                .style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(pick(theme, err_pinned.clone()).error),
                })
                .into();
            // The field names this line as its error message.
            Some(match &error_id {
                Some(id) => crate::semantics_container::SemanticsContainer::leaf(
                    message,
                    Semantics::new(SemRole::Label).label(err.into_owned()),
                )
                .id(id.clone())
                .into(),
                None => message,
            })
        } else if let Some(desc) = self.description {
            let desc_pinned = pinned_palette;
            Some(
                text(desc)
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(pick(theme, desc_pinned.clone()).text_muted),
                    })
                    .into(),
            )
        } else {
            None
        };

        // Semantics are published on the input row, not the whole column, and
        // operations descend into it so slot controls stay reachable. The
        // column is keyed, so toggling the label or caption keeps the input's
        // focus, caret and selection.
        crate::field_column::FieldColumn::new(4.0)
            .width(Length::Fill)
            .slot("label", label_el)
            .field("field", field_el)
            .slot("caption", caption_el)
            .into_element()
    }
}

impl<'a, Message: Clone + 'a> From<EnzTextField<'a, Message>> for Element<'a, Message> {
    fn from(field: EnzTextField<'a, Message>) -> Self {
        field.into_element()
    }
}

/// [`text_field_style`], with focus shown as a faint text-colour tint and no
/// ring when `subtle_focus` (an error keeps its edge).
#[allow(
    clippy::too_many_arguments,
    reason = "the style inputs of text_field_style plus one"
)]
pub(crate) fn text_field_style_with(
    subtle_focus: bool,
    palette: &Palette,
    status: Status,
    has_error: bool,
    size_tokens: &crate::tokens::FieldSizeTokens,
    variant: FieldVariant,
    border_variant: FieldVariant,
    quiet_resting: bool,
) -> Style {
    let mut style = text_field_style(
        palette,
        status,
        has_error,
        size_tokens,
        variant,
        border_variant,
        quiet_resting,
    );
    if subtle_focus && !has_error && matches!(status, Status::Focused { .. }) {
        let fill = crate::tokens::field_fill_for(palette, variant);
        let ink = palette.text;
        let mix = |a: f32, b: f32| a + (b - a) * 0.035;
        style.background = Background::Color(iced::Color {
            r: mix(fill.r, ink.r),
            g: mix(fill.g, ink.g),
            b: mix(fill.b, ink.b),
            a: fill.a,
        });
        style.border.width = 0.0;
    }
    style
}

pub(crate) fn text_field_style(
    palette: &Palette,
    status: Status,
    has_error: bool,
    size_tokens: &crate::tokens::FieldSizeTokens,
    variant: FieldVariant,
    border_variant: FieldVariant,
    quiet_resting: bool,
) -> Style {
    let border_color = if has_error {
        palette.error
    } else {
        match status {
            Status::Active => crate::tokens::field_edge(palette, border_variant, false),
            Status::Hovered => crate::tokens::field_edge(palette, border_variant, true),
            Status::Focused { .. } => palette.focus_ring,
            Status::Disabled => crate::tokens::field_edge(palette, border_variant, false),
        }
    };

    let border_width = match status {
        // An inline field has no box to ring: the caret marks the focus.
        _ if border_variant == FieldVariant::Inline && !has_error => 0.0,
        Status::Focused { .. } => enzui_theme::text_defaults::focus_ring_width(),
        // Both resting statuses: a `text_input` that has not yet recorded one
        // styles through `Disabled`, so an enabled resting field lands there.
        Status::Active | Status::Disabled if quiet_resting && !has_error => 0.0,
        _ => 1.0,
    };

    // the resting field fill is `field_fill_for(variant)` — `surface_alt`
    // (a body distinct from the `surface` pane) for the `Filled` default, or
    // `surface` (the compatibility page-coloured fill) for `Outlined`. Routed
    // through the shared helper so a slotted field cannot drift from an
    // unslotted one, nor from the other five family members. Disabled lands on
    // the same fill; it is set apart by dimmed ink/border, not fill.
    let bg = crate::tokens::field_fill_for(palette, variant);

    Style {
        background: Background::Color(bg),
        border: Border {
            color: border_color,
            width: border_width,
            radius: size_tokens.radius.into(),
        },
        icon: palette.text_muted,
        // The placeholder uses `text-muted`, not the fainter `text-subtle`.
        placeholder: palette.text_muted,
        value: palette.text,
        selection: palette.accent_soft,
    }
}
