//! Shared XDG theme-file discovery, bounded loading, atomic saving, and runtime publication.
//!
//! Candidate paths follow XDG precedence. Loads run off the UI thread, reject oversized input while reading, and retain the last known-good theme after failure. Symlinked regular files are accepted; non-regular candidates are skipped.

pub mod document;
pub mod integration;
pub mod registry;

pub use document::{
    Diagnostic, DiagnosticKind, MAX_THEME_NESTING_DEPTH, ModePreference, SCHEMA_VERSION,
    ThemeDocument, ThemeParseError,
};

use enzui_theme::font_preferences::FontPreferencePolicy;
use enzui_theme::text_scale::{TextScale, TextScalePolicy, TextScaleSource};
use enzui_theme::{EnzTheme, EnzThemeMode};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The directory every enzUI app looks in, under each XDG config root.
pub const THEME_DIR: &str = "enzui";
/// The file name of the shared theme.
pub const THEME_FILE: &str = "theme.toml";

/// Maximum theme-file size read by the loader: 64 KiB.
pub const MAX_THEME_FILE_BYTES: u64 = 64 * 1024;

/// Returns the shipped enzUI theme for `mode`, used as the base for partial documents.
pub fn default_theme(mode: EnzThemeMode) -> EnzTheme {
    match mode {
        EnzThemeMode::Dark => EnzTheme::dark(),
        EnzThemeMode::Light => EnzTheme::light(),
    }
}

/// Injectable XDG theme lookup.
///
/// Relative roots are ignored, and an absent configuration home falls back to `$HOME/.config`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemePaths {
    config_home: Option<PathBuf>,
    config_dirs: Vec<PathBuf>,
    home: Option<PathBuf>,
    /// The application whose own theme file layers over the shared one.
    app: AppTheme,
}

/// Which application's own theme file layers over the shared one.
#[derive(Clone, Debug, Default)]
enum AppTheme {
    /// None: the shared theme alone, as an editor of it reads.
    #[default]
    Off,
    /// This application's.
    Explicit(crate::app_data::AppId),
    /// The application this process declared itself
    /// ([`crate::app_identity`]), once it has; read each time paths are
    /// listed, so a declaration after boot still takes effect.
    Declared(fn() -> Option<crate::app_data::AppId>),
}

