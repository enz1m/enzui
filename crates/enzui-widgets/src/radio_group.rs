//! EnzRadioGroup: a styled radio button group.
//!
//! Composes the [`EnzRadioButton`](crate::radio_button) primitive into a
//! mutually-exclusive group with token-driven styling, 3 size variants,
//! vertical/horizontal orientation, and roving focus.
//!
//! # Roving focus (WAI-ARIA `radiogroup`)
//!
//! The group is one Tab stop: Tab enters at the selected option (or the
//! first enabled one) and Tab again leaves entirely. Inside it:
//!
//! | key | effect |
//! |---|---|
//! | `Down` / `Right` | next enabled option |
//! | `Up` / `Left` | previous enabled option |
//! | `Home` / `End` | first / last enabled option |
//! | `Enter` / `Space` | (re)select the option under the cursor |
//!
//! Movement wraps and selects as it moves — the ARIA contract — and
//! the emitted message is the click path's own, so keyboard and pointer are
//! indistinguishable to the caller.
//!
//! # Typed values instead of indices
//!
//! [`enz_radio_group_keyed`] takes one collection of [`RadioChoice`]s (key,
//! value, label), accepts the selected value and emits the selected value.
//! Identity follows the key, not the label or the position.
//!
//! # Heading, name and identity
//!
//! `group_label` is painted as the heading, names the `RadioGroup` node and
//! keys the group's derived identity. [`EnzRadioGroup::label_visible`]`(false)`
//! keeps the last two and drops the painted heading, for a form that lays the
//! caption beside the group; [`EnzRadioGroup::aria_label`] names a group that
//! carries no heading text.

use crate::color::{darken, with_opacity};
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, PressFocus, press_focus_from_cursor, release_focus_if_inert};
use enzui_core::identity::KeyedIds;
use enzui_core::semantics::{Orientation as SemOrientation, Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{column, row, text};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector, keyboard};
use std::borrow::Cow;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Group-scoped option identity
// ---------------------------------------------------------------------------

/// The stable string key for a widget [`Id`], namespacing a group's
/// derived option ids by the group's own identity.
///
/// `Id` exposes no accessor and no `Display` — only `Debug` — so `Debug` is
/// the only total mapping, as the rest of the crate already uses. It
/// guarantees purity and injectivity: same `Id` → same key, differing
/// `Id`s → differing keys.
///
/// The unwrap is conditional, not cosmetic: unwrapping unconditionally erases
/// the branch tag telling a `Custom` from a `Unique`, so `Id::unique()` and
/// `Id::new("Id(Unique(0))")` would share one key and two groups would derive
/// identical option ids. The unwrap applies **only when the payload cannot
/// alias a wrapped form** — not beginning with `Id(` — partitioning keys into
/// three provably disjoint classes (bare payload; whole `Custom` debug;
/// whole `Unique` debug), a categorical screen no payload defeats. Debug
/// escaping never moves a payload's first character into `Id(` (`I`, `d`, `(`
/// need no escaping), and each class is internally injective.
///
/// If upstream changes the `Debug` shape, the fallback returns the whole
/// debug string — still pure and injective, only spelled differently — and
/// `scope_key_unwraps_the_custom_debug_shape` pins the shape so the change
/// fails loudly. The key is only as stable as its `Id`: `.id(Id::unique())`
/// inside a view mints per frame and churns every derived id, a caller bug
/// this neither causes nor can detect.
fn scope_key_of_id(id: &Id) -> String {
    let debug = format!("{id:?}");
    // The documented shape is `Id(Custom("delivery"))`; unwrap to `delivery` —
    // but only when the payload cannot itself be read as a wrapped form, or the
    // unwrap would map `Custom("Id(Unique(0))")` and `Unique(0)` onto one key.
    debug
        .strip_prefix("Id(Custom(\"")
        .and_then(|rest| rest.strip_suffix("\"))"))
        .filter(|payload| !payload.starts_with("Id("))
        .map(str::to_owned)
        .unwrap_or(debug)
}

// ---------------------------------------------------------------------------
// Size enum
// ---------------------------------------------------------------------------

/// Radio group size presets matching the enzUI 3-step scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadioGroupSize {
    /// 14px ring, 7px dot, 12px label.
    Sm,
    /// 16px ring, 8px dot, 13px label (default).
    #[default]
    Md,
    /// 20px ring, 10px dot, 15px label.
    Lg,
}

impl RadioGroupSize {
    /// Returns the radio circle size for iced's `radio.size()`.
    pub fn ring_size(self) -> f32 {
        match self {
            Self::Sm => 14.0,
            Self::Md => 16.0,
            Self::Lg => 20.0,
        }
    }

    /// Returns the inner dot size.
    pub fn dot_size(self) -> f32 {
        match self {
            Self::Sm => 7.0,
            Self::Md => 8.0,
            Self::Lg => 10.0,
        }
    }

    /// Returns the label text size.
    pub fn label_size(self) -> f32 {
        match self {
            Self::Sm => 12.0,
            Self::Md => 13.0,
            Self::Lg => 15.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Orientation
// ---------------------------------------------------------------------------

/// Layout orientation for the radio group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Options stacked vertically.
    #[default]
    Vertical,
    /// Options laid out horizontally.
    Horizontal,
}

// ---------------------------------------------------------------------------
// Typed, keyed choices
// ---------------------------------------------------------------------------

// The index API has no key of its own, so its label doubles as `id_key`.
#[derive(Debug, Clone)]
struct GroupOption<'a> {
    label: Cow<'a, str>,
    id_key: Cow<'a, str>,
}

/// One typed choice for [`enz_radio_group_keyed`]: a stable key, the value the
/// caller stores, and the label the user reads.
///
/// The key is short, untranslated and stable — settings-file material. It
/// decides the option's AccessKit id and the roving cursor's identity, so
/// shared labels still address separately and reordering/filtering moves
/// neither selection nor cursor. Unique per group; a repeat falls back to
/// occurrence disambiguation, which is positional and not reorder-stable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RadioChoice<'a, V> {
    key: &'a str,
    value: V,
    label: Cow<'a, str>,
    enabled: bool,
}

