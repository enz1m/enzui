//! An ICED [`Subscription`] that turns queued events into typed messages.
//!
//! [`connector_subscription`] preserves the connector API; [`message_subscription`]
//! and [`MessageEvents`] apply the same bounded custody to application work queues.
//!
//! A background pump receives blocking source events and holds at most one
//! event in flight. Stream consumption commits that event before calling the
//! mapping closure; it does not acknowledge application reducer completion.
//! The mapper runs wherever the subscription stream is polled.
//!
//! Handles retain the receiver across recipe rebuilds. A newly started stream
//! claims custody, and the outgoing pump returns its receiver and any event
//! the old stream did not consume. A claimant checks a five-second handoff
//! retry budget between mutex acquisitions; lock contention can extend the
//! wait. Exhausting the budget ends the claimant's stream.
//!
//! Available with `iced-consumer`, using the renderer-neutral `iced_futures`
//! runtime. A pump-spawn failure ends the stream without reporting an error.
//! A panic in source cloning can drop the held receiver; a mapper panic occurs
//! after commitment and does not return that event for redelivery.

use std::any::TypeId;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use iced_futures::BoxStream;
use iced_futures::Subscription;
use iced_futures::boxed_stream;
use iced_futures::futures::StreamExt;
use iced_futures::futures::channel::mpsc;
use iced_futures::subscription::{self, EventStream, Hasher, Recipe};

use crate::center::connector::ConnectorEvent;

// Periodic receive wakeups allow stream cancellation and custody handoff.
const PUMP_POLL: Duration = Duration::from_millis(200);

// Retain the event while a full forwarding channel applies backpressure.
const FORWARD_RETRY: Duration = Duration::from_millis(5);

const HANDOFF_BUDGET: Duration = Duration::from_secs(5);
const HANDOFF_SLICE: Duration = Duration::from_millis(5);

struct Custody<Source> {
    events: Option<Receiver<Source>>,
    // Preserve the sole in-flight event across stream cancellation.
    unconsumed: Option<Source>,
    // A changed claim displaces the incumbent pump.
    claim: u64,
}

/// Shared, single-consumer custody of an application event receiver.
///
/// Clones share one receiver. A started subscription claims custody from the
/// incumbent pump; when the pump ends normally it parks the receiver and any
/// unconsumed event for resubscription or headless draining.
#[derive(Clone)]
pub struct ConnectorEvents<Source = ConnectorEvent> {
    inner: Arc<(Mutex<Custody<Source>>, Condvar)>,
}

