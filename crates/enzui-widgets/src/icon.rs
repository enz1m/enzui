//! [`enz_icon`] — bundled Lucide icon widget.
//!
//! Bundled Lucide SVG through iced's `svg` widget, optional colour tint. Bytes
//! embedded at compile time by [`enzui_icons`] (byte-identical manifest:
//! same Lucide pin, same aliases like `action.search`).
//!
//! ##
//!
//! - Canonical names (`search`, `panel-left`) and semantic aliases
//!   (`action.search`, `navigation.sidebar_left`) both resolve.
//! - Unknown names render the bundled `fallback` glyph + one-time warning
//!   (never silently blank).
//! - Rasterisation via iced's `resvg` svg widget; tint is `Svg::style` with
//!   `Style { color: Some(tint) }`.
//!
//! ## A status-reactive tint needs [`tint_with_status`](EnzIcon::tint_with_status)
//!
//! [`tint_resolved`](EnzIcon::tint_resolved) sees only `&Theme`: structurally
//! blind to hover/press. A button glyph brightening on hover must use
//! [`tint_with_status`](EnzIcon::tint_with_status) against the same
//! `FocusStatus` cell the button's `style_focus` writes. Per-status tint
//! tokens without that routing stay INERT (fill moves, glyph doesn't —
//! indistinguishable from the change not landing). See [`crate::toolbar_icon`]
//! (`hover_color`/`pressed_color` wired this way).
//!
//! ## Handle cache
//!
//! `svg::Handle::from_memory` takes `Cow<'static, [u8]>`: embedded bytes go in
//! as `Borrowed` (zero alloc/copy). The handle is a cheap Arc-enum, so
//! per-frame reconstruction is fine — the renderer caches pixels by handle id.

use enzui_icons::bundled_icon_svg;
use enzui_theme::{ContrastFloor, Palette};
use iced::widget::{image as iced_image, svg};
use iced::{Color, ContentFit, Element, Length};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Default icon size in logical pixels: [`ICON_SM`], 16 px.
///
/// `icon-sm` because icons live almost exclusively in chrome (menu/list rows,
/// toolbar buttons), where a hairline glyph beats the 20 px control slot.
/// Bigger glyphs via [`size`](EnzIcon::size) or the ramp (`ICON_XXS`=10 …
/// `ICON_XXL`=48), matching `SizingTokens::{icon_xxs..icon_xxl}`.
pub const DEFAULT_ICON_SIZE: f32 = 16.0;

/// Extra-extra-small icon size, 10 px (smallest role — inline status pips).
pub const ICON_XXS: f32 = 10.0;
/// Extra-small icon size, 12 px (dense list affordances).
pub const ICON_XS: f32 = 12.0;
/// Small icon size, 16 px.
pub const ICON_SM: f32 = 16.0;
/// Medium icon size, 20 px.
pub const ICON_MD: f32 = 20.0;
/// Large icon size, 24 px.
pub const ICON_LG: f32 = 24.0;
/// Extra-large icon size, 32 px.
pub const ICON_XL: f32 = 32.0;
/// Extra-extra-large icon size, 48 px (detail/preview hero icons).
pub const ICON_XXL: f32 = 48.0;

/// Counts the number of `svg::Handle` constructions (calls to
/// `Handle::from_memory`). Used to prove that repeated lookups reuse the
/// cached handle rather than re-creating one. Mirrors the
/// `SEMANTICS_VALUE_MATERIALIZATIONS` pattern in `text_area.rs`.
#[doc(hidden)]
pub static HANDLE_CONSTRUCTIONS: AtomicUsize = AtomicUsize::new(0);

/// Read [`HANDLE_CONSTRUCTIONS`].
#[doc(hidden)]
pub fn handle_constructions() -> usize {
    HANDLE_CONSTRUCTIONS.load(Ordering::Relaxed)
}

/// Reset [`HANDLE_CONSTRUCTIONS`] and return its previous value.
#[doc(hidden)]
pub fn reset_handle_constructions() -> usize {
    HANDLE_CONSTRUCTIONS.swap(0, Ordering::Relaxed)
}

/// Process-wide cache of canonical-name → `svg::Handle`, so repeated
/// construction of the same icon reuses the `Handle` (avoiding repeated
/// full-SVG hashing and `Arc` allocation inside iced).
static HANDLE_CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, svg::Handle>>> =
    OnceLock::new();