// Two declared sources follow the same declaration: equal, whatever reads it.
impl PartialEq for AppTheme {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Off, Self::Off) | (Self::Declared(_), Self::Declared(_)) => true,
            (Self::Explicit(a), Self::Explicit(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for AppTheme {}

impl ThemePaths {
    /// Builds theme lookup from explicit XDG roots.
    #[must_use]
    pub fn new(
        config_home: Option<impl Into<PathBuf>>,
        config_dirs: impl IntoIterator<Item = impl Into<PathBuf>>,
        home: Option<impl Into<PathBuf>>,
    ) -> Self {
        Self {
            config_home: config_home.map(Into::into),
            config_dirs: config_dirs.into_iter().map(Into::into).collect(),
            home: home.map(Into::into),
            app: AppTheme::Off,
        }
    }

    /// These paths for application `app`: its own theme file,
    /// `<app>.toml` beside the shared `theme.toml` under each root, layers
    /// over the shared theme (see [`ThemeDocument::layered`]).
    #[must_use]
    pub fn with_app(mut self, app: crate::app_data::AppId) -> Self {
        self.app = AppTheme::Explicit(app);
        self
    }

    /// These paths for the application this process declared itself
    /// ([`crate::app_identity::declare`]): its own theme file layers over the
    /// shared one as soon as it is declared, even after these paths were
    /// made.
    #[must_use]
    pub fn with_declared_app(self) -> Self {
        self.with_app_source(crate::app_identity::declared)
    }

    /// [`Self::with_declared_app`] reading the declared application from
    /// `declared`.
    #[must_use]
    pub fn with_app_source(mut self, declared: fn() -> Option<crate::app_data::AppId>) -> Self {
        self.app = AppTheme::Declared(declared);
        self
    }

    /// The shared theme alone: no application's file layers over it.
    #[must_use]
    pub fn without_app(mut self) -> Self {
        self.app = AppTheme::Off;
        self
    }

    /// The application whose theme layers over the shared one now, if any.
    #[must_use]
    pub fn app(&self) -> Option<crate::app_data::AppId> {
        let app = match &self.app {
            AppTheme::Off => None,
            AppTheme::Explicit(app) => Some(app.clone()),
            AppTheme::Declared(declared) => declared(),
        }?;
        // `theme.toml` is the shared file, never an application's.
        (app_file(&app) != THEME_FILE).then_some(app)
    }

    /// Snapshot the process environment once, at boot.
    ///
    /// `$XDG_CONFIG_DIRS` defaults to `/etc/xdg` when unset or empty, as the
    /// specification requires.
    #[must_use]
    pub fn from_env() -> Self {
        fn non_empty(key: &str) -> Option<PathBuf> {
            let v = std::env::var_os(key)?;
            if v.is_empty() {
                None
            } else {
                Some(PathBuf::from(v))
            }
        }
        let dirs = match std::env::var_os("XDG_CONFIG_DIRS") {
            Some(v) if !v.is_empty() => std::env::split_paths(&v).collect(),
            _ => vec![PathBuf::from("/etc/xdg")],
        };
        Self {
            config_home: non_empty("XDG_CONFIG_HOME"),
            config_dirs: dirs,
            home: non_empty("HOME"),
            app: AppTheme::Off,
        }
    }

    /// The theme file paths to try, highest precedence first.
    ///
    /// Relative roots are dropped (XDG spec: "must be absolute"), so a
    /// poisoned `XDG_CONFIG_HOME=.` can never make an app read a theme out of
    /// its current working directory.
    #[must_use]
    pub fn candidates(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let user = self
            .config_home
            .clone()
            .filter(|p| p.is_absolute())
            .or_else(|| self.home.as_ref().map(|h| h.join(".config")));
        if let Some(base) = user.filter(|p| p.is_absolute()) {
            out.push(base.join(THEME_DIR).join(THEME_FILE));
        }
        for dir in &self.config_dirs {
            if dir.is_absolute() {
                out.push(dir.join(THEME_DIR).join(THEME_FILE));
            }
        }
        out
    }

    /// Returns the user theme path for editor writes, or `None` when no user root is available.
    #[must_use]
    pub fn user_path(&self) -> Option<PathBuf> {
        self.user_dir().map(|dir| dir.join(THEME_FILE))
    }

    /// The application theme files to try, highest precedence first: the
    /// same roots as [`Self::candidates`], none without an application.
    #[must_use]
    pub fn app_candidates(&self) -> Vec<PathBuf> {
        let Some(app) = self.app() else {
            return Vec::new();
        };
        let file = app_file(&app);
        self.candidates()
            .into_iter()
            .filter_map(|path| path.parent().map(|dir| dir.join(&file)))
            .collect()
    }

    /// The user's theme file for application `app`, for editor writes.
    #[must_use]
    pub fn app_user_path(&self, app: &crate::app_data::AppId) -> Option<PathBuf> {
        (format!("{}.toml", app.as_str()) != THEME_FILE)
            .then(|| self.user_dir().map(|dir| dir.join(app_file(app))))
            .flatten()
    }

    /// Every file whose change can change the theme: the shared candidates,
    /// then the application's.
    #[must_use]
    pub fn watched(&self) -> Vec<PathBuf> {
        let mut paths = self.candidates();
        paths.extend(self.app_candidates());
        paths
    }

    fn user_dir(&self) -> Option<PathBuf> {
        let base = self
            .config_home
            .clone()
            .filter(|p| p.is_absolute())
            .or_else(|| self.home.as_ref().map(|h| h.join(".config")))?;
        base.is_absolute().then(|| base.join(THEME_DIR))
    }
}

/// The file name of `app`'s own theme.
fn app_file(app: &crate::app_data::AppId) -> String {
    format!("{}.toml", app.as_str())
}

/// A deterministic environment-derived mode override for capture tooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProbeOverride {
    /// No override present.
    #[default]
    None,
    /// `ENZUI_PROBE_LIGHT=1`.
    Light,
    /// `ENZUI_PROBE_DARK=1`.
    Dark,
}

impl ProbeOverride {
    /// Reads `ENZUI_PROBE_LIGHT` and `ENZUI_PROBE_DARK`; light wins when both are set.
    #[must_use]
    pub fn from_env() -> Self {
        let set = |k: &str| matches!(std::env::var(k).as_deref(), Ok("1"));
        if set("ENZUI_PROBE_LIGHT") {
            ProbeOverride::Light
        } else if set("ENZUI_PROBE_DARK") {
            ProbeOverride::Dark
        } else {
            ProbeOverride::None
        }
    }

    /// The mode this override forces, if any.
    #[must_use]
    pub const fn mode(self) -> Option<EnzThemeMode> {
        match self {
            ProbeOverride::None => None,
            ProbeOverride::Light => Some(EnzThemeMode::Light),
            ProbeOverride::Dark => Some(EnzThemeMode::Dark),
        }
    }
}

/// Resolve the active colour mode.
///
/// Precedence, highest first: the probe override, the file's `mode`
/// preference, then the OS colour scheme.
#[must_use]
pub fn effective_mode(
    probe: ProbeOverride,
    preference: ModePreference,
    os_mode: EnzThemeMode,
) -> EnzThemeMode {
    if let Some(forced) = probe.mode() {
        return forced;
    }
    match preference {
        ModePreference::Light => EnzThemeMode::Light,
        ModePreference::Dark => EnzThemeMode::Dark,
        ModePreference::System => os_mode,
    }
}

/// Why a load produced no document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeLoadError {
    /// No candidate path exists — the normal first-run case.
    Absent,
    /// The file exceeded [`MAX_THEME_FILE_BYTES`].
    TooLarge {
        /// Observed size in bytes.
        bytes: u64,
    },
    /// The file is not valid UTF-8.
    NotUtf8,
    /// The file is not valid TOML (a duplicate key lands here).
    Syntax(ThemeParseError),
    /// The file could not be read.
    Io(String),
}

