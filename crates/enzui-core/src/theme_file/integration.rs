//! Shared XDG theme integration for applications and daemons.
//!
//! [`ThemeIntegration::subscription`] runs bounded file maintenance off the
//! application message path and emits only changed theme or diagnostic state.
//! Unchanged polls therefore do not invoke the application's update or view.
//! The same seam follows desktop font preferences (see
//! [`crate::font_preferences`]) and reduced motion. Reduced motion is resolved
//! as application override, theme file, platform request, then the
//! animation-enabled default; startup and live events remain distinct so a
//! delayed snapshot cannot overwrite a newer change.

use super::{ProbeOverride, ReducedMotionSource, ThemeLoad, ThemePaths, ThemeRuntime, ThemeStatus};
use crate::file_watch::{FileWatchBackend, FsBackend};
use crate::font_preferences::{
    DesktopFontBackend, DesktopFontFeed, FontFeedStatus, FontPreferenceEvent,
};
use enzui_theme::text_scale::{
    self, TextScale, TextScaleDelivery, TextScaleSource, read_env_text_scale,
};
use enzui_theme::{EnzTheme, EnzThemeMode, ReducedMotionPrecedence};
use iced_futures::Subscription;
use iced_runtime::Task;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod service;
use service::{ServiceConfig, ThemeServiceRecipe};

/// Default interval between probes of the small XDG candidate set.
///
/// Theme edits are not latency-sensitive enough to justify a hot idle loop;
/// embedded hosts may explicitly choose a faster interval with
/// [`ThemeIntegrationConfig::timing`].
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Quiet period after a filesystem transition before the file is loaded.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(120);
// A timeout does not release a probe slot while its worker lease remains alive.
pub(crate) const MIN_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the framework owns the application's theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeManagement {
    /// The default: consume the shared XDG file and configured platform feeds.
    SharedXdg,
    /// Explicit opt-out: keep the compiled theme supplied by the application.
    UnmanagedCompiled,
}

/// Committed changes produced by [`ThemeIntegration::subscription`].
///
/// Applications should carry this type in their message enum and forward it to
/// [`ThemeIntegration::update`]. Maintenance ticks and unchanged probes do not
/// produce an event.
#[derive(Debug, Clone)]
pub enum ThemeEvent {
    /// An off-thread theme-file load that moved the document or status.
    Loaded(ThemeLoad),
    /// The one-shot OS appearance snapshot completed.
    OsBootstrap(Box<EnzTheme>),
    /// The live OS appearance feed changed.
    OsChanged(Box<EnzTheme>),
    /// The one-shot platform reduced-motion snapshot completed.
    ReducedMotionBootstrap(ReducedMotionRead),
    /// The live platform reduced-motion source changed.
    ReducedMotionChanged(ReducedMotionRead),
    /// The desktop font-preference source reported.
    Fonts(FontPreferenceEvent),
    /// The desktop's settings changed: accent colour and text size reach the
    /// theme; the rest apply process-wide and need a view rebuild, which this
    /// event's message causes.
    Desktop(Box<crate::desktop_settings::DesktopSettings>),
    /// The watcher or loader diagnostic changed (`Some`) or cleared (`None`).
    WatchDiagnostic(Option<ThemeWatchDiagnostic>),
}

/// The two states exposed by the platform reduced-motion convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ReducedMotionPreference {
    /// The platform does not request reduced motion.
    #[default]
    NoPreference,
    /// The platform requests that non-essential motion be reduced.
    Reduce,
}

impl ReducedMotionPreference {
    /// Decodes `org.freedesktop.appearance` / `reduced-motion` from the
    /// freedesktop Settings portal: `1` requests reduction and every other
    /// value is no preference, as required for unknown values by version 2.
    #[must_use]
    pub const fn from_portal_value(value: u32) -> Self {
        if value == 1 {
            Self::Reduce
        } else {
            Self::NoPreference
        }
    }

    fn from_system_bool(reduced: bool) -> Self {
        if reduced {
            Self::Reduce
        } else {
            Self::NoPreference
        }
    }

    fn is_reduce(self) -> bool {
        matches!(self, Self::Reduce)
    }
}

