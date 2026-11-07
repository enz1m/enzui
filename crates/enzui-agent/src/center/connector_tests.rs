use super::*;
use crate::center::protocol::{AgentCaller, AppMetadata};
use crate::secret::Secret;
use serde_json::json;
use std::time::Duration as StdDuration;

pub(super) fn app_credential() -> AppCredential {
    AppCredential {
        id: "org.enzui.Example".into(),
        display_name: "Example".into(),
        secret: Secret::new(vec![7u8; 32]),
    }
}

pub(super) fn registration() -> AppRegistration {
    AppRegistration {
        app: AppMetadata {
            id: "org.enzui.Example".into(),
            name: "Example".into(),
            description: "d".into(),
            version: "1.0.0".into(),
        },
        functions: Vec::new(),
    }
}

#[test]
fn serve_session_relays_invoke_app_event_and_cancel() {
    let (app_stream, mut broker) = UnixStream::pair().expect("pair");
    let (event_tx, event_rx) = sync_channel(8);
    let (command_tx, command_rx) = sync_channel(8);
    let app_tx = command_tx.clone();
    let shared = test_shared();
    if let Ok(mut active) = shared.active_stream.lock() {
        *active = app_stream.try_clone().ok();
    }
    let serve_shared = Arc::clone(&shared);
    let serve = std::thread::spawn(move || {
        serve_session(
            app_stream,
            &event_tx,
            command_rx,
            &command_tx,
            &serve_shared,
        )
    });

    let invocation_id = Uuid::new_v4().expect("entropy");
    let invocation = Invocation {
        invocation_id: invocation_id.clone(),
        function_id: "echo".into(),
        arguments: json!({"text": "hi"}),
        caller: AgentCaller {
            credential_id: "agent.reviewer".into(),
            display_name: "Reviewer".into(),
            session_id: Uuid::new_v4().expect("entropy"),
        },
        deadline_unix_ms: 0,
        idempotency_key: None,
    };
    wire::write_message(&mut broker, &BrokerToApp::Invoke { invocation }).expect("invoke");

    match event_rx.recv_timeout(StdDuration::from_secs(2)) {
        Ok(ConnectorEvent::Invoke(delivered)) => {
            assert_eq!(delivered.function_id, "echo");
            assert_eq!(delivered.invocation_id, invocation_id);
        }
        other => panic!("expected Invoke, got {other:?}"),
    }

    app_tx
        .send(Command::App(InvocationEvent::Completed {
            invocation_id: invocation_id.clone(),
            result: json!({"echo": "hi"}),
        }))
        .expect("enqueue app event");
    match wire::read_message::<_, AppToBroker>(&mut broker).expect("app event frame") {
        AppToBroker::Event {
            event:
                InvocationEvent::Completed {
                    invocation_id: got,
                    result,
                },
        } => {
            assert_eq!(got, invocation_id);
            assert_eq!(result, json!({"echo": "hi"}));
        }
        other => panic!("expected a completed event, got {other:?}"),
    }

    wire::write_message(
        &mut broker,
        &BrokerToApp::Cancel {
            invocation_id: invocation_id.clone(),
            reason: "deadline".into(),
        },
    )
    .expect("cancel");
    match event_rx.recv_timeout(StdDuration::from_secs(2)) {
        Ok(ConnectorEvent::Cancel {
            invocation_id: got,
            reason,
        }) => {
            assert_eq!(got, invocation_id);
            assert_eq!(reason, "deadline");
        }
        other => panic!("expected Cancel, got {other:?}"),
    }

    drop(broker);
    let recovered = serve.join().expect("serve returns");
    drop(recovered);
}

#[test]
fn an_idle_writer_ends_with_its_link() {
    let (app_stream, broker) = UnixStream::pair().expect("pair");
    let (event_tx, _event_rx) = sync_channel(8);
    let (command_tx, command_rx) = sync_channel(8);
    let shared = test_shared();
    if let Ok(mut active) = shared.active_stream.lock() {
        *active = app_stream.try_clone().ok();
    }
    let (done_tx, done_rx) = sync_channel(1);
    let serve_shared = Arc::clone(&shared);
    std::thread::spawn(move || {
        let recovered = serve_session(
            app_stream,
            &event_tx,
            command_rx,
            &command_tx,
            &serve_shared,
        );
        let _ = done_tx.send(recovered);
    });

    // The writer is parked on an empty queue when the link ends.
    std::thread::sleep(StdDuration::from_millis(50));
    let closed = std::time::Instant::now();
    drop(broker);
    done_rx
        .recv_timeout(StdDuration::from_secs(2))
        .expect("the session ends once its link does");
    assert!(
        closed.elapsed() < StdDuration::from_millis(100),
        "an idle writer is woken by the link ending, not by a poll: {:?}",
        closed.elapsed()
    );
}

fn write_credential_file(path: &std::path::Path) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let secret_hex = "07".repeat(32);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .expect("create credential");
    file.write_all(
        format!(
            "{{\"version\":1,\"kind\":\"app\",\"id\":\"org.enzui.Example\",\"display_name\":\"Example\",\"secret\":\"{secret_hex}\"}}"
        )
        .as_bytes(),
    )
    .expect("write credential");
}

// Binds a real listening socket at `parent/socket`, forcing the socket to
// 0600 and the parent to `mode` regardless of umask.
pub(super) fn listening_socket(
    parent: &std::path::Path,
    mode: u32,
) -> (std::os::unix::net::UnixListener, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(parent).expect("create parent");
    let path = parent.join("socket");
    let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(mode)).expect("chmod parent");
    (listener, path)
}

fn test_shared() -> Arc<Shared> {
    Arc::new(Shared::new().expect("connector shared state"))
}

pub(super) fn config_for(socket_path: PathBuf, dir: &std::path::Path) -> ConnectorConfig {
    let credential_path = dir.join("app.cred");
    write_credential_file(&credential_path);
    ConnectorConfig::new(socket_path, credential_path, registration())
}

pub(super) fn invocation_named(invocation_id: &Uuid, function_id: &str) -> Invocation {
    Invocation {
        invocation_id: invocation_id.clone(),
        function_id: function_id.into(),
        arguments: json!({"text": "hi"}),
        caller: AgentCaller {
            credential_id: "agent.reviewer".into(),
            display_name: "Reviewer".into(),
            session_id: Uuid::new_v4().expect("entropy"),
        },
        deadline_unix_ms: 0,
        idempotency_key: None,
    }
}
