//! EnzSearchField: search-flavoured text input with a search-icon
//! prefix, optional clear-button suffix, and a "Search…"
//! placeholder default. Layer 1 primitive.
//!
//! # Where this belongs
//!
//! [`command_palette`](crate::command_palette) queries through this (clear glyph +
//! placeholder tone included). Direct `search_field` use: any non-palette
//! search input (table filters, sheet-inline searches).
//!
//! # What ships
//!
//! * Measured responsive collapse: default modes measure usable entry spans
//!   (icons, padding, six wide query glyphs) vs parent-assigned widths — never
//!   requiring whole placeholders (long ones ellipsise). Sub-threshold fields
//!   become single search-icon buttons; activation expands inline + focuses in
//!   the opening frame. Non-empty queries hold selected-fill icons; Escape
//!   closes + refocuses the icon.
//! * Prefix search icon (Lucide `action.search`, `text_muted`-tinted).
//! * Filled chrome by default; [`SearchFieldChrome::Ghost`] keeps resting paint
//!   transparent while retaining hover and focus feedback.
//! * Optional trailing clear (`action.close`): fires `on_clear` on click;
//!   visible only when non-empty.
//! * `Sm`/`Md`/`Lg` sizes matching [`TextFieldSize`].
//! * `on_submit` on Enter.
//!
//! # Keyboard, selection and menu
//!
//! * Stable id + programmatic focus — [`id`](EnzSearchField::id) forwards
//!   caller `Id`s to the inner `text_input` (`focus(id)` + `select_all(id)`
//!   reachable; operations in `operation::text_input` / `operation::focusable`).
//! * `select-all-on-focus` — opt-in via [`select_all_on_focus`](EnzSearchField::select_all_on_focus).
//!   Default off: refocus-eating selections (Tab-away-and-back, caret-
//!   repositioning clicks in long queries) are obnoxious. One-keystroke-
//!   clearing flavours opt in explicitly.
//! * Right-click context menus — [`into_context_host_with`](EnzSearchField::into_context_host_with)
//!   hosts fields in [`EnzContextMenuHost`](crate::context_menu_host::EnzContextMenuHost)s
//!   (Cut/Copy/Paste/Select All ordinaries).
//! * Squeezed-placeholder ellipsising. iced `text_input`s lay placeholders at
//!   `INFINITY`-width `Ellipsis::None`: `Fixed(240)`-resolved-to-~126px fields
//!   word-break placeholders mid-string with zero more-signals. Fields paint
//!   placeholders through [`placeholder_overlay`] (`Wrapping::None` +
//!   [`PLACEHOLDER_ELLIPSIS`] over empty-placeholder `text_input`s): cosmic-text
//!   appends `\u{2026}` over hard-cuts. Accessible names still carry wholes;
//!   AT reads full strings.

use crate::field_chrome::{ChromePaint, ChromeStatus, FieldChrome};
use crate::focus_button::{FocusStatus, focus_button};
use crate::icon::enz_icon;
use crate::text_field::TextFieldSize;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::widget::Id;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::text_input::{Status, Style};
use iced::widget::{Row, text_input};
use iced::{Background, Border, Color, Element, Length, Padding};

/// Shared measured-collapse core: width measurement, compact
/// toolbar button, focus routing to the hidden input, Escape
/// restoration, and active-query paint. Consumers supply their
/// own expanded field.
pub(crate) mod responsive;

/// Native accessible shortcut carried by both the expanded input and the
/// collapsed icon unless the caller overrides it.
pub const DEFAULT_SHORTCUT: &str = responsive::DEFAULT_SHORTCUT;

/// Accessible name shared by expanded result dropdowns and collapsed search popovers.
pub const RESULT_SURFACE_LABEL: &str = responsive::RESULT_SURFACE_LABEL;

/// How a responsive search field presents after its compact icon is activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchPresentation {
    /// Expand the field back into its original layout slot.
    #[default]
    Inline,
    /// Keep the square trigger in-flow and open the field in a viewport-centred
    /// card below it. This is the titlebar-safe presentation.
    Popover,
    /// Keep the trigger in-flow and open the field below it, clamped to the viewport.
    AnchoredPopover,
}

/// The width [`enz_titlebar_search`] fills up to, in logical
/// pixels. Also the field's declared width above that rung (see
/// [`EnzSearchField::max_width`]), so a caller reserving band
/// space or gating on that declaration reads the same number.
pub const TITLEBAR_SEARCH_MAX_WIDTH: f32 = 340.0;

/// Titlebar search preset: small (26 px), measured collapse,
/// viewport-centred popover activation, and the native `Ctrl+F`
/// shortcut. Fills the space the titlebar assigns up to
/// [`TITLEBAR_SEARCH_MAX_WIDTH`].
pub fn enz_titlebar_search<'a, Message>(
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> EnzSearchField<'a, Message>
where
    Message: Clone + 'a,
{
    enz_search_field(value, on_input)
        .size(TextFieldSize::Sm)
        .presentation(SearchPresentation::Popover)
        .shortcut(DEFAULT_SHORTCUT)
        .width(Length::Fill)
        .max_width(TITLEBAR_SEARCH_MAX_WIDTH)
}

/// Minimum useful visible query span before responsive search
/// collapses. Six characters is enough to recognize and edit a
/// short discriminating search term ("config", "report",
/// "needle"). The shared layout probe measures against six wide
/// `M` glyphs so ordinary Latin queries receive at least this
/// much room.
pub const MIN_VISIBLE_QUERY_CHARACTERS: usize = responsive::MIN_VISIBLE_QUERY_CHARACTERS;

/// Ellipsis strategy applied to the search-field placeholder
/// when it is handed less space than the caller's ideal.
///
/// Sub-asked-space widgets SIGNAL loss, never pretend otherwise: hard-cut
/// placeholder sans trailing `\u{2026}` ≡ badly-typed strings. Visual/AT names
/// split: visuals truncate with `\u{2026}`; accessible names carry wholes.
/// cosmic-text best-effort ellipsising: fits glyphs + `\u{2026}` to widths (or
/// lone `\u{2026}`, or nothing — 20px slots show one-or-zero; zero widths
/// paint nothing). [`placeholder`](EnzSearchField::placeholder) publishes
/// verbatim accessible names, never elided.
pub const PLACEHOLDER_ELLIPSIS: iced::widget::text::Ellipsis = iced::widget::text::Ellipsis::End;