impl std::fmt::Display for ThemeLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeLoadError::Absent => f.write_str("no theme file found; using built-in default"),
            ThemeLoadError::TooLarge { bytes } => write!(
                f,
                "theme file is {bytes} bytes, above the {MAX_THEME_FILE_BYTES}-byte limit"
            ),
            ThemeLoadError::NotUtf8 => f.write_str("theme file is not valid UTF-8"),
            ThemeLoadError::Syntax(e) => write!(f, "theme file has a syntax error: {e}"),
            ThemeLoadError::Io(e) => write!(f, "theme file could not be read: {e}"),
        }
    }
}

/// The result of one off-thread load attempt.
#[derive(Debug, Clone)]
pub struct ThemeLoad {
    /// The generation this load was requested for (see
    /// [`ThemeRuntime::accept`]).
    pub generation: u64,
    /// The file that was read, when one existed.
    pub path: Option<PathBuf>,
    /// The parsed document, or why there is none. With an application's
    /// own theme file it is the shared document with that file layered on.
    pub outcome: Result<ThemeDocument, ThemeLoadError>,
    /// The application's own theme file that was read, when one existed.
    pub app_path: Option<PathBuf>,
}

/// A prepared load that performs I/O only when dispatched or executed.
#[derive(Debug, Clone)]
pub struct ThemeLoadOp {
    candidates: Vec<PathBuf>,
    app_candidates: Vec<PathBuf>,
    generation: u64,
}

impl ThemeLoadOp {
    /// Prepare a load of the first existing candidate path.
    #[must_use]
    pub fn new(paths: &ThemePaths, generation: u64) -> Self {
        Self {
            candidates: paths.candidates(),
            app_candidates: paths.app_candidates(),
            generation,
        }
    }

    /// The paths this op will try, in order.
    #[must_use]
    pub fn candidates(&self) -> &[PathBuf] {
        &self.candidates
    }

    /// Submits this load to a bounded blocking-task owner.
    pub fn dispatch(
        self,
        worker: &crate::blocking_task::BlockingTaskDispatcher,
    ) -> crate::blocking_task::BlockingTask<ThemeLoad> {
        let generation = self.generation;
        worker.submit(generation, move |_| self.execute())
    }

    /// Reads and parses the first candidate that resolves to a regular file,
    /// then layers the application's own theme file over it.
    ///
    /// Symlinks are followed, the byte limit is enforced while reading, non-regular files are skipped, and open failures are reported. An application file that cannot be used leaves the shared theme alone, with a diagnostic naming the file. Do not call this method on an async executor.
    #[must_use]
    pub fn execute(self) -> ThemeLoad {
        let (path, outcome) = read_first(&self.candidates);
        let (app_path, app) = read_first(&self.app_candidates);
        let outcome = match (outcome, app_path.as_ref().map(|_| app)) {
            (outcome, None) => outcome,
            (Ok(shared), Some(Ok(app))) => Ok(shared.layered(&app)),
            (Err(ThemeLoadError::Absent), Some(Ok(app))) => {
                Ok(ThemeDocument::empty().layered(&app))
            }
            (Ok(mut shared), Some(Err(error))) => {
                shared.push_diagnostic(Diagnostic {
                    key: app_path
                        .as_ref()
                        .map_or_else(String::new, |path| path.display().to_string()),
                    kind: DiagnosticKind::AppThemeUnusable,
                    detail: error.to_string(),
                });
                Ok(shared)
            }
            (Err(error), Some(_)) => Err(error),
        };
        ThemeLoad {
            generation: self.generation,
            path,
            outcome,
            app_path,
        }
    }
}

