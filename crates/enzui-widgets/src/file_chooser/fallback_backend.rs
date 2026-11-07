//! A portal backend that answers from the in-app chooser when the portal
//! cannot.

use super::fallback::{FallbackChooser, FallbackEvent, focus_task};
use super::{
    BackendError, BackendReply, CancelReason, ChooserOptions, CompletionSignal, ParentHandle,
    PortalBackend, PortalOperation, PortalOutcome, PortalResponse, RequestId,
};

/// Ids at and above this are the in-app chooser's, below it the portal's.
const FALLBACK_IDS: u64 = 1 << 63;

/// A [`PortalBackend`] for APIs that take a backend rather than a
/// [`ChooserSession`](super::ChooserSession): a
/// [`DestinationBroker`](enzui_core::download_destination::DestinationBroker),
/// a [`FileRequest`](super::FileRequest). Requests go to the portal; when it
/// is unavailable or does not support the operation, the in-app
/// [`FallbackChooser`] asks the user instead and its answer comes back as
/// the portal's would. A destination still comes only from what the user
/// chose, so a broker's grant keeps its meaning.
///
/// Render [`Self::view`] around the window's content, route its events to
/// [`Self::handle`], and pump the request's owner when `handle` returns
/// `true` or [`Self::completion_signal`] wakes. The completion signal is the
/// same for the backend's whole life, the portal's or its own, and the
/// in-app chooser raises it too: a subscription made while the portal was
/// starting keeps waking once the chooser takes over.
pub struct FallbackBackend {
    portal: Option<Box<dyn PortalBackend>>,
    signal: CompletionSignal,
    dialog: Option<(RequestId, FallbackChooser)>,
    reply: Option<(RequestId, BackendReply)>,
    next: u64,
}

impl std::fmt::Debug for FallbackBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FallbackBackend")
            .field(
                "portal",
                &self.portal.as_ref().map(|portal| portal.describe()),
            )
            .field("dialog", &self.dialog.as_ref().map(|(id, _)| id))
            .field("reply", &self.reply)
            .finish()
    }
}

impl FallbackBackend {
    /// Requests go to `portal`, or straight to the in-app chooser without one.
    #[must_use]
    pub fn new(portal: Option<Box<dyn PortalBackend>>) -> Self {
        let signal = portal
            .as_ref()
            .and_then(|portal| portal.completion_signal())
            .unwrap_or_default();
        Self {
            portal,
            signal,
            dialog: None,
            reply: None,
            next: FALLBACK_IDS,
        }
    }

    /// Whether the in-app chooser is open.
    #[must_use]
    pub fn is_dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// Wraps `content` with the in-app chooser while it is open.
    pub fn view<'a, Message: Clone + 'a>(
        &'a self,
        content: impl Into<iced::Element<'a, Message>>,
        on_event: impl Fn(FallbackEvent) -> Message + Clone + 'a,
    ) -> iced::Element<'a, Message> {
        match &self.dialog {
            Some((_, chooser)) => chooser.host(content, on_event),
            None => content.into(),
        }
    }

    /// Applies one in-app chooser event and returns whether the user has
    /// answered, so the request's owner should be pumped now, with the
    /// focus move the event asks for.
    pub fn handle<Message: Send + 'static>(
        &mut self,
        event: FallbackEvent,
    ) -> (bool, iced::Task<Message>) {
        let focus = focus_task(&event);
        let Some((id, chooser)) = self.dialog.as_mut() else {
            return (false, iced::Task::none());
        };
        let Some(outcome) = chooser.update(event) else {
            return (false, focus);
        };
        let reply = match outcome {
            PortalOutcome::Completed(result) => BackendReply {
                response: PortalResponse::Success,
                uris: result
                    .uris
                    .iter()
                    .map(|uri| uri.as_str().to_owned())
                    .collect(),
            },
            PortalOutcome::Cancelled(_) => BackendReply {
                response: PortalResponse::Cancelled,
                uris: Vec::new(),
            },
            _ => BackendReply {
                response: PortalResponse::Other,
                uris: Vec::new(),
            },
        };
        self.reply = Some((*id, reply));
        self.dialog = None;
        (true, iced::Task::none())
    }

    fn open_dialog(&mut self, operation: PortalOperation, options: &ChooserOptions) -> RequestId {
        let id = RequestId::new(self.next);
        self.next = self.next.checked_add(1).unwrap_or(FALLBACK_IDS);
        self.dialog = Some((
            id,
            FallbackChooser::with_signal(operation, options, self.signal.clone()),
        ));
        self.reply = None;
        id
    }

    fn is_fallback(id: RequestId) -> bool {
        id.get() >= FALLBACK_IDS
    }
}

impl PortalBackend for FallbackBackend {
    fn start(&mut self, operation: PortalOperation) -> Result<RequestId, BackendError> {
        self.start_with_options(operation, &ChooserOptions::new(), &ParentHandle::none())
    }

    fn start_with_options(
        &mut self,
        operation: PortalOperation,
        options: &ChooserOptions,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        if self.dialog.is_some() {
            return Err(BackendError::ResourceExhausted);
        }
        if let Some(portal) = self.portal.as_mut() {
            match portal.start_with_options(operation, options, parent) {
                Err(BackendError::Unavailable | BackendError::Unsupported) => {}
                started => return started,
            }
        }
        Ok(self.open_dialog(operation, options))
    }

    fn poll_reply(&mut self, id: RequestId) -> Option<BackendReply> {
        if !Self::is_fallback(id) {
            return self.portal.as_mut()?.poll_reply(id);
        }
        if let Some((open, chooser)) = self.dialog.as_mut()
            && *open == id
        {
            chooser.absorb();
            return None;
        }
        match self.reply.take() {
            Some((answered, reply)) if answered == id => Some(reply),
            other => {
                self.reply = other;
                None
            }
        }
    }