/// Pairs with [`PLACEHOLDER_ELLIPSIS`]. Placeholders are
/// single-line by rule, so wrapping is off and cosmic-text
/// truncates instead of wrapping onto a second row.
pub const PLACEHOLDER_WRAPPING: iced::widget::text::Wrapping = iced::widget::text::Wrapping::None;

/// Placeholder overlay text used by both the enabled and
/// disabled paths: single-line, ellipsised, `\u{2026}`-terminated overlays
/// signalling "there is more" under placeholder-width squeezes (the
/// [`PLACEHOLDER_ELLIPSIS`] framework decision, pinned).
///
/// Stacked over empty-placeholder `text_input`s (iced placeholders sit in
/// infinite-width `Ellipsis::None` boxes: unellipsisable); overlay
/// `Wrapping::None` + `Ellipsis::End` + finite bounds is cosmic-text's exact
/// glyph-trim + `\u{2026}`-append recipe. Valueless-fields-only: typed values
/// switch to `text_input` value rendering. Enabled fields also suppress the
/// placeholder during focused IME composition while preserving the input tree.
pub fn placeholder_overlay<'a, Message: 'a>(
    placeholder: String,
    text_size: f32,
    pinned_palette: Option<Palette>,
    enabled: bool,
) -> Element<'a, Message> {
    placeholder_with_composition(placeholder, text_size, pinned_palette, enabled, None)
}

fn placeholder_with_composition<'a, Message: 'a>(
    placeholder: String,
    text_size: f32,
    pinned_palette: Option<Palette>,
    enabled: bool,
    composition: Option<std::rc::Rc<std::cell::Cell<bool>>>,
) -> Element<'a, Message> {
    iced::widget::text(placeholder)
        .size(text_size)
        .wrapping(PLACEHOLDER_WRAPPING)
        .ellipsis(PLACEHOLDER_ELLIPSIS)
        .align_x(crate::text::TextAlign::Start.into_horizontal())
        .align_y(iced::alignment::Vertical::Center)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |theme: &iced::Theme| {
            let p = pinned_palette
                .clone()
                .unwrap_or_else(|| crate::theme_lookup::palette(theme));
            let base = p.text_muted;
            iced::widget::text::Style {
                color: Some(if composition.as_ref().is_some_and(|active| active.get()) {
                    Color::TRANSPARENT
                } else if enabled {
                    base
                } else {
                    crate::color::with_opacity(base, crate::theme_lookup::opacity(theme).disabled())
                }),
            }
        })
        .into()
}

// Local size-token accessor; matches TextField sizing 1:1.

struct SearchFieldSizeTokens {
    height: f32,
    padding_x: f32,
    text_size: f32,
    icon_size: f32,
}

fn size_tokens(sz: TextFieldSize) -> SearchFieldSizeTokens {
    // Base geometry comes from the shared `TextFieldSize::tokens`
    // resolver so search fields stay 1:1 with `TextField` /
    // `ComboBox`; only `icon_size` is this widget's own ladder.
    let base = sz.tokens();
    let icon_size = responsive::icon_size_for(sz);
    SearchFieldSizeTokens {
        height: base.height,
        padding_x: base.padding_x,
        text_size: base.text_size,
        icon_size,
    }
}

// Builder.

/// Namespace under which the clear-button and stable input ids
/// are derived. Folded with the field's placeholder and a fixed
/// suffix so two fields on one screen do not collide.
pub const DEFAULT_ID_PREFIX: &str = "enz-search-field";

/// Convenience constructor: `enz_search_field(value, on_input)`.
pub fn enz_search_field<'a, Message>(
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> EnzSearchField<'a, Message>
where
    Message: Clone + 'a,
{
    EnzSearchField {
        material: None,
        chrome: SearchFieldChrome::Filled,
        value,
        placeholder: enzui_core::localization::text("enzui-search-field-search"),
        identity_placeholder: "Search…".to_owned(),
        on_input: Box::new(on_input),
        on_clear: None,
        on_submit: None,
        size: TextFieldSize::default(),
        radius: None,
        enabled: true,
        palette: None,
        width: Length::Fill,
        id: None,
        select_all_on_focus: false,
        focus_on_mount: false,
        active_descendant: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        retain_focus: true,
        responsive: true,
        shortcut: DEFAULT_SHORTCUT.to_owned(),
        presentation: SearchPresentation::Inline,
        max_width: None,
        filters: None,
        on_toggle_filters: None,
        results: None,
        results_open: None,
        on_results_open: None,
    }
}

/// Inner search-field paint, independent of its optional material surface.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchFieldChrome {
    /// Ordinary opaque field fill and border.
    #[default]
    Filled,
    /// Transparent at rest, with a hover wash and the ordinary focus ring.
    Ghost,
}

/// Builder returned by [`enz_search_field`].
/// Enabled fields retain focus and the exact caret across ancestry changes by
/// default. Direct UI harnesses need an isolated [`FocusScope`](crate::focus_retention::FocusScope).
#[must_use]
pub struct EnzSearchField<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    chrome: SearchFieldChrome,
    value: &'a str,
    placeholder: String,
    identity_placeholder: String,
    on_input: Box<dyn Fn(String) -> Message + 'a>,
    on_clear: Option<Message>,
    on_submit: Option<Message>,
    size: TextFieldSize,
    radius: Option<f32>,
    enabled: bool,
    palette: Option<Palette>,
    width: Length,
    // Caller-supplied id for the inner `text_input`. `None`
    // leaves it anonymous, so `focus(id)` / `select_all(id)`
    // cannot target this field.
    id: Option<Id>,
    // If `true`, the inner text input selects its whole value on
    // any focus event (click, Tab-in, programmatic `focus(id)`).
    select_all_on_focus: bool,
    // If `true`, the field claims keyboard focus the first frame
    // it is mounted, and never again — fires on the opening
    // transition of an open/close surface, cannot re-fire mid-type.
    // See [`EnzSearchField::focus_on_mount`].
    focus_on_mount: bool,
    // `aria-activedescendant`: the row a combobox host has
    // highlighted inside its popup. Kept on the focused element
    // (this input) because focus stays here during navigation.
    // Threaded per-frame as an `Option` so passing `None` removes
    // the relation the same frame the highlight clears.
    active_descendant: Option<Id>,
    // Namespace prefix for derived ids. Two fields sharing a
    // placeholder under the default prefix derive the same
    // clear-button id; a per-instance prefix keeps both routable.
    id_prefix: String,
    // Keeps focus and caret across a rebuild that discards this
    // field's subtree. On by default; see
    // [`EnzSearchField::retain_focus`].
    retain_focus: bool,
    // Measured compact-icon mode. On by default.
    responsive: bool,
    // Native accessible shortcut published by both the field and
    // its compact trigger.
    shortcut: String,
    // Activation presentation for the compact trigger.
    presentation: SearchPresentation,
    // Optional cap applied after the parent assigns the fill share.
    max_width: Option<f32>,
    // Optional focusable filter/chip strip above results.
    filters: Option<Element<'a, Message>>,
    // Trailing preferences action that toggles the filter surface.
    on_toggle_filters: Option<Message>,
    // Optional result surface. Arbitrary elements preserve rich /
    // multi-line rows; the caller owns listbox / arrow-key
    // semantics.
    results: Option<Element<'a, Message>>,
    // Explicit caller control of the result-surface open state.
    results_open: Option<bool>,
    // Publication of the effective result-surface open state.
    on_results_open: Option<Box<dyn Fn(bool) -> Message + 'a>>,
}

