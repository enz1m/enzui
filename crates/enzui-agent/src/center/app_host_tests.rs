use super::*;
use crate::center::protocol::{AgentCaller, ConsentScope, FunctionRisk};
use crate::center::registry::{AppFunction, FunctionSpec};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, PartialEq, Deserialize)]
struct Echo {
    text: String,
}

#[derive(Debug, PartialEq)]
enum Call {
    Echo(String),
}

fn registry() -> AppFunctionRegistry<Call> {
    let spec = FunctionSpec::new(
        "echo",
        "Echo",
        "Returns its text.",
        json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["text"],
            "properties": {"text": {"type": "string"}}
        }),
        json!({"type": "object"}),
        FunctionRisk::ReadOnly,
        vec![ConsentScope::Once],
    )
    .max_result_bytes(64);
    AppFunctionRegistry::from_functions(vec![
        AppFunction::typed(spec, |input: Echo| Call::Echo(input.text)).expect("valid spec"),
    ])
    .expect("valid registry")
}

fn invocation(function_id: &str, arguments: Value) -> Invocation {
    Invocation {
        invocation_id: Uuid::new_v4().expect("entropy"),
        function_id: function_id.into(),
        arguments,
        caller: AgentCaller {
            credential_id: "agent.test".into(),
            display_name: "Test".into(),
            session_id: Uuid::new_v4().expect("entropy"),
        },
        deadline_unix_ms: 0,
        idempotency_key: None,
    }
}

fn invoke(host: &mut CenterHost<Call>, invocation: Invocation) -> CenterTurn<Call> {
    host.receive_with_ends(ConnectorEvent::Invoke(Box::new(invocation)))
        .turn
}

fn failure_code(events: &[InvocationEvent]) -> Option<&str> {
    events.iter().find_map(|event| match event {
        InvocationEvent::Failed { code, .. } => Some(code.as_str()),
        _ => None,
    })
}

#[test]
fn an_unknown_function_fails_closed() {
    let mut host = CenterHost::offline(registry());
    assert!(matches!(
        invoke(&mut host, invocation("delete_everything", json!({}))),
        CenterTurn::Handled
    ));
    assert_eq!(failure_code(&host.take_sent()), Some("unknown_function"));
}

/// A full connector queue never makes the reducer wait: events wait in order
/// in the host and go out on later turns.
#[test]
fn a_full_connector_queue_parks_events_in_order_without_waiting() {
    let mut host = CenterHost::offline(registry());
    let (connector, queue) = crate::center::connector::detached(1);
    host.connector = Some(connector);
    let ids: Vec<_> = (0..20)
        .map(|_| invocation("missing", json!({})).invocation_id)
        .collect();
    let started = std::time::Instant::now();
    for id in &ids {
        let mut call = invocation("missing", json!({}));
        call.invocation_id = id.clone();
        let _ = invoke(&mut host, call);
    }
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "the reducer waited {:?}",
        started.elapsed()
    );
    assert!(host.next_deadline().is_some(), "asks to be polled again");
    let mut delivered = Vec::new();
    for _ in 0..ids.len() {
        delivered.extend(queue.drain());
        let _ = host.poll_ended();
    }
    delivered.extend(queue.drain());
    let order: Vec<_> = delivered
        .iter()
        .map(|event| event.invocation_id().clone())
        .collect();
    assert_eq!(order, ids, "every terminal event, in order");
    assert_eq!(host.next_deadline(), None);
}

/// A connection that never drains ends its calls instead of losing their
/// answers silently.
#[test]
fn an_outbox_that_overflows_ends_the_sessions_calls() {
    let mut host = CenterHost::offline(registry());
    let (connector, _never_drained) = crate::center::connector::detached(1);
    host.connector = Some(connector);
    for _ in 0..OUTBOX_CAPACITY + 2 {
        let _ = invoke(&mut host, invocation("echo", json!({"text": "hi"})));
    }
    let ended = host.poll_ended();
    assert!(!ended.is_empty(), "the calls ended");
    assert!(
        ended
            .iter()
            .all(|end| end.reason == EndReason::Disconnected)
    );
    assert_eq!(host.live_calls(), 0);
}

