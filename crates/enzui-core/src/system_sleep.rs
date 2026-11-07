//! Prepare for suspend, and learn of the resume: logind's `PrepareForSleep`.
//!
//! Before the machine suspends, logind tells every listener and waits for
//! the ones holding a *delay* lock (up to its `InhibitDelayMaxSec`, five
//! seconds by default). [`run_watch`] holds such a lock, so an application
//! has time to save, pause playback or close connections: a
//! [`SleepEvent::Suspending`] carries a [`SleepAck`], and the lock is
//! released when the application calls [`SleepAck::done`] or drops every
//! copy. After the resume a [`SleepEvent::Resumed`] arrives and the lock is
//! taken again for next time.
//!
//! logind is on the system bus. A Flatpak sandbox reaches it only with
//! `--system-talk-name=org.freedesktop.login1`; without it the subscription
//! delivers nothing. To *prevent* suspend while work is unsaved, use
//! [`crate::portal::inhibit::Inhibitor`] with `InhibitKinds::SUSPEND`
//! instead, which works in a sandbox.

use iced_futures::futures::StreamExt as _;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

/// A suspend or resume of the machine.
#[derive(Debug, Clone)]
pub enum SleepEvent {
    /// The machine is about to suspend; finish up and drop the ack.
    Suspending(SleepAck),
    /// The machine woke up.
    Resumed,
}

/// Holds suspend off until [`Self::done`] or until every copy is dropped.
#[derive(Debug, Clone)]
pub struct SleepAck {
    lock: Arc<Mutex<Option<OwnedFd>>>,
}

impl SleepAck {
    /// Let the suspend go ahead.
    pub fn done(&self) {
        let _ = self
            .lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait Login1Manager {
    fn inhibit(
        &self,
        what: &str,
        who: &str,
        why: &str,
        mode: &str,
    ) -> zbus::Result<zbus::zvariant::OwnedFd>;

    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

/// Watch suspends on `system` (the system bus), naming the lock after `who`
/// (the application's name), sending each to `events` until it closes.
///
/// # Errors
/// The bus error, when logind cannot be reached.
pub async fn run_watch(
    system: zbus::Connection,
    who: &str,
    events: tokio::sync::mpsc::UnboundedSender<SleepEvent>,
) -> zbus::Result<()> {
    let manager = Login1ManagerProxy::new(&system).await?;
    let mut signals = manager.receive_prepare_for_sleep().await?;
    let take_lock = || async {
        manager
            .inhibit("sleep", who, "Preparing for suspend", "delay")
            .await
            .ok()
            .map(OwnedFd::from)
    };
    let mut lock = take_lock().await;
    loop {
        let signal = {
            use iced_futures::futures::future::{Either, select};
            match select(Box::pin(signals.next()), Box::pin(events.closed())).await {
                Either::Left((signal, _)) => signal,
                Either::Right(_) => None,
            }
        };
        let Some(signal) = signal else {
            return Ok(());
        };
        let Ok(arguments) = signal.args() else {
            continue;
        };
        let event = if arguments.start {
            SleepEvent::Suspending(SleepAck {
                lock: Arc::new(Mutex::new(lock.take())),
            })
        } else {
            if lock.is_none() {
                lock = take_lock().await;
            }
            SleepEvent::Resumed
        };
        if events.send(event).is_err() {
            return Ok(());
        }
    }
}

/// Suspends and resumes of the machine, for the application `who`.
/// Nothing arrives when the system bus or logind is out of reach.
pub fn subscription(who: &'static str) -> iced_futures::Subscription<SleepEvent> {
    struct Recipe(&'static str);
    impl iced_futures::subscription::Recipe for Recipe {
        type Output = SleepEvent;

        fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
            use std::hash::Hash;
            "enzui-system-sleep".hash(state);
            self.0.hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: iced_futures::subscription::EventStream,
        ) -> iced_futures::BoxStream<Self::Output> {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let who = self.0;
            let _ = std::thread::Builder::new()
                .name("enzui-system-sleep".into())
                .spawn(move || {
                    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    runtime.block_on(async {
                        let watched = async {
                            let system = zbus::Connection::system().await?;
                            run_watch(system, who, sender).await
                        };
                        if let Err(error) = watched.await {
                            log::info!(target: "enzui::session", "no suspend notifications: {error}");
                        }
                    });
                });
            Box::pin(iced_futures::futures::stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) },
            ))
        }
    }
    iced_futures::subscription::from_recipe(Recipe(who))
}
