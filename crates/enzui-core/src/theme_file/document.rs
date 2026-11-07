//! Parsing, validation, overlay resolution, and canonical serialization for theme-file TOML.
//!
//! Syntax and excessive-nesting errors reject the document. Invalid token values are dropped individually and reported as [`Diagnostic`] values.
//!
//! A parsed document writes back as an edit of the file it came from: keys
//! and values this build does not know (a newer release's), and comments,
//! stay as they were.

use super::registry::{self, TokenKind, TokenScope, TokenValue};
use enzui_theme::{EnzTheme, EnzThemeMode};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use toml_edit::{DocumentMut, Item, Table, Value};

/// The schema version this build writes and fully understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Maximum accepted table-path and value-container nesting depth.
///
/// Dots inside quoted keys, strings, and comments do not add depth.
pub const MAX_THEME_NESTING_DEPTH: usize = 16;

/// Which colour mode the user asked every enzUI app to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModePreference {
    /// Follow the OS colour scheme (the default).
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl ModePreference {
    /// Parse the `mode` key. Returns `None` for an unknown word.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "system" => Some(ModePreference::System),
            "light" => Some(ModePreference::Light),
            "dark" => Some(ModePreference::Dark),
            _ => None,
        }
    }

    /// The word written back to the file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ModePreference::System => "system",
            ModePreference::Light => "light",
            ModePreference::Dark => "dark",
        }
    }
}

/// Why a key in the file was not applied (or deserves attention anyway).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// The key matches no token in the registry.
    UnknownKey,
    /// The key exists but the value has the wrong TOML type.
    WrongType,
    /// A number that is NaN or infinite.
    NotFinite,
    /// A number outside the token's documented range.
    OutOfRange,
    /// A colour string that is not `#rgb`/`#rgba`/`#rrggbb`/`#rrggbbaa`.
    BadColor,
    /// An enum word outside the token's closed set.
    BadEnum,
    /// A font description that resolves to no family.
    BadFontFamily,
    /// A mode-scoped token written at the top level, or a common token
    /// written inside `[light]`/`[dark]`.
    WrongScope,
    /// The key names a real token that nothing in this build renders
    /// ([`TokenSupport::Unsupported`](super::registry::TokenSupport::Unsupported)).
    ///
    /// Reported only when the value differs from the compiled default, so a
    /// file that merely serialises those defaults loads silently. The key is
    /// dropped either way: storing it would leave an override that changes
    /// nothing and would be written back on the next save.
    Unsupported,
    /// The file declares a schema version this build does not know.
    UnknownVersion,
    /// The file omits `version`.
    MissingVersion,
    /// An application's own theme file could not be used; the system theme
    /// applies alone.
    AppThemeUnusable,
}

impl DiagnosticKind {
    /// Whether the key was dropped (`true`) or applied anyway (`false`).
    #[must_use]
    pub const fn dropped(self) -> bool {
        !matches!(
            self,
            DiagnosticKind::UnknownVersion | DiagnosticKind::MissingVersion
        )
        // `Unsupported` is dropped: the value reaches no renderer, so keeping
        // it would be indistinguishable from applying it.
    }
}

/// One human-actionable complaint about the theme file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The dotted lookup path assembled for the offending key.
    pub key: String,
    /// What is wrong.
    pub kind: DiagnosticKind,
    /// A one-line explanation naming the offending value and the expectation.
    pub detail: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.key, self.detail)
    }
}

/// A TOML parse failure — the only error that rejects a whole file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeParseError {
    /// Invalid TOML, including duplicate keys.
    Syntax {
        /// The parser's message, including line/column.
        message: String,
    },
    /// A table path or nested array/inline-table value exceeded the bound.
    NestingTooDeep {
        /// The first observed depth beyond the limit.
        depth: usize,
        /// The configured maximum accepted depth.
        limit: usize,
    },
}

impl std::fmt::Display for ThemeParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syntax { message } => f.write_str(message),
            Self::NestingTooDeep { depth, limit } => {
                write!(f, "TOML nesting depth {depth} exceeds the limit of {limit}")
            }
        }
    }
}

impl std::error::Error for ThemeParseError {}