impl<'a, V> RadioChoice<'a, V> {
    /// A choice whose label is borrowed for the choice's own lifetime; `const`,
    /// so a whole collection can be a `const` item.
    pub const fn new(key: &'a str, value: V, label: &'a str) -> Self {
        Self {
            key,
            value,
            label: Cow::Borrowed(label),
            enabled: true,
        }
    }

    /// A choice that also accepts an owned label, for one computed at runtime.
    pub fn with_label(key: &'a str, value: V, label: impl Into<Cow<'a, str>>) -> Self {
        Self {
            key,
            value,
            label: label.into(),
            enabled: true,
        }
    }

    /// Mark this choice unavailable: not clickable, skipped by the roving
    /// cursor, and `disabled` in the AT tree.
    #[must_use]
    pub const fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// The untranslated key this choice is identified by.
    pub const fn key(&self) -> &str {
        self.key
    }

    /// The value emitted when this choice is picked.
    pub const fn value(&self) -> &V {
        &self.value
    }

    /// The label the user reads.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Whether the choice can be picked.
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Paint tokens shared by standalone radio buttons and radio groups.
#[derive(Debug, Clone, Copy)]
pub struct RadioTokens {
    /// Unselected outline width in logical pixels; `None` follows the live hairline.
    /// The selected circle uses twice this width.
    /// Invalid overrides use the live hairline. Zero hides the outline.
    pub border_width: Option<f32>,
    /// Keyboard focus-ring width in logical pixels; defaults to `2.0`.
    /// Invalid values use the default. Zero hides the ring.
    pub focus_ring_width: f32,
    /// Focus-ring corner radius in logical pixels; defaults to `4.0`.
    /// Invalid values use the default. Zero makes square corners.
    pub focus_ring_radius: f32,
    /// Disabled opacity in `[0, 1]`; defaults to `0.62` and multiplies colour alpha.
    /// Finite values are clamped; non-finite values use the default.
    pub disabled_alpha: f32,
    /// Border color of unselected radio circle.
    pub border: Color,
    /// Fill of the unselected radio circle: a solid control, as an
    /// unchecked checkbox, not a bare outline.
    pub background: Color,
    /// Border color on hover (unselected).
    pub hover_border: Color,
    /// Selected circle fill and border.
    pub accent: Color,
    /// Contrasting selected dot colour.
    pub dot_color: Color,
    /// Label text color.
    pub text_color: Color,
    /// Group label text color (slightly muted).
    pub group_label_color: Color,
    /// Focus-visible ring color (keyboard focus).
    pub focus_ring: Color,
}

impl RadioTokens {
    /// Derive radio tokens from a theme palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            border_width: None,
            focus_ring_width: enzui_theme::text_defaults::focus_ring_width(),
            focus_ring_radius: 4.0,
            disabled_alpha: 0.62,
            // The unchecked checkbox's border and fill, so an empty radio
            // reads as a control on any surface.
            border: p.control_border_strong,
            background: crate::checkbox::empty_control_fill(p),
            hover_border: p.accent,
            accent: p.filled_color(p.accent).color,
            dot_color: p.filled_color(p.accent).text,
            text_color: p.text,
            group_label_color: p.text_muted,
            focus_ring: p.focus_ring,
        }
    }

    /// Convenience: tokens for dark theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: tokens for light theme.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

// ---------------------------------------------------------------------------
// Ring chrome — painted by the shared `HoverControl` widget
// ---------------------------------------------------------------------------

pub(crate) fn ring_fill_border(
    t: &RadioTokens,
    selected: bool,
    hovered: bool,
    enabled: bool,
) -> (Color, Color) {
    let a = if enabled {
        1.0
    } else {
        crate::color::disabled_alpha(t.disabled_alpha)
    };
    if selected {
        // Selected hover darkens the accent ring + dot 8%, matching the
        // checkbox's checked-hover darken.
        let (dot, border) = if hovered && enabled {
            (darken(t.dot_color, 0.08), darken(t.accent, 0.08))
        } else {
            (t.dot_color, t.accent)
        };
        (with_opacity(dot, a), with_opacity(border, a))
    } else {
        // Unselected hover raises the ring border to `hover_border` (accent).
        let border = if hovered && enabled {
            t.hover_border
        } else {
            t.border
        };
        (Color::TRANSPARENT, with_opacity(border, a))
    }
}

/// The unselected circle's fill, faded like the rest when disabled.
pub(crate) fn ring_background(t: &RadioTokens, enabled: bool) -> Color {
    let a = if enabled {
        1.0
    } else {
        crate::color::disabled_alpha(t.disabled_alpha)
    };
    with_opacity(t.background, a)
}

