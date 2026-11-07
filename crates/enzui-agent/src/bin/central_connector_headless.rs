//! A GUI-free `enzui-center` app connector for external conformance testing.
//!
//! It enrolls no credentials itself; point it at a daemon socket and a
//! version-1 app credential file that `enzui-center enroll app` produced. It
//! registers two functions and answers dispatched calls off the connector's
//! background thread, honouring central cancellation and clean shutdown.
//!
//! ```text
//! enzui-center enroll app --id org.enzui.Example --name Example --output app.cred
//! cargo run -p enzui-agent --features central-connector \
//!     --bin enzui-agent-connector -- \
//!     --socket "$XDG_RUNTIME_DIR/enzui/center.sock" --credential app.cred
//! ```
//!
//! The daemon's enrollment flags are `--id`, `--name` and `--output`; the
//! socket it serves lives in a 0700 directory, which this connector requires
//! before it authenticates.
//!
//! # Shutdown
//!
//! The connector's own `Stopped` event, or the daemon closing for good, ends
//! the loop: every in-flight invocation is cancelled *before* its worker is
//! joined, so a long call cannot hold exit open. A signal is not a graceful
//! path here: `SIGTERM`'s default disposition ends the process, and the
//! framework has no signal capability to hook (`close_guard` documents
//! session teardown and signals as non-vetoable). A real application that
//! wants one installs its own handler and calls
//! [`CentralConnector::shutdown`] from it.
//!
//! Flags (an `ENZUI_CENTER_*` environment variable is a fallback for each):
//! - `--socket <PATH>`      the daemon socket (`ENZUI_CENTER_SOCKET`).
//! - `--credential <PATH>`  the app credential file (`ENZUI_CENTER_CREDENTIAL`).
//! - `--app-id <ID>`        the app id, default `org.enzui.Example`
//!   (`ENZUI_CENTER_APP_ID`); it must equal the credential id.
//! - `--help` / `-h`        print usage and the function schemas.
//!
//! `--export-agent-manifest` writes the package catalog from this same registry,
//! before credential loading or any connection.
//!
//! Neither credentials nor call arguments/results are ever printed.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a command-line tool reports by panicking and printing, and none of it is public API"
)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::{Sender, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use enzui_agent::cancellation::CancelToken;
use enzui_agent::center::{
    AppFunction, AppFunctionRegistry, AppMetadata, CentralConnector, ConnectorConfig,
    ConnectorEvent, ConsentScope, EmitDecision, FunctionRisk, FunctionSpec, Invocation,
    InvocationEvent, InvocationTable, SendError, Uuid,
};
use serde::Deserialize;
use serde_json::{Value, json};

const USAGE: &str = "\
enzui-agent-connector - enzui-center app conformance connector

USAGE:
    enzui-agent-connector --socket <PATH> --credential <PATH> [--app-id <ID>]

FLAGS:
    --socket <PATH>      Daemon Unix socket path      [env: ENZUI_CENTER_SOCKET]
    --credential <PATH>  Version-1 app credential file [env: ENZUI_CENTER_CREDENTIAL]
    --app-id <ID>        App id (default org.enzui.Example) [env: ENZUI_CENTER_APP_ID]
    --export-agent-manifest  Export the default app package catalog without connecting
    -h, --help           Print this help and the function schemas

FUNCTIONS:
    echo   (read_only, consent: once|session)
      input:  {\"type\":\"object\",\"additionalProperties\":false,
               \"required\":[\"text\"],
               \"properties\":{\"text\":{\"type\":\"string\"}}}
      output: {\"type\":\"object\",\"additionalProperties\":false,
               \"required\":[\"echo\"],
               \"properties\":{\"echo\":{\"type\":\"string\"}}}

    sleep  (mutating, cancellable, progress, consent: once)
      input:  {\"type\":\"object\",\"additionalProperties\":false,
               \"required\":[\"duration_ms\"],
               \"properties\":{\"duration_ms\":
                 {\"type\":\"integer\",\"minimum\":0,\"maximum\":600000}}}
      output: {\"type\":\"object\",\"additionalProperties\":false,
               \"required\":[\"slept_ms\"],
               \"properties\":{\"slept_ms\":{\"type\":\"integer\"}}}
";

#[derive(Debug, Deserialize)]
struct EchoInput {
    text: String,
}

#[derive(Debug, Deserialize)]
struct SleepInput {
    duration_ms: u64,
}

/// The app's own message type, produced by the registry from typed arguments.
#[derive(Debug)]
enum AppMessage {
    Echo(EchoInput),
    Sleep(SleepInput),
}

struct Args {
    socket: PathBuf,
    credential: PathBuf,
    app_id: String,
}

enum ArgError {
    Help,
    UnknownFlag(String),
    MissingValue(&'static str),
    Unexpected(String),
    MissingRequired(&'static str),
}

fn echo_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["text"],
        "properties": {"text": {"type": "string"}}
    })
}

fn echo_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["echo"],
        "properties": {"echo": {"type": "string"}}
    })
}

fn sleep_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["duration_ms"],
        "properties": {"duration_ms": {"type": "integer", "minimum": 0, "maximum": 600_000}}
    })
}