// Use parser events to reject excessive container depth before materializing the TOML tree.
fn check_nesting(text: &str) -> Result<(), ThemeParseError> {
    let source = toml_parser::Source::new(text);
    let tokens = source.lex().into_vec();
    let mut probe = NestingProbe::default();
    toml_parser::parser::parse_document(&tokens, &mut probe, &mut ());
    match probe.exceeded {
        Some(depth) => Err(ThemeParseError::NestingTooDeep {
            depth,
            limit: MAX_THEME_NESTING_DEPTH,
        }),
        None => Ok(()),
    }
}

#[derive(Default)]
struct NestingProbe {
    header_depth: usize,
    key_components: usize,
    in_header: bool,
    container_paths: Vec<usize>,
    pending_value_path: Option<usize>,
    exceeded: Option<usize>,
}

impl NestingProbe {
    fn path_base(&self) -> usize {
        self.container_paths
            .last()
            .copied()
            .unwrap_or(self.header_depth)
    }

    fn observe(&mut self, depth: usize) {
        if depth > MAX_THEME_NESTING_DEPTH && self.exceeded.is_none() {
            self.exceeded = Some(depth);
        }
    }

    fn table_open(&mut self) {
        self.in_header = true;
        self.key_components = 0;
        self.pending_value_path = None;
    }

    fn table_close(&mut self) {
        self.header_depth = self.key_components;
        self.observe(self.header_depth);
        self.key_components = 0;
        self.in_header = false;
    }

    fn container_open(&mut self) -> bool {
        let path = self
            .pending_value_path
            .take()
            .unwrap_or_else(|| self.path_base());
        self.container_paths.push(path);
        let container_depth = self.container_paths.len();
        self.observe(container_depth);
        container_depth <= MAX_THEME_NESTING_DEPTH
    }

    fn container_close(&mut self) {
        let _ = self.container_paths.pop();
        self.pending_value_path = None;
        self.key_components = 0;
    }
}

impl toml_parser::parser::EventReceiver for NestingProbe {
    fn std_table_open(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.table_open();
    }

    fn std_table_close(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.table_close();
    }

    fn array_table_open(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.table_open();
    }

    fn array_table_close(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.table_close();
    }

    fn inline_table_open(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) -> bool {
        self.container_open()
    }

    fn inline_table_close(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.container_close();
    }

    fn array_open(
        &mut self,
        _span: toml_parser::Span,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) -> bool {
        self.container_open()
    }

    fn array_close(&mut self, _span: toml_parser::Span, _errors: &mut dyn toml_parser::ErrorSink) {
        self.container_close();
    }

    fn simple_key(
        &mut self,
        _span: toml_parser::Span,
        _kind: Option<toml_parser::decoder::Encoding>,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.key_components = self.key_components.saturating_add(1);
        let depth = if self.in_header {
            self.key_components
        } else {
            self.path_base().saturating_add(self.key_components)
        };
        self.observe(depth);
    }

    fn key_val_sep(&mut self, _span: toml_parser::Span, _errors: &mut dyn toml_parser::ErrorSink) {
        let depth = self.path_base().saturating_add(self.key_components);
        self.observe(depth);
        self.pending_value_path = Some(depth);
        self.key_components = 0;
    }

    fn scalar(
        &mut self,
        _span: toml_parser::Span,
        _kind: Option<toml_parser::decoder::Encoding>,
        _errors: &mut dyn toml_parser::ErrorSink,
    ) {
        self.pending_value_path = None;
    }

    fn value_sep(&mut self, _span: toml_parser::Span, _errors: &mut dyn toml_parser::ErrorSink) {
        self.pending_value_path = None;
        self.key_components = 0;
    }
}

/// A validated theme document containing overrides and parse diagnostics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeDocument {
    version: u32,
    name: Option<String>,
    mode: ModePreference,
    common: BTreeMap<String, TokenValue>,
    light: BTreeMap<String, TokenValue>,
    dark: BTreeMap<String, TokenValue>,
    diagnostics: Vec<Diagnostic>,
    /// The text the document was parsed from; [`Self::to_toml`] edits it.
    source: SourceText,
    /// Full file paths of overrides cleared since parsing.
    cleared: BTreeSet<String>,
    /// Whether the mode was set since parsing.
    mode_set: bool,
    /// Whether the file names a mode, or one was set since: only then does
    /// an app's theme decide the mode over the system theme it is layered on.
    mode_declared: bool,
    application_fonts: BTreeSet<String>,
}