/// The first of `candidates` that is a regular file, read and parsed.
fn read_first(candidates: &[PathBuf]) -> (Option<PathBuf>, Result<ThemeDocument, ThemeLoadError>) {
    for path in candidates {
        let outcome = match read_bounded(path) {
            // Not a regular file (missing, a directory, a symlink to one):
            // try the next candidate, exactly like an absent file.
            BoundedRead::NotRegular => continue,
            BoundedRead::TooLarge { bytes } => Err(ThemeLoadError::TooLarge { bytes }),
            BoundedRead::Io(e) => Err(ThemeLoadError::Io(e)),
            BoundedRead::Bytes(bytes) => match String::from_utf8(bytes) {
                Err(_) => Err(ThemeLoadError::NotUtf8),
                Ok(text) => ThemeDocument::parse(&text).map_err(ThemeLoadError::Syntax),
            },
        };
        return (Some(path.clone()), outcome);
    }
    (None, Err(ThemeLoadError::Absent))
}

// Outcome of one bounded read attempt against a candidate path.
enum BoundedRead {
    NotRegular,
    TooLarge { bytes: u64 },
    Io(String),
    Bytes(Vec<u8>),
}

// Open nonblocking so a FIFO cannot stall, while following symlinks used by configuration managers.
#[cfg(unix)]
fn open_candidate(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(enzui_security::platform::libc_o_nonblock())
        .open(path)
}

// Non-Unix platforms lack a portable nonblocking-open flag; descriptor checks still reject non-regular files.
#[cfg(not(unix))]
fn open_candidate(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::open(path)
}

// Read through `take(MAX_THEME_FILE_BYTES + 1)` so exactly-at-limit and oversized inputs remain distinguishable.
fn read_bounded(path: &Path) -> BoundedRead {
    use std::io::Read;

    let mut file = match open_candidate(path) {
        Ok(f) => f,
        // Skip missing paths; report every other open failure so precedence cannot change silently.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return BoundedRead::NotRegular,
        Err(e) => return BoundedRead::Io(e.to_string()),
    };
    // Inspect the opened descriptor so validation and reading use the same inode.
    let stat_len = match file.metadata() {
        Ok(md) if md.is_file() => md.len(),
        Ok(_) => return BoundedRead::NotRegular,
        Err(e) => return BoundedRead::Io(e.to_string()),
    };

    let cap = MAX_THEME_FILE_BYTES;
    let mut buf = Vec::with_capacity((stat_len.min(cap) as usize) + 1);
    match file.by_ref().take(cap + 1).read_to_end(&mut buf) {
        Ok(_) => {}
        Err(e) => return BoundedRead::Io(e.to_string()),
    }
    if buf.len() as u64 > cap {
        // Report an already-oversized descriptor length; otherwise report the capped read length, which also catches a file that grows after metadata inspection.
        let bytes = if stat_len > cap {
            stat_len
        } else {
            buf.len() as u64
        };
        return BoundedRead::TooLarge { bytes };
    }
    BoundedRead::Bytes(buf)
}

/// Writes exact theme text atomically with mode `0o644`.
///
/// The file and parent directory are synchronized before success is reported; unsupported directory synchronization is tolerated.
///
/// # Errors
///
/// An I/O error when the parent cannot be created or staging, writing or publishing fails.
pub fn save_theme_atomic(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    // `TempStage` creates restrictively, sets mode on the open descriptor, binds cleanup to that inode, and orders write, file sync, rename, and parent sync.
    let mut stage = enzui_security::tempstage::TempStage::create_document(path)?;
    stage.write_all(text.as_bytes())?;
    stage.commit()
}