fn handle_cache() -> &'static std::sync::Mutex<HashMap<&'static str, svg::Handle>> {
    HANDLE_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// Suffix asking for a bundled outline icon as a solid silhouette
/// (`"play.fill"`, `"heart.fill"`), the SF Symbols convention. Suits glyphs
/// that read as one shape; an icon with inner detail fills to a blob.
pub const FILLED_SUFFIX: &str = ".fill";

/// Filled variants, keyed by the base icon's canonical name.
static FILLED_CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, svg::Handle>>> =
    OnceLock::new();

fn filled_cache() -> &'static std::sync::Mutex<HashMap<&'static str, svg::Handle>> {
    FILLED_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// The outline SVG with its root fill set to the stroke colour, so every
/// closed shape paints solid under the same rounded stroke.
fn filled_svg(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    text.replacen("fill=\"none\"", "fill=\"currentColor\"", 1)
        .into_bytes()
}

/// Handle for the fallback glyph, resolved once.
static FALLBACK_HANDLE: OnceLock<svg::Handle> = OnceLock::new();

fn fallback_handle() -> svg::Handle {
    FALLBACK_HANDLE
        .get_or_init(|| {
            HANDLE_CONSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
            let (_name, bytes) =
                bundled_icon_svg("fallback").expect("fallback icon must be bundled");
            svg::Handle::from_memory(Cow::Borrowed(bytes))
        })
        .clone()
}

/// Resolve `name` to an `svg::Handle`. Cached per canonical name so
/// repeated construction of the same icon reuses the handle without
/// re-hashing the full SVG or re-allocating an `Arc`.
///
/// If `name` is not registered, returns the fallback glyph and emits a
/// one-time warning per name (bounded, deduplicated, panic-free).
fn resolve_handle(name: &str) -> svg::Handle {
    if let Some(base) = name.strip_suffix(FILLED_SUFFIX)
        && let Some((canonical, bytes)) = bundled_icon_svg(base)
    {
        let mut cache = filled_cache()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        return cache
            .entry(canonical)
            .or_insert_with(|| {
                HANDLE_CONSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
                svg::Handle::from_memory(Cow::Owned(filled_svg(bytes)))
            })
            .clone();
    }
    match bundled_icon_svg(name) {
        Some((canonical, bytes)) => {
            let mut cache = handle_cache()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            cache
                .entry(canonical)
                .or_insert_with(|| {
                    HANDLE_CONSTRUCTIONS.fetch_add(1, Ordering::Relaxed);
                    svg::Handle::from_memory(Cow::Borrowed(bytes))
                })
                .clone()
        }
        None => {
            warn_unknown_once(name);
            fallback_handle()
        }
    }
}

/// Crate-internal: the [`svg::Handle`] that [`enz_icon`] would render for
/// `name`. Lets consumer widgets assert (in tests) that they wire a specific
/// icon — comparing `id()` against this proves the *handle*, not mere
/// presence of some element. Unknown names resolve to the fallback glyph
/// (see [`fallback_handle`]).
#[doc(hidden)]
pub(crate) fn handle_for(name: &str) -> svg::Handle {
    resolve_handle(name)
}

// Global one-time unknown-name warner: process-wide, so one warning per icon
// name total.
use std::sync::Mutex;

/// Maximum number of unique unknown icon names tracked before the
/// diagnostic set stops growing. Past the cap, new unknowns still render
/// fallback but are not retained and warn nothing — bounding the diagnostic
/// path's memory (data-generated names cannot grow it unboundedly). Generous
/// deliberately: steady state is a handful of typos; 1024 covers pathological
/// apps under a page of memory.
const UNKNOWN_NAMES_CAP: usize = 1024;

/// Process-wide set of unknown icon names already warned about, so each
/// unique typo reports exactly once (no per-render log spam). Module scope so
/// [`reset_unknown_warnings`] can clear it (process-wide and resettable).
/// Bounded by [`UNKNOWN_NAMES_CAP`]. Locks use
/// `unwrap_or_else(|e| e.into_inner())`: a poisoned diagnostic mutex degrades,
/// never panics — diagnostics must not take down rendering.
static UNKNOWN_SEEN: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();

fn unknown_seen() -> &'static Mutex<std::collections::HashSet<String>> {
    UNKNOWN_SEEN.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn warn_unknown_once(name: &str) {
    // Decide inside the lock whether to warn, but emit outside the lock
    // so I/O cannot hold the mutex.
    let should_warn = {
        let mut set = unknown_seen()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if set.len() >= UNKNOWN_NAMES_CAP {
            // Bounded: stop growing. Don't warn for names past the cap.
            false
        } else {
            set.insert(name.to_owned())
        }
    };
    if should_warn {
        log::warn!(
            target: "enzui::icon",
            "unknown enz_icon name {name:?} — rendering the fallback glyph. \
             Check the spelling/alias against the registry \
             (see enzui_icons::bundled_icons)."
        );
    }
}

/// Returns whether `name` has already been recorded as an unknown-name
/// warning. Test-only helper so dedup behaviour can be asserted.
#[doc(hidden)]
pub fn unknown_was_warned(name: &str) -> bool {
    unknown_seen()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .contains(name)
}

/// Reset the internal one-time unknown-name warning tracker.
///
/// Test-only; production code should not need this. Exposed so unit
/// tests can assert warning behaviour deterministically. Actually clears
/// the recorded-name set (the OnceLock stays occupied; the `HashSet`
/// inside is emptied).
#[doc(hidden)]
pub fn reset_unknown_warnings() {
    unknown_seen()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
}

/// How to colour an [`EnzIcon`] SVG.
#[derive(Debug, Clone, Copy, PartialEq)]
enum TintMode {
    /// Default: pull `Palette::text_muted` from the runtime theme every
    /// render.
    ThemeMuted,
    /// Pull `Palette::accent` from the runtime theme every render.
    ThemeAccent,
    /// Pull a semantic tone colour from the runtime theme every render.
    ThemeTone(IconTone),
    /// Resolve subtle ink against the surface the icon lands on, using the
    /// caller-selected WCAG contrast floor.
    ThemeSubtleOn {
        backdrop: Color,
        floor: ContrastFloor,
    },
    /// Resolve disabled ink against the opaque surface actually behind it.
    ThemeDisabledOn { backdrop: Color },
    /// Resolve a button's foreground colour from the runtime theme every
    /// render, so an icon inside a button re-tints on dark/light switch in
    /// lock-step with the label; a build-time tint cannot track a runtime theme.
    ThemeButtonFg { role: ButtonFgRole, enabled: bool },
    /// Caller pinned an explicit colour (`.tint(color)`).
    Explicit(Color),
    /// Caller opted out (`.no_tint()`) — keep the SVG's native colours.
    Native,
}

/// A semantic tone an icon can follow, resolved from the runtime theme so the
/// tint re-themes on dark/light switch (e.g. the link external-link glyph
/// matching link tone). Covers the full text-ink ladder
/// (`Text`/`Muted`/`Subtle`) + status fills
/// (`Accent`/`Success`/`Warning`/`Danger`) + `Disabled`: any label-toning
/// widget can match its icon instead of approximating.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconTone {
    /// `Palette::accent`.
    Accent,
    /// `Palette::text` (primary foreground).
    Text,
    /// `Palette::text_muted`.
    Muted,
    /// `Palette::text_subtle` — the faintest ink tier.
    Subtle,
    /// `Palette::destructive`.
    Danger,
    /// `Palette::success`.
    Success,
    /// `Palette::warning`.
    Warning,
    /// `Palette::disabled` — greyed-out state.
    Disabled,
}

/// The ink an icon painted with `tone` takes under `palette`.
///
/// The entire [`ThemeTone`](TintMode::ThemeTone) arm of
/// [`into_element`](EnzIcon::into_element)'s style closure — factored out (the
/// [`button_fg_ink`] shape) so the tone→token table measures directly instead
/// of duplicating in a test. Status tones read FILL tokens
/// (`accent`/`success`/`warning`/`destructive`) — what a glyph stroke wants;
/// tone-coloured TEXT reads the audited `tone_*_fg` foregrounds via
/// [`TextTokens`](crate::enz_text::TextTokens).
pub(crate) fn tone_ink(tone: IconTone, palette: &Palette) -> Color {
    match tone {
        IconTone::Accent => palette.accent,
        IconTone::Text => palette.text,
        IconTone::Muted => palette.text_muted,
        IconTone::Subtle => palette.text_subtle,
        IconTone::Danger => palette.destructive,
        IconTone::Success => palette.success,
        IconTone::Warning => palette.warning,
        IconTone::Disabled => palette.disabled,
    }
}

/// Backdrop-resolved disabled icon ink. Factored from the render closure so
/// gates exercise the exact policy the SVG painter consumes.
fn disabled_tone_ink(palette: &Palette, backdrop: Color) -> Color {
    palette.disabled_on_with_preferred(palette.disabled, backdrop)
}

/// Which button foreground token an in-button icon follows. Mirrors the
/// per-kind label colour so the icon tint tracks the runtime theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonFgRole {
    /// Primary button label colour (`text`).
    Primary,
    /// Secondary / outlined / plain button label colour (`text`).
    Secondary,
    /// Ghost button label colour (`accent`).
    Ghost,
    /// Destructive button label colour — the ink chosen by measured contrast
    /// against the `destructive` fill ([`crate::tokens::ink_for`]), so it is
    /// legible on a pale danger colour instead of assuming white.
    Destructive,
    /// Accent ("suggested") button label colour — the ink chosen by measured
    /// contrast against the accent fill ([`crate::tokens::accent_fg`]), so the
    /// glyph tracks the label instead of assuming white.
    Accent,
    /// Success ("positive confirmation") button label colour — the ink chosen
    /// by measured contrast against the `success` fill
    /// ([`crate::tokens::ink_for`]), so the glyph is legible on a pale green
    /// instead of assuming white.
    Success,
    /// Outlined-destructive button label colour — the danger tone itself
    /// (`destructive`), painted as ink on a transparent fill, matching the
    /// low-emphasis danger border.
    DestructiveOutlined,
    /// Borderless-destructive button label colour — the
    /// contrast-audited danger *text* role (`tone_error_fg` /
    /// [`crate::tokens::ButtonTokens::quiet_destructive_fg`]), painted as ink
    /// on a transparent fill with no border, so a `QuietDestructive` button's
    /// glyph reads legibly on the app surface rather than as the raw chip fill.
    QuietDestructive,
}