/// The parsed text, which two documents with the same overrides may differ in.
#[derive(Debug, Clone, Default)]
struct SourceText(Option<String>);

impl PartialEq for SourceText {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl ThemeDocument {
    /// An empty document: no overrides, `mode = "system"`, current version.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: SCHEMA_VERSION,
            ..Self::default()
        }
    }

    /// Parse theme-file text.
    ///
    /// Fails on invalid TOML (duplicate keys included) or excessive nesting.
    /// Every other fault becomes a [`Diagnostic`] and leaves the rest of the
    /// file usable.
    ///
    /// # Errors
    ///
    /// A [`ThemeParseError`] for nesting beyond the bound or invalid TOML.
    pub fn parse(text: &str) -> Result<Self, ThemeParseError> {
        check_nesting(text)?;

        let doc: DocumentMut =
            text.parse()
                .map_err(|e: toml_edit::TomlError| ThemeParseError::Syntax {
                    message: e.to_string(),
                })?;

        let mut out = Self::empty();
        out.source = SourceText(Some(text.to_owned()));
        let mut flat: Vec<(String, Value)> = Vec::new();

        for (key, item) in doc.iter() {
            match key {
                "version" => match item.as_integer() {
                    Some(v) if v > 0 => {
                        out.version = u32::try_from(v).unwrap_or(u32::MAX);
                        if out.version != SCHEMA_VERSION {
                            out.diagnostics.push(Diagnostic {
                                key: "version".into(),
                                kind: DiagnosticKind::UnknownVersion,
                                detail: format!(
                                    "file declares schema version {} but this build \
                                     understands {SCHEMA_VERSION}; \
                                     unknown keys are reported, known keys still apply",
                                    out.version
                                ),
                            });
                        }
                    }
                    _ => out.diagnostics.push(Diagnostic {
                        key: "version".into(),
                        kind: DiagnosticKind::WrongType,
                        detail: "expected a positive integer schema version".into(),
                    }),
                },
                "name" => match item.as_str() {
                    Some(s) => out.name = Some(s.to_owned()),
                    None => out.diagnostics.push(Diagnostic {
                        key: "name".into(),
                        kind: DiagnosticKind::WrongType,
                        detail: "expected a string theme name".into(),
                    }),
                },
                "mode" => match item.as_str().and_then(ModePreference::parse) {
                    Some(m) => {
                        out.mode = m;
                        out.mode_declared = true;
                    }
                    None => out.diagnostics.push(Diagnostic {
                        key: "mode".into(),
                        kind: DiagnosticKind::BadEnum,
                        detail: "expected \"system\", \"light\" or \"dark\"".into(),
                    }),
                },
                _ => flatten(key, item, &mut flat, &mut out.diagnostics),
            }
        }

        if !doc.contains_key("version") {
            out.diagnostics.push(Diagnostic {
                key: "version".into(),
                kind: DiagnosticKind::MissingVersion,
                detail: format!("no `version` key; assuming schema version {SCHEMA_VERSION}"),
            });
        }

        for (path, value) in flat {
            out.accept(&path, &value);
        }
        Ok(out)
    }

    fn accept(&mut self, path: &str, value: &Value) {
        let (branch, token_path) = match path.split_once('.') {
            Some(("light", rest)) => (Some(EnzThemeMode::Light), rest),
            Some(("dark", rest)) => (Some(EnzThemeMode::Dark), rest),
            _ => (None, path),
        };

        let Some(desc) = registry::token(token_path) else {
            self.diagnostics.push(Diagnostic {
                key: path.to_owned(),
                kind: DiagnosticKind::UnknownKey,
                detail: format!("no theme token named `{token_path}`; key ignored"),
            });
            return;
        };

        match (desc.scope(), branch) {
            (TokenScope::Mode, None) => {
                self.diagnostics.push(Diagnostic {
                    key: path.to_owned(),
                    kind: DiagnosticKind::WrongScope,
                    detail: format!(
                        "`{token_path}` is mode-scoped; write it under [light.{group}] \
                         or [dark.{group}]",
                        group = desc.group()
                    ),
                });
                return;
            }
            (TokenScope::Common, Some(_)) => {
                self.diagnostics.push(Diagnostic {
                    key: path.to_owned(),
                    kind: DiagnosticKind::WrongScope,
                    detail: format!(
                        "`{token_path}` is shared by both modes; write it at the top \
                         level as [{group}], not inside a mode table",
                        group = desc.group()
                    ),
                });
                return;
            }
            _ => {}
        }

        match coerce(desc.kind(), value) {
            Ok(v) => {
                // Type and range are checked first, so a malformed value on a
                // read-only token still reports its own fault.
                if let Some(reason) = desc.support().reason() {
                    // A file repeating the compiled default asks for nothing
                    // this build cannot deliver, so only a moved value reports.
                    let default =
                        desc.read(&super::default_theme(branch.unwrap_or(EnzThemeMode::Dark)));
                    if v != default {
                        self.diagnostics.push(Diagnostic {
                            key: path.to_owned(),
                            kind: DiagnosticKind::Unsupported,
                            detail: format!(
                                "`{token_path}` is read-only in this build: {reason}; \
                                 the key is ignored"
                            ),
                        });
                    }
                    return;
                }
                let map = match branch {
                    None => &mut self.common,
                    Some(EnzThemeMode::Light) => &mut self.light,
                    Some(EnzThemeMode::Dark) => &mut self.dark,
                };
                map.insert(token_path.to_owned(), v);
            }
            Err((kind, detail)) => self.diagnostics.push(Diagnostic {
                key: path.to_owned(),
                kind,
                detail,
            }),
        }
    }

    /// Returns the effective schema version; a missing or invalid declaration leaves it at [`SCHEMA_VERSION`].
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    /// The optional user-facing theme name.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// The global light/dark/system preference.
    #[must_use]
    pub const fn mode_preference(&self) -> ModePreference {
        self.mode
    }

    /// Set the global light/dark/system preference.
    pub fn set_mode_preference(&mut self, mode: ModePreference) {
        self.mode_set = true;
        self.mode_declared = true;
        self.mode = mode;
    }

    /// Whether the document names a mode: in an application's theme, whether
    /// it decides the mode instead of following the shared theme's.
    #[must_use]
    pub const fn declares_mode(&self) -> bool {
        self.mode_declared
    }

    /// Drop the mode, so the document follows the system (or, as an
    /// application's theme, the shared theme's mode). Saving removes the key.
    pub fn clear_mode_preference(&mut self) {
        self.mode_set = true;
        self.mode_declared = false;
        self.mode = ModePreference::System;
    }

    /// This document with `app`'s overrides on top: an application's own
    /// theme file layered over the system theme. Every token `app` sets
    /// replaces this document's; every token it omits is this document's.
    /// The mode is `app`'s only when its file names one. The result is for
    /// resolving a theme: an editor saves each file from its own document.
    #[must_use]
    pub fn layered(&self, app: &ThemeDocument) -> ThemeDocument {
        let mut out = self.clone();
        out.application_fonts.extend(
            app.common
                .keys()
                .filter(|key| key.starts_with("fonts."))
                .cloned(),
        );
        out.common.extend(app.common.clone());
        out.light.extend(app.light.clone());
        out.dark.extend(app.dark.clone());
        if app.mode_declared {
            out.mode = app.mode;
            out.mode_declared = true;
        }
        if app.name.is_some() {
            out.name.clone_from(&app.name);
        }
        out.diagnostics.extend(app.diagnostics.iter().cloned());
        out.source = SourceText(None);
        out.cleared.clear();
        out.mode_set = false;
        out
    }

    pub(crate) fn has_application_font(&self, role: enzui_theme::FontRole) -> bool {
        let key = match role {
            enzui_theme::FontRole::Ui => "fonts.ui",
            enzui_theme::FontRole::Document => "fonts.document",
            enzui_theme::FontRole::Monospace => "fonts.monospace",
        };
        self.application_fonts.contains(key)
    }

    /// Every diagnostic produced by the parse.
    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Record a complaint found outside this document's own text, such as an
    /// application theme file that could not be layered on it.
    pub fn push_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    /// The number of token overrides this document carries.
    #[must_use]
    pub fn override_count(&self) -> usize {
        self.common.len() + self.light.len() + self.dark.len()
    }

    /// Read one override, if the document sets it.
    ///
    /// `mode` is ignored for common tokens.
    #[must_use]
    pub fn get(&self, path: &str, mode: EnzThemeMode) -> Option<&TokenValue> {
        let desc = registry::token(path)?;
        match (desc.scope(), mode) {
            (TokenScope::Common, _) => self.common.get(path),
            (TokenScope::Mode, EnzThemeMode::Light) => self.light.get(path),
            (TokenScope::Mode, EnzThemeMode::Dark) => self.dark.get(path),
        }
    }

    /// Set one override. Returns `false` when the path is unknown, the token is
    /// [read-only](super::registry::TokenSupport::Unsupported), or the value
    /// does not satisfy the token's kind and range — an editor should surface
    /// that rather than write a file whose key is inert or unloadable.
    pub fn set(&mut self, path: &str, mode: EnzThemeMode, value: TokenValue) -> bool {
        let Some(desc) = registry::token(path) else {
            return false;
        };
        if !desc.support().is_editable() || !value_fits(desc.kind(), &value) {
            return false;
        }
        let _ = self.cleared.remove(&file_path(desc.scope(), path, mode));
        match (desc.scope(), mode) {
            (TokenScope::Common, _) => self.common.insert(path.to_owned(), value),
            (TokenScope::Mode, EnzThemeMode::Light) => self.light.insert(path.to_owned(), value),
            (TokenScope::Mode, EnzThemeMode::Dark) => self.dark.insert(path.to_owned(), value),
        };
        true
    }

    /// Drop one override so the token inherits the built-in default again.
    /// Returns `true` when something was removed.
    pub fn clear(&mut self, path: &str, mode: EnzThemeMode) -> bool {
        let Some(desc) = registry::token(path) else {
            return false;
        };
        let _ = self.cleared.insert(file_path(desc.scope(), path, mode));
        match (desc.scope(), mode) {
            (TokenScope::Common, _) => self.common.remove(path).is_some(),
            (TokenScope::Mode, EnzThemeMode::Light) => self.light.remove(path).is_some(),
            (TokenScope::Mode, EnzThemeMode::Dark) => self.dark.remove(path).is_some(),
        }
    }

    /// Apply this document's overrides on top of the compiled default theme
    /// for `mode`.
    ///
    /// Every token the document does not mention inherits, field by field,
    /// from [`EnzTheme::dark`] / [`EnzTheme::light`].
    pub fn resolve(&self, mode: EnzThemeMode) -> EnzTheme {
        let mut theme = super::default_theme(mode);
        self.apply(&mut theme, mode);
        theme
    }

    /// Applies this document’s overrides to an existing theme for `mode`.
    ///
    /// Existing values remain unchanged for tokens the document omits.
    pub fn apply(&self, theme: &mut EnzTheme, mode: EnzThemeMode) {
        let branch = match mode {
            EnzThemeMode::Light => &self.light,
            EnzThemeMode::Dark => &self.dark,
        };
        // Registry order, not map order: `breakpoints.*` normalises on every
        // write, so the applied order has to be deterministic.
        for desc in registry::tokens() {
            let source = match desc.scope() {
                TokenScope::Common => &self.common,
                TokenScope::Mode => branch,
            };
            if let Some(value) = source.get(desc.path()) {
                desc.write(theme, value);
            }
        }
    }

    /// Serializes the document as theme-file TOML.
    ///
    /// A parsed document is written as an edit of its source: overrides set
    /// or cleared since parsing change their keys, and everything else stays,
    /// including keys and values this build does not understand and comments.
    /// A document built from [`Self::empty`] is written canonically. Tokens
    /// without overrides are omitted and inherit their defaults.
    #[must_use]
    pub fn to_toml(&self) -> String {
        match self
            .source
            .0
            .as_deref()
            .and_then(|text| text.parse::<DocumentMut>().ok())
        {
            Some(doc) => self.edit_source(doc),
            None => self.canonical_toml(),
        }
    }

    fn edit_source(&self, mut doc: DocumentMut) -> String {
        if !doc.contains_key("version") {
            doc["version"] = toml_edit::value(i64::from(self.version));
        }
        if self.mode_set {
            if self.mode_declared {
                doc["mode"] = toml_edit::value(self.mode.as_str());
            } else {
                let _ = doc.remove("mode");
            }
        }
        for path in &self.cleared {
            remove_path(doc.as_table_mut(), path);
        }
        let branches = [
            (TokenScope::Common, EnzThemeMode::Dark, &self.common),
            (TokenScope::Mode, EnzThemeMode::Light, &self.light),
            (TokenScope::Mode, EnzThemeMode::Dark, &self.dark),
        ];
        for (scope, mode, map) in branches {
            for (path, value) in map {
                if let Ok(value) = value.to_toml_value().parse::<Value>() {
                    set_path(doc.as_table_mut(), &file_path(scope, path, mode), value);
                }
            }
        }
        doc.to_string()
    }

    fn canonical_toml(&self) -> String {
        let mut out = String::new();
        out.push_str("# enzUI shared theme. Every enzUI application reads this file.\n");
        out.push_str("# Omitted values inherit from the built-in enzUI Nebula default.\n");
        let _ = writeln!(out, "version = {}", self.version);
        if let Some(name) = &self.name {
            // Use the TOML escaper because Rust `Debug` can emit escapes TOML does not accept.
            let _ = writeln!(out, "name = {}", registry::escape_toml_string(name));
        }
        // An undeclared mode stays out of the file: in an application's theme
        // it would otherwise decide the mode over the shared theme's.
        if self.mode_declared || self.mode != ModePreference::System {
            let _ = writeln!(
                out,
                "mode = {}",
                registry::escape_toml_string(self.mode.as_str())
            );
        }

        write_branch(&mut out, None, &self.common);
        write_branch(&mut out, Some("light"), &self.light);
        write_branch(&mut out, Some("dark"), &self.dark);
        out
    }
}

