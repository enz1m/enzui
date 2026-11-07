//! Learn that the session is ending: the Inhibit portal's session monitor.
//!
//! When the user logs out, the desktop first asks every monitoring
//! application ([`SessionState::QueryEnd`]). An application with unsaved
//! work answers by holding a [`crate::portal::inhibit::Inhibitor`] for
//! [`InhibitKinds::LOGOUT`](crate::portal::inhibit::InhibitKinds::LOGOUT);
//! the desktop then shows its reason and lets the user decide. The portal
//! waits for the answer at most a second: a [`SessionEvent`] for the query
//! carries a [`QueryEndAck`], and the monitor answers when the application
//! calls [`QueryEndAck::done`] (after inhibiting), when every copy of the ack
//! is dropped, or just before the second runs out.
//!
//! [`subscription`] delivers the events to an iced application;
//! [`run_monitor`] is the same loop on a connection the caller owns.

use ashpd::desktop::inhibit::{CreateMonitorOptions, InhibitProxy};
use iced_futures::futures::StreamExt as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the monitor waits for an application's answer to a query.
pub const QUERY_END_ANSWER_WITHIN: Duration = Duration::from_millis(900);

/// Where the user's session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// The session is running.
    Running,
    /// The user asked to end the session (log out); inhibit now to object.
    QueryEnd,
    /// The session is ending: save what can be saved.
    Ending,
}

/// A change of the session's state.
#[derive(Debug, Clone)]
pub struct SessionEvent {
    /// The session's state.
    pub state: SessionState,
    /// Whether the screensaver (or lock screen) is showing.
    pub screensaver_active: bool,
    /// For [`SessionState::QueryEnd`]: answer it once any inhibitor is held.
    pub ack: Option<QueryEndAck>,
}

/// The application's answer to a query-end; see the module docs. Dropping
/// every copy answers too.
#[derive(Debug, Clone)]
pub struct QueryEndAck {
    answer: Arc<AckSender>,
}

#[derive(Debug)]
struct AckSender(Mutex<Option<tokio::sync::oneshot::Sender<()>>>);

impl Drop for AckSender {
    fn drop(&mut self) {
        self.send();
    }
}

impl AckSender {
    fn send(&self) {
        let sender = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
    }
}

impl QueryEndAck {
    /// Answer the query now: the application has inhibited what it needs to.
    pub fn done(&self) {
        self.answer.send();
    }
}

/// Monitor the session on `connection`, sending every change to `events`,
/// until `events` closes. Ends early when the portal cannot be reached.
///
/// # Errors
/// The portal error, when the monitor cannot be created.
pub async fn run_monitor(
    connection: zbus::Connection,
    events: tokio::sync::mpsc::UnboundedSender<SessionEvent>,
) -> Result<(), ashpd::Error> {
    let proxy = Arc::new(InhibitProxy::with_connection(connection).await?);
    let mut changes = proxy.receive_state_changed().await?;
    let session = Arc::new(
        proxy
            .create_monitor(None, CreateMonitorOptions::default())
            .await?,
    );
    loop {
        let change = {
            use iced_futures::futures::future::{Either, select};
            match select(Box::pin(changes.next()), Box::pin(events.closed())).await {
                Either::Left((change, _)) => change,
                Either::Right(_) => None,
            }
        };
        let Some(change) = change else {
            return Ok(());
        };
        let state = match change.session_state() {
            ashpd::desktop::inhibit::SessionState::Running => SessionState::Running,
            ashpd::desktop::inhibit::SessionState::QueryEnd => SessionState::QueryEnd,
            ashpd::desktop::inhibit::SessionState::Ending => SessionState::Ending,
        };
        let ack = (state == SessionState::QueryEnd).then(|| {
            let (sender, answered) = tokio::sync::oneshot::channel();
            let proxy = Arc::clone(&proxy);
            let session = Arc::clone(&session);
            tokio::spawn(async move {
                // An answer, every copy dropped, or the deadline: all reply.
                let _ = tokio::time::timeout(QUERY_END_ANSWER_WITHIN, answered).await;
                let _ = proxy.query_end_response(&session).await;
            });
            QueryEndAck {
                answer: Arc::new(AckSender(Mutex::new(Some(sender)))),
            }
        });
        let event = SessionEvent {
            state,
            screensaver_active: change.screensaver_active(),
            ack,
        };
        if events.send(event).is_err() {
            return Ok(());
        }
    }
}

/// The session's state changes, on the session bus. Nothing arrives when
/// the desktop has no Inhibit portal.
pub fn subscription() -> iced_futures::Subscription<SessionEvent> {
    struct Recipe;
    impl iced_futures::subscription::Recipe for Recipe {
        type Output = SessionEvent;

        fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
            use std::hash::Hash;
            "enzui-session-monitor".hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: iced_futures::subscription::EventStream,
        ) -> iced_futures::BoxStream<Self::Output> {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let _ = std::thread::Builder::new()
                .name("enzui-session-monitor".into())
                .spawn(move || {
                    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    runtime.block_on(async {
                        if let Ok(connection) = zbus::Connection::session().await
                            && let Err(error) = run_monitor(connection, sender).await
                        {
                            log::info!(target: "enzui::session", "no session monitor: {error}");
                        }
                    });
                });
            Box::pin(iced_futures::futures::stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) },
            ))
        }
    }
    iced_futures::subscription::from_recipe(Recipe)
}
