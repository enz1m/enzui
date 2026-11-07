//! Desktop integration files generated from one application identity.
//!
//! A Linux desktop finds an application through several files that must
//! agree on its id: the desktop entry (`<id>.desktop`, which also names the
//! Wayland app id windows must carry), the AppStream metainfo
//! (`<id>.metainfo.xml`, read by software centres), the icon (`<id>.svg`),
//! the D-Bus service file (`<id>.service`) of a D-Bus activatable
//! application, and the GNOME Shell search provider (`<id>.search-provider.ini`).
//! [`AppPackage`] holds the identity once; [`AppPackage::files`] writes all
//! of them for an install prefix, and [`AppPackage::check`] reads an
//! installed tree back and reports every file that disagrees with it.
//!
//! The same id goes to
//! `enzui_widgets::app_shell::DesktopApplicationIdentity` for the window
//! app id and to `ApplicationService::connect` for D-Bus activation; see
//! `docs/packaging.md`.

use std::fmt;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Why an [`AppPackage`] was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageError {
    /// The id is not a reverse-DNS application id.
    InvalidId {
        /// The id given.
        id: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// A required text is empty.
    Empty(&'static str),
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId { id, reason } => {
                write!(f, "`{id}` is not an application id: {reason}")
            }
            Self::Empty(field) => write!(f, "the {field} is empty"),
        }
    }
}

impl std::error::Error for PackageError {}

/// Checks `id` against the rules the desktop entry, D-Bus and Flatpak
/// specifications share: at least two dot-separated elements of ASCII
/// letters, digits, `_` and `-`, none empty or starting with a digit, at
/// most 255 bytes.
///
/// # Errors
/// Returns [`PackageError::InvalidId`] naming the rule broken.
pub fn validate_app_id(id: &str) -> Result<(), PackageError> {
    let invalid = |reason| {
        Err(PackageError::InvalidId {
            id: id.to_owned(),
            reason,
        })
    };
    if id.len() > 255 {
        return invalid("longer than 255 bytes");
    }
    let elements: Vec<&str> = id.split('.').collect();
    if elements.len() < 2 {
        return invalid("needs at least two dot-separated elements, like org.example.App");
    }
    for element in elements {
        if element.is_empty() {
            return invalid("has an empty element");
        }
        if element.starts_with(|c: char| c.is_ascii_digit()) {
            return invalid("has an element starting with a digit");
        }
        if !element
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return invalid("may only hold ASCII letters, digits, `_` and `-`");
        }
    }
    Ok(())
}

/// One release for the metainfo: a version and its ISO 8601 date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version, like `1.4.0`.
    pub version: String,
    /// The release date, `YYYY-MM-DD`.
    pub date: String,
    /// What changed, one paragraph per entry.
    pub notes: Vec<String>,
}

/// A file [`AppPackage::files`] produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    /// Where it goes, under the install root.
    pub path: PathBuf,
    /// Its contents.
    pub contents: String,
}

/// An installed file that disagrees with the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageMismatch {
    /// The file.
    pub path: PathBuf,
    /// What is wrong.
    pub problem: String,
}

impl fmt::Display for PackageMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.problem)
    }
}

/// An application's identity and catalogue data, from which every desktop
/// integration file is generated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPackage {
    id: String,
    name: String,
    summary: String,
    command: String,
    description: Vec<String>,
    developer: Option<(String, String)>,
    categories: Vec<String>,
    keywords: Vec<String>,
    mime_types: Vec<String>,
    homepage: Option<String>,
    bug_tracker: Option<String>,
    project_license: Option<String>,
    metadata_license: String,
    dbus_activatable: bool,
    search_provider: bool,
    single_main_window: bool,
    releases: Vec<Release>,
    screenshots: Vec<(String, String)>,
    translations: Vec<(String, String, String)>,
    icon_svg: Option<String>,
}