impl<Message> EnzSearchField<'_, Message> {
    /// Derives a collapsed-trigger ID for a field without an explicit input ID.
    ///
    /// `placeholder` is the authored identity seed, which is `Search…` for the
    /// translated default. Use [`Self::collapsed_trigger_id`] when an explicit
    /// field ID is present. The trigger exists only at the collapsed rung.
    pub fn collapsed_trigger_id_for(id_prefix: &str, placeholder: &str) -> Id {
        enzui_core::identity::derive_id(&format!("{id_prefix}-{placeholder}"), "collapsed-trigger")
    }

    /// Returns the collapsed trigger's ID, rooted in the explicit input ID when present.
    pub fn collapsed_trigger_id(&self) -> Id {
        enzui_core::identity::derive_id(&self.identity_namespace(), "collapsed-trigger")
    }

    fn identity_namespace(&self) -> String {
        match &self.id {
            Some(id) => format!("{}-{id:?}", self.id_prefix),
            None => format!("{}-{}", self.id_prefix, self.identity_placeholder),
        }
    }
}

impl<'a, Message> EnzSearchField<'a, Message>
where
    Message: Clone + 'a,
{
    /// Override the placeholder text (default `"Search…"`).
    pub fn placeholder(mut self, s: impl Into<String>) -> Self {
        self.placeholder = s.into();
        self.identity_placeholder.clone_from(&self.placeholder);
        self
    }

    // The semantics this field publishes into the AT tree: a
    // `Role::TextInput` node whose accessible name is the
    // placeholder (a search field carries no separate visual
    // label), plus the current value and disabled state.
    // Published as a leaf around the whole composition via
    // [`crate::semantics_container::SemanticsContainer::leaf`].
    fn semantics(&self) -> Semantics {
        // A search field is focusable / editable, not
        // "activatable": no default (Click) action, which would
        // be unrouted. `editable` declares the editing capability
        // (rather than letting it be inferred from a published
        // value): an empty search field is still a field an AT
        // can type into.
        let mut s = Semantics::new(Role::TextInput)
            .disabled(!self.enabled)
            .editable(true)
            .keyboard_shortcut(self.shortcut.clone());
        // Search fields have no separate visual label; the
        // placeholder is the accessible name.
        if !self.placeholder.is_empty() {
            s = s.label(self.placeholder.clone());
        }
        if !self.value.is_empty() {
            s = s.value(self.value.to_owned());
        }
        s
    }

    /// Enable measured small-screen collapse (default `true`).
    ///
    /// Thresholds are not breakpoint literals: intrinsic usability probes (this
    /// size's search glyph + [`MIN_VISIBLE_QUERY_CHARACTERS`] wide query chars +
    /// clear slots + padding) compare against parent-assigned widths.
    /// Placeholders never probe (they ellipsise). Disable only inside other
    /// search surfaces (command palettes with own compact presentations).
    pub fn responsive(mut self, responsive: bool) -> Self {
        self.responsive = responsive;
        self
    }

    /// Set the keyboard shortcut announced by both the expanded field and the
    /// collapsed icon (default [`DEFAULT_SHORTCUT`]).
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = shortcut.into();
        self
    }

    /// Choose how activation behaves after measured collapse.
    pub fn presentation(mut self, presentation: SearchPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    /// Cap the field width while letting it shrink with its own
    /// slot.
    ///
    /// Capped fills ask parents for bounded fills (`Fill.max(cap)`), never
    /// paint-partial shares: rows allocate ≤ `cap`, returning remainders to
    /// siblings — trailing clusters stay beside followers (never holes).
    /// Sub-cap declarations are ordinary fills: slots shrink fields; collapse
    /// measures against those shares.
    pub fn max_width(mut self, max_width: f32) -> Self {
        self.max_width = max_width.is_finite().then_some(max_width.max(0.0));
        self
    }

    /// Add an optional chip/segment/filter strip above the results.
    ///
    /// Presented by the same surface, and under the same open rule, as
    /// [`Self::results`].
    pub fn filters(mut self, filters: impl Into<Element<'a, Message>>) -> Self {
        self.filters = Some(filters.into());
        self
    }

    /// Add a trailing preferences button which toggles the anchored filters
    /// surface. The button is part of the field chrome and remains in the
    /// measured titlebar allocation instead of becoming an unrelated sibling.
    pub fn on_toggle_filters(mut self, message: Message) -> Self {
        self.on_toggle_filters = Some(message);
        self
    }

    /// Add optional results below the field and filters.
    ///
    /// Arbitrary-element slots (rich multi-line rows unflattened): keyboard /
    /// listbox semantics belong to supplied surfaces. [`EnzSuggestionList`](crate::suggestion_list::EnzSuggestionList)s
    /// fit option-navigation contracts: drive highlights from
    /// [`on_results_open`](Self::on_results_open).
    ///
    /// # When the surface is presented
    ///
    /// * compact + activated — in-popover, with fields.
    /// * in-flow rungs — under-field dropdowns at field widths while
    ///   [`results_open`](Self::results_open) (or by default: focused +
    ///   undismissed); typing re-opens.
    /// * compact + closed — unpresented: square triggers own no visible-owner
    ///   dropdowns.
    ///
    /// # Documented refusal
    ///
    /// Disabled fields present nothing at any rung (focusless, activationless):
    /// the one supplied-elements-hidden configuration;
    /// [`results_open`](Self::results_open) never overrides.
    pub fn results(mut self, results: impl Into<Element<'a, Message>>) -> Self {
        self.results = Some(results.into());
        self
    }

    /// Control filter/result visibility independently of input focus.
    ///
    /// Disabled fields and fields without either slot never present a surface.
    /// An outside press withdraws the surface and blurs the input even when
    /// `open` is true. Refocusing or changing `open` from false to true restores
    /// the caller's rule. A collapsed field still requires activation.
    pub fn results_open(mut self, open: bool) -> Self {
        self.results_open = Some(open);
        self
    }

    /// Publish result-surface visibility changes at every responsive width.
    ///
    /// Outside mouse or touch presses publish `false`, blur the input, and
    /// continue to the pressed destination. Presses inside the field, filters,
    /// or results preserve the surface. Escape retains the existing query policy.
    pub fn on_results_open(mut self, on_open: impl Fn(bool) -> Message + 'a) -> Self {
        self.on_results_open = Some(Box::new(on_open));
        self
    }

    /// Set the size tier (default `Md`). Unpinned expanded-field corners take
    /// the theme's `shape.search_field` step (a pill by default); other
    /// metrics use fixed presets.
    pub fn size(mut self, sz: TextFieldSize) -> Self {
        self.size = sz;
        self
    }

    /// Pin expanded-field corners in logical pixels instead of following the theme.
    /// Negative or non-finite input pins square corners. The compact trigger
    /// remains circular; result surfaces keep their own radius with this as a minimum.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }

    /// Enable/disable the field.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Attach a clear-button message. The clear button only renders
    /// when the field is non-empty and the field is enabled.
    pub fn on_clear(mut self, msg: Message) -> Self {
        self.on_clear = Some(msg);
        self
    }

    /// Fires when the user presses Enter .
    pub fn on_submit(mut self, msg: Message) -> Self {
        self.on_submit = Some(msg);
        self
    }

    /// Pin explicit palette (skip runtime-theme lookup).
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Override the outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Name a `aria-activedescendant` for the current frame.
    ///
    /// Relations belong on focus-holders (inputs), never unfocused popup lists:
    /// focus never leaves inputs during combobox navigation; relations tell AT
    /// which rows are current. `Option<Id>`-passed DELIBERATELY: single-frame
    /// builder `Id`s would stale-name arrowed-past rows. Callers recompute per
    /// frame; unhighlighted/unrealised highlights pass `None` (same-frame
    /// relation removals). Published on enabled + caller-[`id`](Self::id)'d
    /// fields only (focusless disabled; unroutable anonymous).
    pub fn active_descendant(mut self, row_id: Option<Id>) -> Self {
        self.active_descendant = row_id;
        self
    }

    /// Assign a stable `iced::advanced::widget::Id` for
    /// programmatic focus.
    ///
    /// Forwarded into inner `iced::widget::text_input`s:
    /// `operation::focusable::focus` + `operation::text_input::select_all`
    /// targets. Disabled paths render static displays (zero live `TextInput`s):
    /// focus targets don't exist there — disabled fields unfocusable.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Namespace prefix for derived internal ids.
    ///
    /// Namespaces both the clear button and the inner input's
    /// stable fallback id, so [`crate::app_shell::EnzAppShell`]
    /// can retain focus and caret across a replaced ancestor
    /// even without a public routing id. Two fields sharing a
    /// placeholder under the default prefix derive the same
    /// fallback ids; give each a distinct prefix (or explicit
    /// [`Self::id`]) to preserve logical identity.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// When `true`, the inner text input selects its whole value
    /// when it gains focus (click, keyboard Tab, or programmatic
    /// `iced::widget::text_input::State::focus`).
    ///
    /// Default off: refocus-eaten selections (Tab-away-and-back,
    /// caret-repositioning long-query clicks) are obnoxious. Enable for
    /// one-keystroke query-clearing flavours (palette entrypoints, usually-
    /// retyped filter bars). Requires [`id`](Self::id)s: `select_all`
    /// operations need targets — idless flags stay inert.
    pub fn select_all_on_focus(mut self, enabled: bool) -> Self {
        self.select_all_on_focus = enabled;
        self
    }

    /// Keep focus and the exact caret across a rebuild that
    /// discards this field's subtree. On by default; turn it off
    /// only when the caller owns focus movement itself.
    ///
    /// Framework rule: field identities ≠ widget trees. First-character typing
    /// usually flips empty-query/result branches: ancestor tags change,
    /// `Tree::diff` discards subtrees, rebuilt `text_input`s unfocus at caret
    /// zero — second characters go nowhere. Retention records live outside
    /// trees ([`focus_retention`](crate::focus_retention)): new-container wrappers,
    /// parent replacers, [`EnzAppShell`](crate::app_shell::EnzAppShell)-less hosts all
    /// survive. Negative rules there (deliberate moves revoke, stale expires,
    /// focused targets untouched).
    pub fn retain_focus(mut self, enabled: bool) -> Self {
        self.retain_focus = enabled;
        self
    }

    /// Claim keyboard focus the first frame this field is
    /// mounted.
    ///
    /// Shortcut-toggled search bars (Ctrl+F, collapsed-icon taps) should caret
    /// fields immediately: keyboard-only users type sans apps remembering
    /// `.id(id)` + `text_input::focus(id)` reducer-arm pairs in sync.
    ///
    /// # Fires on the opening transition only
    ///
    /// Persistent tree states carry "focused yet" flags. iced creates them once
    /// at first tree appearance, reusing across rerenders: `open`-flag-
    /// conditional rendering focuses appearance frames; visible-state typing /
    /// re-rendering / re-entry never re-focuses (set flags can't mid-type caret-
    /// steal). Shut-toggling drops states (with fields); next opens mount fresh
    /// + refocus.
    ///
    /// # Not for always-visible fields
    ///
    /// Default off. Always-visible fields must never startup-grab focus
    /// pre-search-requests. Set only on explicit-open-action surfaces.
    ///
    /// # Focus target
    ///
    /// [`id`](Self::id)-independent: inner `text_input`s always carry probe ids
    /// (caller-supplied, else [`id_prefix`](Self::id_prefix)+placeholder-derived);
    /// mount-focus aims there. Explicit `.id(..)`s still pay off for
    /// elsewhere-initiated focus moves; unneeded for `focus_on_mount`.
    pub fn focus_on_mount(mut self, enabled: bool) -> Self {
        self.focus_on_mount = enabled;
        self
    }

    /// Host this search field inside a context menu (Cut / Copy
    /// / Paste / Select All is the ordinary set).
    ///
    /// Hosts open on secondary-click/<kbd>Menu</kbd>/<kbd>Shift+F10</kbd>;
    /// viewport clamp/flip-placed; arrow/Home/End-navigated while open;
    /// dismissal refocuses fields. Caller-owned menu models: offered commands
    /// (+ enabled states) depend on clipboard/selection states widgets don't
    /// own. Disabled fields publish inert hosts (uneditable-control commands).
    /// Restoration needs [`id`](Self::id)s: invokerless dismissals restore to
    /// nothing (never arbitrary widgets).
    #[must_use]
    pub fn into_context_host_with(
        self,
        model: &'a enzui_core::menu_model::MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        self.into_context_menu_host(model, state, on_event).into()
    }

    /// The configured
    /// [`EnzContextMenuHost`](crate::context_menu_host::EnzContextMenuHost),
    /// before it is built into an [`Element`]. Returning the
    /// host (rather than an [`Element`]) lets callers assert on
    /// what the host was handed — invoker id, label and disabled
    /// state are all read here.
    pub fn into_context_menu_host(
        self,
        model: &'a enzui_core::menu_model::MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> crate::context_menu_host::EnzContextMenuHost<'a, Message> {
        // Read invoker / enabled / label before the builder is consumed by
        // `Element::from(self)`. Matching text_area's shape.
        let invoker = self.id.clone();
        let enabled = self.enabled;
        let label = (!self.placeholder.is_empty()).then(|| self.placeholder.clone());

        let element: Element<'a, Message> = self.into();
        let mut host =
            crate::context_menu_host::enz_context_menu_host(element, model, state, on_event)
                .enabled(enabled);
        if let Some(invoker) = invoker {
            host = host.invoker(invoker);
        }
        if let Some(label) = label {
            host = host.label(label);
        }
        host
    }
}

impl<'a, Message> EnzSearchField<'a, Message>
where
    Message: Clone + 'a,
{
    /// Choose inner field paint. The default is [`SearchFieldChrome::Filled`].
    /// Ghost omits the ordinary fill and border, retains the hover wash and
    /// focus ring, and can be paired with [`Self::material`]. Unpinned colours
    /// follow the live theme.
    pub fn chrome(mut self, chrome: SearchFieldChrome) -> Self {
        self.chrome = chrome;
        self
    }

    /// Put the field on liquid glass of `style` (see [`crate::glass`]). When
    /// the field collapses to its compact icon, the icon gets a glass circle.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Build the search field and its responsive presentation.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzSearchField<'a, Message>> for Element<'a, Message>
where
    Message: Clone + 'a,
{
    fn from(w: EnzSearchField<'a, Message>) -> Self {
        let tokens = size_tokens(w.size);
        let pinned_input = w.palette.clone();
        let pinned_outer = w.palette.clone();

        // Responsive values must be captured before `on_input` and the
        // optional messages are moved into the expanded field below.
        let responsive_enabled = w.responsive;
        // A cap bounds the fill request rather than shrinking
        // the paint inside a full share: `Length::Fill.max(cap)`
        // makes iced's flex allocate at most `cap` and return
        // the rest to the row, so a right-aligned action cluster
        // stays beside the window controls. Below the cap the
        // length is an ordinary fill, so the compact ladder
        // still measures the share this field is allocated.
        let responsive_width = match w.max_width {
            Some(cap) if w.width.fill_factor() > 0 => w.width.max(cap),
            _ => w.width,
        };
        let responsive_palette = w.palette.clone();
        let responsive_label = w.placeholder.clone();
        let responsive_shortcut = w.shortcut.clone();
        let responsive_active = !w.value.is_empty();
        let responsive_value = w.value.to_owned();
        let responsive_interactive = w.enabled;
        let responsive_presentation = w.presentation;
        let responsive_max_width = w.max_width;

        // AT field nodes pre-`w`-moves (into inputs/closures below).
        //
        // `aria-activedescendant` attaches to FOCUSED field nodes (combobox
        // focused elements), never unfocused ListBox siblings. `w.enabled`-
        // gated (focusless disabled) + `w.id`-gated (`active-descendant`
        // resolves vs focused-node ids; anonymous fields unaddressable via
        // `NodeIdMap`).
        let mut field_semantics = w.semantics();
        if w.enabled
            && w.id.is_some()
            && let Some(row_id) = w.active_descendant.clone()
        {
            field_semantics = field_semantics.active_descendant(row_id);
        }

        // Field nodes publish input-live focus (never constant `false`):
        // `TextFieldSemantics` per-`operate` probes `text_input`s under
        // `probe_id`s for `Focusable` states, republishing observed-focus nodes:
        // AT trees ≡ live focus registries. `probe_id`s must be inner-
        // `text_input`-carried (caller ids, else minted observables); `user_id`s
        // advertise `NodeIdMap`s (minted ids unpublished: caller-unknown ids
        // aren't routing targets).
        let user_id = w.id.clone();
        // This id also lets EnzAppShell recognise the same logical input after
        // an ancestor subtree is discarded. An `Id::unique()` here would make
        // focus-on-mount work within one mount but guarantee a remount could
        // never match the checkpoint. Keep it private (`user_id` below remains
        // `None`) while deriving it stably from the caller-configurable
        // prefix.
        let probe_id = w.id.clone().unwrap_or_else(|| {
            enzui_core::identity::derive_id(&w.id_prefix, &w.identity_placeholder)
        });
        let collapsed_trigger_id = w.collapsed_trigger_id();
        let identity_namespace = w.identity_namespace();

        // Whether to claim focus on first mount, and the target the
        // mount-focus points at (the inner input's probe id, which exists
        // whether or not the caller supplied `.id(..)`). Captured before `w`
        // is partially moved below.
        let focus_on_mount = w.focus_on_mount && w.enabled;
        let mount_focus_target = probe_id.clone();

        // A disabled field has no focus to retain, and a caller who
        // deliberately owns focus movement can opt out.
        let retain_focus = w.retain_focus && w.enabled;

        // --- Prefix: Lucide search icon, tinted text_muted. When the field is
        // disabled it fades with the rest of the field by the theme's
        // disabled opacity, carried in the tint's alpha (the SVG recolour
        // folds it into the glyph's coverage).
        let prefix_pinned = w.palette.clone();
        let prefix_enabled = w.enabled;
        let prefix: Element<'a, Message> = enz_icon("action.search")
            .size(tokens.icon_size)
            .tint_resolved(move |theme| {
                let ink = prefix_pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                    .text_muted;
                if prefix_enabled {
                    ink
                } else {
                    crate::color::with_opacity(ink, crate::theme_lookup::opacity(theme).disabled())
                }
            })
            .into();

        // --- Middle: the text input, or a static display when disabled.
        //
        // Disabled fields are truly inert. Stock iced `text_input`s without
        // `on_input` stay pointer-focusable and selectable, so a disabled field
        // renders static `text` instead (zero `TextInput` widgets: no focus
        // targets, no selections).
        // Advertised `SetValue`s (`.editable(true)` nodes under
        // `semantics::ak` TextInput/PasswordInput/EditableComboBox gates) need
        // carriers or routed AccessKit `SetValue`s resolve to nothing (AT
        // success-reports over unchanged values). Caller `on_input`s wrap in
        // `Rc`s: one closure serves keystrokes (`text_input.on_input(...)`) +
        // routed requests (`TextFieldSemantics::on_input(...)`).
        let on_input: std::rc::Rc<dyn Fn(String) -> Message + 'a> = std::rc::Rc::from(w.on_input);

        let composition = std::rc::Rc::new(std::cell::Cell::new(false));
        let middle: Element<'a, Message> = if w.enabled {
            // Strip horizontal padding here — `FieldChrome`
            // owns `padding_x` for the whole field, so anything
            // below leaves it at zero. Vertical padding is the
            // field height minus a line of text
            // (`FieldSizeTokens::padding_y`) so the input fills
            // the chrome's full height and the click hit region
            // matches the painted border rather than a shorter,
            // centred sub-slot.
            let padding_y = w.size.tokens().padding_y();
            // Inner `text_input`s carry EMPTY placeholders: iced placeholders lay
            // out infinite-width `Ellipsis::None` (caller-knob-unpersuadable).
            // [`placeholder_overlay`] stacks above: squeezed fields truncate
            // `\u{2026}`-suffixed over bare-comma hard-cuts. Typed values unpush
            // overlays: `text_input` value rendering paints (caller-identical
            // appearances).
            let mut input = text_input("", w.value)
                .align_x(crate::text::TextAlign::Start.into_horizontal())
                .size(tokens.text_size)
                .padding(Padding::from([padding_y, 0.0]))
                .width(Length::Fill);
            // Forward `probe_id` to the inner text_input so
            // `text_input::focus(id)` / `select_all(id)` reach
            // it and the `TextFieldSemantics` focus probe below
            // matches it. Caller-supplied id when present,
            // otherwise a minted one; either way the input
            // carries an id so its focus is observable.
            input = input.id(probe_id.clone());
            {
                let typed = std::rc::Rc::clone(&on_input);
                input = input.on_input(move |s| typed(s));
            }
            if let Some(msg) = w.on_submit {
                input = input.on_submit(msg);
            }
            let style_pinned = pinned_input.clone();
            input = input.style(move |theme: &iced::Theme, status: Status| {
                let palette = style_pinned
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                // The outer `FieldChrome` paints background + border for the
                // whole field, so strip both here — leaving them in would
                // draw a second border inside the ring. Foreground colours
                // (value / placeholder / selection) still come from
                // `input_style`.
                Style {
                    background: Background::Color(Color::TRANSPARENT),
                    border: Border::default(),
                    ..input_style(
                        &palette,
                        status,
                        crate::theme_lookup::opacity(theme).disabled(),
                    )
                }
            });
            // `select_all_on_focus` needs a target; the input
            // carries `probe_id`, so the select can point at it.
            // The flag is still gated on a caller-supplied id
            // (`w.id`): selecting the whole value on focus is
            // only meaningful for a field the caller deliberately
            // made addressable.
            let input_element: Element<'a, Message> = match (w.select_all_on_focus, w.id.clone()) {
                (true, Some(id)) => Element::from(
                    select_all_on_focus_wrapper::SelectAllOnFocus::new(input.into(), id),
                ),
                _ => input.into(),
            };
            // Ellipsised-placeholder overlays on empty fields. Slots persist
            // while placeholders configure: overlays carry empty strings
            // post-typing (never `Stack`-disappearing). Load-bearing: first-
            // keystroke `Stack` drops would re-ancestor inputs
            // (`TextFieldSemantics` child `Stack` → `TextInput`) — `Tree::diff`
            // discards inputs; fields lose focus+carets pre-caller-involvement.
            if !w.placeholder.is_empty() {
                let overlay = placeholder_with_composition::<Message>(
                    if w.value.is_empty() {
                        w.placeholder.clone()
                    } else {
                        String::new()
                    },
                    tokens.text_size,
                    pinned_input,
                    true,
                    Some(std::rc::Rc::clone(&composition)),
                );
                // The `Stack` must report `Length::Fill` so its
                // parent `Row` gives it the flex share the inner
                // `text_input` would otherwise have received. If
                // the row treated the stack as `Fit` (its
                // default) the placeholder overlay would measure
                // against the text_input's intrinsic width, not
                // its allocated width, and crop the placeholder.
                iced::widget::stack![input_element, overlay]
                    .width(Length::Fill)
                    .into()
            } else {
                input_element
            }
        } else {
            // Inert disabled display: static text, dimmed. Placeholder shown
            // (muted) when empty, else the value. The placeholder path
            // routes through the same [`placeholder_overlay`] helper
            // the enabled path uses, so a disabled squeezed field also
            // ellipsises instead of hard-cutting on a bare comma.
            let is_placeholder = w.value.is_empty();
            if is_placeholder {
                placeholder_overlay::<Message>(
                    w.placeholder.clone(),
                    tokens.text_size,
                    pinned_input,
                    false,
                )
            } else {
                let disp_pinned = pinned_input;
                iced::widget::text(w.value.to_owned())
                    .size(tokens.text_size)
                    .wrapping(PLACEHOLDER_WRAPPING)
                    .ellipsis(PLACEHOLDER_ELLIPSIS)
                    .style(move |theme: &iced::Theme| {
                        let p = disp_pinned
                            .clone()
                            .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                        let base = p.text;
                        iced::widget::text::Style {
                            color: Some(crate::color::with_opacity(
                                base,
                                crate::theme_lookup::opacity(theme).disabled(),
                            )),
                        }
                    })
                    .width(Length::Fill)
                    .into()
            }
        };

        // Field semantics nodes publish at input bounds with live focus.
        // Disabled-branch static-`text` middles carry zero `text_input`s:
        // probes find nothing, focus reads `false` — correct inertness.
        // `.on_input(...)` services routed AccessKit `SetValue`s through
        // keystroke-identical `on_input` lines. `TextFieldSemantics` re-checks
        // own `editable` gates (semantics-derived `disabled`/`read_only`):
        // closure-handed disabled search fields refuse routed edits.
        let routed = std::rc::Rc::clone(&on_input);
        let field_node: Element<'a, Message> =
            crate::text_field_semantics::TextFieldSemantics::new(
                middle,
                probe_id.clone(),
                user_id,
                field_semantics,
            )
            .on_input(move |s| routed(s))
            .composition_indicator((w.enabled && !w.placeholder.is_empty()).then_some(composition))
            .into();

        // --- Suffixes: optional filter preferences, then clear.
        let has_text = !w.value.is_empty();
        let show_clear = has_text && w.enabled && w.on_clear.is_some();
        let show_filters = w.enabled && w.filters.is_some() && w.on_toggle_filters.is_some();
        // Row fills the full chrome height so the clickable hit region for the
        // input matches the painted border rather than a shorter, centred
        // sub-slot. Horizontal padding lives on the
        // outer `FieldChrome`, not here — two levels of `padding_x` would
        // double-inset the icon and shrink the input's usable width.
        // A right-to-left locale puts the search icon on the right.
        let mut row = Row::new()
            .mirrored(
                enzui_core::placement::ambient_direction()
                    == enzui_core::placement::Direction::RightToLeft,
            )
            .spacing(6.0)
            .align_y(iced::alignment::Vertical::Center)
            .width(Length::Fill)
            .height(Length::Fill)
            .push(prefix)
            .push(field_node);

        if show_filters {
            let filter_icon = enz_icon("sliders-horizontal").size(tokens.icon_size);
            let msg = w.on_toggle_filters.clone().expect("checked above");
            let pinned_filters = pinned_outer.clone();
            let filter_id = enzui_core::identity::derive_id(&identity_namespace, "filters");
            let active = w.results_open.unwrap_or(false);
            let filter_btn = focus_button(filter_icon, msg)
                .id(filter_id)
                .padding(Padding::from([2.0, 4.0]))
                .semantics(
                    Semantics::new(Role::Button)
                        .label(enzui_core::localization::text(
                            "enzui-search-field-search-preferences",
                        ))
                        .selected(active)
                        .default_action(),
                )
                .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                    let palette = pinned_filters
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                    clear_button_style(&palette, status)
                });
            row = row.push(filter_btn);
        }

        if !w.enabled && has_text {
            // Disabled but with a query: the clear affordance stays visible
            // but faded by the theme's disabled opacity, as an inert × glyph
            // (no focus target), whether or not an `on_clear` handler was
            // attached.
            let clear_pinned = pinned_outer.clone();
            let inert_clear: Element<'a, Message> = enz_icon("action.close")
                .size(tokens.icon_size)
                .tint_resolved(move |theme| {
                    let ink = clear_pinned
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme))
                        .text_muted;
                    crate::color::with_opacity(ink, crate::theme_lookup::opacity(theme).disabled())
                })
                .into();
            row = row.push(inert_clear);
        } else if show_clear {
            let clear_icon = enz_icon("action.close").size(tokens.icon_size);
            let msg = w.on_clear.clone().expect("show_clear requires on_clear");
            let pinned_clear = pinned_outer.clone();
            let clear_id = enzui_core::identity::derive_id(&identity_namespace, "clear");
            let clear_btn = focus_button(clear_icon, msg)
                .id(clear_id)
                .padding(Padding::from([2.0, 4.0]))
                .semantics(
                    Semantics::new(Role::Button)
                        .label(enzui_core::localization::text(
                            "enzui-search-field-clear-search",
                        ))
                        .default_action(),
                )
                .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                    let palette = pinned_clear
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                    clear_button_style(&palette, status)
                });
            row = row.push(clear_btn);
        }

        // Text entry paints its focus ring in both pointer and keyboard modalities.
        let ghost = w.chrome == SearchFieldChrome::Ghost;
        let disabled = !w.enabled;
        let interactive = w.enabled;
        let size = w.size;
        let radius_pin = w.radius;

        let chrome: Element<'a, Message> = FieldChrome::new(
            row,
            tokens.height,
            Padding::from([0.0, tokens.padding_x]),
            move |theme: &iced::Theme, status: ChromeStatus| {
                // The theme's `shape.search_field` step unless the caller
                // pins the corners.
                let radius = radius_pin.unwrap_or_else(|| {
                    enzui_core::metrics::FiniteNonNeg::new_or(
                        crate::theme_lookup::shape_radius(theme, |shape| shape.search_field),
                        size.resolve_radius(theme, None),
                    )
                    .get()
                });
                let palette = pinned_outer
                    .clone()
                    .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                let (rest_bg, rest_border) = if ghost {
                    (iced::Color::TRANSPARENT, iced::Color::TRANSPARENT)
                } else if disabled {
                    let opacity = crate::theme_lookup::opacity(theme).disabled();
                    (
                        crate::color::with_opacity(
                            crate::tokens::field_fill_for(
                                &palette,
                                crate::tokens::FieldVariant::Filled,
                            ),
                            opacity,
                        ),
                        crate::color::with_opacity(palette.field_border, opacity),
                    )
                } else {
                    // The same filled body as every other field.
                    (
                        crate::tokens::field_fill_for(
                            &palette,
                            crate::tokens::FieldVariant::Filled,
                        ),
                        palette.field_border,
                    )
                };

                match status {
                    ChromeStatus::Focused => ChromePaint {
                        background: rest_bg,
                        border_color: palette.focus_ring,
                        border_width: 2.0,
                        radius,
                    },
                    ChromeStatus::Hovered => ChromePaint {
                        background: if ghost {
                            palette.overlay_hover()
                        } else {
                            rest_bg
                        },
                        border_color: if ghost {
                            iced::Color::TRANSPARENT
                        } else {
                            palette.field_border_strong
                        },
                        border_width: if ghost { 0.0 } else { 1.0 },
                        radius,
                    },
                    ChromeStatus::Active | ChromeStatus::Inert => ChromePaint {
                        background: rest_bg,
                        border_color: rest_border,
                        border_width: if ghost { 0.0 } else { 1.0 },
                        radius,
                    },
                }
            },
        )
        .interactive(interactive)
        .into();

        // The responsive owner assigns width; a popover gives the same field a wider slot.
        let outer: Element<'a, Message> =
            iced::widget::container(chrome).width(Length::Fill).into();
        let outer = if focus_on_mount {
            focus_on_mount_wrapper::FocusOnMount::new(outer, mount_focus_target).into()
        } else {
            outer
        };

        // Retention must restore the caret before mount focus can move it to the end.
        let outer = if retain_focus {
            crate::focus_retention::FocusRetention::new(outer, probe_id.clone()).into()
        } else {
            outer
        };

        // Changing material must also remount the retention boundary.
        let (glass_size, glass_pin) = (w.size, w.radius);
        let outer = crate::glass::wrap_resolved(outer, w.material, move |theme| {
            glass_size.resolve_radius(theme, glass_pin)
        });

        // The width cap reports Shrink; glass must read the field's Fill request.
        let outer: Element<'a, Message> = match responsive_max_width {
            Some(cap) => crate::max_width::max_width(outer, cap).into(),
            None => outer,
        };

        // The compact trigger is a real focusable surface owned
        // by the responsive wrapper. Its visual child is
        // decorative; the wrapper publishes one Button node with
        // the same accessible name and native keyboard shortcut
        // as the expanded TextInput node.
        let compact_icon: Element<'a, Message> = enz_icon("action.search")
            // Once collapsed this is a titlebar/toolbar affordance, not an
            // inline field prefix. Read the existing icon-scale authority
            // directly; `tokens.icon_size` remains the field-size ladder used
            // by the expanded prefix and usability probe.
            .size(crate::icon::DEFAULT_ICON_SIZE)
            .tint_resolved({
                let palette = responsive_palette.clone();
                move |theme| {
                    let palette = palette
                        .clone()
                        .unwrap_or_else(|| crate::theme_lookup::palette(theme));
                    let toolbar = crate::toolbar_icon::ToolbarIconTokens::from_palette(&palette);
                    if responsive_active {
                        toolbar.active_color
                    } else {
                        toolbar.icon_color
                    }
                }
            })
            .into();
        let compact_icon = match w.material {
            Some(style) => {
                let side = responsive::ResponsiveSearch::<Message>::compact_control_size();
                crate::glass::enz_glass(iced::widget::container(compact_icon).center(side))
                    .style(style)
                    .radius(side / 2.0)
                    .into()
            }
            None => compact_icon,
        };
        let trigger_semantics = Semantics::new(Role::Button)
            .label(responsive_label)
            .keyboard_shortcut(responsive_shortcut)
            .selected(responsive_active)
            .default_action();

        // Layout-only intrinsic usability probe: the shared
        // responsive-search seam measures search glyph + six
        // wide query characters + clear glyph + padding, never
        // the whole placeholder. Sharing the helper keeps the
        // threshold identical across every consumer.
        let probe = responsive::usability_probe(
            tokens.icon_size,
            tokens.text_size,
            tokens.padding_x,
            tokens.height,
        );

        responsive::ResponsiveSearch::new(
            outer,
            compact_icon,
            probe,
            responsive_width,
            responsive_interactive,
            responsive_enabled,
            responsive_active,
            responsive_value,
            probe_id,
            collapsed_trigger_id,
            trigger_semantics,
            responsive_palette,
            w.size,
            w.radius,
            responsive_presentation,
            responsive::ResultSurface {
                filters: w.filters,
                results: w.results,
                open: w.results_open,
                on_open: w.on_results_open,
            },
        )
        .into()
    }
}

