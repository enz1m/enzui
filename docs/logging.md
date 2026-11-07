# Diagnostics and logging

The [`log`](https://docs.rs/log) facade carries framework diagnostics: an icon name that does not exist, two controls publishing
the same accessibility id, an actionable control with no name, a layout floor
that cannot be met, a window size that is a placeholder rather than a
measurement. Each message has a target under `enzui::`, so it can be filtered
or routed like any other library's.

| target | what it reports |
|---|---|
| `enzui::a11y` | accessibility tree problems: duplicate ids, actionable nodes without an id or a name |
| `enzui::icon` | icon names missing from the registry (once per name) |
| `enzui::layout` | layout floors the parent cannot honour |
| `enzui::placement` | window sizes ignored as placeholders |
| `enzui::chart` | chart data dropped for a reserved, empty or duplicate id |
| `enzui::drag_drop` | duplicate drag-and-drop collection ids |
| `enzui::agent` | background tasks the agent refused |
| `enzui::trace::*` | opt-in traces, each turned on by its own environment variable (`ENZUI_FRAME_COST`, the diagnostics sink) |

## The default logger

`enzui_widgets::app_setup::run` installs a small logger before the event
loop starts, unless the application installed a logger first. It prints
`LEVEL target: message` lines to stderr and reads `RUST_LOG`:

```sh
RUST_LOG=warn                     # every crate's warnings
RUST_LOG=info,enzui::a11y=off     # more detail, without accessibility warnings
RUST_LOG=enzui=debug              # everything enzUI says
```

Without `RUST_LOG` the filter is `error,enzui=warn,enzui::trace=info`: enzUI's
warnings, other crates' errors only, and the opt-in traces once their variable
is set. The longest
matching target prefix decides, by whole path segments. Applications that do
not use `app_setup::run` call `enzui_core::logging::init_default()` themselves.

## The journal

When stderr already goes to the systemd journal (a systemd service, or a
session that starts apps with their output in the journal; systemd says so
in `JOURNAL_STREAM`), the default logger writes to the journal instead, in
its native protocol: each record keeps its priority (error 3, warning 4,
info 6, debug and trace 7), its target as `LOG_TARGET`, and its source as
`CODE_FILE`, `CODE_LINE` and `CODE_MODULE`. The executable's name is the
`SYSLOG_IDENTIFIER`. `enzui_core::logging::init_journal("notes")` sends to
the journal whatever stderr is.

```sh
journalctl --user -t notes                 # this application
journalctl --user -t notes -p warning      # warnings and errors only
journalctl --user -t notes LOG_TARGET=enzui::a11y
```

## Your own logger

Install any `log` implementation before calling `run` and the default steps
aside:

```rust
fn main() -> iced::Result {
    env_logger::init(); // or tracing_subscriber with its log bridge
    enzui_widgets::app_setup::run(/* .. */)
}
```

With `tracing`, enable `tracing-subscriber`'s `tracing-log` feature (or
`tracing_log::LogTracer::init()`) so `log` records become events.