impl From<crate::button::ButtonKind> for ButtonFgRole {
    /// Map a [`ButtonKind`](crate::button::ButtonKind) to the foreground role
    /// its in-button icon follows — the same mapping as the private
    /// `button_fg_role()` in `button.rs`, exposed here so composed controls
    /// (e.g. `EnzButtonGroup`) can derive the correct icon tint for an
    /// arbitrary kind without duplicating the table.
    fn from(kind: crate::button::ButtonKind) -> Self {
        use crate::button::ButtonKind;
        match kind {
            ButtonKind::Primary => Self::Primary,
            ButtonKind::Secondary
            | ButtonKind::Outlined
            | ButtonKind::Plain
            | ButtonKind::Quiet => Self::Secondary,
            ButtonKind::Ghost => Self::Ghost,
            ButtonKind::Destructive => Self::Destructive,
            ButtonKind::Accent => Self::Accent,
            ButtonKind::Success => Self::Success,
            ButtonKind::DestructiveOutlined => Self::DestructiveOutlined,
            ButtonKind::QuietDestructive => Self::QuietDestructive,
        }
    }
}

/// A runtime tint resolver: given the current `iced::Theme`, returns the
/// colour to tint the icon with this frame. Used by widgets that own a token
/// set icon.rs doesn't know about (badge tone fg, tab pill label, …) so the
/// icon re-tints on light/dark/custom switch in lock-step with that widget.
type TintResolver = std::rc::Rc<dyn Fn(&iced::Theme) -> Color + 'static>;

use crate::focus_button::FocusStatus;
use std::cell::Cell;
use std::rc::Rc;

