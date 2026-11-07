// ---------------------------------------------------------------------------
// The ambient shell-band tone
// ---------------------------------------------------------------------------

// Thread-local, not a global, for the reason `focus.rs`'s `MODALITY_UNKEYED`
// and `scroll_view.rs`'s registry are: parallel headless test trees each build
// on their own thread, and a process-global cell would let one shell's band
// scope leak into another thread's standalone bar. In production the view tree
// is built on one event-loop thread, so thread-local is indistinguishable from
// global there.
thread_local! {
    /// The tone of the surface the shell already painted behind the slot
    /// currently being built, or `None` when no shell slot is on the stack.
    ///
    /// Set only for the duration of a slot closure's invocation, by
    /// [`with_shell_band_tone`].
    static SHELL_BAND_TONE: std::cell::Cell<Option<SurfaceTone>>
        = const { std::cell::Cell::new(None) };

    /// The height of the band the shell has already reserved for the slot
    /// currently being built, or `None` outside any shell slot.
    ///
    /// Published for the same reason and by the same mechanism as
    /// [`SHELL_BAND_TONE`]. See [`with_shell_band`].
    static SHELL_BAND_HEIGHT: std::cell::Cell<Option<f32>>
        = const { std::cell::Cell::new(None) };

    /// Whether the shell building the current slot draws the window
    /// controls itself. See [`with_shell_window_controls`].
    static SHELL_OWNS_CONTROLS: std::cell::Cell<bool>
        = const { std::cell::Cell::new(false) };
}

struct ShellControlsGuard(bool);

impl Drop for ShellControlsGuard {
    fn drop(&mut self) {
        SHELL_OWNS_CONTROLS.with(|c| c.set(self.0));
    }
}

/// Build `f`'s value knowing whether the enclosing shell draws the window
/// controls. A titlebar built inside then draws none of its own, so a
/// window never shows two close buttons and no bar needs
/// `.show_close(false)`.
pub(crate) fn with_shell_window_controls<R>(owns: bool, f: impl FnOnce() -> R) -> R {
    let previous = SHELL_OWNS_CONTROLS.with(|c| c.replace(owns));
    let _guard = ShellControlsGuard(previous);
    f()
}

/// Whether the shell building the current slot draws the window controls.
#[must_use]
pub fn shell_owns_window_controls() -> bool {
    SHELL_OWNS_CONTROLS.with(std::cell::Cell::get)
}

