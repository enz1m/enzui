use super::tests::invocation_named;
use super::*;
use serde_json::json;
use std::time::Duration as StdDuration;

// A served session over a socket pair: the test plays the broker on `broker`.
struct Session {
    broker: UnixStream,
    events: Receiver<ConnectorEvent>,
    connector: CentralConnector,
    serve: JoinHandle<Receiver<Command>>,
}

fn shared_with(wire_version: u64) -> Arc<Shared> {
    let shared = Arc::new(Shared::new().expect("connector shared state"));
    shared.wire_version.store(wire_version, Ordering::SeqCst);
    shared.commits().side_effects = HashSet::from(["set_clock".to_owned()]);
    shared
}

fn serve(shared: &Arc<Shared>) -> Session {
    let (app_stream, broker) = UnixStream::pair().expect("pair");
    broker
        .set_read_timeout(Some(StdDuration::from_secs(2)))
        .expect("timeout");
    let (event_tx, events) = sync_channel(16);
    let (command_tx, command_rx) = sync_channel(16);
    let connector = CentralConnector {
        events: None,
        commands: command_tx.clone(),
        shared: Arc::clone(shared),
        supervisor: None,
    };
    let serve_shared = Arc::clone(shared);
    let reports = shared.wire_version.load(Ordering::SeqCst) == 3;
    let serve = std::thread::spawn(move || {
        serve_session(
            app_stream,
            &event_tx,
            command_rx,
            &command_tx,
            &serve_shared,
        )
    });
    // A real session is registered before any call can be reported; here,
    // wait until the session has started instead.
    let deadline = Instant::now() + StdDuration::from_secs(2);
    while reports && shared.commits().active.is_none() {
        assert!(Instant::now() < deadline, "the session did not start");
        std::thread::sleep(StdDuration::from_millis(2));
    }
    Session {
        broker,
        events,
        connector,
        serve,
    }
}

impl Session {
    fn invoke(&mut self, function_id: &str) -> Uuid {
        let id = Uuid::new_v4().expect("entropy");
        let invocation = invocation_named(&id, function_id);
        wire::write_message(&mut self.broker, &BrokerToApp::Invoke { invocation }).expect("invoke");
        match self.events.recv_timeout(StdDuration::from_secs(2)) {
            Ok(ConnectorEvent::Invoke(delivered)) => assert_eq!(delivered.invocation_id, id),
            other => panic!("expected Invoke, got {other:?}"),
        }
        id
    }

    fn read_frame(&mut self) -> serde_json::Value {
        wire::read_message(&mut self.broker).expect("a frame from the app")
    }

    fn answer(&mut self, id: &Uuid, disposition: &str) {
        wire::write_message(
            &mut self.broker,
            &json!({"type": "commit_report_ack", "invocation_id": id, "disposition": disposition}),
        )
        .expect("ack");
    }

    fn outcome(&self) -> (Uuid, CommitReportOutcome) {
        match self.events.recv_timeout(StdDuration::from_secs(2)) {
            Ok(ConnectorEvent::CommitReport {
                invocation_id,
                outcome,
            }) => (invocation_id, outcome),
            other => panic!("expected CommitReport, got {other:?}"),
        }
    }

    // Ends the session from the broker side and returns the connector.
    fn close(self) -> (CentralConnector, Receiver<ConnectorEvent>) {
        drop(self.broker);
        drop(self.serve.join().expect("serve returns"));
        (self.connector, self.events)
    }
}

/// A committed side effect is reported on the version-3 connection that
/// dispatched it, in the contract's exact frame, and the broker's answer comes
/// back once. A storage failure is retried on the same connection.
#[test]
fn a_committed_side_effect_is_reported_on_its_connection_and_answered() {
    let shared = shared_with(3);
    let mut session = serve(&shared);
    let id = session.invoke("set_clock");

    session.connector.report_commit(&id).expect("queued");
    assert_eq!(
        session.read_frame(),
        json!({"type": "commit_report", "invocation_id": id})
    );
    assert_eq!(
        session.connector.report_commit(&id),
        Err(CommitReportError::AwaitingAnswer),
        "one report per call is in flight at a time"
    );
    session.answer(&id, "storage_unavailable");
    assert_eq!(
        session.outcome(),
        (
            id.clone(),
            CommitReportOutcome::Broker(CommitReportDisposition::StorageUnavailable)
        )
    );

    session
        .connector
        .report_commit(&id)
        .expect("a retry is queued");
    assert_eq!(
        session.read_frame(),
        json!({"type": "commit_report", "invocation_id": id})
    );
    session.answer(&id, "persisted");
    let (answered, outcome) = session.outcome();
    assert_eq!(answered, id);
    assert!(
        matches!(outcome, CommitReportOutcome::Broker(disposition) if disposition.is_durable())
    );
    drop(session.close());
}