pub(crate) fn ring_border_width(selected: bool, tokens: &RadioTokens, theme: &iced::Theme) -> f32 {
    let hairline = enzui_theme::StrokeToken::Hairline.resolve(&crate::theme_lookup::stroke(theme));
    let width = tokens.border_width.map_or(hairline, |value| {
        enzui_core::metrics::FiniteNonNeg::new_or(value, hairline).get()
    });
    width * if selected { 2.0 } else { 1.0 }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// The namespace a group's own id is derived under when the caller supplies no
/// [`EnzRadioGroup::id`]: the group label is the key.
///
/// Distinct from [`DEFAULT_OPTION_ID_PREFIX`](EnzRadioGroup::DEFAULT_OPTION_ID_PREFIX),
/// the root of the *option* namespace (`{option-prefix}-{group id}`).
pub const DEFAULT_GROUP_ID_PREFIX: &str = "enz-radio-group";

/// Builder for an EnzRadioGroup element.
#[must_use]
pub struct EnzRadioGroup<'a, Message> {
    group_label: Cow<'a, str>,
    aria_label: Option<String>,
    label_visible: bool,
    options: Vec<GroupOption<'a>>,
    selected: Option<usize>,
    on_select: Box<dyn Fn(usize) -> Message + 'a>,
    size: RadioGroupSize,
    orientation: Orientation,
    spacing: f32,
    enabled: bool,
    /// Per-option enable flags, `true` unless the caller disabled that index.
    /// Empty means "every option enabled" (the common case allocates nothing).
    option_enabled: Vec<bool>,
    tokens: Option<RadioTokens>,
    id: Option<Id>,
    /// The caller's explicit namespace prefix for per-option ID derivation.
    ///
    /// Holds *nothing* when the prefix merely derives from [`.id()`](Self::id):
    /// the group-scoped default computes on demand from the builder's final
    /// `id`. Latching it in `.id()` would make the builder order-dependent
    /// (`.id(a).id(b)` scoping options to `a` while the group answers to `b`).
    /// See `option_scope_follows_the_final_group_id_not_the_first`.
    option_id_prefix: Option<String>,
    /// Explicit per-option IDs supplied by the caller, keyed by index.
    /// When provided, these override any prefix-derived IDs for that index.
    option_ids: Vec<Option<Id>>,
}

/// Create an EnzRadioGroup element.
///
/// # Arguments
///
/// * `group_label` — label for the group (can be empty).
/// * `options` — slice of option labels.
/// * `selected` — index of currently selected option, or `None`.
/// * `on_select` — callback producing a message with the new selected index.
pub fn enz_radio_group<'a, Message: 'a>(
    group_label: impl Into<Cow<'a, str>>,
    options: &'a [&'a str],
    selected: Option<usize>,
    on_select: impl Fn(usize) -> Message + 'a,
) -> EnzRadioGroup<'a, Message> {
    EnzRadioGroup {
        group_label: group_label.into(),
        aria_label: None,
        label_visible: true,
        // The label is this API's only logical key.
        options: options
            .iter()
            .map(|&label| GroupOption {
                label: Cow::Borrowed(label),
                id_key: Cow::Borrowed(label),
            })
            .collect(),
        selected,
        on_select: Box::new(on_select),
        size: RadioGroupSize::default(),
        orientation: Orientation::default(),
        spacing: 8.0,
        enabled: true,
        option_enabled: Vec::new(),
        tokens: None,
        id: None,
        option_id_prefix: None,
        option_ids: Vec::new(),
    }
}

/// Create an EnzRadioGroup over typed, keyed choices: it takes the selected
/// value and emits the selected value, so an enum-valued setting needs no
/// index adapters.
///
/// # Arguments
///
/// * `group_label` — label for the group; may be empty (see
///   [`EnzRadioGroup::aria_label`] and [`EnzRadioGroup::label_visible`]).
/// * `choices` — the choices, in display order.
/// * `selected` — the currently selected value, matched by `PartialEq`. A
///   value `choices` does not offer leaves the group unselected; of equal
///   values, the first is selected.
/// * `on_select` — called with the picked choice's value.
///
/// The ordinary [`EnzRadioGroup`], with its sizing, orientation, ids and
/// one-Tab-stop roving contract. Availability is per choice; per-choice
/// AccessKit ids come from the key, not the label.
pub fn enz_radio_group_keyed<'a, Message: 'a, V: PartialEq + 'a>(
    group_label: impl Into<Cow<'a, str>>,
    choices: &'a [RadioChoice<'a, V>],
    selected: Option<&V>,
    on_select: impl Fn(&V) -> Message + 'a,
) -> EnzRadioGroup<'a, Message> {
    // Unknown value -> `None`. Not `unwrap_or(0)`: see the `selected` argument.
    let selected = selected.and_then(|want| choices.iter().position(|c| c.value == *want));
    let mut group = enz_radio_group(
        group_label,
        &[],
        selected,
        // Every index the group publishes indexes arrays sized `choices.len()`,
        // so an out-of-range index is a framework bug that panics rather than
        // emitting a neighbour's value.
        move |index| on_select(&choices[index].value),
    );
    group.options = choices
        .iter()
        .map(|choice| GroupOption {
            label: choice.label.clone(),
            id_key: Cow::Borrowed(choice.key),
        })
        .collect();
    group.option_enabled = choices.iter().map(RadioChoice::is_enabled).collect();
    group
}