fn sleep_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["slept_ms"],
        "properties": {"slept_ms": {"type": "integer"}}
    })
}

fn parse_args<I: Iterator<Item = String>>(mut args: I) -> Result<Args, ArgError> {
    let mut socket: Option<PathBuf> = None;
    let mut credential: Option<PathBuf> = None;
    let mut app_id: Option<String> = None;

    while let Some(token) = args.next() {
        let (name, inline) = match token.split_once('=') {
            Some((name, value)) => (name.to_owned(), Some(value.to_owned())),
            None => (token.clone(), None),
        };
        let mut value_for = |flag: &'static str| -> Result<String, ArgError> {
            match inline.clone() {
                Some(value) => Ok(value),
                None => args.next().ok_or(ArgError::MissingValue(flag)),
            }
        };
        match name.as_str() {
            "-h" | "--help" => return Err(ArgError::Help),
            "--socket" => socket = Some(PathBuf::from(value_for("--socket")?)),
            "--credential" => credential = Some(PathBuf::from(value_for("--credential")?)),
            "--app-id" => app_id = Some(value_for("--app-id")?),
            other if other.starts_with('-') => return Err(ArgError::UnknownFlag(other.to_owned())),
            other => return Err(ArgError::Unexpected(other.to_owned())),
        }
    }

    let socket = socket
        .or_else(|| std::env::var_os("ENZUI_CENTER_SOCKET").map(PathBuf::from))
        .ok_or(ArgError::MissingRequired("--socket"))?;
    let credential = credential
        .or_else(|| std::env::var_os("ENZUI_CENTER_CREDENTIAL").map(PathBuf::from))
        .ok_or(ArgError::MissingRequired("--credential"))?;
    let app_id = app_id
        .or_else(|| std::env::var("ENZUI_CENTER_APP_ID").ok())
        .unwrap_or_else(|| "org.enzui.Example".to_owned());

    Ok(Args {
        socket,
        credential,
        app_id,
    })
}

fn build_registry() -> AppFunctionRegistry<AppMessage> {
    let echo = AppFunction::typed(
        FunctionSpec::new(
            "echo",
            "Echo",
            "Returns the text argument unchanged",
            echo_input_schema(),
            echo_output_schema(),
            FunctionRisk::ReadOnly,
            vec![ConsentScope::Once, ConsentScope::Session],
        ),
        AppMessage::Echo,
    )
    .expect("echo spec is valid");

    let sleep = AppFunction::typed(
        FunctionSpec::new(
            "sleep",
            "Sleep",
            "Waits the requested milliseconds, honouring cancellation",
            sleep_input_schema(),
            sleep_output_schema(),
            FunctionRisk::Mutating,
            vec![ConsentScope::Once],
        )
        .supports_cancellation(true)
        .supports_progress(true),
        AppMessage::Sleep,
    )
    .expect("sleep spec is valid");

    AppFunctionRegistry::from_functions([echo, sleep]).expect("registry is valid")
}

// The app-owned side channel from a worker back to the single lifecycle owner.
enum WorkerEvent {
    Emit(InvocationEvent),
}

/// Progress events emitted across one `sleep`, independent of its duration.
const SLEEP_PROGRESS_STEPS: u64 = 20;

fn spawn_worker(
    message: AppMessage,
    invocation_id: Uuid,
    token: CancelToken,
    worker_tx: Sender<WorkerEvent>,
    deadline_unix_ms: u64,
) -> JoinHandle<()> {
    std::thread::spawn(move || match message {
        AppMessage::Echo(input) => {
            let _ = worker_tx.send(WorkerEvent::Emit(InvocationEvent::Completed {
                invocation_id,
                result: json!({"echo": input.text}),
            }));
        }
        AppMessage::Sleep(input) => {
            run_sleep(input, invocation_id, &token, &worker_tx, deadline_unix_ms);
        }
    })
}

