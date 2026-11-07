//! Resolve palettes and scales from the live iced theme.
//!
//! Themes produced by `EnzTheme::iced_theme()` retain their configured scales.
//! Stock iced themes use default scales and a palette derived from iced.
//! An explicit component token override is a snapshot; `resolve_pinned` gives
//! it precedence over live palette derivation.

use enzui_theme::{
    ElevationScale, MaterialPolicy, MotionScale, Palette, RadiusScale, ScrollbarTokens,
    SizingTokens, SpacingScale, StrokeScale, TypographyScale,
};
use iced::Theme;

/// Resolve the enzUI [`Palette`] for the runtime `theme`.
///
/// Derives from the theme's (extended) palette via [`Palette::from_iced`], so
/// Dracula/Nord/custom themes drive real hues instead of snapping to a fixed
/// dark/light preset.
#[inline]
pub fn palette(theme: &Theme) -> Palette {
    Palette::from_iced(theme)
}

/// Resolve the runtime theme's elevation drop-shadow colour.
///
/// This is the draw-time channel for the shadow ink cast by the elevation
/// ladder. A hardcoded `is_dark`-selected constant in
/// `elevation::elevation_shadow_color` would let a custom `EnzTheme` retune
/// shadow *geometry* but never its *colour*: a warm dark background would get
/// neutral black and a brand wanting a tinted shadow would have no route. The
/// colour lives on the palette as [`Palette::shadow`], resolved here from the *actual* runtime
/// palette (which round-trips byte-for-byte through the extension payload for
/// an `EnzTheme`, and falls back to the structural preset's dark/light default
/// for a stock iced theme). Call this
/// from inside a `container::Style` closure so it follows a live theme switch.
#[inline]
pub fn shadow_color(theme: &Theme) -> iced::Color {
    Palette::from_iced(theme).shadow
}

/// Resolve a component's `*Tokens` from the runtime `theme` by deriving the
/// enzUI palette (via [`Palette::from_iced`]) and applying the token type's
/// `from_palette` constructor.
///
/// This is the correct replacement for [`dark_or_light`]: instead of
/// picking a fixed `T::dark`/`T::light` preset off a single `is_dark` bit
/// (which discards all of a custom theme's colours), it runs the token
/// derivation against the *actual* runtime palette.
#[inline]
pub fn resolve<T>(theme: &Theme, from_palette: fn(&Palette) -> T) -> T {
    from_palette(&Palette::from_iced(theme))
}

/// A declared pinned-token resolution site: one place in this crate that
/// resolves a widget's `*Tokens` from a builder-time pin or the live theme.
///
/// # Why [`resolve_pinned`] demands one
///
/// Without it, the resolver's callers would be enumerable only by grepping for
/// `resolve_pinned(`, and a text scan cannot see a call reached through a `use
/// … as` alias, a re-export, a `macro_rules!` expansion or a wrapper function
/// — an unbypassable-looking claim a grep cannot make.
///
/// The fields of this type are private and the only values that exist are
/// the constants declared by `declare_pin_sites!` below, which also builds
/// [`PIN_SITES`]. So the set of resolution sites is enumerable *from the type
/// system*: a new caller anywhere — aliased, re-exported, macro-expanded or
/// wrapped — cannot compile without naming a site, and it cannot name one
/// without appearing in [`PIN_SITES`]. `tests/focus/ring_registry.rs`
/// compares [`PIN_SITES`] with the pixel-gated subject table in both
/// directions, so a newly declared site is red until it has a rendered case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinSite {
    name: &'static str,
    home: &'static str,
}

impl PinSite {
    /// The site's stable name (`"slider"`, `"toast::resolve_tone"`).
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// The source file, relative to this crate's `src/`, that this site is
    /// declared to live in. [`pin_audit`] reports the real `#[track_caller]`
    /// location of every invocation, and the pixel gate compares the two for
    /// every observation it captures, so a call that migrates into another file
    /// (a wrapper, an alias, a macro in a different module) or a new caller
    /// borrowing this site is observable rather than assumed — for any draw
    /// path a registered fixture renders.
    #[must_use]
    pub const fn home(self) -> &'static str {
        self.home
    }
}

