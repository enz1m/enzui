use super::*;

fn uri(text: &str) -> OpenUri {
    OpenUri::parse(text).expect("valid fixture URI")
}

#[test]
fn settle_during_poll_does_not_consume_the_live_close() {
    struct BlockingPollBackend {
        entered: std::sync::mpsc::SyncSender<()>,
        resume: std::sync::mpsc::Receiver<()>,
        closed: Vec<(RequestId, CancelReason)>,
    }

    impl OpenUriBackend for BlockingPollBackend {
        fn start_open_uri(
            &mut self,
            _uri: &OpenUri,
            _parent: &ParentHandle,
        ) -> Result<RequestId, BackendError> {
            Ok(RequestId::new(93))
        }

        fn poll_open_uri(&mut self, _id: RequestId) -> Option<OpenUriReply> {
            self.entered.send(()).expect("poll observer");
            self.resume
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("test must release blocked poll");
            None
        }

        fn close_open_uri(&mut self, id: RequestId, reason: CancelReason) {
            self.closed.push((id, reason));
        }

        fn describe_open_uri(&self) -> String {
            "deterministic polling backend".into()
        }
    }

    let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(1);
    let mut backend = BlockingPollBackend {
        entered: entered_tx,
        resume: resume_rx,
        closed: Vec::new(),
    };
    let mut owner =
        OpenUriRequest::new(uri("https://example.test/polling-race"), PortalParent::None)
            .start(&mut backend, |_| false)
            .unwrap();
    let mut poller = owner.clone();
    let worker = std::thread::spawn(move || {
        let outcome = poller.poll(&mut backend);
        (outcome, backend)
    });

    entered_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("the clone must be inside poll_open_uri");
    owner.cancel(CancelReason::Application);
    let mut unrelated = MockOpenUriBackend::new();
    assert_eq!(owner.settle(&mut unrelated), None);
    assert!(
        !owner.cancel_ref().close_taken(),
        "Polling must retain the close obligation until the reply check finishes"
    );

    resume_tx.send(()).expect("release blocked poll");
    let (outcome, mut backend) = worker.join().expect("poll worker must finish");
    assert_eq!(
        outcome,
        Some(PortalOutcome::Cancelled(CancelReason::Application))
    );
    assert_eq!(owner.settle(&mut backend), Some(CancelReason::Application));
    assert_eq!(
        backend.closed,
        vec![(RequestId::new(93), CancelReason::Application)]
    );
    assert_eq!(owner.settle(&mut backend), None);
}