fn run_sleep(
    input: SleepInput,
    invocation_id: Uuid,
    token: &CancelToken,
    worker_tx: &Sender<WorkerEvent>,
    deadline_unix_ms: u64,
) {
    let step = Duration::from_millis(50);
    let total = Duration::from_millis(input.duration_ms);
    let deadline = (deadline_unix_ms != 0)
        .then(|| std::time::UNIX_EPOCH + Duration::from_millis(deadline_unix_ms));
    let mut elapsed = Duration::ZERO;
    let mut last_bucket = 0u64;
    while elapsed < total {
        if token.is_terminal() {
            let _ = worker_tx.send(WorkerEvent::Emit(InvocationEvent::Cancelled {
                invocation_id,
            }));
            return;
        }
        if deadline.is_some_and(|deadline| std::time::SystemTime::now() >= deadline) {
            let _ = worker_tx.send(WorkerEvent::Emit(InvocationEvent::Failed {
                invocation_id,
                code: "deadline_exceeded".into(),
                message: "the invocation deadline elapsed".into(),
            }));
            return;
        }
        let slice = step.min(total - elapsed);
        std::thread::sleep(slice);
        elapsed += slice;
        let bucket = ((elapsed.as_millis() as u64).saturating_mul(SLEEP_PROGRESS_STEPS)
            / input.duration_ms.max(1))
        .min(SLEEP_PROGRESS_STEPS);
        if bucket > last_bucket {
            last_bucket = bucket;
            let _ = worker_tx.send(WorkerEvent::Emit(InvocationEvent::Progress {
                invocation_id: invocation_id.clone(),
                completed: bucket as f32 / SLEEP_PROGRESS_STEPS as f32,
                message: None,
            }));
        }
    }
    let terminal = if token.is_terminal() {
        InvocationEvent::Cancelled { invocation_id }
    } else {
        InvocationEvent::Completed {
            invocation_id,
            result: json!({"slept_ms": input.duration_ms}),
        }
    };
    let _ = worker_tx.send(WorkerEvent::Emit(terminal));
}

fn emit(connector: &CentralConnector, event: InvocationEvent) {
    for _ in 0..50 {
        match connector.send_event(event.clone()) {
            Ok(()) => return,
            Err(SendError::QueueFull) => std::thread::sleep(Duration::from_millis(20)),
            Err(SendError::Stopped) => return,
        }
    }
}

fn app_metadata(id: String) -> AppMetadata {
    AppMetadata {
        id,
        name: "Example headless app".into(),
        description: "Conformance connector for enzui-center".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

fn main() -> ExitCode {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments == ["--export-agent-manifest"] {
        use std::io::Write;
        let bytes = match enzui_agent::center::installed::manifest(
            &build_registry(),
            app_metadata("org.enzui.Example".into()),
        ) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("manifest export failed: {error}");
                return ExitCode::FAILURE;
            }
        };
        return match std::io::stdout().lock().write_all(&bytes) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("manifest output failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let args = match parse_args(arguments.into_iter()) {
        Ok(args) => args,
        Err(ArgError::Help) => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            report_arg_error(&error);
            return ExitCode::from(2);
        }
    };

    let registry = build_registry();
    let registration = registry
        .registration(app_metadata(args.app_id))
        .expect("registration is valid");
    let config = ConnectorConfig::new(args.socket, args.credential, registration);
    let connector = match CentralConnector::spawn(config) {
        Ok(connector) => connector,
        Err(error) => {
            eprintln!("connector failed to start: {error}");
            return ExitCode::from(1);
        }
    };

    run(&connector, &registry);

    connector.shutdown();
    ExitCode::SUCCESS
}

fn report_arg_error(error: &ArgError) {
    match error {
        ArgError::Help => {}
        ArgError::UnknownFlag(flag) => eprintln!("unknown flag: {flag}"),
        ArgError::MissingValue(flag) => eprintln!("flag {flag} needs a value"),
        ArgError::Unexpected(token) => eprintln!("unexpected argument: {token}"),
        ArgError::MissingRequired(flag) => {
            eprintln!("missing required {flag} (or its environment fallback)");
        }
    }
    eprint!("{USAGE}");
}