/// A token's key in the file: mode-scoped tokens sit under `light.` or `dark.`.
fn file_path(scope: TokenScope, path: &str, mode: EnzThemeMode) -> String {
    match (scope, mode) {
        (TokenScope::Common, _) => path.to_owned(),
        (TokenScope::Mode, EnzThemeMode::Light) => format!("light.{path}"),
        (TokenScope::Mode, EnzThemeMode::Dark) => format!("dark.{path}"),
    }
}

/// Set `value` at a dotted `path`, creating tables as needed and keeping the
/// comment around a value it replaces.
fn set_path(table: &mut Table, path: &str, mut value: Value) {
    let mut segments: Vec<&str> = path.split('.').collect();
    let Some(leaf) = segments.pop() else {
        return;
    };
    let mut current: &mut dyn toml_edit::TableLike = table;
    for segment in segments {
        let item = current.entry(segment).or_insert_with(|| {
            let mut table = Table::new();
            table.set_implicit(true);
            Item::Table(table)
        });
        match item.as_table_like_mut() {
            Some(next) => current = next,
            None => return,
        }
    }
    // Replacing the item in place keeps its key, and the comments the key carries.
    if let Some(item) = current.get_mut(leaf) {
        if let Some(old) = item.as_value() {
            *value.decor_mut() = old.decor().clone();
        }
        *item = Item::Value(value);
    } else {
        let _ = current.insert(leaf, Item::Value(value));
    }
}

