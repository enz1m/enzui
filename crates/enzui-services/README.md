# enzui-services

App and platform services for enzUI applications: the effectful half of
the framework. `enzui-core` keeps the UI architecture and
the models widgets render (the media state machine, downloads, paged text,
annotations); this crate holds the services an app drives from outside its
view.

| module | what | feature |
|---|---|---|
| `media` | GStreamer audio and video services, the curl redirect probe for remote streams, the MPRIS adapter; re-exports core's media model | `gstreamer`, `mpris` |
| `process_transport` | bounded subprocess runs: captured output, deadlines, process groups, background scopes | - |
| `detached_launch` | launching a long-lived program the app does not wait for | - |
| `durable_activation` | activations that outlive the app (systemd user timers) | - |
| `credential_store` | the typed credential store: the Secret Service, or inside Flatpak the Secret portal (`connect_for_app` picks) | `secret-service`, `secret-portal` |
| `document_preview` | PDF page and video poster previews with a bounded cache | `pdf-preview`, `gstreamer` |
| `vcs` | Git repository state read from its files, running nothing | - |
| `oauth` | authorization code + PKCE through the system browser | `oauth` |
| `dns` | SRV and MX lookups for mail server discovery | - |
| `mail_transport` | safe libcurl SMTP/IMAP options | `mail-transport` |
| `spell_check` | spell checkers for `enzui_core::spelling`: Enchant, or Hunspell dictionaries read directly | `spell-check` |
| `event_sound` | play a sound theme's event sound (`message-new-instant`, `complete`) | `gstreamer` |
| `mime_apps` | the desktop's default applications for MIME types, read and written | - |
| `archive` | bounded archive extraction through the system's libarchive | `archive` |
| `desktop_icons` | off-thread desktop icon lookup, PNG and SVG | `desktop-icons`, `desktop-icon-portal` |
| `extension_package` | manifests and package admission for an app's third-party extensions | `extension-package` |

Every heavy backend is off by default, so an app compiles only what it uses.
Widgets never depend on this crate.