/// A runtime tint resolver that also receives the live
/// [`FocusStatus`] from the parent button, so the icon can re-tint on
/// hover/pressed/disabled transitions — the exact gap this module's
/// theme-only resolvers cannot bridge.
type StatusTintResolver = std::rc::Rc<dyn Fn(&iced::Theme, FocusStatus) -> Color + 'static>;

/// Builder returned by [`enz_icon`].
#[must_use]
pub struct EnzIcon<'a> {
    handle: svg::Handle,
    size: f32,
    tint: TintMode,
    /// When set, overrides `tint` with a per-render resolver.
    tint_resolver: Option<TintResolver>,
    /// When set, overrides `tint` and `tint_resolver` with a status-aware
    /// resolver that reads the parent button's live [`FocusStatus`] from a
    /// shared cell.
    status_tint: Option<(Rc<Cell<FocusStatus>>, StatusTintResolver)>,
    opacity: f32,
    _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Create an EnzIcon by bundled Lucide name (canonical or semantic alias).
///
/// Unknown names render the fallback glyph (with a one-time `log` warning
/// under `enzui::icon`). Never returns silently blank. A directional semantic name
/// (`navigation.back`, `navigation.forward`, `navigation.sidebar_left`)
/// mirrors in a right-to-left locale; see [`enz_icon_for_direction`] for a
/// widget with its own direction override.
pub fn enz_icon<'a>(name: &str) -> EnzIcon<'a> {
    enz_icon_for_direction(name, enzui_core::placement::ambient_direction())
}

/// The icon name to draw `name` with in `direction`: a directional semantic
/// name becomes its canonical glyph for that direction (`navigation.forward`
/// is `chevron-left` right-to-left and `chevron-right` left-to-right), so the
/// result no longer depends on the ambient locale. Other names pass through.
pub fn icon_name_for_direction(name: &str, direction: enzui_core::placement::Direction) -> &str {
    let Some(rtl) = enzui_icons::rtl_icon_name(name) else {
        return name;
    };
    match direction {
        enzui_core::placement::Direction::RightToLeft => rtl,
        enzui_core::placement::Direction::LeftToRight => {
            enzui_icons::canonical_icon_name(name).unwrap_or(name)
        }
    }
}

/// [`enz_icon`] drawn for an explicit writing direction.
pub fn enz_icon_for_direction<'a>(
    name: &str,
    direction: enzui_core::placement::Direction,
) -> EnzIcon<'a> {
    let name = icon_name_for_direction(name, direction);
    EnzIcon {
        handle: resolve_handle(name),
        size: DEFAULT_ICON_SIZE,
        tint: TintMode::ThemeMuted,
        tint_resolver: None,
        status_tint: None,
        opacity: 1.0,
        _lifetime: std::marker::PhantomData,
    }
}

/// Create an EnzIcon from an explicit [`svg::Handle`] (advanced; skip
/// the bundled registry). For app-owned SVGs.
///
/// Default tint is theme-MUTED (like [`enz_icon`]). App brand marks keeping
/// their colours chain [`no_tint`](EnzIcon::no_tint). Rasters (`.ico`/`.png`)
/// go to [`enz_icon_from_image`] — untinted by design (`image` never recolours
/// pixels like `svg` overrides `currentColor`).
pub fn enz_icon_from_handle<'a>(handle: svg::Handle) -> EnzIcon<'a> {
    EnzIcon {
        handle,
        size: DEFAULT_ICON_SIZE,
        tint: TintMode::ThemeMuted,
        tint_resolver: None,
        status_tint: None,
        opacity: 1.0,
        _lifetime: std::marker::PhantomData,
    }
}

/// Render an app-owned raster icon (`.ico`, `.png`, `.jpg`) with the
/// same size / opacity model as the bundled icon widget.
///
/// The raster counterpart of [`enz_icon_from_handle`]: before it, favicons
/// went through `iced::widget::image` directly, losing `EnzIcon`'s sizing and
/// opacity. UNtinted intentionally (`image` cannot recolour pixels): a
/// text-toned brand mark loses the recognisability it is shown for.
///
/// - `size` → `Length::Fixed(size)` both axes + `ContentFit::Contain` (the
///   [`EnzIcon`] envelope).
/// - `opacity` clamps to `0.0..=1.0`; non-finite reads fully opaque (as
///   [`opacity`](EnzIcon::opacity)).
pub fn enz_icon_from_image<'a, Message: 'a>(
    handle: iced_image::Handle,
    size: f32,
    opacity: f32,
) -> Element<'a, Message> {
    let size = if size.is_finite() { size.max(0.0) } else { 0.0 };
    let opacity = if opacity.is_finite() {
        opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    iced::widget::image(handle)
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .content_fit(ContentFit::Contain)
        .opacity(opacity)
        .into()
}

