use super::*;
use futures_util::StreamExt;
use zbus::zvariant::{OwnedValue, Str};

struct Binding {
    local_id: NotificationId,
    actions: HashMap<String, NotificationAction>,
    reply: Option<NotificationAction>,
    token: Option<String>,
}

pub(super) fn unsupported(
    notice: &DesktopNotification,
    capabilities: &HashSet<String>,
) -> Option<UnsupportedNotificationFeature> {
    if notice.has_exported_actions() {
        return Some(UnsupportedNotificationFeature::ExportedAction);
    }
    if notice.inline_reply.is_some() && !capabilities.contains("inline-reply") {
        return Some(UnsupportedNotificationFeature::InlineReply);
    }
    if (notice.default_action.is_some()
        || !notice.buttons.is_empty()
        || notice.inline_reply.is_some())
        && !capabilities.contains("actions")
    {
        return Some(UnsupportedNotificationFeature::ActionDelivery);
    }
    if notice.markup_body.is_some() && !capabilities.contains("body-markup") {
        return Some(UnsupportedNotificationFeature::MarkupBody);
    }
    if matches!(notice.icon, Some(NotificationIcon::Bytes(_))) {
        return Some(UnsupportedNotificationFeature::EncodedIcon);
    }
    if notice.display_hints.iter().any(|hint| {
        !matches!(
            hint,
            NotificationDisplayHint::Transient | NotificationDisplayHint::Persistent
        )
    }) {
        return Some(UnsupportedNotificationFeature::DisplayHints);
    }
    if let Some(purpose) = notice
        .buttons
        .iter()
        .find_map(|button| button.purpose.as_ref())
    {
        return Some(UnsupportedNotificationFeature::ButtonPurpose(
            purpose.as_str().into(),
        ));
    }
    None
}

fn string(value: &str) -> OwnedValue {
    OwnedValue::from(Str::from(value))
}

async fn connect(address: Option<String>) -> zbus::Result<zbus::Connection> {
    let builder = match address.as_deref() {
        Some(address) => zbus::connection::Builder::address(address)?,
        None => zbus::connection::Builder::session()?,
    };
    builder
        .method_timeout(CALL_TIMEOUT)
        .max_queued(SIGNAL_QUEUE_CAPACITY)
        .build()
        .await
}

async fn owner(bus: &zbus::fdo::DBusProxy<'_>) -> Option<String> {
    tokio::time::timeout(CALL_TIMEOUT, bus.get_name_owner(SERVICE.try_into().ok()?))
        .await
        .ok()?
        .ok()
        .map(|name| name.to_string())
}

async fn owner_or_activate(bus: &zbus::fdo::DBusProxy<'_>) -> Option<String> {
    if let Some(owner) = owner(bus).await {
        return Some(owner);
    }
    let reply = tokio::time::timeout(
        CALL_TIMEOUT,
        bus.start_service_by_name(SERVICE.try_into().ok()?, 0),
    )
    .await
    .ok()?
    .ok()?;
    zbus::fdo::StartServiceReply::try_from(reply).ok()?;
    owner(bus).await
}

async fn negotiate(connection: &zbus::Connection, owner: String, generation: u64) -> Option<Ready> {
    if !owner.starts_with(':') || owner.len() > MAX_NOTIFICATION_ID_BYTES {
        return None;
    }
    let result = tokio::time::timeout(
        CALL_TIMEOUT,
        connection.call_method(
            Some(owner.as_str()),
            PATH,
            Some(SERVICE),
            "GetCapabilities",
            &(),
        ),
    )
    .await
    .ok()?
    .ok()?;
    if result.body().len() > 16 * 1024 {
        return None;
    }
    let capabilities = result.body().deserialize::<Vec<String>>().ok()?;
    if capabilities.len() > 128 || capabilities.iter().any(|value| value.len() > 128) {
        return None;
    }
    Some(Ready {
        owner,
        generation,
        capabilities: capabilities.into_iter().collect(),
    })
}

