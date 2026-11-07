# enzui-agent

The app side of the optional enzUI agent center. An app enrolls with the
external `enzui-center` daemon, authenticates, registers typed functions, and
answers the invocations the daemon routes to it. Without the daemon an app runs
unchanged.

## Without the agent service

The service ships with the enzUI OS; elsewhere it is usually absent, and the
app must work the same. `CenterProvision::discover` returns `None` when no
credential is enrolled, so keep the host as an `Option` and skip it then. A
provisioned app whose daemon is down retries on the connector's thread with
backoff, and a daemon speaking another protocol version stops the connector
for good; neither reaches the UI. `CenterHost::is_available` (registered with
the daemon) is the one test for showing agent-only controls.

## What an app does

1. Declare functions in an `AppFunctionRegistry`, mapping each to the app's
   own `Message` type. Declared consent scopes and capabilities are an upper
   bound; the daemon decides what a call may do.
2. Build a `ConnectorConfig` from the socket path, the credential file and the
   registry's `registration()`.
3. `CentralConnector::spawn` it, drain `CentralConnector::events`, dispatch each
   `ConnectorEvent::Invoke` through the registry, run the work against the
   `InvocationTable`'s cancellation token, and answer with `complete`, `fail`
   or `cancelled`.
4. `CentralConnector::shutdown` joins the background thread with no task left
   running.

`src/bin/central_connector_headless.rs` is a runnable consumer with no GUI.

## Features

| feature | what |
|---|---|
| `central-connector` | the connector itself; nothing is reachable without it |
| `iced-consumer` | an iced `Subscription` that maps connector events to messages off the UI thread |
| `task-host` | one renderer-free owner for reducer tasks across background and windowed delivery |
| `background` | the installed app's windowless reducer/subscription runtime; implies `task-host` |

## Security posture

The connector never listens on a socket, never exposes MCP, never mints grants
and never carries the management credential. Credentials load with the same
descriptor-bound checks as every framework secret (`enzui-security`) and are
held in wiped memory. Enabling the feature is not enough to connect: the app
must also opt in explicitly, so a stray environment variable cannot expose app
data.