/// What one platform reduced-motion read produced.
///
/// The built-in iced adapter exposes a boolean and therefore maps its
/// unsupported/error fallback to delivered no-preference. Injected adapters
/// that retain more information can report [`Self::Unsupported`] or
/// [`Self::Failed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReducedMotionRead {
    /// The source answered with a preference or an explicit lack of one.
    Delivered(ReducedMotionPreference),
    /// This host exposes no reduced-motion source; lower layers remain active.
    Unsupported,
    /// The source exists but this read failed; the last delivered value stays active.
    Failed {
        /// Provider-supplied failure detail.
        detail: String,
    },
}

impl ReducedMotionRead {
    fn from_system_bool(reduced: bool) -> Self {
        Self::Delivered(ReducedMotionPreference::from_system_bool(reduced))
    }
}

/// State of the reduced-motion platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReducedMotionStatus {
    /// No platform adapter is enabled.
    Disabled,
    /// The startup snapshot has not arrived yet.
    AwaitingSnapshot,
    /// The adapter delivered a preference.
    Delivered(ReducedMotionPreference),
    /// The host has no supported source.
    Unsupported,
    /// The adapter failed; its last successful delivery remains active.
    Failed {
        /// Provider-supplied failure detail.
        detail: String,
    },
}

/// A typed diagnostic from the live-watch layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeWatchDiagnostic {
    /// The injected backend could not inspect one candidate path.
    ProbeFailed {
        /// Candidate that could not be inspected.
        path: PathBuf,
        /// Backend-provided failure detail.
        error: String,
    },
    /// A candidate probe exceeded the watch timeout.
    ///
    /// Its admission slot remains occupied until completion arrives or worker abandonment is observed.
    ProbeTimedOut {
        /// Candidate whose probe has not returned.
        path: PathBuf,
    },
    /// A timed-out probe worker was destroyed before delivering completion, so its slot was reclaimed.
    ProbeAbandoned {
        /// Candidate whose probe worker disappeared.
        path: PathBuf,
    },
    /// The single active file-read exceeded the watch timeout and is still
    /// running. No second read is started; live reload is paused for that read
    /// until it completes or is retired. Cleared by the next successful read.
    LoadStalled,
    /// A file-read worker vanished before delivering its result, so the read
    /// slot was retired and will be retried. Cleared by the next successful read.
    LoadAbandoned,
    /// A maintenance thread could not be spawned (resource exhaustion). Live
    /// watching is degraded or unavailable; `detail` explains which thread
    /// failed. No retry is attempted, because thread-spawn failure indicates
    /// severe host exhaustion where an immediate retry would only busy-spin.
    WatcherUnavailable {
        /// Which maintenance thread failed to spawn.
        detail: String,
    },
    /// A completion referred to no candidate. This normally indicates an
    /// internal wiring bug in the service.
    InvalidResultIndex {
        /// Index carried by the completion.
        index: usize,
    },
}

/// Reads one session variable, so no test has to mutate process environment
/// shared with every other thread.
pub type EnvReader = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Injectable construction settings for theme integration.
pub struct ThemeIntegrationConfig {
    paths: ThemePaths,
    watch_backend: Arc<dyn FileWatchBackend>,
    probe: ProbeOverride,
    initial_os_mode: EnzThemeMode,
    follow_system: bool,
    follow_reduced_motion: bool,
    poll_interval: Duration,
    debounce: Duration,
    directory_watch: service::DirectoryWatch,
    env: EnvReader,
    application_text_scale: Option<TextScale>,
    fonts: DesktopFontFeed,
}