/// Remove the key at a dotted `path`, if the file has it.
fn remove_path(table: &mut Table, path: &str) {
    let mut segments: Vec<&str> = path.split('.').collect();
    let Some(leaf) = segments.pop() else {
        return;
    };
    let mut current: &mut dyn toml_edit::TableLike = table;
    for segment in segments {
        match current.get_mut(segment).and_then(Item::as_table_like_mut) {
            Some(next) => current = next,
            None => return,
        }
    }
    let _ = current.remove(leaf);
}

fn write_branch(out: &mut String, prefix: Option<&str>, map: &BTreeMap<String, TokenValue>) {
    if map.is_empty() {
        return;
    }
    let mut by_table: BTreeMap<&str, Vec<(&str, &TokenValue)>> = BTreeMap::new();
    for (path, value) in map {
        let (table, leaf) = path.rsplit_once('.').unwrap_or(("", path.as_str()));
        by_table.entry(table).or_default().push((leaf, value));
    }
    for (table, entries) in by_table {
        let header = match prefix {
            Some(p) if table.is_empty() => p.to_owned(),
            Some(p) => format!("{p}.{table}"),
            None => table.to_owned(),
        };
        let _ = write!(out, "\n[{header}]\n");
        for (leaf, value) in entries {
            let _ = writeln!(out, "{leaf} = {}", value.to_toml_value());
        }
    }
}