mod focus_on_mount_wrapper;
mod select_all_on_focus_wrapper;

// TextInput style: mirrors `text_field`'s palette wiring but
// strips the outer border/bg (those are owned by the
// surrounding container).

// The text-field family's input colours: `accent_soft` selection, the
// full `text_muted` placeholder in every enabled state (fading it on focus
// dropped it to 2.5-4.3:1), and the theme's disabled opacity on the value.
fn input_style(palette: &Palette, status: Status, disabled_opacity: f32) -> Style {
    let disabled = matches!(status, Status::Disabled);
    Style {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: palette.text_muted,
        placeholder: if disabled {
            crate::color::disabled_text_ink(palette, palette.surface)
        } else {
            palette.text_muted
        },
        value: if disabled {
            crate::color::with_opacity(palette.text, disabled_opacity)
        } else {
            palette.text
        },
        selection: palette.accent_soft,
    }
}

// Trailing-"clear"-affordance styles: transparent ghost buttons carrying
// x glyphs. `background: None` paints straight onto field `surface`s: inks must
// beat white-on-light (WCAG AA 4.5:1) — `text_muted` measures 7.17:1 dark /
// 5.25:1 light. Visible on future text labels; svg glyphs tint from
// `svg::Style` ignoring enclosing-button `text_color`s.
fn clear_button_style(palette: &Palette, status: FocusStatus) -> ButtonStyle {
    let focus_w = crate::tokens::ButtonTokens::from_palette(palette).focus_ring_width;
    let border = if matches!(status, FocusStatus::Focused) {
        Border {
            color: palette.focus_ring,
            width: focus_w,
            radius: 4.0.into(),
        }
    } else {
        Border::default()
    };
    ButtonStyle {
        background: None,
        text_color: palette.text_muted,
        border,
        ..Default::default()
    }
}