impl ThemeIntegrationConfig {
    /// Production settings captured once from the process environment.
    ///
    /// Desktop appearance and reduced motion are followed unless a deterministic
    /// probe override suppresses system feeds. The application's own theme
    /// file layers over the shared theme once the application has declared
    /// its id ([`crate::app_identity`]); [`Self::shared_theme_only`] opts out.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            paths: ThemePaths::from_env().with_declared_app(),
            watch_backend: Arc::new(FsBackend),
            probe: ProbeOverride::from_env(),
            initial_os_mode: EnzThemeMode::Dark,
            follow_system: true,
            follow_reduced_motion: true,
            poll_interval: DEFAULT_POLL_INTERVAL,
            debounce: DEFAULT_DEBOUNCE,
            directory_watch: service::DirectoryWatch::Native,
            env: Arc::new(|key| std::env::var(key).ok()),
            application_text_scale: None,
            fonts: watched_desktop_fonts(),
        }
    }

    /// Layer application `app`'s own theme file, `<app>.toml` beside the
    /// shared `theme.toml`, over the shared theme, and follow its changes,
    /// whatever the process declared itself.
    #[must_use]
    pub fn app(mut self, app: crate::app_data::AppId) -> Self {
        self.paths = self.paths.with_app(app);
        self
    }

    /// Read the shared theme alone: no application theme file layers over
    /// it, the declared application's included. For an application that must
    /// look like the desktop whatever its users set for it.
    #[must_use]
    pub fn shared_theme_only(mut self) -> Self {
        self.paths = self.paths.without_app();
        self
    }

    /// Creates injected settings with platform feeds disabled.
    ///
    /// Enable only the host feeds needed with [`Self::follow_system_theme`] and
    /// [`Self::follow_system_reduced_motion`].
    #[must_use]
    pub fn injected(
        paths: ThemePaths,
        watch_backend: Arc<dyn FileWatchBackend>,
        probe: ProbeOverride,
        initial_os_mode: EnzThemeMode,
    ) -> Self {
        Self {
            paths,
            watch_backend,
            probe,
            initial_os_mode,
            follow_system: false,
            follow_reduced_motion: false,
            poll_interval: DEFAULT_POLL_INTERVAL,
            debounce: DEFAULT_DEBOUNCE,
            directory_watch: service::DirectoryWatch::Off,
            env: Arc::new(|_| None),
            application_text_scale: None,
            fonts: DesktopFontFeed::disabled(),
        }
    }

    /// Follows `backend` for the desktop's UI/prose/code font choices.
    #[must_use]
    pub fn desktop_fonts(mut self, backend: Arc<dyn DesktopFontBackend>) -> Self {
        self.fonts = DesktopFontFeed::with_backend(backend);
        self
    }

    /// Replaces the whole font feed, for a host that configured its own.
    #[must_use]
    pub fn font_feed(mut self, feed: DesktopFontFeed) -> Self {
        self.fonts = feed;
        self
    }

    /// Supplies the session reader the text-scale source is read through.
    ///
    /// [`Self::from_env`] reads the real process environment; an injected
    /// integration reads nothing until given one, so a test drives delivery
    /// without touching the host session.
    #[must_use]
    pub fn env_reader(mut self, env: EnvReader) -> Self {
        self.env = env;
        self
    }

    /// Sets the application's compiled text scale: the layer that applies when
    /// no session, theme file or in-application preference asks for one.
    #[must_use]
    pub fn application_text_scale(mut self, scale: Option<TextScale>) -> Self {
        self.application_text_scale = scale;
        self
    }

    /// Enables or disables the iced OS appearance snapshot and subscription.
    ///
    /// A deterministic [`ProbeOverride`] still suppresses this feed.
    #[must_use]
    pub fn follow_system_theme(mut self, follow: bool) -> Self {
        self.follow_system = follow;
        self
    }

    /// Enables or disables the iced/mundy reduced-motion snapshot and subscription.
    ///
    /// A deterministic [`ProbeOverride`] still suppresses this feed. Injected
    /// configurations start disabled, allowing custom hosts to deliver typed
    /// [`ThemeEvent`] values without reading the process session.
    #[must_use]
    pub fn follow_system_reduced_motion(mut self, follow: bool) -> Self {
        self.follow_reduced_motion = follow;
        self
    }

    /// Reports theme-file changes through `backend`'s directory watches.
    ///
    /// While the watch covers every theme path, a change is picked up when
    /// the backend reports it, and the regular poll runs only as a one-minute
    /// safety net. [`Self::from_env`] uses the platform's native watch;
    /// [`Self::injected`] polls only, unless this is set.
    #[must_use]
    pub fn watch_directories_with(
        mut self,
        backend: Arc<dyn crate::file_tree_watch::DirectoryWatchBackend>,
    ) -> Self {
        self.directory_watch = service::DirectoryWatch::Backend(backend);
        self
    }

    /// Sets polling and debounce durations, clamping each to at least one millisecond.
    #[must_use]
    pub fn timing(mut self, poll_interval: Duration, debounce: Duration) -> Self {
        self.poll_interval = poll_interval.max(Duration::from_millis(1));
        self.debounce = debounce.max(Duration::from_millis(1));
        self
    }
}

