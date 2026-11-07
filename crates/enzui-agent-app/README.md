# enzui-agent-app

The agent-process glue every installed enzUI app shares, so an app writes only
what is its own: how it boots, what keeps it busy and what it presents.

An app an agent can use while no window is open runs as **one process**:

- `--agent-background` (the package's user unit) starts the app's state and
  reducer with no display, renderer or window;
- a desktop launch while that worker runs is forwarded to it over
  `org.freedesktop.Application`, and the worker opens its window over the state
  the agent left, instead of a second process starting;
- a background launch while a window is open exits at once, because that
  window already serves the broker.

## What to reach for first

- `AgentProcess`: the per-process state an app keeps beside its domain model.
- `claim_desktop` / `claim_background`: take or reuse the single instance.
- `print_manifest`: answers `--export-agent-manifest`.
- `disposition`: the usual answer to `BackgroundApp::disposition`.
- `activation_origin`: tells a background start the OS made for a Search query
  apart from the others.

An installed app's session D-Bus service file makes its name activatable, so a
desktop launch, a `DBusActivatable` desktop entry and a direct run of the
executable all reach the unit's one process. That needs no agent credential;
the credential only connects the worker to the broker. A build that is not the
installed executable claims the single instance only when it is provisioned for
agent use, and otherwise stays an independent process.

Off the enzUI OS an installed package may have no user unit for the bus to
start. `claim_desktop` then claims the single instance itself and the launch
opens its window, or forwards to the unit if it came up after all; without a
session bus the app runs independently.

`claim_desktop` forwards both `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID`
without relabeling them. On the application wake, use
`AgentProcess::take_application_calls` to receive each intent with its owned
activation context and action parameters. A windowless worker also requests
presentation; retain the calls until their target window exists. Route each
context through the application window or child launch policy. The compatibility
`take_launches` method returns intents only.

Built on `enzui-agent`.