/// Declare every pinned-token resolution site, and build [`PIN_SITES`] from the
/// same list so "declared" and "enumerated" cannot drift.
macro_rules! declare_pin_sites {
    ($($konst:ident => ($name:literal, $home:literal, $doc:literal)),+ $(,)?) => {
        $(
            #[doc = $doc]
            pub const $konst: PinSite = PinSite { name: $name, home: $home };
        )+

        /// Every declared pinned-token resolution site.
        ///
        /// Built by the same macro invocation that declares the constants, so a
        /// site cannot exist without being listed here, and [`PinSite`]'s
        /// private fields mean no other value can be constructed.
        pub const PIN_SITES: &[PinSite] = &[$($konst),+];
    };
}

declare_pin_sites! {
    PIN_SITE_AREA_CHART => ("area_chart", "area_chart.rs", "`AreaChartProgram::tokens`."),
    PIN_SITE_AVATAR => ("avatar", "avatar.rs", "`EnzAvatar`'s container style closure."),
    PIN_SITE_BADGE => ("badge", "badge.rs", "`EnzBadge`'s tone style closure."),
    PIN_SITE_BAR_CHART => ("bar_chart", "bar_chart.rs", "`BarChartProgram::tokens`."),
    PIN_SITE_BREADCRUMB => ("breadcrumb", "breadcrumb.rs", "`crumb_link_style`."),
    PIN_SITE_CARD => ("card", "card.rs", "`EnzCard`'s surface style closure."),
    PIN_SITE_GAUGE => ("gauge", "gauge.rs", "`GaugeProgram::tokens`."),
    PIN_SITE_LINE_CHART => ("line_chart", "line_chart.rs", "`LineChartProgram::tokens`."),
    PIN_SITE_MARQUEE_AREA => ("marquee_area", "marquee_area.rs", "`marquee_area::resolve_tokens`."),
    PIN_SITE_MEDIA_CONTROLS => (
        "media_controls",
        "media_controls.rs",
        "`media_controls::resolve_tokens`, called from every transport style closure."
    ),
    PIN_SITE_MENU_ROW => ("menu_row", "menu_row.rs", "`EnzMenuRow`'s ink resolver closure."),
    PIN_SITE_PIE_CHART => ("pie_chart", "pie_chart.rs", "`PieChartProgram::tokens`."),
    PIN_SITE_RADIO_BUTTON => ("radio_button", "radio_button.rs", "`EnzRadioButton`'s style closure."),
    PIN_SITE_RADIO_GROUP => (
        "radio_group",
        "radio_group.rs",
        "the group's focus-ring draw path."
    ),
    PIN_SITE_SCATTER_CHART => ("scatter_chart", "scatter_chart.rs", "`ScatterChartProgram::tokens`."),
    PIN_SITE_SHEET => ("sheet", "sheet/tokens.rs", "`sheet::resolve_tokens`."),
    PIN_SITE_SLIDER => ("slider", "slider.rs", "`EnzSlider`'s token resolver closure."),
    PIN_SITE_SPINNER => ("spinner", "spinner.rs", "`SpinnerProgram::colors`."),
    PIN_SITE_STATUS_DOT => ("status_dot", "status_dot.rs", "`EnzStatusDot::resolved_style`."),
    PIN_SITE_TAB_PILL => ("tab_pill", "tab_pill.rs", "`tab_pill::resolve_tokens_with`."),
    PIN_SITE_TOAST_TONE => (
        "toast::resolve_tone",
        "toast.rs",
        "the toast's tone (background / border / foreground) resolver."
    ),
    PIN_SITE_TOAST_SECONDARY => (
        "toast::resolve_secondary",
        "toast.rs",
        "the toast's contrast-muted description-ink resolver."
    ),
    PIN_SITE_TOOLTIP => ("tooltip", "tooltip/fade.rs", "`faded_bubble`'s bubble token resolver."),
}