impl<Source> ConnectorEvents<Source> {
    /// Takes custody of a receiver without draining or duplicating its events.
    #[must_use]
    pub fn new(events: Receiver<Source>) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(Custody {
                    events: Some(events),
                    unconsumed: None,
                    claim: 0,
                }),
                Condvar::new(),
            )),
        }
    }

    /// Whether the receiver is parked here rather than held by a running pump.
    ///
    /// False while a subscription is consuming events. It returns to true once
    /// that subscription's stream is dropped and its pump has handed the
    /// receiver back.
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.guard().events.is_some()
    }

    /// Receives a parked event without creating a GUI subscription.
    ///
    /// Once the custody mutex is acquired, the receive wait is capped at
    /// 200 ms. Mutex contention can add to that wait. A retained event is
    /// delivered before queued events.
    ///
    /// # Errors
    /// Reports active subscription custody, a quiet timeout or a closed sender.
    pub fn recv_timeout(&self, timeout: Duration) -> Result<Source, MessageReceiveError> {
        let mut custody = self.guard();
        if custody.events.is_none() {
            return Err(MessageReceiveError::InUse);
        }
        if let Some(event) = custody.unconsumed.take() {
            return Ok(event);
        }
        let Some(events) = custody.events.as_ref() else {
            return Err(MessageReceiveError::InUse);
        };
        match events.recv_timeout(timeout.min(PUMP_POLL)) {
            Ok(event) => Ok(event),
            Err(RecvTimeoutError::Timeout) => Err(MessageReceiveError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(MessageReceiveError::Closed),
        }
    }

    fn claim(&self) -> u64 {
        let mut custody = self.guard();
        custody.claim = custody.claim.wrapping_add(1);
        custody.claim
    }

    fn superseded(&self, claim: u64) -> bool {
        self.guard().claim != claim
    }

    fn acquire(&self, claim: u64, abandoned: &dyn Fn() -> bool) -> Option<Held<Source>> {
        let deadline = Instant::now() + HANDOFF_BUDGET;
        let (lock, handoff) = &*self.inner;
        let mut custody = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if custody.claim != claim || abandoned() {
                return None;
            }
            if let Some(events) = custody.events.take() {
                return Some(Held {
                    events,
                    unconsumed: custody.unconsumed.take(),
                });
            }
            if Instant::now() >= deadline {
                return None;
            }
            custody = handoff
                .wait_timeout(custody, HANDOFF_SLICE)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    fn park(&self, held: Held<Source>) {
        let (_, handoff) = &*self.inner;
        {
            let mut custody = self.guard();
            custody.events = Some(held.events);
            custody.unconsumed = held.unconsumed;
        }
        handoff.notify_all();
    }

    fn guard(&self) -> MutexGuard<'_, Custody<Source>> {
        self.inner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct Held<Source> {
    events: Receiver<Source>,
    unconsumed: Option<Source>,
}

// Channel acceptance is not consumption; one commitment flag arbitrates stream/pump ownership.
struct Delivery {
    armed: Mutex<bool>,
    consumed: Condvar,
}

impl Delivery {
    fn new() -> Self {
        Self {
            armed: Mutex::new(false),
            consumed: Condvar::new(),
        }
    }

    fn arm(&self) {
        *self.guard() = true;
    }

    fn claim(&self) -> bool {
        std::mem::replace(&mut *self.guard(), false)
    }

    fn consume(&self) -> bool {
        let claimed = self.claim();
        if claimed {
            self.consumed.notify_all();
        }
        claimed
    }

    fn still_armed(&self, slice: Duration) -> bool {
        let guard = self.guard();
        if !*guard {
            return false;
        }
        let (guard, _) = self
            .consumed
            .wait_timeout(guard, slice)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *guard
    }

    fn guard(&self) -> MutexGuard<'_, bool> {
        self.armed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<Source> core::fmt::Debug for ConnectorEvents<Source> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConnectorEvents")
            .field("idle", &self.is_idle())
            .finish()
    }
}

/// Builds a subscription that maps source events into application messages.
///
/// Offer the same handle and `id` on each subscription pass to keep its stream
/// alive. Identity includes source, message, and mapper types plus `id`; it
/// excludes the handle and mapper's captured values. Distinct receivers need
/// distinct identities, and replacing a mapper's captured state requires a new
/// identity or a shared state mechanism.
///
/// If multiple streams are explicitly started over one handle, the newest
/// claim displaces the incumbent. Handoff retries use a five-second budget,
/// with mutex contention able to extend the wait. The stream ends after all
/// queued events are consumed and source senders close, when
/// custody cannot be acquired, or when its pump cannot be spawned.
pub fn message_subscription<Source, Message, F>(
    events: &ConnectorEvents<Source>,
    id: u64,
    map: F,
) -> Subscription<Message>
where
    Source: Clone + Send + 'static,
    Message: Send + 'static,
    F: Fn(Source) -> Message + Send + 'static,
{
    subscription::from_recipe(ConnectorRecipe {
        id,
        events: events.clone(),
        map,
        message: PhantomData,
    })
}

/// Maps the connector's default event type into application messages.
pub fn connector_subscription<Message, F>(
    events: &ConnectorEvents,
    id: u64,
    map: F,
) -> Subscription<Message>
where
    Message: Send + 'static,
    F: Fn(ConnectorEvent) -> Message + Send + 'static,
{
    message_subscription(events, id, map)
}

/// Why a parked message receiver could not produce an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageReceiveError {
    /// A GUI subscription currently owns the receiver.
    InUse,
    /// No event arrived within the bounded wait.
    Timeout,
    /// All senders have closed.
    Closed,
}

impl std::fmt::Display for MessageReceiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InUse => "GUI subscription owns message receiver",
            Self::Timeout => "message receive timed out",
            Self::Closed => "message receiver closed",
        })
    }
}
impl std::error::Error for MessageReceiveError {}

/// Single-consumer custody of any cloneable application-message receiver.
pub type MessageEvents<Source> = ConnectorEvents<Source>;

struct ConnectorRecipe<Source, Message, F> {
    id: u64,
    events: ConnectorEvents<Source>,
    map: F,
    // The marker must not impose Message ownership or auto-trait bounds.
    message: PhantomData<fn() -> Message>,
}