fn run(connector: &CentralConnector, registry: &AppFunctionRegistry<AppMessage>) {
    let mut table = InvocationTable::new();
    let mut workers: HashMap<String, JoinHandle<()>> = HashMap::new();
    let (worker_tx, worker_rx) = channel::<WorkerEvent>();

    loop {
        while let Ok(WorkerEvent::Emit(event)) = worker_rx.try_recv() {
            let invocation_id = event.invocation_id().clone();
            match table.record(&event) {
                Ok(EmitDecision::Send) => {
                    let terminal = event.is_terminal();
                    emit(connector, event);
                    if terminal {
                        table.forget(&invocation_id);
                        if let Some(handle) = workers.remove(invocation_id.as_str()) {
                            let _ = handle.join();
                        }
                    }
                }
                Ok(EmitDecision::Suppress) => {}
                Err(error) => eprintln!("lifecycle rejected an event: {error}"),
            }
        }

        match connector.events().recv_timeout(Duration::from_millis(100)) {
            Ok(ConnectorEvent::Connected { display_name }) => {
                eprintln!("connected as {display_name}");
            }
            Ok(ConnectorEvent::Registered) => eprintln!("registration accepted"),
            Ok(ConnectorEvent::Invoke(invocation)) => {
                handle_invoke(
                    registry,
                    connector,
                    &mut table,
                    &mut workers,
                    &worker_tx,
                    *invocation,
                );
            }
            Ok(ConnectorEvent::Cancel {
                invocation_id,
                reason,
            }) => {
                eprintln!("cancel {invocation_id}: {reason}");
                table.cancel(&invocation_id);
            }
            Ok(ConnectorEvent::Disconnected { reason }) => {
                eprintln!("disconnected: {reason}");
                // Stop in-flight work: its invocation ids belonged to the closed
                // connection, so nothing may complete them on a reconnection.
                table.cancel_all_disconnected();
            }
            Ok(ConnectorEvent::Stopped { reason }) => {
                eprintln!("stopped: {reason}");
                break;
            }
            Ok(ConnectorEvent::CommitReport {
                invocation_id,
                outcome,
            }) => eprintln!("commit report {invocation_id}: {outcome:?}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    stop_workers(&mut table, workers);
}

/// Cancels every still-live invocation, then joins its worker.
///
/// The order is the whole point. Once the loop has ended, the connection whose
/// invocation ids these workers answer is gone, so nothing they produce can be
/// sent; joining a live `sleep` first would hold exit open for its entire
/// remaining duration. Returns how many invocations this cancelled, so a
/// caller (or a test) can tell "nothing was live" from "the cancel worked".
fn stop_workers(table: &mut InvocationTable, workers: HashMap<String, JoinHandle<()>>) -> usize {
    let cancelled = table.cancel_all_disconnected();
    #[allow(
        clippy::iter_over_hash_type,
        reason = "every entry gets the same treatment, so the order does not matter"
    )]
    for (_, handle) in workers {
        let _ = handle.join();
    }
    cancelled
}

fn handle_invoke(
    registry: &AppFunctionRegistry<AppMessage>,
    connector: &CentralConnector,
    table: &mut InvocationTable,
    workers: &mut HashMap<String, JoinHandle<()>>,
    worker_tx: &Sender<WorkerEvent>,
    invocation: Invocation,
) {
    let invocation_id = invocation.invocation_id.clone();
    let deadline_unix_ms = invocation.deadline_unix_ms;
    let message = match registry.dispatch(&invocation) {
        Ok(message) => message,
        Err(error) => {
            emit(
                connector,
                InvocationEvent::Failed {
                    invocation_id,
                    code: "invalid_arguments".into(),
                    message: error.to_string(),
                },
            );
            return;
        }
    };
    let token = match table.open(&invocation_id) {
        Ok(token) => token,
        Err(error) => {
            emit(
                connector,
                InvocationEvent::Failed {
                    invocation_id,
                    code: "duplicate_invocation".into(),
                    message: error.to_string(),
                },
            );
            return;
        }
    };
    emit(
        connector,
        InvocationEvent::Accepted {
            invocation_id: invocation_id.clone(),
        },
    );
    let handle = spawn_worker(
        message,
        invocation_id.clone(),
        token,
        worker_tx.clone(),
        deadline_unix_ms,
    );
    workers.insert(invocation_id.as_str().to_owned(), handle);
}