    fn completion_signal(&self) -> Option<CompletionSignal> {
        Some(self.signal.clone())
    }

    fn close(&mut self, id: RequestId, reason: CancelReason) {
        if !Self::is_fallback(id) {
            if let Some(portal) = self.portal.as_mut() {
                portal.close(id, reason);
            }
            return;
        }
        if self.dialog.as_ref().is_some_and(|(open, _)| *open == id) {
            self.dialog = None;
        }
        if self
            .reply
            .as_ref()
            .is_some_and(|(answered, _)| *answered == id)
        {
            self.reply = None;
        }
    }

    fn describe(&self) -> String {
        match &self.portal {
            Some(portal) => format!("{} with the in-app chooser", portal.describe()),
            None => "the in-app chooser".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_core::download::DownloadId;
    use enzui_core::download_destination::{DestinationBroker, DestinationOutcome};
    use enzui_core::portal::PortalParent;
    use enzui_core::portal::backend::MockBackend;

    struct Folder(std::path::PathBuf);

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn folder(tag: &str) -> Folder {
        let path = std::env::temp_dir().join(format!(
            "enzui-fallback-backend-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Folder(path)
    }

    fn request(broker: &mut DestinationBroker, backend: &mut FallbackBackend, dir: &Folder) {
        let outcome = broker.request_with_options(
            DownloadId(1),
            backend,
            PortalParent::None,
            ChooserOptions::new().current_folder(&dir.0),
            |_| true,
        );
        assert_eq!(outcome, None, "the request starts");
    }

    #[test]
    fn download_chooser_fallback_cancels_and_retains_its_wake_route() {
        // without_a_portal_the_user_chooses_a_download_destination_in_app
        {
            let dir = folder("chosen");
            let mut broker = DestinationBroker::new();
            let mut backend = FallbackBackend::new(Some(Box::new(MockBackend::failing(
                BackendError::Unavailable,
            ))));
            request(&mut broker, &mut backend, &dir);
            assert!(backend.is_dialog_open());
            assert!(broker.pump(&mut backend).is_empty(), "no answer yet");

            let _ = backend.handle::<()>(FallbackEvent::NameChanged("report.pdf".into()));
            let (answered, _) = backend.handle::<()>(FallbackEvent::Accept);
            assert!(answered);
            let settled = broker.pump(&mut backend);
            assert!(matches!(
                settled.as_slice(),
                [(DownloadId(1), DestinationOutcome::Completed(_))]
            ));
            let destination = broker.destination(DownloadId(1)).expect("granted");
            let reserved = destination.reserve_file().expect("create_new reservation");
            assert_eq!(reserved.path(), dir.0.join("report.pdf"));
        }

        // cancelling_the_download_closes_the_in_app_chooser
        {
            let dir = folder("cancel");
            let mut broker = DestinationBroker::new();
            let mut backend = FallbackBackend::new(None);
            request(&mut broker, &mut backend, &dir);
            assert!(backend.is_dialog_open());
            let _ = broker.cancel(DownloadId(1), CancelReason::Application);
            let settled = broker.pump(&mut backend);
            assert_eq!(
                settled,
                [(
                    DownloadId(1),
                    DestinationOutcome::Cancelled(CancelReason::Application)
                )]
            );
            assert!(!backend.is_dialog_open());
            assert!(broker.destination(DownloadId(1)).is_none());
        }

        // the_wake_signal_stays_the_same_when_the_chooser_takes_over
        {
            let dir = folder("signal");
            let portal = CompletionSignal::new();
            let mut backend =
                FallbackBackend::new(Some(Box::new(SignalledUnavailable(portal.clone()))));
            // A subscription made while the portal starts waits on this signal.
            let subscribed = backend.completion_signal().expect("a wake signal");
            assert!(subscribed.same_signal(&portal));
            let seen = portal.generation();

            let mut broker = DestinationBroker::new();
            request(&mut broker, &mut backend, &dir);
            assert!(backend.is_dialog_open());
            assert!(
                backend
                    .completion_signal()
                    .unwrap()
                    .same_signal(&subscribed),
                "the chooser does not switch the wake source"
            );
            // The chooser's folder listing wakes that same subscription.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while portal.generation() == seen {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the listing never woke it"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        // a_reachable_portal_answers_without_the_dialog
        {
            let dir = folder("portal");
            let mut portal = MockBackend::new();
            portal.script(BackendReply {
                response: PortalResponse::Success,
                uris: vec!["file:///tmp/from-portal.bin".into()],
            });
            let mut broker = DestinationBroker::new();
            let mut backend = FallbackBackend::new(Some(Box::new(portal)));
            request(&mut broker, &mut backend, &dir);
            assert!(!backend.is_dialog_open());
            let _ = broker.pump(&mut backend);
            assert_eq!(
                broker
                    .destination(DownloadId(1))
                    .map(enzui_core::download_destination::DownloadDestination::uri),
                Some("file:///tmp/from-portal.bin")
            );
        }
    }

    /// A portal with its own wake signal that turns out to be unavailable.
    struct SignalledUnavailable(CompletionSignal);

    impl PortalBackend for SignalledUnavailable {
        fn start(&mut self, _: PortalOperation) -> Result<RequestId, BackendError> {
            Err(BackendError::Unavailable)
        }
        fn poll_reply(&mut self, _: RequestId) -> Option<BackendReply> {
            None
        }
        fn completion_signal(&self) -> Option<CompletionSignal> {
            Some(self.0.clone())
        }
        fn close(&mut self, _: RequestId, _: CancelReason) {}
        fn describe(&self) -> String {
            "signalled-unavailable".to_owned()
        }
    }
}