thread_local! {
    /// How many px the band this subtree is being laid out in is
    /// over-subscribed by. See [`band_deficit`].
    static BAND_DEFICIT: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Restores the previous band deficit on drop, for the [`BandToneGuard`]
/// reason: a panicking child layout must not leave every later widget on this
/// thread believing it is under pressure.
struct BandDeficitGuard(f32);

impl Drop for BandDeficitGuard {
    fn drop(&mut self) {
        BAND_DEFICIT.with(|c| c.set(self.0));
    }
}

/// How many px the chrome band currently being laid out is over-subscribed
/// by — `0.0` outside a band, and outside the second measurement pass.
///
/// # Why a widget needs to be told, and cannot work it out
///
/// A row hands its first `Shrink` child every pixel that is left, so a
/// hugging child takes its natural width and never learns that a *later*
/// sibling needed some of it. `iced`'s flex has no channel for "the child
/// after you has a minimum", so nothing inside a row can prevent a hugging
/// child from starving a still-unmeasured trailing sibling.
///
/// [`EnzTitleBar`] is the one party that can see the whole band. It lays
/// the content slot out, measures the realised extent of the subtree, and
/// — when that extent spills — publishes the deficit here and lays the
/// slot out once more. A child that can give width back does so on that
/// second pass; one that cannot is unchanged, and the slot boundary (see
/// `content_slot_clip`) still removes its ink, its hit target and its
/// node, with [`CONTENT_OVERSUBSCRIPTION_ID`] reporting what was left
/// over.
///
/// This is deliberately a pressure signal, not a width: the deficit
/// measured at the slot edge is not the number a child must give back (a
/// `Length::Fill` spacer between them halves the effect), so a child
/// responds by stepping to its next narrower presentation and the band
/// re-measures the consequence. [`crate::view_switcher`] is the first
/// responder — see its *compressing hug* docs for what it gives up, and
/// in which order.
///
/// Published by the same thread-local mechanism, and for the same reason,
/// as [`shell_band_height`]: the value is a layout fact the parent has
/// already computed and no child can re-derive.
#[must_use]
pub fn band_deficit() -> f32 {
    BAND_DEFICIT.with(std::cell::Cell::get)
}

/// Publish `px` as the ambient band deficit for the duration of `f`.
///
/// Crate-private: only a bar that has *measured* an over-subscription may
/// claim one. A caller-settable pressure signal would let an app request
/// compression at a width where the band is not actually short, which is the
/// per-app breakpoint tuning `EnzTitleBar`'s measured model exists to remove.
pub(crate) fn with_band_deficit<R>(px: f32, f: impl FnOnce() -> R) -> R {
    let publish = if px.is_finite() && px > 0.0 { px } else { 0.0 };
    let previous = BAND_DEFICIT.with(|c| c.replace(publish));
    let _guard = BandDeficitGuard(previous);
    f()
}

/// Restores the previous ambient tone on drop, so a panicking slot closure
/// cannot strand the scope set for every later bar on this thread.
struct BandToneGuard(Option<SurfaceTone>);

impl Drop for BandToneGuard {
    fn drop(&mut self) {
        SHELL_BAND_TONE.with(|c| c.set(self.0));
    }
}

/// Restores the previous ambient band height on drop, for the
/// [`BandToneGuard`] reason.
struct BandHeightGuard(Option<f32>);

impl Drop for BandHeightGuard {
    fn drop(&mut self) {
        SHELL_BAND_HEIGHT.with(|c| c.set(self.0));
    }
}

/// Build `f`'s value with `tone` published as the ambient chrome-band
/// tone.
///
/// `tone` is the tone of the surface the shell has already painted under
/// the slot: the titlebar pane's tone for a titlebar band, the sidebar
/// pane's tone for a header that lives inside the sidebar pane, the
/// subtitlebar pane's tone for the second band. A bar that adopts it
/// therefore repaints exactly what is already there — which for the
/// default `Transparent` means it paints nothing at all and the shell's
/// chrome shows through. This keeps a single owner for the chrome colour,
/// so a shell that paints its titlebar as `Transparent` cannot be covered
/// by an inner bar rendering an opaque `surface_alt` slab on top.
///
/// The slot is `Box<dyn Fn() -> Element<'_, Message>>`: the shell cannot
/// reach *into* the returned element to retone a bar buried three
/// containers deep, and threading a tone parameter through would change
/// the signature of every app's view function for a decision the shell
/// owns. The scope is live only for the closure call, which is exactly
/// when `EnzTitleBar::into_element` runs (an `Element` cannot be built
/// earlier and stored — it is not `Clone`), so the read lands on the
/// right bar and on no other.
///
/// An explicit [`EnzTitleBar::tone`] still wins — a caller that wants an
/// opaque `Surface` band merging with the action bar below still gets it
/// — and a bar built outside any scope still resolves
/// [`DEFAULT_TITLEBAR_TONE`].
///
/// This form publishes no band height, so a contained bar centres its
/// window controls against its own declared height rather than the band
/// it was actually given. When the two differ the close button's corner
/// comes out unsquare — see [`with_shell_band`], which publishes both
/// and is what [`crate::app_shell::EnzAppShell`] uses. Prefer it for any
/// host that reserves a band; this remains for hosts that genuinely only
/// tone one.
pub fn with_shell_band_tone<R>(tone: SurfaceTone, f: impl FnOnce() -> R) -> R {
    let previous = SHELL_BAND_TONE.with(|c| c.replace(Some(tone)));
    let _guard = BandToneGuard(previous);
    // The height scope is deliberately cleared, not left as-is: a nested bar
    // toned by this form must not inherit an outer band's height and centre
    // its controls against a band it is not in.
    let previous_height = SHELL_BAND_HEIGHT.with(|c| c.replace(None));
    let _height_guard = BandHeightGuard(previous_height);
    f()
}

/// The ambient chrome-band tone currently in scope, or `None` outside any
/// [`with_shell_band_tone`] call. See that function for the full reasoning.
#[must_use]
pub fn shell_band_tone() -> Option<SurfaceTone> {
    SHELL_BAND_TONE.with(std::cell::Cell::get)
}

/// Build `f`'s value with `tone` and `height` published as the ambient
/// chrome-band properties — the tone the shell has already painted, and
/// the height it has already reserved.
///
/// Supersedes [`with_shell_band_tone`], which is the tone-only form kept
/// for callers outside the shell.
///
/// The shell reserves a band of `ShellTokens::titlebar_height` and then
/// vertically centres the slot's content in it. A contained bar declares
/// a band height of its own, and when that is *shorter* than the shell's
/// the difference becomes slack the shell adds above and below — slack
/// the bar cannot see. Its window controls therefore sit
/// `(shell_h - bar_h)/2 + (bar_h - control)/2` from the window's top,
/// which telescopes to `(shell_h - control)/2`, while their trailing
/// inset (derived from the bar's own height) is only
/// `(bar_h - control)/2`. The corner unsquares by exactly half the
/// height difference.
///
/// The published height is also what a bar with no explicit `.height(..)`
/// paints its own band at ([`EnzTitleBar::resolved_height`]), so the
/// band's painted height and the height the controls are centred in are
/// the same number by construction: retune the shell's band and both the
/// painted band and the controls' inset follow it, so the corner stays
/// square. An explicit `.height(..)` still opts out and centres in the
/// shell's band.
///
/// The bar cannot compute this itself: the shell's band height is a
/// number it does not own, which is the same argument
/// `app_shell::tree::centred_in_band` already makes for vertical
/// centring. So the shell publishes it, exactly as it publishes the tone.
pub fn with_shell_band<R>(tone: SurfaceTone, height: f32, f: impl FnOnce() -> R) -> R {
    let previous_tone = SHELL_BAND_TONE.with(|c| c.replace(Some(tone)));
    let _tone_guard = BandToneGuard(previous_tone);
    // A non-finite or non-positive band height is not expressible intent and
    // would poison the derived inset, so it is published as "no band" and the
    // bar falls back to its own height.
    let publish = (height.is_finite() && height > 0.0).then_some(height);
    let previous_height = SHELL_BAND_HEIGHT.with(|c| c.replace(publish));
    let _height_guard = BandHeightGuard(previous_height);
    f()
}

/// The ambient chrome-band height currently in scope, or `None` outside
/// any [`with_shell_band`] call. See that function for the full reasoning.
#[must_use]
pub fn shell_band_height() -> Option<f32> {
    SHELL_BAND_HEIGHT.with(std::cell::Cell::get)
}

/// Resolve exactly one colour — the band fill for `tone` — from the live
/// runtime `theme`.
///
/// This preserves the module's "one colour per render" discipline: it projects
/// a single palette field per tone rather than deriving the whole
/// [`crate::surface::SurfaceTokens`] struct. The mapping matches
/// [`crate::surface::SurfaceTokens::fill_for`] field-for-field, so a bar toned
/// `Surface` resolves the same token an `enz_surface` toned `Surface` would.
fn resolve_band_fill(theme: &iced::Theme, tone: SurfaceTone) -> Color {
    match tone {
        SurfaceTone::Surface => crate::theme_lookup::resolve(theme, |p| p.surface),
        SurfaceTone::SurfaceAlt => crate::theme_lookup::resolve(theme, |p| p.surface_alt),
        SurfaceTone::Elevated => crate::theme_lookup::resolve(theme, |p| p.surface_elevated),
        SurfaceTone::Selected => crate::theme_lookup::resolve(theme, |p| p.surface_selected),
        SurfaceTone::Hover => crate::theme_lookup::resolve(theme, |p| p.surface_hover),
        SurfaceTone::Background => crate::theme_lookup::resolve(theme, |p| p.background),
        SurfaceTone::AccentSoft => crate::theme_lookup::resolve(theme, |p| p.accent_soft),
        SurfaceTone::ImageCanvas => crate::theme_lookup::resolve(theme, |p| p.image_canvas),
        SurfaceTone::VideoCanvas => crate::theme_lookup::resolve(theme, |p| p.video_canvas),
        SurfaceTone::Transparent => Color::TRANSPARENT,
    }
}

/// Build the band's `container::Style` closure for `tone`.
///
/// This is the single style boundary the band paints through, and
/// [`into_element`](EnzTitleBar::into_element) uses exactly this. Extracting it
/// means [`tests::tone_changes_the_resolved_band_colour`] drives the *same*
/// closure the widget renders with, so a fill that never reaches the closure
/// fails it.
///
/// `tone` is `Copy` and captured by `move`; the colour is still resolved from
/// the live `theme` on every call, so the band stays theme-reactive.
fn band_style(tone: SurfaceTone) -> impl Fn(&iced::Theme) -> container::Style {
    move |theme: &iced::Theme| container::Style {
        background: Some(resolve_band_fill(theme, tone).into()),
        ..container::Style::default()
    }
}

// ---------------------------------------------------------------------------
// Pane headers
// ---------------------------------------------------------------------------

thread_local! {
    /// Whether the slot being built is a pane's own header band (the shell's
    /// `sidebar_titlebar`), rather than the window's titlebar. See
    /// [`with_pane_header`].
    static SHELL_PANE_HEADER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// Which edge of the pane header the shell puts the sidebar toggle on.
    static SHELL_PANE_HEADER_TOGGLE: std::cell::Cell<PaneHeaderToggle> =
        const { std::cell::Cell::new(PaneHeaderToggle::None) };
}

/// Where the shell places the sidebar toggle beside a pane header, in logical
/// terms (leading is the left edge in a left-to-right layout).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PaneHeaderToggle {
    /// No toggle beside the header.
    #[default]
    None,
    /// The toggle comes before the header: the title follows it.
    Leading,
    /// The toggle comes after the header: trailing actions sit next to it.
    Trailing,
}

