# Crate features

Every Cargo feature of the enzUI crates. **Default** features are on unless
you turn them off; **opt-in** features are for applications; **test-only**
features exist for test targets and measurement builds and belong in
`[dev-dependencies]`, never in what you ship. `crates/enzui-core/tests/feature_reference.rs`
fails when a crate gains a feature this page does not list.

An application usually depends on `enzui-widgets` (with its defaults) and
reaches the rest through its forwarding features; add another crate directly
only for what it alone offers.

## enzui-widgets

| feature | kind | what it turns on |
|---|---|---|
| `x11` | opt-in | the X11 window backend (iced, clipboard) |
| `wayland` | default | the Wayland window backend (iced, clipboard) |
| `desktop-portal` | default | follow the desktop's settings over the Settings portal: accent, contrast, text size, window buttons, input preferences and the UI font; quiet without a session bus |
| `portal-dbus` | opt-in | the XDG desktop portal backend (file chooser, open URI, screen cast, notifications) through `enzui-core/portal-dbus`; starts a Tokio runtime when used |
| `application-dbus` | opt-in | single-instance launching and D-Bus activation (`ApplicationService`) through `enzui-core/application-dbus` |
| `shell-integration` | opt-in | the tray icon and launcher badge through `enzui-core/shell-integration` |
| `emoji-picker` | opt-in | `emoji_picker`: the emoji chooser and the emoji names and shortcodes it searches |
| `locale-formatters` | opt-in | `enzui-core/locale-formatters`: currency, percent, relative-time and duration formatting |
| `internal-test-seams` | test-only | pause seams the `text_baseline_shape_race` target uses |

## enzui-core