/// Why the runtime is serving its current theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeStatus {
    /// No file has been loaded yet; serving the compiled default.
    Default,
    /// A file loaded cleanly.
    Loaded {
        /// The file that was read.
        path: PathBuf,
        /// Number of value-level complaints (unknown keys, bad values).
        diagnostics: usize,
    },
    /// A load failed and the previous theme is still being served.
    LastKnownGood {
        /// The typed failure; presentation code may format it with `Display`.
        error: ThemeLoadError,
    },
    /// Shared theming was explicitly bypassed with
    /// [`integration::ThemeIntegration::unmanaged_compiled_theme`].
    UnmanagedCompiled,
}

/// The theme-file layer of the text-scale policy: the factor the shared file
/// asks every enzUI application to use, or `None` when it does not mention one.
///
/// An explicit `typography.text_scale = 1.0` is a request for 100%, and outranks
/// a lower layer asking for more, exactly as `mode = "light"` outranks the OS.
#[must_use]
pub fn document_text_scale(document: &ThemeDocument) -> Option<TextScale> {
    match document.get("typography.text_scale", EnzThemeMode::Dark) {
        Some(registry::TokenValue::Number(factor)) => Some(TextScale::from_backend(*factor)),
        _ => None,
    }
}

/// Which source supplies the effective reduced-motion policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReducedMotionSource {
    /// An explicit in-application override.
    ApplicationOverride,
    /// The shared theme document's `motion.reduced_motion` key.
    ThemeDocument,
    /// A host platform preference requesting reduced motion.
    PlatformPreference,
    /// The animation-enabled framework default.
    Default,
    /// A compiled theme outside shared preference management.
    UnmanagedCompiled,
}

/// The reduced-motion value and the layer that supplied it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedReducedMotion {
    /// Whether non-essential motion should be reduced.
    pub reduced: bool,
    /// The winning precedence layer.
    pub source: ReducedMotionSource,
}

/// Reduced-motion precedence independent of any particular desktop source.
///
/// Resolution is application override, theme document, platform request, then
/// the animation-enabled default. No platform request skips that layer; an
/// explicit `false` from the application or theme remains a real override.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReducedMotionPolicy {
    application_override: Option<bool>,
    platform_reduced: bool,
}

impl ReducedMotionPolicy {
    /// Creates a policy with no application or platform preference.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            application_override: None,
            platform_reduced: false,
        }
    }

    /// Sets or clears the highest-precedence application override.
    pub fn set_application_override(&mut self, reduced: Option<bool>) -> bool {
        if self.application_override == reduced {
            return false;
        }
        self.application_override = reduced;
        true
    }

    /// Sets whether the platform currently requests reduced motion.
    ///
    /// `false` withdraws the platform layer because the standardized
    /// preference represented by mundy has only "reduce" and "no preference"
    /// states.
    pub fn set_platform_reduced(&mut self, reduced: bool) -> bool {
        if self.platform_reduced == reduced {
            return false;
        }
        self.platform_reduced = reduced;
        true
    }

    /// Resolves the effective value against an optional theme-file override.
    #[must_use]
    pub fn resolve(self, theme_override: Option<bool>) -> ResolvedReducedMotion {
        if let Some(reduced) = self.application_override {
            return ResolvedReducedMotion {
                reduced,
                source: ReducedMotionSource::ApplicationOverride,
            };
        }
        if let Some(reduced) = theme_override {
            return ResolvedReducedMotion {
                reduced,
                source: ReducedMotionSource::ThemeDocument,
            };
        }
        if self.platform_reduced {
            return ResolvedReducedMotion {
                reduced: true,
                source: ReducedMotionSource::PlatformPreference,
            };
        }
        ResolvedReducedMotion {
            reduced: false,
            source: ReducedMotionSource::Default,
        }
    }
}

/// The theme-file reduced-motion override, preserving an explicit `false`.
#[must_use]
pub fn document_reduced_motion(document: &ThemeDocument) -> Option<bool> {
    match document.get("motion.reduced_motion", EnzThemeMode::Dark) {
        Some(registry::TokenValue::Bool(reduced)) => Some(*reduced),
        _ => None,
    }
}