/// One of three icon sources a widget with a leading icon slot accepts
/// Constructed via `From` so the string-name legacy API stays
/// call-site compatible.
///
/// | source | when to use | tinting |
/// |---|---|---|
/// | [`Named`](IconSource::Named) | bundled Lucide name / semantic alias | tinted to `text_muted` |
/// | [`Svg`](IconSource::Svg) | app-owned SVG (favicon, vendor mark) | untinted |
/// | [`SymbolicSvg`](IconSource::SymbolicSvg) | app-owned monochrome SVG glyph | tinted like a bundled name |
/// | [`Image`](IconSource::Image) | app-owned raster (`.ico`/`.png` favicon) | untinted (pixels) |
///
/// # Tinting rule
///
/// BUNDLED names tint by default (single-colour strokes meant to follow text).
/// APP handles do not (a text-toned brand mark loses recognisability: the
/// favicon stays untinted beside a tinted folder fallback — the same slot).
/// An app-owned glyph that should follow the text, such as a monochrome logo
/// authored white, opts in with [`SymbolicSvg`](IconSource::SymbolicSvg).
///
/// # Accessibility
///
/// Set through a labelled row/expander/menu item: the row's label names, the
/// icon stays decorative (the [`enz_icon`] shape). A labelless app-owned icon
/// must publish its name another way (`.aria_label(..)` on the container) —
/// never on the icon itself, decorative to its parent.
#[derive(Debug, Clone)]
pub enum IconSource {
    /// A bundled Lucide canonical name or semantic alias, resolved
    /// through [`enzui_icons::bundled_icon_svg`]. Tinted by default,
    /// exactly like [`enz_icon`].
    Named(String),
    /// An app-owned SVG handle. Untinted by default (see the tinting
    /// rule); pass through [`enz_icon_from_handle`] with `.no_tint()`
    /// omitted to have it follow the text tone.
    Svg(svg::Handle),
    /// An app-owned monochrome SVG painted like a bundled name: every icon
    /// slot recolours it with the ink it gives bundled icons, so it follows
    /// the slot's tone, hover and light/dark theme. Whatever colour the file
    /// was authored in is replaced.
    SymbolicSvg(svg::Handle),
    /// An app-owned raster handle (`.ico`, `.png`, `.jpg`). Untinted —
    /// the raster path does not recolour pixels.
    Image(iced_image::Handle),
}

impl IconSource {
    /// Whether the source is a bundled Lucide name (`.icon("save")` shape).
    #[must_use]
    pub fn is_bundled(&self) -> bool {
        matches!(self, Self::Named(_))
    }

    /// Whether the source is an app-owned handle (SVG or raster).
    #[must_use]
    pub fn is_app_owned(&self) -> bool {
        matches!(self, Self::Svg(_) | Self::SymbolicSvg(_) | Self::Image(_))
    }

    /// Whether slots recolour the source with their icon ink: a bundled name
    /// or a [`SymbolicSvg`](Self::SymbolicSvg).
    #[must_use]
    pub fn is_symbolic(&self) -> bool {
        matches!(self, Self::Named(_) | Self::SymbolicSvg(_))
    }

    /// Whether the source is the empty bundled name, which slots treat as
    /// "no icon".
    #[must_use]
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Named(name) if name.is_empty())
    }

    /// Render the source at `size` px for an icon slot whose bundled glyphs
    /// use `tint`.
    ///
    /// Symbolic sources (bundled names and [`SymbolicSvg`](Self::SymbolicSvg))
    /// go through `tint`; app-owned artwork renders untinted. Slot widgets
    /// call this so both symbolic kinds always share the slot's ink.
    pub fn into_slot_element<'a, Message: 'a>(
        self,
        size: f32,
        tint: impl FnOnce(EnzIcon<'a>) -> EnzIcon<'a>,
    ) -> Element<'a, Message> {
        match self {
            Self::Named(name) => tint(enz_icon(&name).size(size)).into(),
            Self::SymbolicSvg(handle) => tint(enz_icon_from_handle(handle).size(size)).into(),
            artwork => artwork.into_app_element(size),
        }
    }

    /// Render the source as an `Element` at `size` px, with the default
    /// tint rule (bundled = theme-muted, app-owned = untinted).
    ///
    /// The composition helper every icon-slot widget (`enz_sidebar_row`,
    /// `enz_expander`, `enz_menu_row`, …) calls: one home for the rule.
    /// Non-default bundled tints (sidebar's `tint_resolved(icon_color)`) stay
    /// inline; this helper serves only the app-owned branches there (see
    /// [`into_app_element`](Self::into_app_element)).
    pub fn into_element<'a, Message: 'a>(self, size: f32) -> Element<'a, Message> {
        match self {
            symbolic @ (Self::Named(_) | Self::SymbolicSvg(_)) => {
                symbolic.into_slot_element(size, |icon| icon)
            }
            other => other.into_app_element(size),
        }
    }

    /// Render only the app-owned branch (SVG or raster) at `size` px.
    /// A [`SymbolicSvg`](Self::SymbolicSvg) takes the default muted ink; slots
    /// with their own ink use [`into_slot_element`](Self::into_slot_element).
    /// Panics on [`Self::Named`] — callers use this after they have
    /// resolved the bundled path themselves.
    ///
    /// # Panics
    ///
    /// When called on a bundled name; the widget renders those itself.
    pub fn into_app_element<'a, Message: 'a>(self, size: f32) -> Element<'a, Message> {
        match self {
            #[allow(
                clippy::panic,
                reason = "a bundled name here is a caller bug; the widget handles it itself"
            )]
            Self::Named(_) => panic!(
                "IconSource::into_app_element called on a bundled name; \
                 the widget must handle Named itself (it has its own tint model)"
            ),
            Self::Svg(handle) => enz_icon_from_handle(handle).no_tint().size(size).into(),
            Self::SymbolicSvg(handle) => enz_icon_from_handle(handle).size(size).into(),
            Self::Image(handle) => enz_icon_from_image::<Message>(handle, size, 1.0),
        }
    }
}

impl From<String> for IconSource {
    fn from(s: String) -> Self {
        Self::Named(s)
    }
}

impl From<&str> for IconSource {
    fn from(s: &str) -> Self {
        Self::Named(s.to_owned())
    }
}

impl From<svg::Handle> for IconSource {
    fn from(h: svg::Handle) -> Self {
        Self::Svg(h)
    }
}

