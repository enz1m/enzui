//! Debug diagnostics for unrouted or inert sidebar toggles and discarded headers.
//!
//! Assertion controls are independent. Explicit thread-scoped arming takes
//! precedence over unrouted-toggle suppression and the cached process setting.

/// Default stable disclosure id for the shell's sidebar toggle.
///
/// Override with [`super::EnzAppShell::sidebar_toggle_id`] when multiple shells
/// share a semantics tree, so their disclosure actions have distinct targets.
pub const SIDEBAR_TOGGLE_ID: &str = "enz-app-shell-sidebar-toggle";

/// Debug diagnostic for a placeable collapsible sidebar without a toggle route.
///
/// Wire [`super::EnzAppShell::on_sidebar_state`] or
/// [`super::EnzAppShell::on_toggle_sidebar`], or declare a
/// [`super::ShellTokens::permanent_sidebar`].
pub const SIDEBAR_TOGGLE_UNROUTED: &str = "EnzAppShell: this shell has a COLLAPSIBLE sidebar, so its sidebar can be \
     put away and must be bringable back — but nothing routes the toggle, so \
     the shell renders none. Wire `.on_sidebar_state(..)` (preferred: the \
     shell applies `NavSplitState::toggle_for_presentation` for you and hands \
     back the flipped state) or `.on_toggle_sidebar(..)`, and the toggle \
     appears at the leading edge of the leading bar, next to the sidebar it \
     controls. A sidebar that cannot collapse at all wants \
     `ShellTokens::permanent_sidebar()`, which withdraws the toggle and this \
     assert with it.";

/// Process setting that suppresses the unrouted-toggle assertion.
///
/// A nonempty value disables the armed default. The environment is cached on
/// first use; explicit thread-scoped arming takes precedence.
pub const SIDEBAR_TOGGLE_ASSERT_ENV: &str = "ENZUI_ALLOW_UNROUTED_SIDEBAR_TOGGLE";