/// Publishes an immutable resolved theme and rejects completed loads older than the newest accepted generation.
#[derive(Debug, Clone)]
pub struct ThemeRuntime {
    document: Arc<ThemeDocument>,
    resolved: Arc<EnzTheme>,
    probe: ProbeOverride,
    os_mode: EnzThemeMode,
    mode_override: Option<EnzThemeMode>,
    generation: u64,
    accepted: u64,
    theme_generation: u64,
    status: ThemeStatus,
    text_scale: TextScalePolicy,
    fonts: FontPreferencePolicy,
    reduced_motion: ReducedMotionPolicy,
    /// The desktop's accent colour and contrast preference.
    platform: PlatformAppearance,
}

impl ThemeRuntime {
    /// A runtime serving the compiled default until a load arrives.
    #[must_use]
    pub fn new(probe: ProbeOverride, os_mode: EnzThemeMode) -> Self {
        let document = Arc::new(ThemeDocument::empty());
        let mode = effective_mode(probe, document.mode_preference(), os_mode);
        let text_scale = TextScalePolicy::new();
        let fonts = FontPreferencePolicy::new();
        let reduced_motion = ReducedMotionPolicy::new();
        Self {
            resolved: Arc::new(resolve_theme(
                &document,
                mode,
                text_scale,
                &fonts,
                reduced_motion,
                PlatformAppearance::default(),
            )),
            document,
            probe,
            os_mode,
            mode_override: None,
            generation: 0,
            accepted: 0,
            theme_generation: 0,
            status: ThemeStatus::Default,
            text_scale,
            fonts,
            reduced_motion,
            platform: PlatformAppearance::default(),
        }
    }

    /// Installs the desktop/application font precedence applied on top of the
    /// document's own `fonts.*` keys.
    ///
    /// Returns `true` when the published theme changed. The policy is
    /// re-applied on every publication, so a desktop preference that
    /// disappears falls back to the document's family instead of sticking.
    pub fn set_font_policy(&mut self, fonts: FontPreferencePolicy) -> bool {
        if self.fonts == fonts {
            return false;
        }
        self.fonts = fonts;
        let resolved = resolve_theme(
            &self.document,
            self.mode(),
            self.text_scale,
            &self.fonts,
            self.reduced_motion,
            self.platform,
        );
        if same_families(&self.resolved.font_catalog, &resolved.font_catalog)
            && self.resolved.typography == resolved.typography
        {
            return false;
        }
        self.resolved = Arc::new(resolved);
        self.theme_generation = self.theme_generation.saturating_add(1);
        true
    }

    /// The font precedence currently applied over the document.
    #[must_use]
    pub fn font_policy(&self) -> &FontPreferencePolicy {
        &self.fonts
    }

    /// The text-size factor the published theme carries, after full precedence.
    #[must_use]
    pub fn text_scale(&self) -> TextScale {
        self.text_scale_policy().effective()
    }

    /// Which layer supplied [`Self::text_scale`].
    #[must_use]
    pub fn text_scale_source(&self) -> TextScaleSource {
        self.text_scale_policy().source()
    }

    /// The whole layered policy, including the theme file's own request.
    #[must_use]
    pub fn text_scale_policy(&self) -> TextScalePolicy {
        let mut policy = self.text_scale;
        let _ = policy.set_theme(document_text_scale(&self.document));
        policy
    }

    /// Sets or clears the explicit in-application text scale, the layer that
    /// outranks every other. Returns `true` when the published theme changed.
    pub fn set_user_text_scale(&mut self, scale: Option<TextScale>) -> bool {
        self.update_text_scale(|policy| policy.set_user(scale))
    }

    /// Sets or clears session/desktop delivery. Clearing is what a missing or
    /// withdrawn backend does, and it falls back to the layers below.
    pub fn set_platform_text_scale(&mut self, scale: Option<TextScale>) -> bool {
        self.update_text_scale(|policy| policy.set_platform(scale))
    }

    /// Sets or clears the application's compiled default, the layer used when
    /// nothing else asks for a scale.
    pub fn set_application_text_scale(&mut self, scale: Option<TextScale>) -> bool {
        self.update_text_scale(|policy| policy.set_application(scale))
    }

    /// Drops the user, platform and application layers, leaving whatever the
    /// shared theme file asks for (and 100% when it asks for nothing).
    pub fn reset_text_scale(&mut self) -> bool {
        self.update_text_scale(enzui_theme::TextScalePolicy::reset)
    }

    fn update_text_scale(&mut self, edit: impl FnOnce(&mut TextScalePolicy) -> bool) -> bool {
        let before = self.text_scale();
        let _ = edit(&mut self.text_scale);
        if self.text_scale() == before {
            return false;
        }
        self.republish();
        true
    }

