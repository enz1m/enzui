//! Shortcuts that work while the app is in the background:
//! `org.freedesktop.portal.GlobalShortcuts`.
//!
//! The app names what it can do ([`GlobalShortcut`]: an ID, a description
//! and, optionally, a preferred trigger such as `CTRL+ALT+space`) and the
//! desktop decides the keys: it asks the user the first time and remembers
//! the answer. [`subscription`] opens the portal session, binds the shortcuts
//! and delivers [`ShortcutEvent`]s: what got bound (with the trigger the user
//! sees), each press and release, and later changes the user makes in the
//! desktop's settings. The session ends when the subscription does.
//! [`run_shortcuts`] is the same loop on a connection the caller owns.
//!
//! GNOME (47 and later) and KDE Plasma implement the portal; elsewhere the
//! subscription delivers a single [`ShortcutEvent::Unavailable`].

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{
    BindShortcutsOptions, GlobalShortcuts, NewShortcut, Shortcut,
};
use iced_futures::futures::StreamExt as _;

/// A shortcut the app offers.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GlobalShortcut {
    id: String,
    description: String,
    preferred_trigger: Option<String>,
}

impl GlobalShortcut {
    /// A shortcut with a stable `id` and a `description` the desktop shows
    /// ("Start or stop recording").
    pub fn new(id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            description: description.into(),
            preferred_trigger: None,
        }
    }

    /// Suggests keys in the XDG shortcuts format, such as `CTRL+ALT+space`;
    /// the desktop may choose others.
    #[must_use]
    pub fn preferred_trigger(mut self, trigger: impl Into<String>) -> Self {
        self.preferred_trigger = Some(trigger.into());
        self
    }
}

/// A shortcut as the desktop bound it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BoundShortcut {
    /// The app's ID for it.
    pub id: String,
    /// Its description.
    pub description: String,
    /// The keys, as the desktop describes them to the user; empty when the
    /// user left it unassigned.
    pub trigger: String,
}

impl From<&Shortcut> for BoundShortcut {
    fn from(shortcut: &Shortcut) -> Self {
        Self {
            id: shortcut.id().to_owned(),
            description: shortcut.description().to_owned(),
            trigger: shortcut.trigger_description().to_owned(),
        }
    }
}

/// What happened to the app's global shortcuts.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ShortcutEvent {
    /// The desktop bound these shortcuts (the user may have declined some).
    Bound(Vec<BoundShortcut>),
    /// The user changed the shortcuts in the desktop's settings.
    Changed(Vec<BoundShortcut>),
    /// The shortcut with this ID was pressed.
    Activated(String),
    /// The shortcut with this ID was released.
    Deactivated(String),
    /// No global shortcuts: the desktop has no portal, or refused; the text
    /// says why.
    Unavailable(String),
}

/// The event for a GlobalShortcuts signal of `session`.
fn signal_event(
    message: &zbus::Message,
    session: &zbus::zvariant::OwnedObjectPath,
) -> Option<ShortcutEvent> {
    type Press = (
        zbus::zvariant::OwnedObjectPath,
        String,
        u64,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    );
    let header = message.header();
    let body = message.body();
    match header.member()?.as_str() {
        "Activated" => {
            let (path, id, ..): Press = body.deserialize().ok()?;
            (path == *session).then_some(ShortcutEvent::Activated(id))
        }
        "Deactivated" => {
            let (path, id, ..): Press = body.deserialize().ok()?;
            (path == *session).then_some(ShortcutEvent::Deactivated(id))
        }
        "ShortcutsChanged" => {
            let (path, shortcuts): (zbus::zvariant::OwnedObjectPath, Vec<Shortcut>) =
                body.deserialize().ok()?;
            (path == *session).then(|| {
                ShortcutEvent::Changed(shortcuts.iter().map(BoundShortcut::from).collect())
            })
        }
        _ => None,
    }
}

/// Binds `shortcuts` on `connection` and sends what happens to `events`,
/// until `events` closes; then ends the session.
///
/// # Errors
/// The portal error when the session cannot be created or the shortcuts
/// cannot be bound.
pub async fn run_shortcuts(
    connection: zbus::Connection,
    shortcuts: Vec<GlobalShortcut>,
    events: tokio::sync::mpsc::UnboundedSender<ShortcutEvent>,
) -> Result<(), ashpd::Error> {
    // One stream for all three signals keeps a press before its release.
    let signals = zbus::MessageStream::for_match_rule(
        zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.GlobalShortcuts")?
            .path("/org/freedesktop/portal/desktop")?
            .build(),
        &connection,
        None,
    )
    .await?;
    let portal = GlobalShortcuts::with_connection(connection).await?;
    let session = portal
        .create_session(CreateSessionOptions::default())
        .await?;
    // The session serializes as its object path; signals name it.
    let session_path: zbus::zvariant::OwnedObjectPath = zbus::zvariant::to_bytes(
        zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0),
        &session,
    )
    .and_then(|bytes| bytes.deserialize().map(|(path, _)| path))
    .map_err(zbus::Error::from)?;
    let new: Vec<NewShortcut> = shortcuts
        .iter()
        .map(|shortcut| {
            NewShortcut::new(&shortcut.id, &shortcut.description)
                .preferred_trigger(shortcut.preferred_trigger.as_deref())
        })
        .collect();
    let bound = portal
        .bind_shortcuts(&session, &new, None, BindShortcutsOptions::default())
        .await?
        .response()?;
    let _ = events.send(ShortcutEvent::Bound(
        bound.shortcuts().iter().map(BoundShortcut::from).collect(),
    ));
    let mut signals = std::pin::pin!(signals.filter_map(|message| {
        let event = message
            .ok()
            .and_then(|message| signal_event(&message, &session_path));
        async move { event }
    }));
    loop {
        use iced_futures::futures::future::{Either, select};
        let event = match select(signals.as_mut().next(), Box::pin(events.closed())).await {
            Either::Left((Some(event), _)) => event,
            Either::Left((None, _)) | Either::Right(_) => break,
        };
        if events.send(event).is_err() {
            break;
        }
    }
    session.close().await
}

/// Binds `shortcuts` on the session bus for as long as the subscription
/// runs; see the module docs.
pub fn subscription(shortcuts: Vec<GlobalShortcut>) -> iced_futures::Subscription<ShortcutEvent> {
    struct Recipe(Vec<GlobalShortcut>);
    impl iced_futures::subscription::Recipe for Recipe {
        type Output = ShortcutEvent;

        fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
            use std::hash::Hash;
            ("enzui-global-shortcuts", &self.0).hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: iced_futures::subscription::EventStream,
        ) -> iced_futures::BoxStream<Self::Output> {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let shortcuts = self.0;
            let _ = std::thread::Builder::new()
                .name("enzui-global-shortcuts".into())
                .spawn(move || {
                    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    runtime.block_on(async move {
                        let result = match zbus::Connection::session().await {
                            Ok(connection) => {
                                run_shortcuts(connection, shortcuts, sender.clone()).await
                            }
                            Err(error) => Err(error.into()),
                        };
                        if let Err(error) = result {
                            let _ = sender.send(ShortcutEvent::Unavailable(error.to_string()));
                        }
                    });
                });
            Box::pin(iced_futures::futures::stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) },
            ))
        }
    }
    iced_futures::subscription::from_recipe(Recipe(shortcuts))
}