impl AppPackage {
    /// An application `id` (reverse-DNS, such as `org.example.Notes`) named
    /// `name`, described in one line by `summary`, started by `command` (the
    /// executable's name in the prefix's `bin`).
    ///
    /// # Errors
    /// Returns [`PackageError`] for an invalid id or an empty name, summary
    /// or command.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        summary: impl Into<String>,
        command: impl Into<String>,
    ) -> Result<Self, PackageError> {
        let id = id.into();
        validate_app_id(&id)?;
        let (name, summary, command) = (name.into(), summary.into(), command.into());
        if name.trim().is_empty() {
            return Err(PackageError::Empty("name"));
        }
        if summary.trim().is_empty() {
            return Err(PackageError::Empty("summary"));
        }
        if command.trim().is_empty() {
            return Err(PackageError::Empty("command"));
        }
        Ok(Self {
            id,
            name,
            summary,
            command,
            description: Vec::new(),
            developer: None,
            categories: Vec::new(),
            keywords: Vec::new(),
            mime_types: Vec::new(),
            homepage: None,
            bug_tracker: None,
            project_license: None,
            metadata_license: "CC0-1.0".to_owned(),
            dbus_activatable: false,
            search_provider: false,
            single_main_window: false,
            releases: Vec::new(),
            screenshots: Vec::new(),
            translations: Vec::new(),
            icon_svg: None,
        })
    }

    /// The application id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The long description, one paragraph per entry.
    #[must_use]
    pub fn description(mut self, paragraphs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.description = paragraphs.into_iter().map(Into::into).collect();
        self
    }

    /// The developer: a reverse-DNS id (`org.example`) and a display name.
    #[must_use]
    pub fn developer(mut self, id: impl Into<String>, name: impl Into<String>) -> Self {
        self.developer = Some((id.into(), name.into()));
        self
    }

    /// Menu categories from the Desktop Menu specification (`Office`,
    /// `Utility`, …).
    #[must_use]
    pub fn categories(mut self, categories: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.categories = categories.into_iter().map(Into::into).collect();
        self
    }

    /// Search keywords.
    #[must_use]
    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// MIME types the application opens; the desktop entry then passes files
    /// and URIs on its command line.
    #[must_use]
    pub fn mime_types(mut self, types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.mime_types = types.into_iter().map(Into::into).collect();
        self
    }

    /// The project home page.
    #[must_use]
    pub fn homepage(mut self, url: impl Into<String>) -> Self {
        self.homepage = Some(url.into());
        self
    }

    /// Where to report bugs.
    #[must_use]
    pub fn bug_tracker(mut self, url: impl Into<String>) -> Self {
        self.bug_tracker = Some(url.into());
        self
    }

    /// The project's SPDX licence expression (`GPL-3.0-or-later`).
    #[must_use]
    pub fn project_license(mut self, spdx: impl Into<String>) -> Self {
        self.project_license = Some(spdx.into());
        self
    }

    /// The metainfo file's own licence; default `CC0-1.0`.
    #[must_use]
    pub fn metadata_license(mut self, spdx: impl Into<String>) -> Self {
        self.metadata_license = spdx.into();
        self
    }

    /// Mark the application D-Bus activatable: it serves
    /// `org.freedesktop.Application` (`ApplicationService::connect`), the
    /// desktop entry says `DBusActivatable=true`, and a service file starts
    /// it.
    #[must_use]
    pub fn dbus_activatable(mut self, activatable: bool) -> Self {
        self.dbus_activatable = activatable;
        self
    }

    /// Install the GNOME Shell search provider and KRunner plugin files for
    /// an application that serves search (`ApplicationService::serve_search`).
    /// Implies D-Bus activation.
    #[must_use]
    pub fn search_provider(mut self, provider: bool) -> Self {
        self.search_provider = provider;
        if provider {
            self.dbus_activatable = true;
        }
        self
    }

    /// Declare that the application has one main window (no New Window).
    #[must_use]
    pub fn single_main_window(mut self, single: bool) -> Self {
        self.single_main_window = single;
        self
    }

    /// Add a release, newest first.
    #[must_use]
    pub fn release(mut self, release: Release) -> Self {
        self.releases.push(release);
        self
    }

    /// Add a screenshot by URL with a caption.
    #[must_use]
    pub fn screenshot(mut self, url: impl Into<String>, caption: impl Into<String>) -> Self {
        self.screenshots.push((url.into(), caption.into()));
        self
    }

    /// The name and summary in `language` (`de`, `pt_BR`).
    #[must_use]
    pub fn translation(
        mut self,
        language: impl Into<String>,
        name: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        self.translations
            .push((language.into(), name.into(), summary.into()));
        self
    }

    /// The scalable icon, installed as `<id>.svg` in the hicolor theme.
    #[must_use]
    pub fn icon_svg(mut self, svg: impl Into<String>) -> Self {
        self.icon_svg = Some(svg.into());
        self
    }

    /// The desktop entry (`share/applications/<id>.desktop`).
    #[must_use]
    pub fn desktop_entry(&self) -> String {
        let mut out = String::from("[Desktop Entry]\nType=Application\n");
        let _ = writeln!(out, "Name={}", desktop_value(&self.name));
        for (language, name, _) in &self.translations {
            let _ = writeln!(out, "Name[{language}]={}", desktop_value(name));
        }
        let _ = writeln!(out, "Comment={}", desktop_value(&self.summary));
        for (language, _, summary) in &self.translations {
            let _ = writeln!(out, "Comment[{language}]={}", desktop_value(summary));
        }
        let arguments = if self.mime_types.is_empty() {
            ""
        } else {
            " %U"
        };
        let _ = writeln!(out, "Exec={}{arguments}", self.command);
        let _ = writeln!(out, "Icon={}", self.id);
        out.push_str("Terminal=false\nStartupNotify=true\n");
        let _ = writeln!(out, "StartupWMClass={}", self.id);
        if !self.categories.is_empty() {
            let _ = writeln!(out, "Categories={};", self.categories.join(";"));
        }
        if !self.keywords.is_empty() {
            let _ = writeln!(out, "Keywords={};", self.keywords.join(";"));
        }
        if !self.mime_types.is_empty() {
            let _ = writeln!(out, "MimeType={};", self.mime_types.join(";"));
        }
        if self.dbus_activatable {
            out.push_str("DBusActivatable=true\n");
        }
        if self.single_main_window {
            out.push_str("SingleMainWindow=true\n");
        }
        out
    }

    /// The AppStream metainfo (`share/metainfo/<id>.metainfo.xml`).
    #[must_use]
    pub fn metainfo(&self) -> String {
        let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str("<component type=\"desktop-application\">\n");
        let _ = writeln!(out, "  <id>{}</id>", xml(&self.id));
        let _ = writeln!(out, "  <name>{}</name>", xml(&self.name));
        for (language, name, _) in &self.translations {
            let _ = writeln!(
                out,
                "  <name xml:lang=\"{}\">{}</name>",
                xml(language),
                xml(name)
            );
        }
        let _ = writeln!(out, "  <summary>{}</summary>", xml(&self.summary));
        for (language, _, summary) in &self.translations {
            let _ = writeln!(
                out,
                "  <summary xml:lang=\"{}\">{}</summary>",
                xml(language),
                xml(summary)
            );
        }
        let _ = writeln!(
            out,
            "  <metadata_license>{}</metadata_license>",
            xml(&self.metadata_license)
        );
        if let Some(license) = &self.project_license {
            let _ = writeln!(out, "  <project_license>{}</project_license>", xml(license));
        }
        if let Some((id, name)) = &self.developer {
            let _ = writeln!(out, "  <developer id=\"{}\">", xml(id));
            let _ = writeln!(out, "    <name>{}</name>", xml(name));
            out.push_str("  </developer>\n");
        }
        if !self.description.is_empty() {
            out.push_str("  <description>\n");
            for paragraph in &self.description {
                let _ = writeln!(out, "    <p>{}</p>", xml(paragraph));
            }
            out.push_str("  </description>\n");
        }
        let _ = writeln!(
            out,
            "  <launchable type=\"desktop-id\">{}.desktop</launchable>",
            xml(&self.id)
        );
        if let Some(url) = &self.homepage {
            let _ = writeln!(out, "  <url type=\"homepage\">{}</url>", xml(url));
        }
        if let Some(url) = &self.bug_tracker {
            let _ = writeln!(out, "  <url type=\"bugtracker\">{}</url>", xml(url));
        }
        if !self.categories.is_empty() {
            out.push_str("  <categories>\n");
            for category in &self.categories {
                let _ = writeln!(out, "    <category>{}</category>", xml(category));
            }
            out.push_str("  </categories>\n");
        }
        if !self.keywords.is_empty() {
            out.push_str("  <keywords>\n");
            for keyword in &self.keywords {
                let _ = writeln!(out, "    <keyword>{}</keyword>", xml(keyword));
            }
            out.push_str("  </keywords>\n");
        }
        if !self.mime_types.is_empty() {
            out.push_str("  <provides>\n");
            for mime in &self.mime_types {
                let _ = writeln!(out, "    <mediatype>{}</mediatype>", xml(mime));
            }
            out.push_str("  </provides>\n");
        }
        if !self.screenshots.is_empty() {
            out.push_str("  <screenshots>\n");
            for (index, (url, caption)) in self.screenshots.iter().enumerate() {
                let kind = if index == 0 { " type=\"default\"" } else { "" };
                let _ = writeln!(out, "    <screenshot{kind}>");
                let _ = writeln!(out, "      <caption>{}</caption>", xml(caption));
                let _ = writeln!(out, "      <image>{}</image>", xml(url));
                out.push_str("    </screenshot>\n");
            }
            out.push_str("  </screenshots>\n");
        }
        if !self.releases.is_empty() {
            out.push_str("  <releases>\n");
            for release in &self.releases {
                let _ = writeln!(
                    out,
                    "    <release version=\"{}\" date=\"{}\">",
                    xml(&release.version),
                    xml(&release.date)
                );
                if !release.notes.is_empty() {
                    out.push_str("      <description>\n");
                    for note in &release.notes {
                        let _ = writeln!(out, "        <p>{}</p>", xml(note));
                    }
                    out.push_str("      </description>\n");
                }
                out.push_str("    </release>\n");
            }
            out.push_str("  </releases>\n");
        }
        out.push_str("  <content_rating type=\"oars-1.1\"/>\n");
        out.push_str("  <supports>\n    <control>pointing</control>\n    <control>keyboard</control>\n  </supports>\n");
        out.push_str("</component>\n");
        out
    }

    /// The D-Bus session service file (`share/dbus-1/services/<id>.service`)
    /// starting `<bindir>/<command>`, or `None` unless D-Bus activatable.
    #[must_use]
    pub fn dbus_service(&self, bindir: &Path) -> Option<String> {
        self.dbus_activatable.then(|| {
            format!(
                "[D-BUS Service]\nName={}\nExec={}\n",
                self.id,
                bindir.join(&self.command).display()
            )
        })
    }

    /// The GNOME Shell search provider file
    /// (`share/gnome-shell/search-providers/<id>.search-provider.ini`), or
    /// `None` unless the application provides search.
    #[must_use]
    pub fn search_provider_ini(&self) -> Option<String> {
        self.search_provider.then(|| {
            format!(
                "[Shell Search Provider]\nDesktopId={id}.desktop\nBusName={id}\nObjectPath={path}\nVersion=2\n",
                id = self.id,
                path = crate::activation::application_object_path(&self.id),
            )
        })
    }

    /// The KRunner plugin file (`share/krunner/dbusplugins/<id>.desktop`),
    /// or `None` unless the application provides search.
    #[must_use]
    pub fn krunner_plugin(&self) -> Option<String> {
        self.search_provider.then(|| {
            format!(
                "[Desktop Entry]\nType=Service\nName={name}\nComment={summary}\nIcon={id}\n\
                 X-KDE-ServiceTypes=Plasma/Runner\nX-KDE-PluginInfo-Name={id}\n\
                 X-KDE-PluginInfo-EnabledByDefault=true\nX-Plasma-API=DBus\n\
                 X-Plasma-API-Minimum-Version=2.0\nX-Plasma-DBusRunner-Service={id}\n\
                 X-Plasma-DBusRunner-Path={path}\n",
                name = desktop_value(&self.name),
                summary = desktop_value(&self.summary),
                id = self.id,
                path = crate::activation::application_object_path(&self.id),
            )
        })
    }

    /// Every file, with its path under an install root whose prefix is
    /// `prefix` (`/usr`, `/app` in a Flatpak). Paths are relative to the
    /// install root, so `DESTDIR` staging joins them onto its directory.
    #[must_use]
    pub fn files(&self, prefix: &Path) -> Vec<PackageFile> {
        let relative = prefix.strip_prefix("/").unwrap_or(prefix);
        let share = relative.join("share");
        let id = &self.id;
        let mut files = vec![
            PackageFile {
                path: share.join("applications").join(format!("{id}.desktop")),
                contents: self.desktop_entry(),
            },
            PackageFile {
                path: share.join("metainfo").join(format!("{id}.metainfo.xml")),
                contents: self.metainfo(),
            },
        ];
        if let Some(svg) = &self.icon_svg {
            files.push(PackageFile {
                path: share
                    .join("icons/hicolor/scalable/apps")
                    .join(format!("{id}.svg")),
                contents: svg.clone(),
            });
        }
        if let Some(service) = self.dbus_service(&prefix.join("bin")) {
            files.push(PackageFile {
                path: share.join("dbus-1/services").join(format!("{id}.service")),
                contents: service,
            });
        }
        if let Some(provider) = self.search_provider_ini() {
            files.push(PackageFile {
                path: share
                    .join("gnome-shell/search-providers")
                    .join(format!("{id}.search-provider.ini")),
                contents: provider,
            });
        }
        if let Some(plugin) = self.krunner_plugin() {
            files.push(PackageFile {
                path: share
                    .join("krunner/dbusplugins")
                    .join(format!("{id}.desktop")),
                contents: plugin,
            });
        }
        files
    }

    /// Write [`Self::files`] for `prefix` under `root` (a `DESTDIR` staging
    /// directory, or `/`), creating directories. Returns the paths written.
    ///
    /// # Errors
    /// Returns the first I/O error.
    pub fn write(&self, root: &Path, prefix: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut written = Vec::new();
        for file in self.files(prefix) {
            let path = root.join(&file.path);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, file.contents)?;
            written.push(path);
        }
        Ok(written)
    }

    /// Read the files installed for `prefix` under `root` and report every
    /// one that is missing or disagrees with this identity: the desktop
    /// entry's command, icon, window class and activation, the metainfo's
    /// id and launchable, the service file's name and command, and an icon
    /// to show.
    #[must_use]
    pub fn check(&self, root: &Path, prefix: &Path) -> Vec<PackageMismatch> {
        let mut found = Vec::new();
        let mut report = |path: &Path, problem: String| {
            found.push(PackageMismatch {
                path: path.to_owned(),
                problem,
            });
        };
        let relative = prefix.strip_prefix("/").unwrap_or(prefix);
        let share = root.join(relative).join("share");
        let id = &self.id;

        let desktop_path = share.join("applications").join(format!("{id}.desktop"));
        match std::fs::read_to_string(&desktop_path) {
            Err(_) => report(&desktop_path, "missing".into()),
            Ok(text) => {
                let entry = desktop_group(&text, "Desktop Entry");
                let mut expect = |key: &str, want: Option<&str>| {
                    let have = entry
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(_, v)| v.as_str());
                    if have != want {
                        report(
                            &desktop_path,
                            format!("{key} is {have:?}, the identity says {want:?}"),
                        );
                    }
                };
                expect("Icon", Some(id));
                expect("StartupWMClass", Some(id));
                expect("DBusActivatable", self.dbus_activatable.then_some("true"));
                let exec = entry
                    .iter()
                    .find(|(k, _)| k == "Exec")
                    .map(|(_, v)| v.as_str());
                let program = exec
                    .and_then(|exec| exec.split_whitespace().next())
                    .map(|program| program.rsplit('/').next().unwrap_or(program));
                if program != Some(self.command.as_str()) {
                    report(
                        &desktop_path,
                        format!(
                            "Exec runs {program:?}, the identity's command is {:?}",
                            self.command
                        ),
                    );
                }
            }
        }

        let metainfo_path = share.join("metainfo").join(format!("{id}.metainfo.xml"));
        match std::fs::read_to_string(&metainfo_path) {
            Err(_) => report(&metainfo_path, "missing".into()),
            Ok(text) => {
                if !text.contains(&format!("<id>{}</id>", xml(id))) {
                    report(&metainfo_path, format!("its <id> is not {id}"));
                }
                if !text.contains(&format!(
                    "<launchable type=\"desktop-id\">{}.desktop</launchable>",
                    xml(id)
                )) {
                    report(&metainfo_path, format!("it does not launch {id}.desktop"));
                }
            }
        }

        let service_path = share.join("dbus-1/services").join(format!("{id}.service"));
        match (
            self.dbus_activatable,
            std::fs::read_to_string(&service_path),
        ) {
            (true, Err(_)) => report(
                &service_path,
                "missing for a D-Bus activatable application".into(),
            ),
            (false, Ok(_)) => report(
                &service_path,
                "present, but the application is not D-Bus activatable".into(),
            ),
            (true, Ok(text)) => {
                let service = desktop_group(&text, "D-BUS Service");
                let name = service
                    .iter()
                    .find(|(k, _)| k == "Name")
                    .map(|(_, v)| v.as_str());
                if name != Some(id.as_str()) {
                    report(&service_path, format!("Name is {name:?}, not {id}"));
                }
                let exec = service
                    .iter()
                    .find(|(k, _)| k == "Exec")
                    .map(|(_, v)| v.as_str());
                let program = exec
                    .and_then(|exec| exec.split_whitespace().next())
                    .map(|program| program.rsplit('/').next().unwrap_or(program));
                if program != Some(self.command.as_str()) {
                    report(
                        &service_path,
                        format!(
                            "Exec runs {program:?}, the identity's command is {:?}",
                            self.command
                        ),
                    );
                }
            }
            (false, Err(_)) => {}
        }

        let icons = share.join("icons/hicolor");
        let has_icon = std::fs::read_dir(&icons).ok().is_some_and(|sizes| {
            sizes.flatten().any(|size| {
                ["svg", "png"].iter().any(|ext| {
                    size.path()
                        .join("apps")
                        .join(format!("{id}.{ext}"))
                        .is_file()
                })
            })
        });
        if !has_icon {
            report(&icons, format!("no {id}.svg or {id}.png in any size"));
        }
        found
    }
}

/// A value for a desktop entry string key: newlines and backslashes escaped.
fn desktop_value(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Text for XML element content or an attribute value.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The `key=value` pairs of group `name` in a desktop-entry-style file.
fn desktop_group(text: &str, name: &str) -> Vec<(String, String)> {
    let header = format!("[{name}]");
    let mut inside = false;
    let mut pairs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            inside = line == header;
            continue;
        }
        if !inside || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            pairs.push((key.trim().to_owned(), value.trim().to_owned()));
        }
    }
    pairs
}