    /// Sets or clears the desktop's accent colour. It replaces the built-in
    /// accent family; a theme file's own `palette.accent` wins over it.
    /// Returns `true` when the published theme changed.
    pub fn set_platform_accent(&mut self, accent: Option<iced_core::Color>) -> bool {
        if self.platform.accent == accent {
            return false;
        }
        self.platform.accent = accent;
        self.republish();
        true
    }

    /// Sets whether the desktop asks for more contrast, which turns the
    /// palette into its high-contrast form ([`enzui_theme::Palette::high_contrast`]).
    /// Returns `true` when the published theme changed.
    pub fn set_platform_high_contrast(&mut self, high_contrast: bool) -> bool {
        if self.platform.high_contrast == high_contrast {
            return false;
        }
        self.platform.high_contrast = high_contrast;
        self.republish();
        true
    }

    /// The effective reduced-motion value carried by the published theme.
    #[must_use]
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion_resolution().reduced
    }

    /// Which precedence layer supplies [`Self::reduced_motion`].
    #[must_use]
    pub fn reduced_motion_source(&self) -> ReducedMotionSource {
        self.reduced_motion_resolution().source
    }

    /// Sets or clears the explicit application reduced-motion override.
    ///
    /// Returns `true` only when the published theme's effective value changes.
    pub fn set_reduced_motion_override(&mut self, reduced: Option<bool>) -> bool {
        self.update_reduced_motion(|policy| policy.set_application_override(reduced))
    }

    /// Applies or withdraws the platform reduced-motion request.
    ///
    /// Returns `true` only when the published theme's effective value changes.
    pub fn set_platform_reduced_motion(&mut self, reduced: bool) -> bool {
        self.update_reduced_motion(|policy| policy.set_platform_reduced(reduced))
    }

    fn reduced_motion_resolution(&self) -> ResolvedReducedMotion {
        self.reduced_motion
            .resolve(document_reduced_motion(&self.document))
    }

    fn update_reduced_motion(
        &mut self,
        edit: impl FnOnce(&mut ReducedMotionPolicy) -> bool,
    ) -> bool {
        let before = self.reduced_motion();
        let _ = edit(&mut self.reduced_motion);
        if self.reduced_motion() == before {
            return false;
        }
        self.republish();
        true
    }

    fn republish(&mut self) {
        let mode = self.mode();
        self.resolved = Arc::new(resolve_theme(
            &self.document,
            mode,
            self.text_scale,
            &self.fonts,
            self.reduced_motion,
            self.platform,
        ));
        self.theme_generation = self.theme_generation.saturating_add(1);
    }

    /// Borrows the resolved theme without performing parsing or filesystem I/O.
    pub fn theme(&self) -> &EnzTheme {
        &self.resolved
    }

    /// The shared handle, for state that stores the theme itself.
    #[must_use]
    pub fn theme_arc(&self) -> Arc<EnzTheme> {
        Arc::clone(&self.resolved)
    }

    /// The document currently in force (an editor starts from this).
    #[must_use]
    pub fn document(&self) -> &ThemeDocument {
        &self.document
    }

    /// The active colour mode after full precedence.
    #[must_use]
    pub fn mode(&self) -> EnzThemeMode {
        self.resolve_mode(&self.document)
    }

    fn resolve_mode(&self, document: &ThemeDocument) -> EnzThemeMode {
        let preference = match self.mode_override {
            Some(EnzThemeMode::Light) => ModePreference::Light,
            Some(EnzThemeMode::Dark) => ModePreference::Dark,
            None => document.mode_preference(),
        };
        effective_mode(self.probe, preference, self.os_mode)
    }

    /// Sets a process-local colour mode above the file and desktop preferences.
    ///
    /// `None` resumes the file/desktop mode. Capture overrides retain priority.
    /// Returns whether the published theme changed; never writes the theme file.
    pub fn set_mode_override(&mut self, mode: Option<EnzThemeMode>) -> bool {
        let before = self.mode();
        self.mode_override = mode;
        if self.mode() == before {
            return false;
        }
        self.republish();
        true
    }

    /// Why the current theme is what it is.
    #[must_use]
    pub fn status(&self) -> &ThemeStatus {
        &self.status
    }

    /// The generation a new request should carry.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// The probe override captured for this runtime.
    #[must_use]
    pub const fn probe_override(&self) -> ProbeOverride {
        self.probe
    }

    /// Monotonic generation of the published theme.
    ///
    /// Unlike [`Self::generation`], which identifies load requests, this
    /// advances when the document or the effective mode changes, and when a
    /// resolved preference policy changes: text scale, desktop font families,
    /// or reduced motion. It is not a diff of the rendered tokens, so a
    /// replacement document that resolves identically still advances it, while
    /// baseline reconciliation, malformed loads, stale loads, and a repeated
    /// identical preference value do not. Applications use it to invalidate
    /// build-time font, breakpoint, and layout caches.
    #[must_use]
    pub const fn theme_generation(&self) -> u64 {
        self.theme_generation
    }

    /// Start a new request: bumps and returns the generation to hand to
    /// [`ThemeLoadOp::new`].
    pub fn next_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Fold in an OS colour-scheme change. Returns `true` when the rendered
    /// theme actually changed (a `mode = "dark"` file ignores the OS).
    pub fn set_os_mode(&mut self, os_mode: EnzThemeMode) -> bool {
        if self.os_mode == os_mode {
            return false;
        }
        let before = self.mode();
        self.os_mode = os_mode;
        let after = self.mode();
        if before == after {
            return false;
        }
        self.republish();
        true
    }

    /// Adopts a completed load unless its generation predates the newest accepted load.
    ///
    /// Older completions return `false`. Failures retain the last known-good theme, except absence returns to the compiled default.
    pub fn accept(&mut self, load: ThemeLoad) -> bool {
        if load.generation < self.accepted {
            return false;
        }
        self.accepted = load.generation;
        self.generation = self.generation.max(load.generation);
        match load.outcome {
            Ok(document) => {
                let diagnostics = document.diagnostics().len();
                self.publish_if_changed(document);
                self.status = ThemeStatus::Loaded {
                    path: load.path.unwrap_or_default(),
                    diagnostics,
                };
                true
            }
            Err(ThemeLoadError::Absent) => {
                self.publish_if_changed(ThemeDocument::empty());
                self.status = ThemeStatus::Default;
                true
            }
            Err(error) => {
                self.status = ThemeStatus::LastKnownGood { error };
                true
            }
        }
    }

    // Increment publication generation only when the document or effective mode changes.
    fn publish_if_changed(&mut self, document: ThemeDocument) -> bool {
        let mode = self.resolve_mode(&document);
        let changed = self.document.as_ref() != &document || self.mode() != mode;
        let document = Arc::new(document);
        if changed {
            self.resolved = Arc::new(resolve_theme(
                &document,
                mode,
                self.text_scale,
                &self.fonts,
                self.reduced_motion,
                self.platform,
            ));
            self.theme_generation = self.theme_generation.saturating_add(1);
        }
        self.document = document;
        changed
    }
}

