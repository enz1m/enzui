use super::*;
use crate::file_chooser::{
    BackendError, BackendReply, CancelReason, CompletionSignal, ParentHandle, PortalResponse,
    RequestId,
};
use enzui_core::portal::backend::MockBackend;
use iced::futures::{StreamExt, task::noop_waker};
use iced_test::runtime::{Action, task::into_stream};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::task::{Context, Poll};

#[derive(Default)]
struct Records {
    backend: MockBackend,
    replies: HashMap<RequestId, BackendReply>,
    events: Vec<&'static str>,
}

struct ObservedBackend {
    records: Rc<RefCell<Records>>,
    signal: CompletionSignal,
}

impl PortalBackend for ObservedBackend {
    fn start(&mut self, operation: PortalOperation) -> Result<RequestId, BackendError> {
        self.records.borrow_mut().backend.start(operation)
    }

    fn start_with_options(
        &mut self,
        operation: PortalOperation,
        options: &ChooserOptions,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        self.records
            .borrow_mut()
            .backend
            .start_with_options(operation, options, parent)
    }

    fn poll_reply(&mut self, id: RequestId) -> Option<BackendReply> {
        self.records.borrow_mut().replies.remove(&id)
    }

    fn completion_signal(&self) -> Option<CompletionSignal> {
        Some(self.signal.clone())
    }

    fn close(&mut self, id: RequestId, reason: CancelReason) {
        let mut records = self.records.borrow_mut();
        records.backend.close(id, reason);
        records.replies.remove(&id);
        records.events.push("close");
    }

    fn describe(&self) -> String {
        String::from("observed portal")
    }
}

impl Drop for ObservedBackend {
    fn drop(&mut self) {
        self.records.borrow_mut().events.push("backend dropped");
    }
}

fn session<Tag>(backend: MockBackend) -> (ChooserSession<Tag>, Rc<RefCell<Records>>) {
    let records = Rc::new(RefCell::new(Records {
        backend,
        ..Records::default()
    }));
    let session = ChooserSession::with_backend(Box::new(ObservedBackend {
        records: Rc::clone(&records),
        signal: CompletionSignal::new(),
    }));
    (session, records)
}

fn messages<T: Send + 'static>(task: iced::Task<T>) -> Vec<T> {
    let Some(mut stream) = into_stream(task) else {
        return vec![];
    };
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut messages = vec![];
    for _ in 0..8 {
        match stream.poll_next_unpin(&mut cx) {
            Poll::Ready(Some(Action::Output(message))) => messages.push(message),
            Poll::Ready(None) => return messages,
            _ => panic!("chooser task must contain only immediate terminal messages"),
        }
    }
    panic!("chooser task did not terminate");
}

fn success(uri: &str) -> BackendReply {
    BackendReply {
        response: PortalResponse::Success,
        uris: vec![uri.to_owned()],
    }
}