impl<Source, Message, F> Recipe for ConnectorRecipe<Source, Message, F>
where
    Source: Clone + Send + 'static,
    Message: Send + 'static,
    F: Fn(Source) -> Message + Send + 'static,
{
    type Output = Message;

    fn hash(&self, state: &mut Hasher) {
        TypeId::of::<Self>().hash(state);
        self.id.hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<Self::Output> {
        // One sender slot plus consumption acknowledgement bounds the forwarding hop.
        let (sender, receiver) = mpsc::channel::<Source>(0);
        let custody = self.events;
        let delivery = Arc::new(Delivery::new());
        // Claim before spawning so scheduling order cannot reverse replacement custody.
        let claim = custody.claim();
        let pump_delivery = Arc::clone(&delivery);
        let _ = std::thread::Builder::new()
            .name("enzui-center-iced-pump".into())
            .spawn(move || pump(&custody, claim, sender, &pump_delivery));
        let map = self.map;
        boxed_stream(receiver.filter_map(move |event| {
            // Commitment prevents a reclaimed channel copy from being emitted twice.
            let emit = delivery.consume().then(|| map(event));
            std::future::ready(emit)
        }))
    }
}

fn pump<Source: Clone + Send + 'static>(
    custody: &ConnectorEvents<Source>,
    claim: u64,
    mut sender: mpsc::Sender<Source>,
    delivery: &Delivery,
) {
    let Some(mut held) = custody.acquire(claim, &|| sender.is_closed()) else {
        return;
    };
    while !custody.superseded(claim) {
        // Deliver inherited custody before dequeuing later events.
        let event = match held.unconsumed.take() {
            Some(event) => event,
            None => match held.events.recv_timeout(PUMP_POLL) {
                Ok(event) => event,
                Err(RecvTimeoutError::Timeout) => {
                    if sender.is_closed() {
                        break;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            },
        };
        match forward(&mut sender, delivery, event, custody, claim) {
            Forwarded::Consumed => {}
            Forwarded::Retained(event) => {
                held.unconsumed = Some(event);
                break;
            }
        }
    }
    custody.park(held);
}

enum Forwarded<Source> {
    Consumed,
    Retained(Source),
}

// Keep a source copy until the stream commits consumption, allowing cancellation recovery.
fn forward<Source: Clone>(
    sender: &mut mpsc::Sender<Source>,
    delivery: &Delivery,
    event: Source,
    custody: &ConnectorEvents<Source>,
    claim: u64,
) -> Forwarded<Source> {
    delivery.arm();
    let mut queued = event.clone();
    loop {
        match sender.try_send(queued) {
            Ok(()) => break,
            Err(error) if error.is_full() => {
                queued = error.into_inner();
                if sender.is_closed() || custody.superseded(claim) {
                    return reclaim(delivery, event);
                }
                std::thread::sleep(FORWARD_RETRY);
            }
            Err(_) => return reclaim(delivery, event),
        }
    }
    loop {
        if !delivery.still_armed(HANDOFF_SLICE) {
            return Forwarded::Consumed;
        }
        if sender.is_closed() || custody.superseded(claim) {
            return reclaim(delivery, event);
        }
    }
}

fn reclaim<Source>(delivery: &Delivery, event: Source) -> Forwarded<Source> {
    if delivery.claim() {
        Forwarded::Retained(event)
    } else {
        Forwarded::Consumed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::task::{Context, Poll, Waker};
    use std::time::Instant;

    use iced_futures::futures::stream;
    use iced_futures::subscription::into_recipes;

    #[derive(Debug, PartialEq)]
    enum Message {
        Registered,
        Other,
    }

    fn empty_input() -> EventStream {
        boxed_stream(stream::empty())
    }

    fn next_message(produced: &mut BoxStream<Message>, what: &str) -> Message {
        let mut context = Context::from_waker(Waker::noop());
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match produced.as_mut().poll_next(&mut context) {
                Poll::Ready(Some(message)) => return message,
                Poll::Ready(None) => panic!("{what}: stream ended before delivering a message"),
                Poll::Pending => {
                    assert!(Instant::now() < deadline, "{what}: no message in budget");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    #[test]
    fn subscription_delivers_mapped_connector_events() {
        let (events_tx, events_rx) = channel();
        let events = ConnectorEvents::new(events_rx);
        let subscription = connector_subscription(&events, 7, |event| match event {
            ConnectorEvent::Registered => Message::Registered,
            _ => Message::Other,
        });

        let mut recipes = into_recipes(subscription);
        assert_eq!(
            recipes.len(),
            1,
            "the adapter must yield exactly one recipe"
        );
        let mut produced = recipes.remove(0).stream(empty_input());

        events_tx
            .send(ConnectorEvent::Registered)
            .expect("deliver a connector event");

        assert_eq!(
            next_message(&mut produced, "mapped event"),
            Message::Registered
        );
        drop(events_tx);
    }
}