/// Observing every pinned-token resolution, however the resolver is reached.
///
/// # Purpose
///
/// [`resolve_pinned`] is `#[track_caller]` and reports each invocation here:
/// which [`PinSite`] it named, the real file and line of the call, and whether
/// a builder-time pin was present. That makes two things checkable that no
/// source scan can honestly claim:
///
/// * the fallback was not consulted. A pinned frame must produce only
///   observations with [`Observation::pinned`](pin_audit::Observation::pinned) `== true`; if any field of the
///   token set came from the live palette instead of the caller's pin, the
///   resolver ran its fallback and it is recorded here.
/// * the call really is where it says it is. The recorded file is the
///   `#[track_caller]` location, so an alias, a re-export, a wrapper function
///   or a `macro_rules!` expansion is observed at its actual invocation site
///   rather than assumed away.
///
/// # Cost
///
/// Recording is off unless a [`capture`](pin_audit::capture) is running, and the check that says so
/// is a single relaxed atomic load — no thread-local access and no borrow
/// flag on a shipped frame (the reasoning is recorded at `ACTIVE_CAPTURES` in
/// this module; the thread-local log is touched only while capturing). It is deliberately not
/// `#[cfg(test)]`: an integration test links the crate without `cfg(test)`, so a
/// mechanism that existed only in unit builds could not observe the real widget
/// draw paths.
///
/// # Example
///
/// ```
/// use enzui_widgets::theme_lookup::{self, pin_audit};
///
/// let (tokens, seen) = pin_audit::capture(|| {
///     theme_lookup::resolve_pinned(
///         theme_lookup::PIN_SITE_SLIDER,
///         None,
///         &iced::Theme::Dark,
///         enzui_widgets::slider::SliderTokens::from_palette,
///     )
/// });
/// assert_eq!(seen.len(), 1);
/// assert!(!seen[0].pinned, "no pin was supplied, so the fallback ran");
/// let _ = tokens;
/// ```
pub mod pin_audit {
    use super::PinSite;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[cfg(test)]
    use std::sync::{Mutex, MutexGuard};