| feature | kind | what it turns on |
|---|---|---|
| `accesskit` | opt-in | the AccessKit bridge for the accessibility tree (enzui-widgets' runtime enables it) |
| `application-dbus` | opt-in | `activation::dbus`: `org.freedesktop.Application`, single instance, search provider |
| `shell-integration` | opt-in | `tray` (a status-notifier tray icon with a dbusmenu menu) and `launcher_badge` (launcher count and urgent mark) |
| `thumbnail-cache` | opt-in | `thumbnail_cache`: the freedesktop thumbnail cache, read and written |
| `locale-formatters` | opt-in | currency, percent, relative-time and duration formatting on `Locale` (ICU4X CLDR data) |
| `portal-dbus` | opt-in | the XDG desktop portal D-Bus backend |
| `desktop-settings-portal` | opt-in | the Settings portal source of `desktop_settings` (enzui-widgets' `desktop-portal`) |
| `desktop-font-portal` | opt-in | the Settings portal source of desktop font preferences |
| `a11y-preferences-portal` | opt-in | the Settings portal source of contrast preferences for materials |
| `notification-freedesktop` | opt-in | desktop notifications through `org.freedesktop.Notifications` outside a sandbox |
| `activation-native-iced` | opt-in | the separately published iced activation ABI |
| `native-invalidation` | opt-in | native invalidation diagnostics for real multi-window runs |
| `transition-capture` | opt-in | exact-frame transition capture for performance or capture builds |
| `perf-counter` | opt-in | renderer-free routed-page counting for production view seams |
| `test-frame` | test-only | the headless frame (`collect::collect_frame`), `keyboard_traversal_validation` and `chained_operation`; see [testing.md](testing.md) |
| `perf-report` | test-only | the repeatable workload report (`perf_report`); implies `test-frame` |
| `a11y-hosted-test` | test-only | the hosted accessibility proof's event-loop hooks |
| `linux-native-backends-test` | test-only | both native window backends for backend tests |
| `rust-source-audit` | test-only | source scanners used by the framework's own gates |
| `secret-observer` | test-only | secret lifecycle counters for secure form tests |
| `security-fault-injection` | test-only | fault seams for cleanup race tests |
| `media-test-support` | test-only | fake resolver, probe and clock for media service tests |

## enzui-theme

| feature | kind | what it turns on |
|---|---|---|
| `system-theme` | default | the operating system's colour scheme as tasks and subscriptions |
| `system-reduced-motion` | default | the operating system's reduced-motion setting |
| `desktop-fonts` | default | the desktop font preference feed |
| `desktop-materials` | default | the desktop material preference feed |
| `material-tuning` | opt-in | process-wide material preset overrides |

## enzui-services

All off by default: each one links a backend or claims a bus name.

| feature | kind | what it turns on |
|---|---|---|
| `gstreamer` | opt-in | GStreamer audio and video services, and event sounds (`event_sound`) |
| `mpris` | opt-in | the MPRIS media-controls adapter |
| `pdf-preview` | opt-in | PDF page rendering through the system pdfium |
| `secret-service` | opt-in | the Secret Service backend of the credential store |
| `secret-portal` | opt-in | the Secret portal backend for sandboxed (Flatpak) applications: the portal's per-app master secret seals a keyring file in the app's data directory; `credential_store::connect_for_app` picks it inside a sandbox |
| `oauth` | opt-in | OAuth 2.0 with PKCE through the system browser |
| `mail-transport` | opt-in | libcurl SMTP and IMAP transport |
| `archive` | opt-in | archive extraction through the system libarchive |
| `spell-check` | opt-in | `spell_check`: spelling dictionaries through Enchant, or Hunspell dictionaries read directly |
| `extension-package` | opt-in | extension packages: manifest, admission, store, worker protocol |
| `desktop-icons` | opt-in | freedesktop icon theme lookup |
| `desktop-icon-portal` | opt-in | icon lookup through the portal; implies `desktop-icons` |

## enzui-resources

| feature | kind | what it turns on |
|---|---|---|
| `curl-transport` | default | HTTP(S) loading through the system libcurl |
| `heif` | opt-in | HEIC decoding through a runtime-loaded libheif |
| `local-images` | opt-in | `local_image`: images from local files |
| `first-frame-previews` | opt-in | GIF and APNG posters from their first frame |
| `derived-previews` | opt-in | `derived_preview`: previews derived from other media |
| `image-ops` | opt-in | `ImageOperations`: crop, encode and strip metadata |

## enzui-clipboard-runtime

| feature | kind | what it turns on |
|---|---|---|
| `x11` | opt-in | the X11 clipboard |
| `wayland` | default | the Wayland clipboard |
| `image` | opt-in | clipboard images (PNG between applications) |

## enzui-activity

| feature | kind | what it turns on |
|---|---|---|
| `launcher-entry` | default | dock progress through `com.canonical.Unity.LauncherEntry` when the activity socket is unreachable |
| `operation-center` | opt-in | reconcile the shared in-app operation model through `CenterJobs` without copying private row text |

## enzui-agent

| feature | kind | what it turns on |
|---|---|---|
| `central-connector` | opt-in | the connector to the external `enzui-center` daemon |
| `iced-consumer` | opt-in | connector events as iced subscription messages |
| `task-host` | opt-in | a renderer-free owner for reducer tasks |
| `background` | opt-in | the background runtime an installed app's user unit starts |

## enzui-terminal

| feature | kind | what it turns on |
|---|---|---|
| `x11` | opt-in | the X11 backend, when the crate is built on its own |
| `wayland` | default | the Wayland backend, when the crate is built on its own |

## enzui-security

| feature | kind | what it turns on |
|---|---|---|
| `fault-injection` | test-only | durability and staged-publication fault seams |
| `secret-observer` | test-only | secret allocation and wipe observation |

## enzui-clipboard

| feature | kind | what it turns on |
|---|---|---|
| `test-seal` | test-only | the `for_test` clipboard bootstrap for other crates' tests |

## enzui-test-support

Internal (`publish = false`); the framework's own test helpers.

| feature | kind | what it turns on |
|---|---|---|
| `rust-syntax` | test-only | Rust syntax parsing for source gates |