// Application-authoritative theme state plus the configuration used to build the
// maintenance service. The watchers, probe ownership, timer, and debounce all
// live in the service (see `service::WatchService`), not here.
struct SharedState {
    runtime: ThemeRuntime,
    fonts: DesktopFontFeed,
    follow_system: bool,
    follow_reduced_motion: bool,
    saw_live_os: bool,
    reduced_motion_precedence: ReducedMotionPrecedence,
    reduced_motion_status: ReducedMotionStatus,
    watch_error: Option<ThemeWatchDiagnostic>,
    config: ServiceConfig,
    text_scale_delivery: TextScaleDelivery,
    /// The material policy store's publisher, when this integration follows
    /// the desktop: contrast and transparency reach sampled materials.
    material: Option<enzui_theme::material::MaterialPolicyOwner>,
}

fn apply_reduced_motion_read(shared: &mut SharedState, read: ReducedMotionRead, live: bool) {
    if !live && shared.reduced_motion_precedence.saw_live {
        return;
    }

    let reduced = match read {
        ReducedMotionRead::Delivered(preference) => {
            shared.reduced_motion_status = ReducedMotionStatus::Delivered(preference);
            preference.is_reduce()
        }
        ReducedMotionRead::Unsupported => {
            shared.reduced_motion_status = ReducedMotionStatus::Unsupported;
            false
        }
        ReducedMotionRead::Failed { detail } => {
            shared.reduced_motion_status = ReducedMotionStatus::Failed { detail };
            return;
        }
    };

    let accepted = if live {
        shared.reduced_motion_precedence.on_live(reduced)
    } else {
        shared.reduced_motion_precedence.on_snapshot(reduced)
    };
    if accepted {
        let _ = shared.runtime.set_platform_reduced_motion(reduced);
    }
}

struct ReducedMotionRecipe {
    source: Box<dyn iced_futures::subscription::Recipe<Output = bool>>,
}

impl iced_futures::subscription::Recipe for ReducedMotionRecipe {
    type Output = ThemeEvent;

    fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
        use std::hash::Hash as _;
        std::any::TypeId::of::<Self>().hash(state);
        self.source.hash(state);
    }

    fn stream(
        self: Box<Self>,
        input: iced_futures::subscription::EventStream,
    ) -> iced_futures::BoxStream<Self::Output> {
        use iced_futures::futures::StreamExt as _;
        use iced_futures::futures::future;

        // Suppressing an unchanged re-read here is what keeps a repeated host
        // answer from advancing `theme_generation`.
        let mut previous = None;
        iced_futures::boxed_stream(self.source.stream(input).filter_map(move |reduced| {
            let changed = previous != Some(reduced);
            previous = Some(reduced);
            future::ready(changed.then(|| {
                ThemeEvent::ReducedMotionChanged(ReducedMotionRead::from_system_bool(reduced))
            }))
        }))
    }
}

// Wrapping per source recipe keeps the source's own hash in the wrapper's
// identity, so reconciliation reuses one live feed instead of restarting it.
fn reduced_motion_events() -> Subscription<ThemeEvent> {
    let recipes =
        iced_futures::subscription::into_recipes(enzui_theme::reduced_motion_subscription());
    Subscription::batch(
        recipes
            .into_iter()
            .map(|source| iced_futures::subscription::from_recipe(ReducedMotionRecipe { source })),
    )
}

enum IntegrationState {
    Shared(Box<SharedState>),
    Unmanaged { theme: Box<EnzTheme> },
}

/// Framework-owned shared-theme state for one application process.
pub struct ThemeIntegration {
    state: IntegrationState,
    /// The iced projection of the current theme, by the generation it was
    /// made for; cleared by every change.
    projected: std::sync::Mutex<Option<(u64, iced_core::Theme)>>,
}

/// Shares the resolved reduced-motion value with widgets that read the input
/// preferences (the text caret stops blinking).
fn publish_reduced_motion(reduced: bool) {
    if iced_core::preferences::get().reduced_motion != reduced {
        iced_core::preferences::update(|preferences| preferences.reduced_motion = reduced);
    }
}

impl ThemeIntegration {
    /// Boot with the default shared-XDG policy.
    ///
    /// The resolved theme is available immediately (the compiled default until
    /// the service delivers the first file load); platform snapshots are
    /// returned as tasks and do not block the application's reducer or view.
    /// The application's own theme file (`<id>.toml` beside `theme.toml`)
    /// layers over the shared theme once the application has declared its id
    /// ([`crate::app_identity`]), which `DesktopApplicationIdentity` does.
    pub fn boot() -> (Self, Task<ThemeEvent>) {
        Self::boot_with(ThemeIntegrationConfig::from_env())
    }