impl From<iced_image::Handle> for IconSource {
    fn from(h: iced_image::Handle) -> Self {
        Self::Image(h)
    }
}

impl<'a> EnzIcon<'a> {
    /// Override the icon size in pixels.
    ///
    /// The default is [`DEFAULT_ICON_SIZE`] = 16 px ([`ICON_SM`]), not
    /// 20 px. The icon renders at `size × size`
    /// (square) with `ContentFit::Contain`. Non-finite (`NaN`/`±inf`) and
    /// negative values are sanitized to `0.0`.
    pub fn size(mut self, px: f32) -> Self {
        self.size = if px.is_finite() { px.max(0.0) } else { 0.0 };
        self
    }

    /// Tint the icon a solid colour (symbolic-icon recolour).
    ///
    /// `svg::Style::color` overrides `currentColor` (Lucide paints
    /// `stroke="currentColor"`: every stroke reached). Overrides the
    /// theme-muted default. The tint's alpha scales the glyph's coverage, so
    /// a translucent tint fades the icon; it composes with
    /// [`opacity`](Self::opacity).
    pub fn tint(mut self, color: Color) -> Self {
        self.tint = TintMode::Explicit(color);
        self
    }

    /// Tint the icon using the runtime theme's accent colour.
    pub fn tint_theme_accent(mut self) -> Self {
        self.tint = TintMode::ThemeAccent;
        self
    }

    /// Tint the icon with a colour resolved from the runtime `iced::Theme`
    /// every render via `resolver`. Use when a widget owns a token set that
    /// icon.rs does not know about (e.g. a badge tone foreground) but the
    /// icon must still re-tint on light/dark/custom theme changes in lock-step
    /// with that widget's own colours. Overrides any other tint mode.
    pub fn tint_resolved(mut self, resolver: impl Fn(&iced::Theme) -> Color + 'static) -> Self {
        self.tint_resolver = Some(std::rc::Rc::new(resolver));
        self
    }

    /// Tint the icon with a colour resolved from both the runtime
    /// `iced::Theme` and the parent button's live [`FocusStatus`].
    ///
    /// The status-aware [`tint_resolved`](Self::tint_resolved) counterpart: the
    /// `resolver` sees Idle/Hovered/Pressed/Focused/Disabled every frame — the
    /// gap `tint_resolved` (Theme-only) cannot bridge. The `status` cell must
    /// be the same `Rc<Cell<FocusStatus>>` the parent's `style_focus` writes:
    /// icon and background never disagree on the active status. Overrides both
    /// `tint` and `tint_resolver`.
    pub fn tint_with_status(
        mut self,
        status: Rc<Cell<FocusStatus>>,
        resolver: impl Fn(&iced::Theme, FocusStatus) -> Color + 'static,
    ) -> Self {
        self.status_tint = Some((status, std::rc::Rc::new(resolver)));
        self
    }

    /// Tint the icon to a semantic tone resolved from the runtime theme.
    pub fn tint_theme_tone(mut self, tone: IconTone) -> Self {
        self.tint = TintMode::ThemeTone(tone);
        self
    }

    /// Tint this icon with subtle ink resolved against `backdrop`.
    ///
    /// The backdrop-aware `.tint_theme_tone(Subtle)` counterpart. Keeps the
    /// 4.5:1 text policy via [`Text`](ContrastFloor::Text): every existing call
    /// stays byte-identical. Meaningful-graphical icons (SC 1.4.11) use
    /// [`tint_theme_subtle_on_with_floor`](Self::tint_theme_subtle_on_with_floor)
    /// and [`NonText`](ContrastFloor::NonText). Raw `Subtle` mapping unchanged
    /// for non-opted callers. A later [`tint`](Self::tint) /
    /// [`tint_theme_tone`](Self::tint_theme_tone) replaces this mode;
    /// [`tint_resolved`](Self::tint_resolved) keeps precedence.
    pub fn tint_theme_subtle_on(self, backdrop: Color) -> Self {
        self.tint_theme_subtle_on_with_floor(backdrop, ContrastFloor::Text)
    }

    /// Tint subtle icon ink against `backdrop` under an explicit WCAG contrast floor.
    ///
    /// [`NonText`](ContrastFloor::NonText) for meaningful graphical objects /
    /// component-or-state identifiers: SC 1.4.11 wants 3:1, not text's 4.5:1 —
    /// preserving a weak-vs-strong distinction a primary-text promotion would
    /// erase. Decorative icons have NO requirement: do not opt in to colour a
    /// return. [`LargeText`](ContrastFloor::LargeText) exists for API
    /// consistency (a text classification — SVG icons are not its consumer);
    /// the enum keeps the reason explicit over an unchecked float.
    pub fn tint_theme_subtle_on_with_floor(
        mut self,
        backdrop: Color,
        floor: ContrastFloor,
    ) -> Self {
        self.tint = TintMode::ThemeSubtleOn { backdrop, floor };
        self
    }

    /// Tint an unavailable-control glyph against its actual opaque `backdrop`.
    ///
    /// The backdrop-aware `.tint_theme_tone(Disabled)` counterpart. Keeps the
    /// established disabled token exactly where it satisfies enzUI's 3:1 floor
    /// and half-enabled ceiling, else delegates to the palette's disabled
    /// resolver. The tint is complete: never opacity-dim it again.
    pub fn tint_theme_disabled_on(mut self, backdrop: Color) -> Self {
        self.tint = TintMode::ThemeDisabledOn { backdrop };
        self
    }