impl<'a, Message: Clone + 'a> EnzRadioGroup<'a, Message> {
    /// Set the size variant.
    pub fn size(mut self, size: RadioGroupSize) -> Self {
        self.size = size;
        self
    }

    /// Set the layout orientation.
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Set the spacing between options. Normalized to a finite, non-negative
    /// value: non-finite input (NaN / ±inf) falls back to `0.0` rather
    /// than reaching the layout `spacing`.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = enzui_core::metrics::FiniteNonNeg::new_or(spacing, 0.0).get();
        self
    }

    /// Set enabled/disabled state for the whole group.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Name the `RadioGroup` node without painting anything, overriding
    /// `group_label` as the accessible name.
    ///
    /// For a group carrying no heading text; when the caption is the group's
    /// own but sits outside it, prefer [`label_visible`](Self::label_visible)`(false)`.
    /// With an empty `group_label` and no [`Self::id`], this name also keys
    /// the derived identity, keeping options routable.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Choose whether `group_label` is painted as the group's own heading;
    /// default `true`.
    ///
    /// `false` is for a form that already lays that caption beside the group:
    /// the string keeps naming the `RadioGroup` node and keying its derived
    /// identity, and only the second painted copy goes away.
    pub const fn label_visible(mut self, visible: bool) -> Self {
        self.label_visible = visible;
        self
    }

    // The aria label wins here; `effective_group_id` prefers the group label.
    fn accessible_name(&self) -> Option<String> {
        self.aria_label
            .clone()
            .or_else(|| (!self.group_label.is_empty()).then(|| self.group_label.to_string()))
    }

    /// Disable individual options by index.
    ///
    /// A disabled option is not clickable, publishes `disabled` semantics, and
    /// is skipped entirely by the roving cursor (step-over, Home/End to
    /// first/last *enabled*, wrap past a disabled endpoint). Out-of-range
    /// indices are ignored. Cumulative across calls.
    pub fn disabled_options(mut self, indices: impl IntoIterator<Item = usize>) -> Self {
        if self.option_enabled.is_empty() {
            self.option_enabled = vec![true; self.options.len()];
        }
        for i in indices {
            if let Some(slot) = self.option_enabled.get_mut(i) {
                *slot = false;
            }
        }
        self
    }

    /// Pin colours, disabled opacity and focus geometry to a snapshot.
    /// `border_width: None` still follows the live hairline stroke.
    pub fn tokens(mut self, tokens: RadioTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// The root of the namespace every derived option id sits under.
    ///
    /// Never a whole prefix alone: [`.id()`](Self::id) derives
    /// `{DEFAULT_OPTION_ID_PREFIX}-{group id}` off it, so two groups share an
    /// option id only by sharing a group id. `pub` as the stable matchable
    /// root — and the namespace an explicit
    /// [`.option_id_prefix()`](Self::option_id_prefix) replaces. With no
    /// `.id()` and no prefix, options are anonymous: the constant is not a
    /// fallback in that case either.
    pub const DEFAULT_OPTION_ID_PREFIX: &'static str = "enz-radio";

    /// Assign a stable widget identity to the group's single focus stop, so
    /// programmatic `focus(id)` can enter it.
    ///
    /// Side-effect on option identity: `.id()` also enables per-option
    /// derivation under `{DEFAULT_OPTION_ID_PREFIX}-{group id}` (e.g.
    /// `enz-radio-delivery-Yes`), so AccessKit can target individual options.
    /// The group id is folded in because a bare-constant prefix is identical
    /// for every group on screen: two `["Yes", "No"]` groups would derive
    /// identical option ids and an AT activating one's "Yes" would check the
    /// other's. Same `.id()` twice is still a caller-side duplicate, caught
    /// at frame level by
    /// [`enzui_core::identity::duplicates`].
    ///
    /// An explicit [`.option_id_prefix()`](Self::option_id_prefix) set
    /// *before* `.id()` is never overwritten, and an explicit
    /// [`.option_id()`](Self::option_id) always wins for its index. Without
    /// `.id()`, the group id falls back to the group label (a genuine
    /// logical key surviving form reorders), then
    /// [`.aria_label()`](Self::aria_label) — never positional, which would
    /// misroute previous-frame requests. No id and no label stays anonymous
    /// by design. Calling `.id()` twice is last-wins for the option scope
    /// too: the prefix computes from the final `id` at build time, never
    /// latched here.
    pub fn id(mut self, id: Id) -> Self {
        // Deliberately does not latch a derived prefix. Per-option derivation is
        // enabled by `id` being `Some`, and the prefix itself is computed from
        // the *final* id in `effective_option_id_prefix` — latching it here
        // would make `.id(a).id(b)` scope the options to `a`.
        self.id = Some(id);
        self
    }

    /// The derived option-id prefix for a group identified by `id`:
    /// `{DEFAULT_OPTION_ID_PREFIX}-{scope_key(id)}`.
    pub(crate) fn scoped_option_id_prefix(id: &Id) -> String {
        format!("{}-{}", Self::DEFAULT_OPTION_ID_PREFIX, scope_key_of_id(id))
    }

    /// The prefix per-option ids are actually derived from, resolved in one
    /// place so the answer cannot depend on builder call order.
    ///
    /// Precedence: explicit [`.option_id_prefix()`](Self::option_id_prefix)
    /// whenever called; else the group scope from the final
    /// [`.id()`](Self::id); else `None` (anonymous options).
    pub(crate) fn effective_option_id_prefix(&self) -> Option<String> {
        self.option_id_prefix.clone().or_else(|| {
            self.effective_group_id()
                .as_ref()
                .map(Self::scoped_option_id_prefix)
        })
    }

    /// The group's identity: an explicit [`.id()`](Self::id), else one derived
    /// from the group label, else from [`.aria_label()`](Self::aria_label).
    ///
    /// Either label is a real logical key surviving form reorders — the
    /// property a previous-frame AT request needs. Positional fallback is
    /// deliberately absent; painted-or-not does not enter into it, or
    /// [`.label_visible()`](Self::label_visible) would move every derived id.
    /// `None` only with neither id nor label: anonymous, advertising nothing.
    pub(crate) fn effective_group_id(&self) -> Option<Id> {
        self.id.clone().or_else(|| {
            // Heading first, unlike `accessible_name`: adding a name to a group
            // that already has a heading must not re-key it.
            let key = (!self.group_label.is_empty())
                .then(|| self.group_label.to_string())
                .or_else(|| self.aria_label.clone())?;
            Some(enzui_core::identity::derive_id(
                DEFAULT_GROUP_ID_PREFIX,
                &key,
            ))
        })
    }

    /// Set the namespace prefix for per-option ID derivation.
    ///
    /// Each option gets `derive_id(prefix, label)` (duplicates disambiguated
    /// by occurrence), enabling per-option activation through AccessKit. With
    /// no explicit prefix but [`.id()`](Self::id), the group-scoped prefix
    /// applies automatically; an explicit prefix set *before* `.id()`
    /// replaces the whole namespace (shared explicit prefixes collide like
    /// shared ids). Neither called: anonymous options, by contract.
    pub fn option_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.option_id_prefix = Some(prefix.into());
        self
    }

    /// Set an explicit per-option ID at `index`.
    ///
    /// Overrides any prefix-derived ID for that option. Out-of-range indices
    /// are silently ignored.
    pub fn option_id(mut self, index: usize, id: Id) -> Self {
        if index < self.options.len() {
            if self.option_ids.is_empty() {
                self.option_ids = vec![None; self.options.len()];
            }
            self.option_ids[index] = Some(id);
        }
        self
    }

    /// Per-option enabled flags, folding in the group-wide `enabled`.
    fn enabled_flags(&self) -> Vec<bool> {
        (0..self.options.len())
            .map(|i| self.enabled && self.option_enabled.get(i).copied().unwrap_or(true))
            .collect()
    }

    /// Build the element. If `.tokens()` was not called, tokens are
    /// derived from the runtime `iced::Theme`.
    pub fn into_element(self) -> Element<'a, Message> {
        let pinned_tokens = self.tokens;
        let sz = self.size;
        let enabled = self.enabled;
        let flags = self.enabled_flags();
        // Resolved from the *final* `id` before `on_select` is moved out below,
        // so builder call order cannot change the derived option ids.
        let effective_prefix = self.effective_option_id_prefix();
        // The group's own routable identity, explicit or label-derived.
        // Resolved here for the same order-independence reason.
        let effective_group_id = self.effective_group_id();
        // `&self` methods stop resolving once `on_select` is moved out below.
        let accessible_name = self.accessible_name();

        // One shared handle to the caller's factory: each option's *lazy*
        // activation clones it rather than the group pre-computing
        // `on_select(i)` per option per rebuild. The owning widget holds the
        // same handle for keyboard selection — literally the same callback on
        // both paths, not two that merely agree.
        let on_select: Rc<dyn Fn(usize) -> Message + 'a> = Rc::from(self.on_select);

        // Derived even without a prefix, where no id is published: the roving
        // cursor is stored as this key so it survives a reorder or filter.
        let mut keyed = KeyedIds::new(effective_prefix.clone().unwrap_or_default());
        let option_keys: Vec<String> = self
            .options
            .iter()
            .map(|option| keyed.key_for(&option.id_key))
            .collect();

        // Without a prefix (no `.id()`, no `.option_id_prefix()`) the options
        // stay anonymous, which is the documented contract.
        let option_ids: Vec<Option<Id>> = option_keys
            .iter()
            .enumerate()
            .map(|(i, key)| {
                self.option_ids
                    .get(i)
                    .and_then(std::clone::Clone::clone)
                    .or_else(|| {
                        effective_prefix
                            .as_deref()
                            .map(|prefix| enzui_core::identity::derive_id(prefix, key))
                    })
            })
            .collect();

        // Compose the focus/keyboard/semantics-aware `EnzRadioButton`
        // primitive so each option keeps hover/pressed visuals, pointer
        // activation, and its semantics node — with activation *gated* when
        // the group is disabled (stock `radio` kept firing when disabled).
        //
        // `.embedded(true)` makes the group one Tab stop: options withdraw
        // from `Operation::focusable`, yield Enter/Space to the owner, and
        // take no pointer focus. Disabled buttons leave traversal the same
        // way; `embedded` is the composite case.
        let radios: Vec<Element<'a, Message>> = self
            .options
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let f = Rc::clone(&on_select);
                let mut rb = crate::radio_button::enz_radio_button(
                    option.label.clone(),
                    i,
                    self.selected,
                    move |v| f(v),
                )
                .size(sz)
                .embedded(true)
                .enabled(flags[i]);
                if let Some(t) = pinned_tokens {
                    rb = rb.tokens(t);
                }
                // Assign the derived or explicit per-option ID so
                // the semantics node published by the option's inner
                // FocusButton carries an identity routable by AccessKit.
                if let Some(oid) = option_ids[i].clone() {
                    rb = rb.id(oid);
                }
                rb.into()
            })
            .collect();

        // Build the group container
        let laid_out: Element<'a, Message> = match self.orientation {
            Orientation::Vertical => {
                let mut col = column![].spacing(self.spacing);
                for r in radios {
                    col = col.push(r);
                }
                col.into()
            }
            Orientation::Horizontal => {
                // A plain `row!` has no wrap: at a bounded width options slice
                // at the container edge. `.wrap()` flows them onto a new line —
                // structurally safe because one layout child per option is what
                // `option_bounds`/`option_at` hit-testing relies on.
                let mut r = row![].spacing(self.spacing);
                for radio_el in radios {
                    r = r.push(radio_el);
                }
                r.wrap().into()
            }
        };

        // The AT tree needs a RadioGroup node parenting the RadioButton options
        // parenting them. Orientation + group label + disabled state are
        // reflected onto the node; layout/visuals are untouched. It is published
        // by the owning widget rather than by a `SemanticsContainer` so the
        // node's `focused` flag can be the group's *live* focus state.
        let group_semantics = Semantics::new(SemRole::RadioGroup)
            .label_opt(accessible_name)
            .disabled(!enabled)
            .orientation(match self.orientation {
                Orientation::Vertical => SemOrientation::Vertical,
                Orientation::Horizontal => SemOrientation::Horizontal,
            });

        let group: Element<'a, Message> = Element::new(RadioGroupWidget {
            child: laid_out,
            selected: self.selected,
            enabled_flags: flags,
            semantics: group_semantics,
            on_select,
            tokens: pinned_tokens,
            // The group's *effective* id, so a labelled group is routable
            // (and its options scoped) without the caller opting in. Using
            // `self.id` alone would leave the whole group anonymous by default,
            // which strips every advertised action.
            id: effective_group_id,
            // The same ids just handed to the options, so the
            // group can name the cursored one as its `active_descendant`.
            option_ids,
            option_keys,
        });

        // Colour tracks the runtime theme.
        if self.group_label.is_empty() || !self.label_visible {
            group
        } else {
            // The same caption every field label uses: 12 px, medium, muted.
            let label = text(self.group_label)
                .size(enzui_theme::text_scale::ui_px(12.0))
                .font(enzui_theme::text_defaults::font(
                    enzui_theme::FontRole::Ui,
                    enzui_theme::WeightRung::Medium,
                ))
                .style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some({
                        let tokens = pinned_tokens.unwrap_or_else(|| {
                            crate::theme_lookup::resolve_live(theme, RadioTokens::from_palette)
                        });
                        if enabled {
                            tokens.group_label_color
                        } else {
                            with_opacity(
                                tokens.group_label_color,
                                crate::color::disabled_alpha(tokens.disabled_alpha),
                            )
                        }
                    }),
                });
            column![label, group].spacing(4).into()
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzRadioGroup<'a, Message>> for Element<'a, Message> {
    fn from(group: EnzRadioGroup<'a, Message>) -> Self {
        group.into_element()
    }
}