    /// One observed [`resolve_pinned`](super::resolve_pinned) invocation.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Observation {
        /// The site the caller named.
        pub site: PinSite,
        /// The `#[track_caller]` file of the invocation.
        pub caller_file: &'static str,
        /// The `#[track_caller]` line of the invocation.
        pub caller_line: u32,
        /// Whether a builder-time pin was present, i.e. whether the fallback
        /// (`Palette::from_iced` + `from_palette`) was skipped.
        pub pinned: bool,
    }

    thread_local! {
        static LOG: RefCell<Option<Vec<Observation>>> = const { RefCell::new(None) };
    }

    /// How many [`capture`] calls are in flight, process-wide.
    ///
    /// This is the release fast path. Performing a thread-local access and a
    /// [`RefCell`] borrow on every pinned-token resolution would charge shipped
    /// applications for integration instrumentation they do not use. A
    /// resolution costs one relaxed load
    /// of this counter — no TLS, no borrow flag — and touches the thread-local
    /// log only while a capture is actually running.
    ///
    /// It is a counter, not a flag, because tests run in parallel: a flag
    /// cleared by the first capture to finish would silence a sibling capture
    /// still in flight on another thread. It is deliberately not `#[cfg(test)]`
    /// or `#[cfg(debug_assertions)]` either: an integration test links this
    /// crate without `cfg(test)`, and a `debug_assertions` gate would delete
    /// the audit under `cargo test --release` — turning a legitimate
    /// release-profile run of the pin-audit gates into a false red.
    static ACTIVE_CAPTURES: AtomicUsize = AtomicUsize::new(0);

    // Unit tests that assert the exact process-wide count share this lock. The
    // overlap test holds it while creating real concurrent captures itself, so
    // this prevents unrelated test-harness scheduling from hiding or inventing
    // a count without serializing the behavior under test.
    #[cfg(test)]
    static UNIT_CAPTURE_TEST: Mutex<()> = Mutex::new(());

    #[cfg(test)]
    pub(super) fn lock_unit_capture_tests() -> MutexGuard<'static, ()> {
        UNIT_CAPTURE_TEST.lock().expect("capture unit-test lock")
    }

    /// Owns one increment of [`ACTIVE_CAPTURES`] and the current thread's log.
    ///
    /// `finish` and `drop` share the same idempotent cleanup so the normal and
    /// unwinding exits both disable recording exactly once. The guard does not
    /// catch a panic: after cleanup, the caller's original unwind continues.
    struct CaptureGuard {
        active: bool,
    }

    impl CaptureGuard {
        fn start() -> Self {
            LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));
            // Install the log before recording is enabled, so this thread
            // cannot observe an active capture without a destination.
            ACTIVE_CAPTURES.fetch_add(1, Ordering::Release);
            Self { active: true }
        }

        fn cleanup(&mut self) -> Vec<Observation> {
            if !self.active {
                return Vec::new();
            }
            self.active = false;

            // Disable recording before taking the log. This ordering also
            // preserves the one-relaxed-load idle fast path in `note`.
            ACTIVE_CAPTURES.fetch_sub(1, Ordering::Release);
            LOG.with(|log| log.borrow_mut().take()).unwrap_or_default()
        }

        fn finish(mut self) -> Vec<Observation> {
            self.cleanup()
        }
    }

    impl Drop for CaptureGuard {
        fn drop(&mut self) {
            let _ = self.cleanup();
        }
    }

    /// Run `f`, returning its value and every pinned-token resolution it
    /// performed on this thread.
    ///
    /// Per-thread log rather than a global one so parallel tests observe only
    /// their own work; nesting is not supported and the inner capture takes
    /// over, so a caller should not nest. If `f` unwinds, an internal guard
    /// clears this thread's log and removes its process-wide active count before
    /// allowing the panic to continue.
    pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<Observation>) {
        let guard = CaptureGuard::start();
        let value = f();
        let seen = guard.finish();
        (value, seen)
    }

    /// Record one invocation, if any capture is running.
    ///
    /// The early return is the whole point: on a shipped frame this is one
    /// relaxed atomic load. `Relaxed` is sufficient — the only thread whose
    /// log this call can write is its own, and that thread's `capture`
    /// installed the log before incrementing the counter in program order. A
    /// thread that observes another thread's capture merely finds its own log
    /// `None` and records nothing.
    #[inline]
    pub(super) fn note(observation: Observation) {
        if ACTIVE_CAPTURES.load(Ordering::Relaxed) == 0 {
            return;
        }
        LOG.with(|log| {
            if let Ok(mut slot) = log.try_borrow_mut()
                && let Some(seen) = slot.as_mut()
            {
                seen.push(observation);
            }
        });
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::theme_lookup::{PIN_SITE_SLIDER, resolve_pinned};
        use std::panic::{AssertUnwindSafe, catch_unwind};

        fn fallback(_: &enzui_theme::Palette) -> u8 {
            0
        }

        /// A caller panic must escape `capture`, but must not strand either
        /// half of its instrumentation state. The idle invocation is a
        /// positive probe for the abandoned-log bug: with a leaked active count
        /// it would enter TLS and append to any stale log. The following normal
        /// capture must then contain its one, and only its one, invocation.
        #[test]
        fn an_unwinding_capture_restores_idle_state_and_discards_its_log() {
            let _capture_test = lock_unit_capture_tests();
            assert_eq!(
                ACTIVE_CAPTURES.load(Ordering::Relaxed),
                0,
                "the calibrated test must start with the audit idle"
            );

            let panic = catch_unwind(AssertUnwindSafe(|| {
                let _: ((), _) = capture(|| {
                    let _ = resolve_pinned(PIN_SITE_SLIDER, None, &iced::Theme::Dark, fallback);
                    panic!("injected panic inside pin_audit::capture");
                });
            }));
            assert!(panic.is_err(), "capture must not catch the caller's panic");
            assert_eq!(
                ACTIVE_CAPTURES.load(Ordering::Relaxed),
                0,
                "unwinding must remove exactly this capture's active count"
            );
            LOG.with(|log| {
                assert!(
                    log.borrow().is_none(),
                    "unwinding must discard the panicking capture's observation log"
                );
            });

            // This shipped resolver call occurs with no capture. A leaked
            // active count would send it through TLS; a leaked log would then
            // retain this second stale observation as well.
            let _ = resolve_pinned(PIN_SITE_SLIDER, None, &iced::Theme::Light, fallback);
            LOG.with(|log| {
                assert!(
                    log.borrow().is_none(),
                    "an idle resolution must not append to an abandoned log"
                );
            });

            let (value, seen) =
                capture(|| resolve_pinned(PIN_SITE_SLIDER, Some(41), &iced::Theme::Dark, fallback));
            assert_eq!(value, 41);
            assert_eq!(
                seen.len(),
                1,
                "the recovered capture must contain only its own invocation: {seen:?}"
            );
            assert_eq!(seen[0].site, PIN_SITE_SLIDER);
            assert!(seen[0].pinned, "the recovered invocation supplied a pin");
            assert_eq!(ACTIVE_CAPTURES.load(Ordering::Relaxed), 0);
            LOG.with(|log| assert!(log.borrow().is_none()));
        }
    }
}