// Independently resolved catalogs may have different counters for the same families.
fn same_families(a: &enzui_theme::FontCatalog, b: &enzui_theme::FontCatalog) -> bool {
    enzui_theme::FontRole::ALL
        .iter()
        .all(|role| a.family(*role) == b.family(*role))
}

/// What the desktop asks of the palette: its accent colour, and more
/// contrast.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct PlatformAppearance {
    accent: Option<iced_core::Color>,
    high_contrast: bool,
}

// Reapply preference policies to a fresh document so withdrawal restores each fallback.
fn resolve_theme(
    document: &ThemeDocument,
    mode: EnzThemeMode,
    scale_policy: TextScalePolicy,
    fonts: &FontPreferencePolicy,
    reduced_motion: ReducedMotionPolicy,
    platform: PlatformAppearance,
) -> EnzTheme {
    let mut theme = document.resolve(mode);
    // The desktop accent replaces the built-in one, never the theme file's.
    if let Some(accent) = platform.accent
        && document.get("palette.accent", mode).is_none()
    {
        theme.palette = theme.palette.with_accent(accent);
    }
    // More contrast is an accessibility need: it applies over every palette.
    if platform.high_contrast {
        theme.palette = theme.palette.high_contrast();
    }
    crate::font_preferences::apply_to_theme(&mut theme, document, mode, fonts);
    theme.motion.reduced_motion = reduced_motion
        .resolve(document_reduced_motion(document))
        .reduced;
    let mut scale_policy = scale_policy;
    let _ = scale_policy.set_theme(document_text_scale(document));
    theme.with_text_scale(scale_policy.effective())
}

#[cfg(test)]
mod tests;