    /// [`Self::boot`] for application `app`: its own theme file, when the
    /// user made one, layers over the shared theme. `app` is the id the
    /// application names its windows and desktop entry with, such as
    /// `org.example.Reader`. [`Self::boot`] does the same for the id the
    /// application declared ([`crate::app_identity`]); this names it before
    /// any declaration.
    pub fn boot_for(app: crate::app_data::AppId) -> (Self, Task<ThemeEvent>) {
        Self::boot_with(ThemeIntegrationConfig::from_env().app(app))
    }

    /// Boot with injected paths/backends.
    pub fn boot_with(config: ThemeIntegrationConfig) -> (Self, Task<ThemeEvent>) {
        let delivery = read_env_text_scale(|key| (config.env)(key));
        let mut runtime = ThemeRuntime::new(config.probe, config.initial_os_mode);
        let _ = runtime.set_application_text_scale(config.application_text_scale);
        let _ = runtime.set_platform_text_scale(delivery.scale());
        // A configured feed may already carry application overrides.
        let policy = config.fonts.policy().clone();
        let _ = runtime.set_font_policy(policy);
        let _ = text_scale::publish(runtime.text_scale());
        let follows_reduced_motion =
            config.follow_reduced_motion && config.probe == ProbeOverride::None;
        let mut shared = SharedState {
            runtime,
            fonts: config.fonts,
            follow_system: config.follow_system,
            follow_reduced_motion: follows_reduced_motion,
            saw_live_os: false,
            reduced_motion_precedence: ReducedMotionPrecedence::new(),
            reduced_motion_status: if follows_reduced_motion {
                ReducedMotionStatus::AwaitingSnapshot
            } else {
                ReducedMotionStatus::Disabled
            },
            watch_error: None,
            text_scale_delivery: delivery,
            material: None,
            config: ServiceConfig {
                paths: config.paths,
                backend: config.watch_backend,
                probe: config.probe,
                initial_os_mode: config.initial_os_mode,
                poll_interval: config.poll_interval,
                debounce: config.debounce,
                directory_watch: config.directory_watch,
            },
        };
        if shared.follow_system && config.probe == ProbeOverride::None {
            let _ = crate::desktop_settings::spawn_watcher();
            // Owning the store means an application's own preference feed,
            // started later, supersedes this one.
            shared.material = Some(enzui_theme::material::install_policy_owner());
        }
        publish_reduced_motion(shared.runtime.reduced_motion());
        let mut tasks = vec![shared.fonts.renderer_sync(shared.runtime.theme())];
        if shared.follow_system && config.probe == ProbeOverride::None {
            // A desktop that expresses no preference (or a colour-scheme portal
            // that timed out) is not a request for light: it is the absence of
            // a request, so the configured `initial_os_mode` stands and no
            // event is emitted at all. See `Self::subscription` for the live
            // half.
            tasks.push(enzui_theme::current_system_color_scheme().then(|scheme| {
                scheme.theme().map_or_else(Task::none, |theme| {
                    Task::done(ThemeEvent::OsBootstrap(Box::new(theme)))
                })
            }));
        }
        if shared.follow_reduced_motion {
            tasks.push(enzui_theme::current_reduced_motion().map(|reduced| {
                ThemeEvent::ReducedMotionBootstrap(ReducedMotionRead::from_system_bool(reduced))
            }));
        }
        (
            Self {
                state: IntegrationState::Shared(Box::new(shared)),
                projected: std::sync::Mutex::new(None),
            },
            Task::batch(tasks),
        )
    }

    /// Explicitly opts out of shared XDG and platform preference management.
    ///
    /// This produces no boot work and no subscription; the supplied compiled
    /// theme remains under application control.
    pub fn unmanaged_compiled_theme(theme: EnzTheme) -> (Self, Task<ThemeEvent>) {
        (
            Self {
                state: IntegrationState::Unmanaged {
                    theme: Box::new(theme),
                },
                projected: std::sync::Mutex::new(None),
            },
            Task::none(),
        )
    }

    /// Which integration policy is active.
    pub fn management(&self) -> ThemeManagement {
        match self.state {
            IntegrationState::Shared(_) => ThemeManagement::SharedXdg,
            IntegrationState::Unmanaged { .. } => ThemeManagement::UnmanagedCompiled,
        }
    }