/// Resolve a component's `*Tokens`, preferring a builder-time pin and
/// falling back to the runtime theme.
///
/// This is the single most repeated line in the crate: 22 draw paths spelled
///
/// ```text
/// pinned.unwrap_or_else(|| theme_lookup::resolve(theme, XTokens::from_palette))
/// ```
///
/// verbatim, and six canvas programs wrapped it in a private
/// `fn tokens(&self, theme)` accessor over a `pinned_tokens` field:
/// `area_chart`, `bar_chart`, `gauge`, `line_chart`, `pie_chart` and
/// `scatter_chart`. (`spinner` reaches the same pin through `fn colors`, and
/// `button` through its style closures, so neither is one of the six.) The
/// precedence rule — *an explicit `.tokens(..)` pin wins; otherwise derive
/// from the live palette so a runtime theme switch is honoured* — is a
/// crate-wide policy, not a per-widget decision, so it lives here once.
///
/// `pinned` is taken by value, so `Copy` token sets pass
/// `self.pinned_tokens` directly and non-`Copy` ones pass `.clone()`; the
/// fallback only runs when the pin is absent, so an unpinned widget pays
/// exactly one `Palette::from_iced` and a pinned one pays none.
///
/// # The `site` argument
///
/// Every caller names its [`PinSite`]. That is what makes the resolver's
/// callers enumerable from the type system rather than from a grep
/// ([`PIN_SITES`]), and it is what [`pin_audit`] reports alongside the real
/// `#[track_caller]` location and whether the fallback was skipped.
#[inline]
#[track_caller]
pub fn resolve_pinned<T>(
    site: PinSite,
    pinned: Option<T>,
    theme: &Theme,
    from_palette: fn(&Palette) -> T,
) -> T {
    let caller = std::panic::Location::caller();
    pin_audit::note(pin_audit::Observation {
        site,
        caller_file: caller.file(),
        caller_line: caller.line(),
        pinned: pinned.is_some(),
    });
    match pinned {
        Some(tokens) => tokens,
        None => resolve(theme, from_palette),
    }
}

/// Resolve the full [`enzui_theme::EnzTheme`] carried by `theme`, if it
/// was produced by
/// [`EnzTheme::iced_theme`](enzui_theme::EnzTheme::iced_theme).
///
/// This is the single point where the non-colour scales (spacing / sizing /
/// radius / typography / motion / elevation) re-enter widget code. It returns
/// `None` for a *stock* iced theme (`Theme::Dracula`, `Theme::Nord`, …) that
/// carries no enzUI payload; the scale accessors below fall back to
/// `Scale::default()` in that case. The whole `EnzTheme` (including the
/// complete palette) rides through `iced::Theme`'s extension slot.
#[inline]
pub fn enz_theme(theme: &Theme) -> Option<enzui_theme::EnzTheme> {
    enzui_theme::EnzTheme::from_iced(theme)
}