// ---------------------------------------------------------------------------
// The roving-focus owner
// ---------------------------------------------------------------------------

/// Per-group state: the single focus stop plus the roving cursor.
#[derive(Debug, Clone, Default)]
struct RadioGroupState {
    focus: FocusState,
    // The option the arrow keys act on, held as an identity key rather than an
    // index: this state outlives the view, so a rebuild that reorders or
    // filters the options would otherwise move the cursor to another option.
    // `None` until the group is entered or an option is clicked, and resolved
    // through `selected` in the meantime (`effective_cursor`).
    cursor: Option<String>,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide this group's ring.
    window_key: u64,
}

impl iced::advanced::widget::operation::Focusable for RadioGroupState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
    }
}

/// What a key press means to a focused radio group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RadioKey {
    /// `Down` / `Right` — next enabled option.
    Next,
    /// `Up` / `Left` — previous enabled option.
    Previous,
    /// `Home` — first enabled option.
    First,
    /// `End` — last enabled option.
    Last,
    /// `Enter` / `Space` — (re)select the option under the cursor.
    Activate,
}

/// The single source of truth for the keys this widget claims and the
/// [`RadioKey`] intent each decodes to. Production and the modifier-guard
/// population both derive from it — fail-closed, not a parallel hand list.
///
/// Order is behavioural: the pinned shape reads duplicates positionally (two
/// `Next`: Down + Right; two `Previous`: Up + Left; two `Activate`: Enter +
/// Space; one `First`: Home; one `Last`: End). Re-tagging a claim moves that
/// shape and reds the pin.
const RADIO_KEY_CLAIMS: &[(keyboard::key::Named, RadioKey)] = &[
    (keyboard::key::Named::ArrowDown, RadioKey::Next),
    (keyboard::key::Named::ArrowRight, RadioKey::Next),
    (keyboard::key::Named::ArrowUp, RadioKey::Previous),
    (keyboard::key::Named::ArrowLeft, RadioKey::Previous),
    (keyboard::key::Named::Home, RadioKey::First),
    (keyboard::key::Named::End, RadioKey::Last),
    (keyboard::key::Named::Enter, RadioKey::Activate),
    (keyboard::key::Named::Space, RadioKey::Activate),
];