struct PaneHeaderGuard(bool, PaneHeaderToggle);

impl Drop for PaneHeaderGuard {
    fn drop(&mut self) {
        SHELL_PANE_HEADER.with(|c| c.set(self.0));
        SHELL_PANE_HEADER_TOGGLE.with(|c| c.set(self.1));
    }
}

/// Build `f`'s value as a pane header: the band above a sidebar.
///
/// A bar built inside starts its title on the sidebar's content rail
/// ([`PANE_HEADER_PADDING_X`]) instead of the window edge, and sets it as a
/// pane heading rather than a window title (one rung lighter and at body
/// size), so the header reads as the top of the sidebar's own column.
pub(crate) fn with_pane_header<R>(toggle: PaneHeaderToggle, f: impl FnOnce() -> R) -> R {
    let previous = SHELL_PANE_HEADER.with(|c| c.replace(true));
    let previous_toggle = SHELL_PANE_HEADER_TOGGLE.with(|c| c.replace(toggle));
    let _guard = PaneHeaderGuard(previous, previous_toggle);
    f()
}

/// The sidebar toggle's position in the pane header currently being built.
/// Returns [`PaneHeaderToggle::None`] outside a pane header.
#[must_use]
pub fn pane_header_toggle() -> PaneHeaderToggle {
    if in_pane_header() {
        SHELL_PANE_HEADER_TOGGLE.with(std::cell::Cell::get)
    } else {
        PaneHeaderToggle::None
    }
}