fn flatten(
    prefix: &str,
    item: &Item,
    out: &mut Vec<(String, Value)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match item {
        Item::Value(Value::InlineTable(table)) => {
            for (k, v) in table {
                flatten(
                    &format!("{prefix}.{k}"),
                    &Item::Value(v.clone()),
                    out,
                    diagnostics,
                );
            }
        }
        Item::Value(value) => out.push((prefix.to_owned(), value.clone())),
        Item::Table(table) => {
            for (k, v) in table {
                flatten(&format!("{prefix}.{k}"), v, out, diagnostics);
            }
        }
        Item::ArrayOfTables(_) | Item::None => diagnostics.push(Diagnostic {
            key: prefix.to_owned(),
            kind: DiagnosticKind::WrongType,
            detail: "arrays of tables are not part of the theme schema".into(),
        }),
    }
}

fn coerce(kind: TokenKind, value: &Value) -> Result<TokenValue, (DiagnosticKind, String)> {
    match kind {
        TokenKind::Color => {
            let text = value.as_str().ok_or_else(|| {
                (
                    DiagnosticKind::WrongType,
                    "expected a colour string like \"#1b1e24\"".to_owned(),
                )
            })?;
            registry::parse_color(text)
                .map(TokenValue::Color)
                .ok_or_else(|| {
                    (
                        DiagnosticKind::BadColor,
                        format!("`{text}` is not #rgb, #rgba, #rrggbb or #rrggbbaa"),
                    )
                })
        }
        TokenKind::Number { min, max, unit } => {
            let n = value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64))
                .ok_or_else(|| {
                    (
                        DiagnosticKind::WrongType,
                        format!("expected a number in {min}..={max}{}", unit.suffix()),
                    )
                })?;
            if !n.is_finite() {
                return Err((
                    DiagnosticKind::NotFinite,
                    format!("`{n}` is not a finite number"),
                ));
            }
            let n = n as f32;
            if n < min || n > max {
                return Err((
                    DiagnosticKind::OutOfRange,
                    format!(
                        "{n}{unit} is outside the accepted range {min}..={max}{unit}",
                        unit = unit.suffix()
                    ),
                ));
            }
            Ok(TokenValue::Number(n))
        }
        TokenKind::Bool => value
            .as_bool()
            .map(TokenValue::Bool)
            .ok_or_else(|| (DiagnosticKind::WrongType, "expected true or false".into())),
        TokenKind::Enum(names) => {
            let text = value.as_str().ok_or_else(|| {
                (
                    DiagnosticKind::WrongType,
                    format!("expected one of {names:?}"),
                )
            })?;
            if names.contains(&text) {
                Ok(TokenValue::Enum(text.to_owned()))
            } else {
                Err((
                    DiagnosticKind::BadEnum,
                    format!("`{text}` is not one of {names:?}"),
                ))
            }
        }
        TokenKind::FontFamily => {
            let text = value.as_str().ok_or_else(|| {
                (
                    DiagnosticKind::WrongType,
                    "expected a font family string".to_owned(),
                )
            })?;
            if registry::parse_font_family(text).is_some() {
                Ok(TokenValue::FontFamily(text.to_owned()))
            } else {
                Err((
                    DiagnosticKind::BadFontFamily,
                    format!("`{text}` names no font family"),
                ))
            }
        }
    }
}

fn value_fits(kind: TokenKind, value: &TokenValue) -> bool {
    kind.accepts(value)
}