/// Decode a key press into a radio-group navigation intent.
///
/// Deliberately not [`navigation_from_key`](enzui_core::focus::navigation_from_key):
/// Up/Right-as-`Increment` suits a *slider*, not a *list* — using it here
/// would send Down backwards. Ctrl/Alt/Logo presses return `None` (event
/// stays `Ignored` for application chords); Shift is kept as the control's
/// own modifier. Reads [`RADIO_KEY_CLAIMS`], so a new key is automatically
/// covered by the 16-state matrix.
fn radio_key(event: &keyboard::Event) -> Option<RadioKey> {
    let keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    // Ctrl / Alt / Logo mark application chords — let them through.
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    RADIO_KEY_CLAIMS
        .iter()
        .find_map(|(claimed, intent)| (claimed == named).then_some(*intent))
}

/// Owns the group's single Tab stop, its roving cursor, its keyboard contract
/// and its `RadioGroup` semantics node. The option column/row is the one child.
struct RadioGroupWidget<'a, Message> {
    child: Element<'a, Message>,
    selected: Option<usize>,
    enabled_flags: Vec<bool>,
    semantics: Semantics,
    on_select: Rc<dyn Fn(usize) -> Message + 'a>,
    tokens: Option<RadioTokens>,
    id: Option<iced::advanced::widget::Id>,
    /// Per-option AT ids, index-aligned with the options, so `operate` can name
    /// the cursored option as the group's `active_descendant`.
    ///
    /// Options are `embedded(true)` and publish no `focused` of their own;
    /// without this a screen reader could not tell which option the cursor
    /// reached. `None` where no id is derivable — a dangling relation is
    /// never emitted, exactly as `tree_view` does.
    option_ids: Vec<Option<Id>>,
    // Index-aligned with the options; resolves `RadioGroupState::cursor`
    // against the current order.
    option_keys: Vec<String>,
}