    /// Tint the icon to match a button's foreground colour, resolved from the
    /// runtime theme every render so it re-tints on dark/light switch in
    /// lock-step with the button label.
    pub fn tint_button_fg(mut self, role: ButtonFgRole, enabled: bool) -> Self {
        self.tint = TintMode::ThemeButtonFg { role, enabled };
        self
    }

    /// Set the icon opacity in `0.0..=1.0`. Out-of-range values are clamped;
    /// non-finite (`NaN`/`±inf`) values are sanitized to fully opaque (`1.0`).
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        self
    }

    /// Opt out of tinting entirely — render the SVG's native colours.
    ///
    /// Default is the runtime `text_muted` (the `EnzIcon.icon-color` default).
    /// Chain `.no_tint()` for full-colour illustrative SVGs (app logos) that
    /// must not recolour.
    pub fn no_tint(mut self) -> Self {
        self.tint = TintMode::Native;
        self
    }

    /// Build the element.
    pub fn into_element<Message: 'a>(self) -> Element<'a, Message> {
        self.svg().into()
    }

    /// Draw this icon turned a quarter clockwise while `turned` (anticlockwise
    /// in right-to-left text), easing between the two whenever `turned`
    /// changes. Built from a disclosure's collapsed glyph
    /// (`navigation.forward`), it swings down as the section opens. It holds
    /// still under reduced motion.
    pub fn quarter_turn<Message: 'a>(self, turned: bool) -> Element<'a, Message> {
        self.quarter_turn_in(turned, enzui_core::placement::ambient_direction())
    }

    /// [`Self::quarter_turn`] for an explicit writing direction.
    pub fn quarter_turn_in<Message: 'a>(
        self,
        turned: bool,
        direction: enzui_core::placement::Direction,
    ) -> Element<'a, Message> {
        let sign = match direction {
            enzui_core::placement::Direction::RightToLeft => -1.0,
            enzui_core::placement::Direction::LeftToRight => 1.0,
        };
        Element::new(TurningIcon {
            child: self.svg().into(),
            icon: self,
            turned: Turned::Fixed(turned),
            angle: sign * std::f32::consts::FRAC_PI_2,
        })
    }

    /// Draw this icon turned upside down while `turned` reads true, easing
    /// between the two: a dropdown's down chevron points up while its list
    /// is open. The owner sets `turned` from its `update` before passing
    /// events on, so the turn follows state the owner keeps itself.
    pub(crate) fn half_turn_following<Message: 'a>(
        self,
        turned: std::rc::Rc<std::cell::Cell<bool>>,
    ) -> Element<'a, Message> {
        Element::new(TurningIcon {
            child: self.svg().into(),
            icon: self,
            turned: Turned::Shared(turned),
            angle: std::f32::consts::PI,
        })
    }

    /// The tinted SVG widget this icon draws as.
    fn svg(&self) -> svg::Svg<'a, iced::Theme> {
        let tint = self.tint;
        let resolver = self.tint_resolver.clone();
        let status_tint = self.status_tint.clone();
        let size = self.size;
        let opacity = self.opacity;
        svg::Svg::new(self.handle.clone())
            .width(Length::Fixed(size))
            .height(Length::Fixed(size))
            .content_fit(iced::ContentFit::Contain)
            .style(move |theme: &iced::Theme, _status: svg::Status| {
                // A status-aware resolver wins over everything: it
                // receives the parent button's live FocusStatus from the
                // shared cell, so the icon re-tints on hover/pressed/disabled
                // in lock-step with the button's own style_focus.
                if let Some((ref cell, ref r)) = status_tint {
                    return svg::Style {
                        color: Some(r(theme, cell.get())),
                    };
                }
                // A per-render resolver (widget-owned token set) wins over the
                // built-in tint modes.
                if let Some(r) = &resolver {
                    return svg::Style {
                        color: Some(r(theme)),
                    };
                }
                let color = match tint {
                    TintMode::Explicit(c) => Some(c),
                    TintMode::Native => None,
                    TintMode::ThemeMuted => {
                        // Picked per-render from the runtime theme so
                        // dark/light switches recolour icons.
                        let palette = crate::theme_lookup::palette(theme);
                        Some(palette.text_muted)
                    }
                    TintMode::ThemeAccent => {
                        let palette = crate::theme_lookup::palette(theme);
                        Some(palette.accent)
                    }
                    TintMode::ThemeTone(tone) => {
                        let palette = crate::theme_lookup::palette(theme);
                        Some(tone_ink(tone, &palette))
                    }
                    TintMode::ThemeSubtleOn { backdrop, floor } => {
                        let palette = crate::theme_lookup::palette(theme);
                        Some(palette.subtle_on(backdrop, floor))
                    }
                    TintMode::ThemeDisabledOn { backdrop } => {
                        let palette = crate::theme_lookup::palette(theme);
                        Some(disabled_tone_ink(&palette, backdrop))
                    }
                    TintMode::ThemeButtonFg { role, enabled } => {
                        let palette = crate::theme_lookup::palette(theme);
                        Some(button_fg_ink(role, enabled, &palette))
                    }
                };
                svg::Style { color }
            })
            .opacity(opacity)
    }
}

/// How long a disclosure chevron takes to turn, in milliseconds.
const TURN_MS: f32 = 160.0;

/// An icon that eases toward a turn of `angle` while `turned`; see
/// [`EnzIcon::quarter_turn`].
struct TurningIcon<'a, Message> {
    /// The unturned icon: it owns layout, which a floating rotation keeps.
    child: Element<'a, Message>,
    icon: EnzIcon<'a>,
    turned: Turned,
    /// The full turn in radians; positive is clockwise.
    angle: f32,
}