fn keyed(caller: &str, text: &str) -> Invocation {
    let mut call = invocation("echo", json!({"text": text}));
    call.idempotency_key = Some("k-1".into());
    call.caller.credential_id = caller.into();
    call
}

/// A stored result answers only the same caller's same request: another
/// caller's identical key runs afresh, and the key reused with other
/// arguments is refused instead of answered with the old result.
#[test]
fn an_idempotent_replay_is_bound_to_its_caller_and_arguments() {
    let mut host = CenterHost::offline(registry());
    let CenterTurn::Call(call, _) = invoke(&mut host, keyed("agent.a", "hi")) else {
        panic!("the first call runs");
    };
    host.complete(call, json!({"echo": "hi"}));
    let _ = host.take_sent();

    // The same caller and request: replayed, not run again.
    assert!(matches!(
        invoke(&mut host, keyed("agent.a", "hi")),
        CenterTurn::Handled
    ));
    assert!(host.take_sent().iter().any(|event| matches!(
        event,
        InvocationEvent::Completed { result, .. } if *result == json!({"echo": "hi"})
    )));

    // Another caller with the same key runs its own call.
    assert!(matches!(
        invoke(&mut host, keyed("agent.b", "hi")),
        CenterTurn::Call(..)
    ));

    // The same caller and key with other arguments is refused.
    assert!(matches!(
        invoke(&mut host, keyed("agent.a", "bye")),
        CenterTurn::Handled
    ));
    assert_eq!(
        failure_code(&host.take_sent()),
        Some("idempotency_conflict")
    );
}

#[test]
fn the_agent_is_available_only_while_registered() {
    let mut host = CenterHost::offline(registry());
    assert!(!host.is_available(), "connecting");
    let _ = host.receive_with_ends(ConnectorEvent::Connected {
        display_name: "Agent".into(),
    });
    assert!(!host.is_available(), "authenticated, not registered");
    let _ = host.receive_with_ends(ConnectorEvent::Registered);
    assert!(host.is_available(), "registered");
    let _ = host.receive_with_ends(ConnectorEvent::Disconnected {
        reason: "gone".into(),
    });
    assert!(!host.is_available(), "disconnected");
    let _ = host.receive_with_ends(ConnectorEvent::Registered);
    let _ = host.receive_with_ends(ConnectorEvent::Stopped {
        reason: "done".into(),
    });
    assert!(!host.is_available(), "stopped");
}

/// Inside a Flatpak sandbox `$XDG_STATE_HOME` is the app's own, while MCP
/// Center enrolls the credential in the host's state directory, which the
/// app is granted read-only: discovery finds it there. Outside a sandbox the
/// host directory is not consulted.
///
/// Sabotage: drop the `FLATPAK_ID` lookup, or consult the host directory
/// without it.
#[test]
fn a_flatpak_app_finds_its_credential_in_the_host_state_directory() {
    let root = crate::center::tests_support::scratch_dir("flatpak-credential");
    let home = root.join("home");
    let enrolled = home.join(".local/state/enzui/apps/org.example.Flat.credential.json");
    std::fs::create_dir_all(enrolled.parent().unwrap()).unwrap();
    std::fs::write(&enrolled, b"{}").unwrap();
    let sandbox_state = home.join(".var/app/org.example.Flat/.local/state");
    let discover = |flatpak: bool| {
        CenterProvision::discover_with("org.example.Flat", |key| match key {
            "HOME" => Some(home.clone().into_os_string()),
            "XDG_STATE_HOME" => Some(sandbox_state.clone().into_os_string()),
            "XDG_CONFIG_HOME" => Some(
                home.join(".var/app/org.example.Flat/config")
                    .into_os_string(),
            ),
            "FLATPAK_ID" if flatpak => Some("org.example.Flat".into()),
            "ENZUI_CENTER_SOCKET" => Some("/run/user/1000/enzui/center.sock".into()),
            _ => None,
        })
    };
    let found = discover(true).expect("the host credential is found in the sandbox");
    assert_eq!(found.credential_path, enrolled);
    assert_eq!(discover(false), None);
}