pub(super) async fn run(
    application_id: AppId,
    address: Option<String>,
    mut commands: mpsc::Receiver<Command>,
    shared: Arc<Mutex<State>>,
) -> zbus::Result<()> {
    let connection = tokio::time::timeout(CALL_TIMEOUT, connect(address))
        .await
        .map_err(|_| zbus::Error::Failure("notification connection timed out".into()))??;
    let signal_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .interface(SERVICE)?
        .path(PATH)?
        .build();
    let owner_rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.DBus")?
        .interface("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .add_arg(SERVICE)?
        .build();
    let mut signals =
        zbus::MessageStream::for_match_rule(signal_rule, &connection, Some(SIGNAL_QUEUE_CAPACITY))
            .await?;
    let mut owners =
        zbus::MessageStream::for_match_rule(owner_rule, &connection, Some(SIGNAL_QUEUE_CAPACITY))
            .await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let mut generation = 1;
    let mut ready = match owner_or_activate(&bus).await {
        Some(owner) => negotiate(&connection, owner, generation).await,
        None => None,
    };
    owner_changed(&shared, generation, ready.clone());
    let mut bindings = HashMap::<u32, Binding>::new();
    loop {
        tokio::select! {
            change = owners.next() => {
                let message = change.ok_or_else(|| zbus::Error::Failure("notification owner stream ended".into()))??;
                let (_, _, owner) = message.body().deserialize::<(String, String, String)>()?;
                if ready.as_ref().is_some_and(|ready| ready.owner == owner) {
                    continue;
                }
                generation = generation.checked_add(1).ok_or_else(|| zbus::Error::Failure("notification owner generation exhausted".into()))?;
                bindings.clear();
                owner_changed(&shared, generation, None);
                ready = negotiate(&connection, owner, generation).await;
                owner_changed(&shared, generation, ready.clone());
            }
            signal = signals.next() => {
                let message = signal.ok_or_else(|| zbus::Error::Failure("notification signal stream ended".into()))??;
                accept_signal(&message, ready.as_ref(), &mut bindings, &shared);
            }
            command = commands.recv() => {
                let Some(command) = command else { return Ok(()); };
                if ready.as_ref().is_none_or(|ready| ready.generation != command.ready.generation || ready.owner != command.ready.owner) {
                    publish(&shared, NotificationBackendEvent::Operation { operation: command.operation, owner_generation: command.ready.generation, outcome: NotificationBackendOutcome::Unavailable });
                    continue;
                }
                let identities: HashMap<_, _> = bindings.iter().map(|(id, binding)| (*id, binding.local_id.clone())).collect();
                let mut changed = None;
                let result = {
                    let call = execute(&connection, &application_id, &command, &identities);
                    tokio::pin!(call);
                    loop {
                        tokio::select! {
                            change = owners.next() => {
                                let message = change.ok_or_else(|| zbus::Error::Failure("notification owner stream ended".into()))??;
                                let (_, _, owner) = message.body().deserialize::<(String, String, String)>()?;
                                changed = Some(owner);
                                break Err(NotificationBackendOutcome::Unavailable);
                            }
                            signal = signals.next() => {
                                let message = signal.ok_or_else(|| zbus::Error::Failure("notification signal stream ended".into()))??;
                                // Keep the prior action registration while Notify is pending.
                                accept_signal(&message, ready.as_ref(), &mut bindings, &shared);
                            }
                            result = &mut call => break result,
                        }
                    }
                };
                let outcome = match result {
                    Ok((id, binding)) => {
                        match binding {
                            Some(binding) => { bindings.retain(|_, old| old.local_id != binding.local_id); bindings.insert(id, binding); NotificationBackendOutcome::Sent }
                            None => { bindings.remove(&id); NotificationBackendOutcome::Withdrawn }
                        }
                    }
                    Err(outcome) => outcome,
                };
                publish(&shared, NotificationBackendEvent::Operation { operation: command.operation, owner_generation: command.ready.generation, outcome });
                if let Some(owner) = changed {
                    generation = generation.checked_add(1).ok_or_else(|| zbus::Error::Failure("notification owner generation exhausted".into()))?;
                    bindings.clear();
                    owner_changed(&shared, generation, None);
                    ready = negotiate(&connection, owner, generation).await;
                    owner_changed(&shared, generation, ready.clone());
                }
            }
        }
    }
}

async fn execute(
    connection: &zbus::Connection,
    application_id: &AppId,
    command: &Command,
    bindings: &HashMap<u32, NotificationId>,
) -> Result<(u32, Option<Binding>), NotificationBackendOutcome> {
    let replacement = bindings
        .iter()
        .find_map(|(id, binding)| {
            (match &command.payload {
                NotificationBackendCommand::Send(notice) => &notice.id,
                NotificationBackendCommand::Withdraw(id) => id,
            } == binding)
                .then_some(*id)
        })
        .unwrap_or(0);
    match &command.payload {
        NotificationBackendCommand::Withdraw(_) => {
            if replacement == 0 {
                return Ok((0, None));
            }
            let response = tokio::time::timeout(
                CALL_TIMEOUT,
                connection.call_method(
                    Some(command.ready.owner.as_str()),
                    PATH,
                    Some(SERVICE),
                    "CloseNotification",
                    &(replacement,),
                ),
            )
            .await
            .map_err(|_| NotificationBackendOutcome::Failed)?
            .map_err(call_error)?;
            response
                .body()
                .deserialize::<()>()
                .map_err(|_| NotificationBackendOutcome::Failed)?;
            Ok((replacement, None))
        }
        NotificationBackendCommand::Send(notice) => {
            if let Some(feature) = unsupported(notice, &command.ready.capabilities) {
                return Err(NotificationBackendOutcome::Unsupported(feature));
            }
            if replacement == 0 && bindings.len() >= MAX_ACTIVE_NOTIFICATIONS {
                return Err(NotificationBackendOutcome::Failed);
            }
            let mut actions = Vec::<String>::new();
            let mut registered = HashMap::new();
            if let Some(action) = &notice.default_action {
                actions.extend(["default".into(), String::new()]);
                registered.insert("default".into(), action.clone());
            }
            for (index, button) in notice.buttons.iter().enumerate() {
                let key = format!("enzui-{}-{index}", command.operation.get());
                actions.extend([key.clone(), button.label.clone()]);
                registered.insert(key, button.action.clone());
            }
            let mut hints = HashMap::<&str, OwnedValue>::new();
            hints.insert("desktop-entry", string(application_id.as_str()));
            hints.insert(
                "urgency",
                OwnedValue::from(match notice.priority {
                    NotificationPriority::Low => 0u8,
                    NotificationPriority::Normal => 1,
                    NotificationPriority::High | NotificationPriority::Urgent => 2,
                }),
            );
            if let Some(category) = &notice.category {
                hints.insert("category", string(category));
            }
            if notice
                .display_hints
                .contains(&NotificationDisplayHint::Transient)
            {
                hints.insert("transient", true.into());
            }
            if notice
                .display_hints
                .contains(&NotificationDisplayHint::Persistent)
            {
                hints.insert("resident", true.into());
            }
            if let Some(reply) = &notice.inline_reply {
                actions.extend(["inline-reply".into(), reply.submit_label.clone()]);
                hints.insert("x-kde-reply-placeholder-text", string(&reply.placeholder));
                hints.insert(
                    "x-kde-reply-submit-button-text",
                    string(&reply.submit_label),
                );
            }
            let icon = match &notice.icon {
                Some(NotificationIcon::Themed(names)) => names[0].as_str(),
                _ => "",
            };
            let body = match &notice.body {
                Some(body) if command.ready.capabilities.contains("body-markup") => body
                    .replace('&', "&amp;")
                    .replace('<', "&lt;")
                    .replace('>', "&gt;"),
                Some(body) => body.clone(),
                None => notice.markup_body.clone().unwrap_or_default(),
            };
            let app_name: String = application_id.as_str().chars().take(80).collect();
            let response = tokio::time::timeout(
                CALL_TIMEOUT,
                connection.call_method(
                    Some(command.ready.owner.as_str()),
                    PATH,
                    Some(SERVICE),
                    "Notify",
                    &(
                        app_name,
                        replacement,
                        icon,
                        &notice.title,
                        body,
                        actions,
                        hints,
                        -1i32,
                    ),
                ),
            )
            .await
            .map_err(|_| NotificationBackendOutcome::Failed)?
            .map_err(call_error)?;
            let id = response
                .body()
                .deserialize::<u32>()
                .map_err(|_| NotificationBackendOutcome::Failed)?;
            if id == 0
                || (replacement != 0 && id != replacement)
                || bindings
                    .get(&id)
                    .is_some_and(|binding| *binding != notice.id)
            {
                return Err(NotificationBackendOutcome::Failed);
            }
            Ok((
                id,
                Some(Binding {
                    local_id: notice.id.clone(),
                    actions: registered,
                    reply: notice
                        .inline_reply
                        .as_ref()
                        .map(|reply| reply.action.clone()),
                    token: None,
                }),
            ))
        }
    }
}

fn call_error(error: zbus::Error) -> NotificationBackendOutcome {
    match error {
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.AccessDenied" | "org.freedesktop.DBus.Error.AuthFailed"
            ) =>
        {
            NotificationBackendOutcome::Denied
        }
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.NameHasNoOwner"
                    | "org.freedesktop.DBus.Error.ServiceUnknown"
            ) =>
        {
            NotificationBackendOutcome::Unavailable
        }
        _ => NotificationBackendOutcome::Failed,
    }
}