/// Project a single field out of the transported [`EnzTheme`] without
/// deep-cloning the whole extension.
///
/// [`enz_theme`] returns the entire `EnzTheme` by value, which clones its
/// [`FontCatalog`](enzui_theme::FontCatalog) — up to three owned
/// `FontFamily::Named(String)` heap strings — even when the caller only wants,
/// say, the [`RadiusScale`]. The scale accessors below run inside widget style
/// closures on the draw path, so that clone allocates per frame.
///
/// This borrows the shared `Arc<EnzTheme>` (a refcount bump, no deep clone) and
/// hands `project` a reference, returning whatever the projection copies out.
/// Every scale is `Copy`, so the projection copies a few scalars and never
/// touches the `FontCatalog`. Returns `None` for a stock iced theme with no
/// enzUI payload, exactly like [`enz_theme`].
#[inline]
fn project_enz<T>(theme: &Theme, project: fn(&enzui_theme::EnzTheme) -> T) -> Option<T> {
    theme
        .extension::<enzui_theme::EnzTheme>()
        .map(|arc| project(&arc))
}

/// The glass the widgets drawn with `theme` sit on, if any.
///
/// Set inside a material surface (see [`on_material`]); controls read it to
/// pick a glass treatment instead of an opaque one.
#[inline]
pub fn backdrop(theme: &Theme) -> Option<enzui_theme::MaterialStyle> {
    project_enz(theme, enzui_theme::EnzTheme::backdrop).flatten()
}

/// `theme` for content drawn on a `style` glass surface: its palette's
/// control fills and borders are translucent
/// ([`EnzTheme::on_material`](enzui_theme::EnzTheme::on_material)).
///
/// The derived theme is cached per thread, keyed by the parent theme's
/// payload, so a glass surface costs no rebuild per frame. Stock iced themes
/// carry no enzUI payload and come back unchanged.
pub fn on_material(theme: &Theme, style: enzui_theme::MaterialStyle) -> Theme {
    use enzui_theme::{EnzTheme, MaterialStyle};
    use std::cell::RefCell;
    use std::sync::Arc;

    // A few entries: one per (theme, style) pair on screen at once.
    const SLOTS: usize = 4;
    thread_local! {
        static CACHE: RefCell<Vec<(Arc<EnzTheme>, MaterialStyle, Theme)>> =
            const { RefCell::new(Vec::new()) };
    }

    let Some(parent) = theme.extension::<EnzTheme>() else {
        return theme.clone();
    };
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        // Holding the parent `Arc` keeps its address from being reused, so a
        // pointer match is a payload match.
        if let Some((_, _, derived)) = cache
            .iter()
            .find(|(source, cached, _)| Arc::ptr_eq(source, &parent) && *cached == style)
        {
            return derived.clone();
        }
        let derived = parent.on_material(style).iced_theme();
        if cache.len() == SLOTS {
            cache.remove(0);
        }
        cache.push((parent, style, derived.clone()));
        derived
    })
}

/// [`on_material`] with the glass's ink chosen: `light_ink` `1.0` for light
/// ink, `0.0` for dark, blending in between during a crossfade.
///
/// Cached like [`on_material`], with the blend quantized to sixteenths so a
/// crossfade costs at most seventeen rebuilds.
pub fn on_material_ink(theme: &Theme, style: enzui_theme::MaterialStyle, light_ink: f32) -> Theme {
    use enzui_theme::{EnzTheme, MaterialStyle};
    use std::cell::RefCell;
    use std::sync::Arc;

    const SLOTS: usize = 8;
    type Slot = (Arc<EnzTheme>, MaterialStyle, u8, Theme);
    thread_local! {
        static CACHE: RefCell<Vec<Slot>> =
            const { RefCell::new(Vec::new()) };
    }

    let Some(parent) = theme.extension::<EnzTheme>() else {
        return theme.clone();
    };
    // In 0..=16 after the clamp, so the cast cannot truncate.
    let step = (light_ink.clamp(0.0, 1.0) * 16.0).round() as u8;
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        // Holding the parent `Arc` keeps its address from being reused, so a
        // pointer match is a payload match.
        if let Some((_, _, _, derived)) = cache.iter().find(|(source, cached, at, _)| {
            Arc::ptr_eq(source, &parent) && *cached == style && *at == step
        }) {
            return derived.clone();
        }
        let derived = parent
            .on_material_ink(style, f32::from(step) / 16.0)
            .iced_theme();
        if cache.len() == SLOTS {
            cache.remove(0);
        }
        cache.push((parent, style, step, derived.clone()));
        derived
    })
}