impl<Message> RadioGroupWidget<'_, Message> {
    fn count(&self) -> usize {
        self.enabled_flags.len()
    }

    fn is_enabled(&self, i: usize) -> bool {
        self.enabled_flags.get(i).copied().unwrap_or(false)
    }

    fn cursor_index(&self, state: &RadioGroupState) -> Option<usize> {
        let key = state.cursor.as_deref()?;
        self.option_keys.iter().position(|k| k == key)
    }

    fn cursor_key(&self, i: usize) -> Option<String> {
        self.option_keys.get(i).cloned()
    }

    /// Whether *any* option can be reached at all. A group of nothing but
    /// disabled options is inert: it is not a Tab stop and answers no key.
    fn any_enabled(&self) -> bool {
        self.enabled_flags.iter().any(|&e| e)
    }

    fn first_enabled(&self) -> Option<usize> {
        (0..self.count()).find(|&i| self.is_enabled(i))
    }

    fn last_enabled(&self) -> Option<usize> {
        (0..self.count()).rev().find(|&i| self.is_enabled(i))
    }

    /// The next enabled option `dir` steps from `from`, wrapping at the
    /// ends — the WAI-ARIA radio-group behaviour (unlike a clamping menubar).
    ///
    /// Bounded by `count` iterations: a sole-enabled-`from` returns
    /// `Some(from)`, none-enabled returns `None` — neither spins.
    fn step(&self, from: usize, dir: isize) -> Option<usize> {
        let n = self.count() as isize;
        if n == 0 {
            return None;
        }
        let mut i = from as isize;
        for _ in 0..self.count() {
            i += dir;
            if i < 0 {
                i = n - 1;
            } else if i >= n {
                i = 0;
            }
            if self.is_enabled(i as usize) {
                return Some(i as usize);
            }
        }
        None
    }

    /// Where the cursor sits right now: the live one if still valid and
    /// enabled, else the selected option, else the first enabled one.
    ///
    /// What makes Tab enter at the selected option with nothing stored on
    /// entry: the cursor is `None` until moved, resolving through the
    /// caller's `selected` meanwhile. A vanished option resolves the same way.
    fn effective_cursor(&self, state: &RadioGroupState) -> Option<usize> {
        self.cursor_index(state)
            .filter(|&i| self.is_enabled(i))
            .or_else(|| self.selected.filter(|&i| self.is_enabled(i)))
            .or_else(|| self.first_enabled())
    }

    /// The option index a key targets, or `None` when it does not move.
    fn key_target(&self, cursor: Option<usize>, key: RadioKey) -> Option<usize> {
        match key {
            RadioKey::First => self.first_enabled(),
            RadioKey::Last => self.last_enabled(),
            RadioKey::Activate => cursor,
            RadioKey::Next => match cursor {
                Some(i) => self.step(i, 1),
                // No cursor at all: entering forwards lands on the first.
                None => self.first_enabled(),
            },
            RadioKey::Previous => match cursor {
                Some(i) => self.step(i, -1),
                None => self.last_enabled(),
            },
        }
    }

    /// The laid-out bounds of every option, in order.
    ///
    /// The child is the option `column!`/`row!`, and this widget delegates its
    /// layout straight to it, so this layout is the column's: the options
    /// are its children, one per index, in the column's tree.
    fn option_bounds(&self, layout: Layout, tree: &Tree) -> Vec<Rectangle> {
        layout
            .children(&tree.children[0].children)
            .map(|l| l.bounds())
            .collect()
    }

    /// The option index under a screen-space point.
    fn option_at(&self, layout: Layout, tree: &Tree, point: iced::Point) -> Option<usize> {
        self.option_bounds(layout, tree)
            .iter()
            .position(|b| b.contains(point))
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for RadioGroupWidget<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RadioGroupState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(RadioGroupState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Same rebuild-time release as the family, on `operate`'s
        // `any_enabled()` condition. The *ring* is double-protected
        // (`effective_cursor()` is `None` with nothing enabled, so `draw`
        // paints nothing — a draw probe cannot fail here). The semantics
        // path has no second guard: it publishes the focus flag straight, so
        // a stale one announces a focused group with no focusable option.
        // Releasing here covers the audible half.
        {
            let can_take_input = self.any_enabled();
            let state = tree.state.downcast_mut::<RadioGroupState>();
            let _ = release_focus_if_inert(&mut state.focus, can_take_input);
        }
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<RadioGroupState>();
        // One focus stop for the whole group (WAI-ARIA radiogroup); the options
        // are `embedded` and withdraw themselves from the ring. A group with no
        // reachable option is not a Tab stop at all — the same rule
        // `focus_button::operate` applies to a button with no `on_press` and
        // `tab_pill` applies to a disabled strip — and drops any focus it still
        // held so Tab can move past it.
        if self.any_enabled() {
            operation.focusable(self.id.as_ref(), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        let state = tree.state.downcast_ref::<RadioGroupState>();
        let focused = state.focus.is_focused();

        // Name the cursored option, not merely the container:
        // `effective_cursor` is what ring and keyboard use, so the announced
        // node is the cell the cursor is visibly on. Deliberately not
        // `self.selected` — cursor and selection differ, and tracking the
        // selection would announce the wrong option until a re-render lands.
        // Mirrors `tree_view`: `and_then` + `flatten` so an un-idd option
        // contributes no relation rather than a dangling one.
        let cursor_option_id = self
            .effective_cursor(state)
            .and_then(|i| self.option_ids.get(i).cloned())
            .flatten();
        let mut semantics = self.semantics.clone().focused(focused);
        if let Some(option) = cursor_option_id {
            semantics = semantics.active_descendant(option);
        }
        let mut group = SemanticsNode::group(semantics);
        operation.custom(self.id.as_ref(), layout.bounds(), &mut group);
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Feed this window's input modality from every raw event, so a
        // pointer-last frame suppresses the keyboard ring; keyed per window and
        // stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<RadioGroupState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        // Options first: a click on an option is that option's completed click,
        // and the option publishes the selection message itself.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        let bounds = layout.bounds();

        // Pointer focus is EXCLUSIVE, and the group — not the option
        // — is the focusable, so this is one `press_focus` against the group's
        // bounds. Runs before the inert early-return so a group with nothing
        // enabled still drops stale focus and never takes any.
        if let Some(decision) = press_focus_from_cursor(event, cursor, bounds) {
            let live = self.any_enabled();
            let state = tree.state.downcast_mut::<RadioGroupState>();
            let was_focused = state.focus.is_focused();
            match decision {
                PressFocus::Take if live => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }

        if !self.any_enabled() {
            return;
        }

        match event {
            // Pointer-to-cursor: pressing an enabled option moves the roving
            // cursor there, so the next arrow key continues from where the user
            // clicked rather than from the last keyboard position.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(key) = cursor
                    .position()
                    .filter(|p| bounds.contains(*p))
                    .and_then(|p| self.option_at(layout, tree, p))
                    .filter(|&i| self.is_enabled(i))
                    .and_then(|i| self.cursor_key(i))
                {
                    tree.state.downcast_mut::<RadioGroupState>().cursor = Some(key);
                }
            }
            // Touch carries its own position — use it, not the cursor.
            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => {
                if let Some(key) = Some(*position)
                    .filter(|p| bounds.contains(*p))
                    .and_then(|p| self.option_at(layout, tree, p))
                    .filter(|&i| self.is_enabled(i))
                    .and_then(|i| self.cursor_key(i))
                {
                    tree.state.downcast_mut::<RadioGroupState>().cursor = Some(key);
                }
            }
            Event::Keyboard(kb) => {
                let state = tree.state.downcast_ref::<RadioGroupState>();
                if !state.focus.is_focused() {
                    return;
                }
                let Some(key) = radio_key(kb) else { return };
                let current = self.effective_cursor(state);
                let Some(target) = self.key_target(current, key) else {
                    return;
                };
                tree.state.downcast_mut::<RadioGroupState>().cursor = self.cursor_key(target);
                // Moving focus also selects — the ARIA contract making
                // the control mouse-free usable. The message is the options'
                // own `on_select` handle: keyboard and click indistinguishable.
                // Enter/Space re-affirm unconditionally; a *move* landing on
                // the selected option publishes nothing, so single-option
                // wrap is silent.
                if key == RadioKey::Activate || self.selected != Some(target) {
                    shell.publish((self.on_select)(target));
                }
                shell.request_redraw();
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        // The focus-visible ring is painted here, by the owner, only while
        // the group holds focus. It cannot live on the option: options build
        // during `view`, before focus is known, so a self-drawn ring would
        // outline the selected radio of an untouched group. Gate on the
        // shared focus-visible policy — a pointer click focuses without the
        // keyboard ring.
        let state = tree.state.downcast_ref::<RadioGroupState>();
        if !enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key) {
            return;
        }
        let Some(index) = self.effective_cursor(state) else {
            return;
        };
        let Some(b) = self.option_bounds(layout, tree).get(index).copied() else {
            return;
        };
        let tokens = crate::theme_lookup::resolve_pinned_live(
            crate::theme_lookup::PIN_SITE_RADIO_GROUP,
            self.tokens,
            theme,
            RadioTokens::from_palette,
        );
        let width = enzui_core::metrics::FiniteNonNeg::new_or(tokens.focus_ring_width, 2.0).get();
        let radius = enzui_core::metrics::FiniteNonNeg::new_or(tokens.focus_ring_radius, 4.0).get();
        crate::focus_button::paint_focus_ring(
            renderer,
            b,
            crate::focus_button::FocusRingQuad {
                ink: tokens.focus_ring,
                width,
                radius: radius.into(),
                outset: width,
                snap: true,
            },
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

impl crate::theme_lookup::FollowsThemeOpacity for RadioTokens {
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self {
        Self {
            disabled_alpha: opacity.disabled(),
            ..self
        }
    }
}