fn accept_signal(
    message: &zbus::Message,
    ready: Option<&Ready>,
    bindings: &mut HashMap<u32, Binding>,
    shared: &Mutex<State>,
) {
    let Some(ready) = ready else {
        return;
    };
    let header = message.header();
    if header.sender().map(zbus::names::UniqueName::as_str) != Some(ready.owner.as_str())
        || message.body().len() > MAX_NOTIFICATION_ACTION_WIRE_BYTES
    {
        let mut state = lock(shared);
        state.rejected = state.rejected.saturating_add(1);
        return;
    }
    let member = header.member().map(zbus::names::MemberName::as_str);
    if member == Some("NotificationClosed") {
        // D-Bus keeps one connection's messages in order, so a server cannot
        // reuse an id before this closure of its previous holder is seen.
        if let Ok((id, code)) = message.body().deserialize::<(u32, u32)>()
            && let Some(binding) = bindings.remove(&id)
        {
            publish(
                shared,
                NotificationBackendEvent::Closed {
                    notification_id: binding.local_id,
                    owner_generation: ready.generation,
                    reason: crate::portal::notification::NotificationCloseReason::from_freedesktop(
                        code,
                    ),
                },
            );
        }
        return;
    }
    let body = message.body();
    let Ok((id, value)) = body.deserialize::<(u32, &str)>() else {
        let mut state = lock(shared);
        state.rejected = state.rejected.saturating_add(1);
        return;
    };
    if value.len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES || value.contains('\0') {
        let mut state = lock(shared);
        state.rejected = state.rejected.saturating_add(1);
        return;
    }
    let Some(binding) = bindings.get_mut(&id) else {
        return;
    };
    let (action, response) = match member {
        Some("ActivationToken") => {
            binding.token = (!value.is_empty()).then(|| value.to_owned());
            return;
        }
        Some("NotificationReplied") => {
            let Some(action) = binding.reply.clone() else {
                return;
            };
            (action, Some(value.to_owned()))
        }
        Some("ActionInvoked") => {
            let Some(action) = binding.actions.get(value).cloned() else {
                return;
            };
            (action, None)
        }
        _ => return,
    };
    publish(
        shared,
        NotificationBackendEvent::Action(NotificationBackendAction {
            notification_id: binding.local_id.as_str().into(),
            action_id: action.id.as_str().into(),
            target: action.target,
            activation_token: binding
                .token
                .take()
                .map(crate::activation::InboundActivationToken::native),
            response,
            malformed: false,
            owner_generation: ready.generation,
            received_at: Instant::now(),
        }),
    );
}