    /// Folds one event produced by the integration subscription.
    pub fn update(&mut self, event: ThemeEvent) -> Task<ThemeEvent> {
        self.forget_projection();
        let IntegrationState::Shared(shared) = &mut self.state else {
            return Task::none();
        };
        match event {
            ThemeEvent::Loaded(load) => {
                shared.runtime.accept(load);
                let _ = text_scale::publish(shared.runtime.text_scale());
            }
            ThemeEvent::OsBootstrap(theme) => {
                if !shared.saw_live_os {
                    shared.runtime.set_os_mode(theme.mode());
                }
            }
            ThemeEvent::OsChanged(theme) => {
                shared.saw_live_os = true;
                shared.runtime.set_os_mode(theme.mode());
            }
            ThemeEvent::ReducedMotionBootstrap(read) => {
                apply_reduced_motion_read(shared, read, false);
            }
            ThemeEvent::ReducedMotionChanged(read) => {
                apply_reduced_motion_read(shared, read, true);
            }
            ThemeEvent::Fonts(event) => {
                if shared.fonts.update(event) {
                    let policy = shared.fonts.policy().clone();
                    let _ = shared.runtime.set_font_policy(policy);
                }
            }
            ThemeEvent::WatchDiagnostic(diagnostic) => {
                shared.watch_error = diagnostic;
            }
            ThemeEvent::Desktop(settings) => {
                let _ = shared.runtime.set_platform_accent(settings.accent);
                let high_contrast = settings.high_contrast();
                let _ = shared
                    .runtime
                    .set_platform_high_contrast(high_contrast.unwrap_or(false));
                if let Some(owner) = &shared.material {
                    let mut preferences = enzui_theme::material::PreferenceSet::NOTHING;
                    preferences.increased_contrast = high_contrast;
                    let _ = owner.publish_system(preferences);
                }
                // A session text scale (the environment) outranks the
                // desktop setting within the platform layer.
                if shared.text_scale_delivery.scale().is_none() {
                    let scale = settings.text_scale.map(|factor| TextScale::admit(factor).0);
                    let _ = shared.runtime.set_platform_text_scale(scale);
                    let _ = text_scale::publish(shared.runtime.text_scale());
                }
            }
        }
        publish_reduced_motion(shared.runtime.reduced_motion());
        // File loads can change the family without a desktop event; unchanged families are deduplicated.
        shared.fonts.renderer_sync(shared.runtime.theme())
    }

    /// Runtime subscriptions for file, appearance, reduced-motion, and font changes.
    ///
    /// The maintenance service is independent of windows, so this works for
    /// `iced::daemon` even while it has no open window. Its recipe identity is
    /// stable across the application lifetime, so the runtime keeps one service
    /// (and its established watcher baselines) alive across every subscription
    /// reconciliation.
    pub fn subscription(&self) -> Subscription<ThemeEvent> {
        let IntegrationState::Shared(shared) = &self.state else {
            return Subscription::none();
        };
        let mut subscriptions = vec![iced_futures::subscription::from_recipe(
            ThemeServiceRecipe {
                config: shared.config.clone(),
            },
        )];
        // Keyed by backend identity, so reconciliation keeps one font worker.
        subscriptions.push(shared.fonts.subscription().map(ThemeEvent::Fonts));
        if shared.follow_system && shared.runtime.probe_override() == ProbeOverride::None {
            // Drop "no preference" so it cannot overwrite an application or file-selected mode.
            subscriptions.push(enzui_theme::system_color_scheme_subscription().filter_map(
                |scheme| {
                    scheme
                        .theme()
                        .map(|theme| ThemeEvent::OsChanged(Box::new(theme)))
                },
            ));
        }
        if shared.follow_reduced_motion {
            subscriptions.push(reduced_motion_events());
        }
        if shared.follow_system && shared.runtime.probe_override() == ProbeOverride::None {
            subscriptions.push(
                crate::desktop_settings::subscription()
                    .map(|settings| ThemeEvent::Desktop(Box::new(settings))),
            );
        }
        Subscription::batch(subscriptions)
    }

    /// [`Self::theme`] projected for iced, for an application's `theme()`.
    /// Made once per theme change and cloned after (a pointer copy), so a
    /// window redraw does not rebuild the palette.
    #[must_use]
    pub fn iced_theme(&self) -> iced_core::Theme {
        let generation = self.theme_generation();
        let mut projected = self
            .projected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((made_for, theme)) = projected.as_ref()
            && *made_for == generation
        {
            return theme.clone();
        }
        let theme = self.theme().iced_theme();
        *projected = Some((generation, theme.clone()));
        theme
    }