/// Whether the current bar is being built as a pane header.
#[must_use]
pub fn in_pane_header() -> bool {
    SHELL_PANE_HEADER.with(std::cell::Cell::get)
}

// ---------------------------------------------------------------------------
// Sidebar panes
// ---------------------------------------------------------------------------

thread_local! {
    /// Whether the shell is building its sidebar's body. See [`with_sidebar_pane`].
    static SHELL_SIDEBAR_PANE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct SidebarPaneGuard(bool);

impl Drop for SidebarPaneGuard {
    fn drop(&mut self) {
        SHELL_SIDEBAR_PANE.with(|c| c.set(self.0));
    }
}

/// Build `f`'s value as the body of the shell's sidebar. Rows built inside
/// (a list row surface, a tree) mark their selection with the sidebar's tint
/// ([`crate::sidebar_row::selected_tint`]), the same as the sidebar's own rows,
/// whatever widget the app lists its places, conversations or sessions with.
pub(crate) fn with_sidebar_pane<R>(f: impl FnOnce() -> R) -> R {
    let previous = SHELL_SIDEBAR_PANE.with(|c| c.replace(true));
    let _guard = SidebarPaneGuard(previous);
    f()
}

/// Whether the current element is being built inside the shell's sidebar.
#[must_use]
pub fn in_sidebar_pane() -> bool {
    SHELL_SIDEBAR_PANE.with(std::cell::Cell::get)
}