/// Resolve the theme's [`SpacingScale`].
///
/// Reads the real ladder from the transported [`EnzTheme`](enzui_theme::EnzTheme) when present,
/// falling back to [`SpacingScale::default`] for stock themes. A widget that
/// hard-codes the default instead silently drops a custom density.
#[inline]
pub fn spacing(theme: &Theme) -> SpacingScale {
    project_enz(theme, |t| t.spacing).unwrap_or_default()
}

/// Resolve the theme's [`SizingTokens`]. See [`spacing`].
#[inline]
pub fn sizing(theme: &Theme) -> SizingTokens {
    project_enz(theme, |t| t.sizing).unwrap_or_default()
}

/// Resolve logical-pixel stroke widths; stock iced themes use default widths.
#[inline]
pub fn stroke(theme: &Theme) -> StrokeScale {
    project_enz(theme, |t| t.stroke).unwrap_or_default()
}

/// The theme's keyboard focus ring width. See [`spacing`].
#[inline]
pub fn focus_ring_width(theme: &Theme) -> f32 {
    enzui_theme::StrokeToken::FocusRing.resolve(&stroke(theme))
}

/// A component token set whose disabled fade follows one of the theme's
/// [`OpacityScale`](enzui_theme::OpacityScale) roles when the caller did
/// not pin it.
///
/// Controls (checkbox, radio, toggle, slider) follow `disabled`; surfaces
/// faded as a whole (clickable, pressable, list rows, swatches, a selected
/// tab segment) follow `disabled_surface`. Pinned tokens are a caller's
/// snapshot and keep their own value.
pub trait FollowsThemeOpacity: Sized {
    /// These tokens with their disabled fade taken from `opacity`.
    #[must_use]
    fn with_theme_opacity(self, opacity: &enzui_theme::OpacityScale) -> Self;
}

/// [`resolve`] for a token set whose disabled fade follows the theme.
#[inline]
pub fn resolve_live<T: FollowsThemeOpacity>(theme: &Theme, from_palette: fn(&Palette) -> T) -> T {
    resolve(theme, from_palette).with_theme_opacity(&opacity(theme))
}

/// [`resolve_pinned`] for a token set whose disabled fade follows the theme
/// when unpinned.
#[inline]
#[track_caller]
pub fn resolve_pinned_live<T: FollowsThemeOpacity>(
    site: PinSite,
    pinned: Option<T>,
    theme: &Theme,
    from_palette: fn(&Palette) -> T,
) -> T {
    let unpinned = pinned.is_none();
    let tokens = resolve_pinned(site, pinned, theme, from_palette);
    if unpinned {
        tokens.with_theme_opacity(&opacity(theme))
    } else {
        tokens
    }
}

/// Resolve whole-control state opacities; stock iced themes use the defaults.
#[inline]
pub fn opacity(theme: &Theme) -> enzui_theme::OpacityScale {
    project_enz(theme, |t| t.opacity).unwrap_or_default()
}

/// Resolve the theme's [`RadiusScale`]. See [`spacing`].
#[inline]
pub fn radius(theme: &Theme) -> RadiusScale {
    project_enz(theme, |t| t.radius).unwrap_or_default()
}

/// The radius a component family's unpinned corners take: the
/// [`ShapeScale`](enzui_theme::ShapeScale) step `family` picks, on the theme's
/// radius scale, or the theme's window radius for the `window` step. See
/// [`spacing`].
#[inline]
pub fn shape_radius(
    theme: &Theme,
    family: fn(&enzui_theme::ShapeScale) -> enzui_theme::RadiusStep,
) -> f32 {
    let shape = project_enz(theme, |t| t.shape).unwrap_or_default();
    match family(&shape) {
        enzui_theme::RadiusStep::Window => project_enz(theme, |t| t.window_radius)
            .unwrap_or_else(enzui_theme::text_defaults::window_radius),
        step => radius(theme).step(step),
    }
}

/// Resolve the theme's [`TypographyScale`]. See [`spacing`].
#[inline]
pub fn typography(theme: &Theme) -> TypographyScale {
    project_enz(theme, |t| t.typography).unwrap_or_default()
}

/// Resolve the theme's [`MotionScale`]. See [`spacing`].
#[inline]
pub fn motion(theme: &Theme) -> MotionScale {
    project_enz(theme, |t| t.motion).unwrap_or_default()
}