    fn forget_projection(&mut self) {
        *self
            .projected
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// The resolved theme to hand to iced. This never performs filesystem I/O.
    pub fn theme(&self) -> &EnzTheme {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.theme(),
            IntegrationState::Unmanaged { theme } => theme,
        }
    }

    /// Sets a process-local colour mode above the file and desktop preferences.
    ///
    /// `None` resumes the file/desktop mode. Capture overrides retain priority.
    /// Returns whether the published theme changed. Unmanaged themes ignore it;
    /// no theme file is written.
    pub fn set_mode_override(&mut self, mode: Option<EnzThemeMode>) -> bool {
        self.forget_projection();
        let IntegrationState::Shared(shared) = &mut self.state else {
            return false;
        };
        shared.runtime.set_mode_override(mode)
    }

    /// Monotonic generation for app-owned build-time cache invalidation.
    ///
    /// Rebuild cached fonts, breakpoints, or layout inputs whenever this value
    /// differs from the value seen at the previous update.
    #[must_use]
    pub fn theme_generation(&self) -> u64 {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.theme_generation(),
            IntegrationState::Unmanaged { .. } => 0,
        }
    }

    /// Typed load status. Unmanaged integrations report
    /// [`ThemeStatus::UnmanagedCompiled`].
    #[must_use]
    pub fn status(&self) -> &ThemeStatus {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.status(),
            IntegrationState::Unmanaged { .. } => &ThemeStatus::UnmanagedCompiled,
        }
    }

    /// Value-level diagnostics from the currently active document.
    #[must_use]
    pub fn diagnostics(&self) -> &[super::Diagnostic] {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.document().diagnostics(),
            IntegrationState::Unmanaged { .. } => &[],
        }
    }

    /// The text-size factor the served theme carries, after full precedence.
    ///
    /// The same factor is published to [`text_scale::effective`], so framework
    /// text that resolves without this theme's typography agrees with text that
    /// resolves through it.
    #[must_use]
    pub fn text_scale(&self) -> TextScale {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.text_scale(),
            IntegrationState::Unmanaged { theme } => theme.text_scale(),
        }
    }

    /// Which layer supplied [`Self::text_scale`].
    ///
    /// An unmanaged integration reports [`TextScaleSource::Application`]: its
    /// theme, and therefore its type ramp, is the application's own.
    #[must_use]
    pub fn text_scale_source(&self) -> TextScaleSource {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.text_scale_source(),
            IntegrationState::Unmanaged { .. } => TextScaleSource::Application,
        }
    }

    /// What the session source said at boot.
    ///
    /// [`TextScaleDelivery::Unavailable`] is the ordinary case on a desktop
    /// that delivers no text scale; the served theme then follows the theme
    /// file, the application default, or 100%.
    #[must_use]
    pub fn text_scale_delivery(&self) -> &TextScaleDelivery {
        match &self.state {
            IntegrationState::Shared(shared) => &shared.text_scale_delivery,
            IntegrationState::Unmanaged { .. } => &TextScaleDelivery::Unavailable,
        }
    }

    /// Sets or clears the explicit in-application text scale — an accessibility
    /// control's value, outranking every other layer.
    ///
    /// Passing `None` gives the preference back to the shared theme file,
    /// session delivery and the application default, in that order. Returns
    /// whether the served theme changed; an unmanaged integration ignores this
    /// and returns `false`.
    pub fn set_user_text_scale(&mut self, scale: Option<TextScale>) -> bool {
        self.forget_projection();
        self.apply_text_scale(|runtime| runtime.set_user_text_scale(scale))
    }

    /// Applies a text scale delivered by a host-owned session or desktop feed.
    ///
    /// [`TextScaleDelivery::Unavailable`] withdraws the platform layer, which
    /// is what a backend that disappears must do: the layers below it take over
    /// rather than the last delivered value sticking.
    pub fn accept_text_scale_delivery(&mut self, delivery: TextScaleDelivery) -> bool {
        self.forget_projection();
        let scale = delivery.scale();
        if let IntegrationState::Shared(shared) = &mut self.state {
            shared.text_scale_delivery = delivery;
        }
        self.apply_text_scale(|runtime| runtime.set_platform_text_scale(scale))
    }

    /// Drops the in-application, session and compiled-default layers, leaving
    /// the shared theme file's request (and 100% when it makes none).
    pub fn reset_text_scale(&mut self) -> bool {
        self.forget_projection();
        if let IntegrationState::Shared(shared) = &mut self.state {
            shared.text_scale_delivery = TextScaleDelivery::Unavailable;
        }
        self.apply_text_scale(ThemeRuntime::reset_text_scale)
    }

    fn apply_text_scale(&mut self, edit: impl FnOnce(&mut ThemeRuntime) -> bool) -> bool {
        let IntegrationState::Shared(shared) = &mut self.state else {
            return false;
        };
        let changed = edit(&mut shared.runtime);
        let _ = text_scale::publish(shared.runtime.text_scale());
        changed
    }

    /// Why the desktop font-preference source is serving what it serves.
    #[must_use]
    pub fn font_status(&self) -> &FontFeedStatus {
        match &self.state {
            IntegrationState::Shared(shared) => shared.fonts.status(),
            IntegrationState::Unmanaged { .. } => &FontFeedStatus::Disabled,
        }
    }

    /// Sets or clears the application's explicit family for `role`, the layer
    /// that outranks the desktop.
    pub fn set_font_override(
        &mut self,
        role: enzui_theme::FontRole,
        family: Option<enzui_theme::FontFamily>,
    ) -> Task<ThemeEvent> {
        let IntegrationState::Shared(shared) = &mut self.state else {
            return Task::none();
        };
        if shared.fonts.set_override(role, family) {
            let policy = shared.fonts.policy().clone();
            let _ = shared.runtime.set_font_policy(policy);
        }
        shared.fonts.renderer_sync(shared.runtime.theme())
    }

    /// The effective reduced-motion value carried by [`Self::theme`].
    #[must_use]
    pub fn reduced_motion(&self) -> bool {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.reduced_motion(),
            IntegrationState::Unmanaged { theme } => theme.motion.reduced_motion,
        }
    }

    /// Which precedence layer supplies [`Self::reduced_motion`].
    #[must_use]
    pub fn reduced_motion_source(&self) -> ReducedMotionSource {
        match &self.state {
            IntegrationState::Shared(shared) => shared.runtime.reduced_motion_source(),
            IntegrationState::Unmanaged { .. } => ReducedMotionSource::UnmanagedCompiled,
        }
    }

    /// Sets or clears the explicit in-application reduced-motion override.
    ///
    /// The override outranks the theme file and platform. Returns whether the
    /// served theme changed; unmanaged integrations ignore it.
    pub fn set_reduced_motion_override(&mut self, reduced: Option<bool>) -> bool {
        self.forget_projection();
        let IntegrationState::Shared(shared) = &mut self.state else {
            return false;
        };
        let changed = shared.runtime.set_reduced_motion_override(reduced);
        publish_reduced_motion(shared.runtime.reduced_motion());
        changed
    }

    /// Why the platform reduced-motion adapter is serving its current value.
    #[must_use]
    pub fn reduced_motion_status(&self) -> &ReducedMotionStatus {
        match &self.state {
            IntegrationState::Shared(shared) => &shared.reduced_motion_status,
            IntegrationState::Unmanaged { .. } => &ReducedMotionStatus::Disabled,
        }
    }

    /// The current watcher or loader degradation, if any.
    #[must_use]
    pub fn watch_diagnostic(&self) -> Option<&ThemeWatchDiagnostic> {
        match &self.state {
            IntegrationState::Shared(shared) => shared.watch_error.as_ref(),
            IntegrationState::Unmanaged { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests;

// Portal settings carry live desktop choices; files cover desktops without font keys.
fn watched_desktop_fonts() -> DesktopFontFeed {
    let files: Arc<dyn DesktopFontBackend> = match crate::file_tree_watch::native_directory_watch()
    {
        Some(watch) => Arc::new(
            crate::font_preferences::watched_file::WatchedSettingsFontBackend::new(
                enzui_theme::font_source::SettingsFileFontBackend::from_env(),
                watch,
            ),
        ),
        None => Arc::new(enzui_theme::font_source::SettingsFileFontBackend::from_env()),
    };
    #[cfg(feature = "desktop-font-portal")]
    let files: Arc<dyn DesktopFontBackend> =
        Arc::new(crate::font_preferences::FallbackFontBackend::new(
            Arc::new(crate::font_preferences::portal::PortalFontBackend::new()),
            files,
        ));
    DesktopFontFeed::with_backend(files)
}