thread_local! {
    static TOGGLE_ASSERT_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct ToggleAssertGuard(bool);

impl Drop for ToggleAssertGuard {
    fn drop(&mut self) {
        TOGGLE_ASSERT_ARMED.with(|c| c.set(self.0));
    }
}

/// Force the unrouted-toggle assertion on this thread while `f` runs.
///
/// Overrides enclosing suppression and the process setting. Restores the
/// previous arming state on return or panic; does not affect other threads.
pub fn with_sidebar_toggle_assert_armed<R>(f: impl FnOnce() -> R) -> R {
    let previous = TOGGLE_ASSERT_ARMED.with(|c| c.replace(true));
    let _guard = ToggleAssertGuard(previous);
    f()
}

thread_local! {
    static TOGGLE_ASSERT_SUPPRESSED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct ToggleAssertSuppressGuard(bool);

impl Drop for ToggleAssertSuppressGuard {
    fn drop(&mut self) {
        TOGGLE_ASSERT_SUPPRESSED.with(|c| c.set(self.0));
    }
}

/// Suppress the unrouted-toggle assertion on this thread while `f` runs.
///
/// Intended for fixtures exercising unrouted shells. Explicit arming takes
/// precedence; the previous suppression state is restored on return or panic.
pub fn without_sidebar_toggle_assert<R>(f: impl FnOnce() -> R) -> R {
    let previous = TOGGLE_ASSERT_SUPPRESSED.with(|c| c.replace(true));
    let _guard = ToggleAssertSuppressGuard(previous);
    f()
}

#[must_use]
/// Whether the unrouted-toggle assertion is enabled on this thread.
///
/// Explicit arming wins, then scoped suppression, then the process setting.
/// Without an override or a nonempty disarming setting, the assertion is enabled.
pub fn sidebar_toggle_assert_armed() -> bool {
    use std::sync::OnceLock;
    if TOGGLE_ASSERT_ARMED.with(std::cell::Cell::get) {
        return true;
    }
    if TOGGLE_ASSERT_SUPPRESSED.with(std::cell::Cell::get) {
        return false;
    }
    static DISARMED_BY_ENV: OnceLock<bool> = OnceLock::new();
    let disarmed = *DISARMED_BY_ENV
        .get_or_init(|| std::env::var_os(SIDEBAR_TOGGLE_ASSERT_ENV).is_some_and(|v| !v.is_empty()));
    !disarmed
}

/// Debug diagnostic for a sidebar toggle whose simulated next state is unchanged.
///
/// Simulation requires [`super::EnzAppShell::on_sidebar_state`] and a
/// [`super::EnzAppShell::forced_by`] rule. A plain forced presentation or an
/// opaque legacy toggle message cannot be judged this way.
pub const SIDEBAR_TOGGLE_INERT: &str = "EnzAppShell: this shell renders a sidebar toggle whose press CANNOT CHANGE \
     ANYTHING. The shell resolved the geometry it would produce for the state \
     the toggle publishes, and it is identical to the current one — same \
     presentation, same drawer visibility — so the control absorbs the click \
     and nothing moves, which is worse than no control at all. The usual cause \
     is a `.forced_by(..)` rule that ignores the `NavSplitState` the shell \
     just published: the app stores \
     the flipped state and the view discards it. Fix it by reading the state \
     in the rule that produces the forced presentation (the framework's own \
     `nav_split::resolve_for_width(state.preference, ..)` does this), or by \
     dropping `.forced_by(..)` and letting the shell resolve. If the sidebar is \
     genuinely not collapsible, say so with `ShellTokens::permanent_sidebar()`, \
     which withdraws the toggle rather than leaving a dead one on screen.";

/// Process setting that enables inert-toggle diagnostics for a nonempty value.
///
/// Cached on first use and independent of [`SIDEBAR_TOGGLE_ASSERT_ENV`].
/// Thread-scoped arming can enable the diagnostic without this setting.
pub const SIDEBAR_TOGGLE_INERT_ASSERT_ENV: &str = "ENZUI_ASSERT_SIDEBAR_TOGGLE_LIVE";

thread_local! {
    static INERT_ASSERT_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct InertAssertGuard(bool);

impl Drop for InertAssertGuard {
    fn drop(&mut self) {
        INERT_ASSERT_ARMED.with(|c| c.set(self.0));
    }
}

/// Enable inert-toggle diagnostics on this thread while `f` runs.
///
/// Restores previous arming on return or panic; the process setting can keep
/// the diagnostic enabled after this scope ends.
pub fn with_sidebar_inert_assert_armed<R>(f: impl FnOnce() -> R) -> R {
    let previous = INERT_ASSERT_ARMED.with(|c| c.replace(true));
    let _guard = InertAssertGuard(previous);
    f()
}

#[must_use]
/// Whether scoped arming or the cached process setting enables inert-toggle diagnostics.
pub fn sidebar_inert_assert_armed() -> bool {
    use std::sync::OnceLock;
    static FROM_ENV: OnceLock<bool> = OnceLock::new();
    let from_env = *FROM_ENV.get_or_init(|| {
        std::env::var_os(SIDEBAR_TOGGLE_INERT_ASSERT_ENV).is_some_and(|v| !v.is_empty())
    });
    from_env || INERT_ASSERT_ARMED.with(std::cell::Cell::get)
}

/// Debug diagnostic for a sidebar titlebar supplied under `SpanningBand` in
/// `TitlebarSidebarContent`, where that slot cannot be rendered.
///
/// Use an inside or split sidebar header, or move the content into the titlebar slot.
pub const SPANNING_BAND_DROPS_SIDEBAR_TITLEBAR: &str = "EnzAppShell: a `sidebar_titlebar` slot was supplied under \
     `SidebarHeader::SpanningBand`, which draws one full-width band with the \
     sidebar BELOW it and has no sidebar-side segment to render that slot in — \
     so it is silently dropped (see `SidebarHeader::uses_sidebar_titlebar_slot`). \
     Use `SidebarHeader::Inside` or `SidebarHeader::SplitBand` if the sidebar \
     header content must appear, or move it into the `titlebar` slot and drop \
     the `sidebar_titlebar` call.";