/// The theme's default [`Grain`](enzui_theme::Grain): what a surface asking
/// for [`GrainSpec::Theme`](enzui_theme::GrainSpec::Theme) draws.
#[inline]
pub fn grain(theme: &Theme) -> enzui_theme::Grain {
    project_enz(theme, |t| t.grain).unwrap_or_default()
}

/// Resolve the theme's [`ElevationScale`]. See [`spacing`].
#[inline]
pub fn elevation(theme: &Theme) -> ElevationScale {
    project_enz(theme, |t| t.elevation).unwrap_or_default()
}

/// Resolve the theme's [`ScrollbarTokens`]. See [`spacing`].
#[inline]
pub fn scrollbar(theme: &Theme) -> ScrollbarTokens {
    project_enz(theme, |t| t.scrollbar)
        .unwrap_or_default()
        .normalized()
}

/// The runtime reduced-motion policy for `theme`
/// (`EnzTheme.motion.reduced_motion`).
///
/// This reads the real motion policy transported through
/// `iced::Theme`'s extension slot: a caller who sets `reduced_motion: true` on
/// their `EnzTheme` has that honoured at draw time. For a stock iced theme
/// (which carries no enzUI payload) it falls back to
/// [`MotionScale::default`]`.reduced_motion`.
///
/// Widgets keep an explicit per-instance override (e.g.
/// `EnzSpinner::reduced_motion`) which takes precedence over whatever this
/// returns, so callers who already know the user's preference are never
/// blocked.
#[inline]
pub fn reduced_motion(theme: &Theme) -> bool {
    motion(theme).reduced_motion
}

/// The runtime material accessibility policy in force for `theme`.
///
/// Two transports meet here, and neither is a test fixture:
///
/// - reduced motion arrives through the live theme's own motion policy, the
///   same one [`reduced_motion`] reads, so an application that already honours
///   the desktop's motion preference gets material settlement for free;
/// - reduced transparency and increased contrast arrive through the
///   process-wide policy store, which an application runtime's
///   `enzui_core::material_preferences::MaterialPreferenceFeed` publishes
///   the desktop's answers and the user's override into.
///
/// With neither transport supplying anything the result is
/// [`MaterialPolicy::default`]: no reduction, and a source of
/// [`enzui_theme::MaterialPolicySource::Unset`] so the absence is
/// readable rather than indistinguishable from an approval.
#[inline]
pub fn material_policy(theme: &Theme) -> MaterialPolicy {
    enzui_core::material::policy_with_theme_motion(reduced_motion(theme))
}

/// Pick between two thunks based on whether `theme` is dark.
///
/// Deprecated in spirit by [`resolve`]: this only reads the
/// `is_dark` bit and returns a fixed preset, so custom themes render as the
/// nearest preset. Retained for callers that intentionally want the preset
/// snap.
#[inline]
pub fn dark_or_light<T>(theme: &Theme, dark: fn() -> T, light: fn() -> T) -> T {
    if theme.palette().is_dark {
        dark()
    } else {
        light()
    }
}

#[cfg(test)]
mod shape_radius_tests {
    /// A family on the `window` step takes the runtime theme's own window
    /// radius at draw time, as the builder-time resolver does.
    #[test]
    fn the_window_step_follows_the_theme_window_radius() {
        for radius in [0.0, 22.0, 48.0] {
            let mut theme = enzui_theme::EnzTheme::light();
            theme.window_radius = radius;
            theme.shape.media = enzui_theme::RadiusStep::Window;
            let runtime = theme.iced_theme();
            assert_eq!(super::shape_radius(&runtime, |s| s.media), radius);
            let _scope = enzui_theme::text_defaults::TextDefaultsScope::enter(&theme);
            assert_eq!(
                enzui_theme::text_defaults::shape_radius(|s| s.media),
                radius
            );
        }
        let theme = enzui_theme::EnzTheme::light();
        assert_eq!(
            super::shape_radius(&theme.iced_theme(), |s| s.card),
            theme.radius.step(theme.shape.card),
            "other steps are unchanged"
        );
    }
}
