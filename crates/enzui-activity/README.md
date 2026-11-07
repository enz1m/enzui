# Background activity publisher

`Publisher` owns one persistent enzui activity v1 connection in the app process
(`$ENZUI_ACTIVITY_SOCKET`, else `$XDG_RUNTIME_DIR/enzui-activity/service.sock`). Its
dedicated current-thread runtime performs all socket I/O. The UI and producer
threads only replace up to 16 desired records under a short memory lock; a
notification coalesces wakeups without retaining a progress-message backlog.

```rust,no_run
use enzui_activity::{Outcome, Publisher, Status};

let publisher = Publisher::new("io.github.enzui.Files").ok();
let job = publisher.as_ref()
    .and_then(|publisher| publisher.start(Status::running("Copying files")).ok());
// Perform the operation regardless of publisher or job admission.
if let Some(job) = job {
    let _ = job.finish(Outcome::Succeeded);
}
```

Retain the publisher for the app lifetime. Clone a job when a worker needs to
report progress; dropping the last unfinished handle removes status without
cancelling or joining work. `finish` reports succeeded, failed or cancelled and
retains the connection for the broker's four-second completion linger. Work that
finishes while disconnected is discarded rather than reported as a new operation
after reconnect. Callers continue their work when local admission or metadata
validation fails.

Use `start_after(status, visibility, delay)` for work that should stay quiet when
it finishes quickly. The publisher retains/coalesces measurements during the delay;
finishing or dropping the handle before the deadline publishes nothing. The SDK
owns this timer, so applications do not need a reveal subscription or polling loop.
The operation itself starts immediately. Ordinary `start` remains immediate, and
`CenterJobs` uses the operation center's existing reveal delay.

Changed records form atomic batches at most once per second. Commands are spaced
at least 260 ms apart, with one outstanding ACK and a two-second connect/write/read
deadline. Active work renews its lease at 30 seconds; terminal work has no heartbeat.
Reconnect backoff starts at 250 ms and caps at eight seconds. Idle publication has
no periodic timer. All framing and record storage is bounded.

Labels and details are caller-localized generic descriptions, never file names,
paths, secrets or raw errors. Unknown progress and ETA remain `None`; counters and
ETA must come from the producer. Pausing clears ETA. A window token must be the
actual native `xdg_toplevel_tag`, using the same tag for origin dialogs. Never
substitute an iced window ID. Without a qualified native tag, omit it and accept
the shell's coarse PID fallback. This crate provides no Flatpak permission or proxy.

## Dock progress elsewhere

Where the activity socket cannot be reached (other desktops, or Flatpak, where
the socket is not exposed), `Publisher::new` shows the live jobs as the
standard `com.canonical.Unity.LauncherEntry.Update` signal for
`application://<app_id>.desktop`: `progress` is the mean completed fraction of
the jobs that measure progress, and `progress-visible` stays true while any job
is live and turns false when the last one ends. Signals are coalesced to at
most 2 Hz, a missing bus is retried at that rate, and none is sent while the
socket serves the jobs, so enzimOS (which bridges the signal into ordinary
jobs) never shows them twice. The dock must implement LauncherEntry progress; emitting the signal does not
guarantee that a particular desktop or extension displays it. Flatpak
manifests need `--talk-name=com.canonical.Unity`. `at_socket` sends no launcher
signal; `at_socket_with_launcher_bus` selects an explicit bus.
`--test launcher_entry` covers emission, rate, hiding, broker suppression and
the missing-bus retry on a private `dbus-daemon`.

`cargo wt --test protocol activity_reconnects_only_live_work_without_resurrecting_completion`
uses a real Unix stream to exercise reconnection, live metadata replacement,
visibility changes and completion removal. The broker's socket/process acceptance
checks live with `enzimos-activity-service`.

## Visibility in the shell

`Publisher::start` defaults to `Visibility::WhenUnfocused`: the shell hides work
while its originating window is active. For work that should remain visible, such
as a timer, use `start_with_visibility(status, Visibility::Always)`. A live job can
change policy through `set_visibility` without replacing progress or window identity.
The default is omitted on the v1 wire for compatibility. The explicit `always`
policy requires the matching enzimOS broker and shell; other desktop launchers
have no corresponding visibility option.

## Native window identity

Use `new_window_token()` to generate a random opaque tag for each origin window; queue it with
`iced::window::set_toplevel_tag(window_id, tag.clone())`, and attach that string
to job metadata only when the task returns `true`. Tag native origin dialogs
with the same string. A successful task queues the protocol request; it is not
an acknowledgement from the compositor. The shell verifies the tag against the
broker's immutable peer PID.

Worker measurements can use `Job::set_progress`, while the UI uses
`Job::set_window_token` after a native reply. Both mutate only their own fields,
so concurrent updates preserve the other producer's metadata and the live phase.

## Shared operation centers

Enable `operation-center` to use `CenterJobs` with the framework's in-app
`ActivityCenter`. Synchronize after reducer turns and its scheduled reveal/expiry.
The adapter follows reveal delays, actual progress, pause/wait states, terminal
outcomes, retries and provider removal without adding a timer. Completed rows
already in the center are never announced as new work. Select rows and provide
localized generic labels through the description callback: private row labels,
paths and error details are deliberately never copied. Pass a successfully queued
native token and the desired `Visibility`. Dropping the adapter removes its live
jobs. Admission failures never interrupt the underlying operation.