#[test]
fn chooser_session_closes_exactly_once_and_restarts_without_losing_parent_state_1() {
    // cancel_settles_queued_reply_once_and_allows_a_fresh_request
    {
        let (mut session, records) = session(MockBackend::new());
        assert!(session.cancel(CancelReason::Application).is_none());
        session
            .start("vpn", PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        let first = records.borrow().backend.started[0].0;
        assert_eq!(session.subscription().units(), 1);
        records
            .borrow_mut()
            .replies
            .insert(first, success("file:///old.ovpn"));
        assert_eq!(
            session.cancel(CancelReason::ParentClosed),
            Some(("vpn", PortalOutcome::Cancelled(CancelReason::ParentClosed)))
        );
        assert!(!session.is_pending());
        assert_eq!(session.subscription().units(), 0);
        assert!(records.borrow().replies.is_empty());
        assert_eq!(
            records.borrow().backend.closed,
            vec![(first, CancelReason::ParentClosed)]
        );
        assert!(session.cancel(CancelReason::Application).is_none());
        assert!(messages(session.pump(|tag, result| (tag, result))).is_empty());
        assert!(records.borrow().backend.repeat_close_attempts.is_empty());

        session
            .start("retry", PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        let second = records.borrow().backend.started[1].0;
        assert_ne!(first, second);
        records
            .borrow_mut()
            .replies
            .insert(first, success("file:///late.ovpn"));
        records
            .borrow_mut()
            .replies
            .insert(second, success("file:///new.ovpn"));
        let outputs = messages(session.pump(|tag, result| (tag, result)));
        assert_eq!(outputs.len(), 1);
        let (tag, PortalOutcome::Completed(result)) = &outputs[0] else {
            panic!("fresh request must complete")
        };
        assert_eq!(*tag, "retry");
        assert_eq!(result.first().unwrap().as_str(), "file:///new.ovpn");
        assert!(messages(session.pump(|tag, result| (tag, result))).is_empty());
        assert!(session.cancel(CancelReason::Application).is_none());
        drop(session);
        assert_eq!(
            records.borrow().backend.closed,
            vec![(first, CancelReason::ParentClosed)]
        );
    }

    // cancel_returns_the_tag_without_a_clone_bound_and_preserves_each_reason
    {
        #[derive(Debug, PartialEq, Eq)]
        struct Tag(u8);
        let (mut session, records) = session(MockBackend::new());
        for (index, reason) in [
            CancelReason::Application,
            CancelReason::ParentClosed,
            CancelReason::User,
        ]
        .into_iter()
        .enumerate()
        {
            session
                .start(
                    Tag(index as u8),
                    PortalOperation::OpenFile,
                    ChooserOptions::new(),
                )
                .unwrap();
            assert_eq!(
                session.cancel(reason),
                Some((Tag(index as u8), PortalOutcome::Cancelled(reason)))
            );
            assert_eq!(records.borrow().backend.closed[index].1, reason);
        }
        assert_eq!(records.borrow().backend.closed.len(), 3);
    }

    // drop_cancels_and_closes_before_dropping_the_backend
    {
        let (mut session, records) = session(MockBackend::new());
        session
            .start((), PortalOperation::SaveFile, ChooserOptions::new())
            .unwrap();
        let id = records.borrow().backend.started[0].0;
        drop(session);
        assert_eq!(
            records.borrow().backend.closed,
            vec![(id, CancelReason::Application)]
        );
        assert_eq!(records.borrow().events, ["close", "backend dropped"]);
    }

    // drop_after_explicit_cancel_does_not_close_twice
    {
        let (mut session, records) = session(MockBackend::new());
        session
            .start((), PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        let id = records.borrow().backend.started[0].0;
        assert!(session.cancel(CancelReason::ParentClosed).is_some());
        drop(session);
        assert_eq!(
            records.borrow().backend.closed,
            vec![(id, CancelReason::ParentClosed)]
        );
        assert!(records.borrow().backend.repeat_close_attempts.is_empty());
        assert_eq!(records.borrow().events, ["close", "backend dropped"]);
    }
}

#[test]
fn chooser_session_closes_exactly_once_and_restarts_without_losing_parent_state_2() {
    // drop_after_completion_has_no_close_obligation
    {
        let (mut session, records) = session(MockBackend::new());
        session
            .start((), PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        let id = records.borrow().backend.started[0].0;
        records
            .borrow_mut()
            .replies
            .insert(id, success("file:///chosen.ovpn"));
        assert_eq!(messages(session.pump(|(), result| result)).len(), 1);
        drop(session);
        assert!(records.borrow().backend.closed.is_empty());
        assert_eq!(records.borrow().events, ["backend dropped"]);
    }

    // cancel_deferred_start_does_not_open_or_close_a_wire_request_and_can_restart
    {
        let (mut session, records) = session(MockBackend::initialising_then_ready(1));
        session
            .start("deferred", PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        assert!(session.is_pending());
        assert!(records.borrow().backend.started.is_empty());
        assert_eq!(
            session.cancel(CancelReason::Application),
            Some((
                "deferred",
                PortalOutcome::Cancelled(CancelReason::Application)
            ))
        );
        assert!(messages(session.pump(|tag, result| (tag, result))).is_empty());
        assert!(records.borrow().backend.started.is_empty());
        assert!(records.borrow().backend.closed.is_empty());
        assert_eq!(session.subscription().units(), 0);
        session
            .start("fresh", PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        assert_eq!(records.borrow().backend.started.len(), 1);
        assert!(session.is_pending());
        assert!(session.cancel(CancelReason::Application).is_some());
    }

    // drop_deferred_start_and_failed_start_never_close_unknown_wire_ids
    {
        for backend in [
            MockBackend::initialising_then_ready(1),
            MockBackend::failing(BackendError::Unavailable),
        ] {
            let (mut session, records) = session(backend);
            let _ = session.start((), PortalOperation::OpenFile, ChooserOptions::new());
            drop(session);
            assert!(records.borrow().backend.started.is_empty());
            assert!(records.borrow().backend.closed.is_empty());
            assert!(
                records
                    .borrow()
                    .backend
                    .close_for_unknown_id_attempts
                    .is_empty()
            );
            assert_eq!(records.borrow().events, ["backend dropped"]);
        }
    }

    // an_unavailable_portal_opens_the_in_app_chooser_when_enabled
    {
        for refusal in [BackendError::Unavailable, BackendError::Unsupported] {
            let (mut plain, _records) = session::<&str>(MockBackend::failing(refusal));
            assert!(
                plain
                    .start("doc", PortalOperation::OpenFile, ChooserOptions::new())
                    .is_err(),
                "without the fallback the refusal is reported"
            );

            let (chooser, _records) = session::<&str>(MockBackend::failing(refusal));
            let mut session = chooser.with_fallback();
            let scratch = enzui_test_support::test_scratch::ScratchDir::new("session-fallback");
            let target = scratch.path().join("out.txt");
            session
                .start(
                    "doc",
                    PortalOperation::SaveFile,
                    ChooserOptions::new()
                        .current_folder(scratch.path())
                        .current_name("out.txt"),
                )
                .expect("the in-app chooser answers instead");
            assert!(session.is_fallback_open() && session.is_pending());
            assert_eq!(
                session.subscription().units(),
                1,
                "the listing wakes the app"
            );

            let outcome =
                messages(session.handle(FallbackEvent::Accept, |tag, outcome| (tag, outcome)));
            let expected = crate::file_chooser::FileUri::new(
                enzui_core::portal::open_uri::OpenUri::from_file_path(&target)
                    .unwrap()
                    .as_str(),
            );
            assert_eq!(
                outcome,
                vec![(
                    "doc",
                    PortalOutcome::Completed(FileResult {
                        uris: vec![expected]
                    })
                )]
            );
            assert!(!session.is_pending());
        }
    }
}

#[test]
fn chooser_session_closes_exactly_once_and_restarts_without_losing_parent_state_3() {
    // cancelling_the_in_app_chooser_reports_once
    {
        let (chooser, _records) = session::<&str>(MockBackend::failing(BackendError::Unavailable));
        let mut session = chooser.with_fallback();
        session
            .start("doc", PortalOperation::OpenFile, ChooserOptions::new())
            .unwrap();
        assert_eq!(
            session.cancel(CancelReason::ParentClosed),
            Some(("doc", PortalOutcome::Cancelled(CancelReason::ParentClosed)))
        );
        assert!(session.cancel(CancelReason::Application).is_none());
        assert!(!session.is_pending());
    }

    // closing_the_parent_window_cancels_only_its_own_request
    {
        let (mut session, records) = session::<u8>(MockBackend::new());
        let parent = iced::window::Id::unique();
        let other = iced::window::Id::unique();
        session
            .start_parented(
                7,
                PortalOperation::OpenFile,
                ChooserOptions::new(),
                ChooserParent::X11 {
                    window: parent,
                    handle: ParentHandle::x11(0x42),
                },
                |_| true,
            )
            .unwrap();
        assert_eq!(session.parent_closed(other), None, "another window closing");
        assert!(session.is_pending());
        assert_eq!(
            session.parent_closed(parent),
            Some((7, PortalOutcome::Cancelled(CancelReason::ParentClosed)))
        );
        assert!(!session.is_pending());
        assert_eq!(records.borrow().backend.closed.len(), 1);
        assert_eq!(session.parent_closed(parent), None, "settled once");
    }

    // the_in_app_chooser_keeps_the_content_s_scroll_offset
    {
        use iced::widget::{Space, column, scrollable};
        use iced::{Element, Event, Length, Point, Size, mouse};
        use iced_test::runtime::user_interface::{Cache, UserInterface};

        #[derive(Debug, Clone)]
        enum Msg {
            Scrolled(f32),
            Chooser,
        }

        fn page() -> Element<'static, Msg> {
            scrollable(column![Space::new().height(3000)].width(Length::Fill))
                .on_scroll(|scroll| Msg::Scrolled(scroll.viewport.absolute_offset().y))
                .height(Length::Fill)
                .into()
        }

        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("software"),
            ),
        )
        .expect("headless renderer");
        let size = Size::new(400.0, 300.0);
        let at = mouse::Cursor::Available(Point::new(200.0, 150.0));
        let wheel = |y: f32| {
            [Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { x: 0.0, y },
            })]
        };
        let offset =
            |ui: &mut UserInterface<'_, Msg, iced::Theme, iced_test::renderer::Renderer>,
             renderer: &mut iced_test::renderer::Renderer,
             y: f32| {
                let mut bus = iced::advanced::shell::Bus::new();
                let _ = ui.update(
                    &iced::window::Headless,
                    &iced::advanced::graphics::core::shell::Waker::noop(),
                    &wheel(y),
                    at,
                    renderer,
                    &mut bus,
                );
                bus.into_iter()
                    .filter_map(|message| match message {
                        Msg::Scrolled(offset) => Some(offset),
                        Msg::Chooser => None,
                    })
                    .next_back()
            };

        let (chooser, _records) = session::<&str>(MockBackend::failing(BackendError::Unavailable));
        let mut session = chooser.with_fallback();
        let (bottom, cache) = {
            let mut ui = UserInterface::build(
                session.view(page(), |_| Msg::Chooser),
                size,
                Cache::default(),
                &mut renderer,
            );
            let bottom = offset(&mut ui, &mut renderer, -5000.0).expect("the page scrolled");
            (bottom, ui.into_cache())
        };
        assert!(bottom > 2000.0, "scrolled to {bottom}");

        session
            .start("doc", PortalOperation::OpenFile, ChooserOptions::new())
            .expect("the in-app chooser opens");
        let cache = UserInterface::build(
            session.view(page(), |_| Msg::Chooser),
            size,
            cache,
            &mut renderer,
        )
        .into_cache();
        let _ = session.cancel(CancelReason::Application);
        let mut ui = UserInterface::build(
            session.view(page(), |_| Msg::Chooser),
            size,
            cache,
            &mut renderer,
        );
        let after = offset(&mut ui, &mut renderer, 1.0).expect("the page scrolled");
        assert!(
            (after - (bottom - 1.0)).abs() < 1.0,
            "the page went from {bottom} to {after}: the chooser rebuilt it"
        );
    }
}