/// Whether a [`TurningIcon`] is turned: fixed when it is built, or read
/// from a cell its owner keeps current.
enum Turned {
    Fixed(bool),
    Shared(std::rc::Rc<std::cell::Cell<bool>>),
}

impl Turned {
    fn get(&self) -> bool {
        match self {
            Turned::Fixed(turned) => *turned,
            Turned::Shared(turned) => turned.get(),
        }
    }
}

struct TurnState {
    turn: crate::motion::Swell,
    frame_at: Option<iced::time::Instant>,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for TurningIcon<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<TurnState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(TurnState {
            turn: crate::motion::Swell::snap(self.turned.get()),
            frame_at: None,
        })
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        _layout: iced::advanced::Layout,
        _cursor: iced::mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &iced::Rectangle,
    ) {
        let state = tree.state.downcast_mut::<TurnState>();
        let TurnState { turn, frame_at } = state;
        turn.track(frame_at, event, self.turned.get(), TURN_MS, shell);
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        let state = tree.state.downcast_ref::<TurnState>();
        let turn = state.turn.paint(state.frame_at, TURN_MS, theme);
        let svg = self
            .icon
            .svg()
            .rotation(iced::Rotation::Floating(iced::Radians(self.angle * turn)));
        iced::advanced::Widget::<Message, iced::Theme, iced::Renderer>::draw(
            &svg,
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
}

/// The glyph ink an in-button icon uses for `role` under `palette`.
///
/// The entire [`ThemeButtonFg`](TintMode::ThemeButtonFg) arm of
/// [`into_element`](EnzIcon::into_element)'s style closure — factored out for
/// direct measurement instead of test duplication. Measured arms vs their
/// fills:
///
/// | role | fill | dark | light |
/// |---|---|---|---|
/// | `Primary` | `button_primary_bg` | 11.80:1 | 12.14:1 |
/// | `Secondary` | `surface` | 13.29:1 | 13.57:1 |
/// | `Destructive` | `destructive` | was 2.77:1 (white), now 7.59:1 | 5.79:1 |
/// | `Accent` | `accent` | was 3.65:1 (white), now 5.76:1 | 5.17:1 |
///
/// `Primary`/`Secondary` audited and left: `text` on near-neutral fills clears
/// AA widely on both palettes. `Ghost` has no fill (transparent) — inherits
/// its surface, nothing to measure against.
pub(crate) fn button_fg_ink(role: ButtonFgRole, enabled: bool, palette: &Palette) -> Color {
    if !enabled {
        return palette.disabled;
    }
    match role {
        ButtonFgRole::Primary => palette.text,
        ButtonFgRole::Secondary => palette.text,
        ButtonFgRole::Ghost => palette.accent,
        ButtonFgRole::Destructive => palette.filled_color(palette.destructive).text,
        ButtonFgRole::Accent => crate::tokens::accent_fg(palette),
        // success glyph is measured against the success fill (legible on
        // a pale green); outlined-destructive glyph is the danger tone itself
        // (ink on a transparent fill), matching its danger border.
        ButtonFgRole::Success => palette.filled_color(palette.success).text,
        ButtonFgRole::DestructiveOutlined => palette.tone_error_fg,
        // borderless-danger glyph is the contrast-audited danger text
        // role, matching `kind_visual`'s `quiet_destructive_fg` ink.
        ButtonFgRole::QuietDestructive => palette.tone_error_fg,
    }
}

/// Status-aware counterpart of [`button_fg_ink_from_tokens`].
///
/// Only `Accent` varies by status: shipped palettes brighten accent on hover,
/// so resting white fails AA on the hover fill — each state carries measured
/// ink. All other roles collapse to [`button_fg_ink_from_tokens`]. An accent
/// button's ICON re-tints in lock-step with per-state label ink instead of
/// holding resting tone on hover.
pub(crate) fn button_fg_ink_from_tokens_for_status(
    role: ButtonFgRole,
    enabled: bool,
    tokens: &crate::tokens::ButtonTokens,
    status: crate::focus_button::FocusStatus,
) -> Color {
    use crate::focus_button::FocusStatus;
    if !enabled {
        return tokens.disabled_fg;
    }
    match role {
        ButtonFgRole::Primary => tokens.primary_fg,
        ButtonFgRole::Secondary => tokens.secondary_fg,
        // per-status foreground so a ghost button's icon tracks
        // the same hover/pressed ink the label paints with.
        ButtonFgRole::Ghost => match status {
            FocusStatus::Hovered => tokens.ghost_hover_fg,
            FocusStatus::Pressed => tokens.ghost_pressed_fg,
            _ => tokens.ghost_fg,
        },
        ButtonFgRole::Destructive => tokens.destructive_fg,
        ButtonFgRole::Accent => match status {
            FocusStatus::Hovered => tokens.accent_hover_fg,
            FocusStatus::Pressed => tokens.accent_pressed_fg,
            _ => tokens.accent_fg,
        },
        // mirror `kind_visual`'s Success/DestructiveOutlined foregrounds
        // so a pinned button's glyph reads byte-for-byte what the label paints.
        ButtonFgRole::Success => tokens.success_fg,
        ButtonFgRole::DestructiveOutlined => tokens.quiet_destructive_fg,
        ButtonFgRole::QuietDestructive => tokens.quiet_destructive_fg,
    }
}

impl<'a, Message: 'a> From<EnzIcon<'a>> for Element<'a, Message> {
    fn from(icon: EnzIcon<'a>) -> Self {
        icon.into_element()
    }
}
