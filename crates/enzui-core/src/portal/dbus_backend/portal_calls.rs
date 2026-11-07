//! Portal request runners and notification plumbing for [`DbusBackend`],
//! split from `dbus_backend.rs` to keep each file under the source-size
//! ceiling. Methods are `pub(super)` so the backend's command loop reaches
//! them exactly as before.

use super::*;

impl DbusBackend {
    // Subscribe to the request Response before invoking the FileChooser method.
    pub(super) async fn run_file_chooser(
        connection: &zbus::Connection,
        portal_owner: &str,
        operation: PortalOperation,
        request_path: &OwnedObjectPath,
        shared: &Arc<Mutex<SharedState>>,
        chooser: &ChooserOptions,
        parent: &ParentHandle,
    ) -> BackendReply {
        let Ok(request_proxy) = zbus::Proxy::new(
            connection,
            portal_owner.to_owned(),
            request_path.clone(),
            REQUEST_IFACE,
        )
        .await
        else {
            return Self::failed_reply();
        };
        let Ok(mut responses) = request_proxy.receive_signal("Response").await else {
            return Self::failed_reply();
        };

        let handle_token = request_path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();

        let (member, multiple, directory) = match operation {
            PortalOperation::OpenFile => ("OpenFile", false, false),
            PortalOperation::OpenMultiple => ("OpenFile", true, false),
            PortalOperation::OpenDirectory => ("OpenFile", false, true),
            PortalOperation::SaveFile => ("SaveFile", false, false),
        };

        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(handle_token.as_str()));
        options.insert("modal", Value::from(true));
        if member == "OpenFile" {
            options.insert("multiple", Value::from(multiple));
            options.insert("directory", Value::from(directory));
        }
        Self::apply_chooser_options(&mut options, chooser, member);

        // The portal accepts an empty `parent_window` as an explicitly unparented dialog.
        let parent_window = parent.as_str();
        let title = chooser.title_str().unwrap_or("");
        let call = connection
            .call_method(
                Some(portal_owner),
                PORTAL_PATH,
                Some(FILE_CHOOSER_IFACE),
                member,
                &(parent_window, title, options),
            )
            .await;
        if let Err(e) = call {
            // Classify method-call failures before mapping the request outcome so diagnostics retain their cause.
            let (kind, raw_name) = Self::classify_call_error(&e);
            {
                let mut s = lock_shared(shared);
                s.last_failure = Some((kind, raw_name));
            }
            return Self::failed_reply();
        }

        use futures_util::StreamExt;
        let Some(signal) = responses.next().await else {
            return Self::failed_reply();
        };

        // Refuse an oversized response from its encoded length, before any typed
        // URI list is allocated.
        if signal.primary_header().body_len() as usize > MAX_REPLY_BODY_BYTES {
            return Self::failed_reply();
        }

        let body = signal.body();
        let (response_code, results): (u32, HashMap<String, OwnedValue>) = match body.deserialize()
        {
            Ok(v) => v,
            Err(_) => return Self::failed_reply(),
        };

        Self::file_chooser_reply(response_code, &results)
    }

    // Keep the live OpenURI request path available for cancellation by separating the method call from response waiting.
    pub(super) async fn run_open_uri(
        connection: &zbus::Connection,
        portal_owner: &str,
        request_path: &OwnedObjectPath,
        shared: &Arc<Mutex<SharedState>>,
        uri: &OpenUri,
        parent: &ParentHandle,
        activation: Option<&OpenUriActivation>,
    ) -> OpenUriReply {
        let Ok(request_proxy) = zbus::Proxy::new(
            connection,
            portal_owner.to_owned(),
            request_path.clone(),
            REQUEST_IFACE,
        )
        .await
        else {
            return OpenUriReply::Failed;
        };
        let Ok(mut responses) = request_proxy.receive_signal("Response").await else {
            return OpenUriReply::Failed;
        };

        let Ok(ashpd_uri) = ashpd::Uri::parse(uri.as_str()) else {
            return OpenUriReply::Failed;
        };
        let handle_token = request_path.as_str().rsplit('/').next().unwrap_or_default();
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(handle_token));
        options.insert("ask", Value::from(false));
        if let Some(activation) = activation
            && let Ok(proxy) = zbus::Proxy::new(
                connection,
                portal_owner.to_owned(),
                PORTAL_PATH,
                OPEN_URI_IFACE,
            )
            .await
            && proxy.get_property::<u32>("version").await.unwrap_or(0) >= 4
            && std::time::Instant::now() < activation.expires_at
        {
            options.insert("activation_token", Value::from(activation.token.as_str()));
        }

        // A local file is opened from a descriptor, not from its URI: the
        // portal's `OpenURI` member rejects the `file:` scheme and answers the
        // request with an error response, so routing a local path through it
        // never opens anything. The descriptor is read-only, so this grants the
        // handler no more than the user already has.
        //
        // Use the underlying proxy because the high-level call returns only after Response.
        let local_file = uri.local_file_path();
        let call = match &local_file {
            Some(path) => match std::fs::File::open(path) {
                Ok(file) => {
                    connection
                        .call_method(
                            Some(portal_owner),
                            PORTAL_PATH,
                            Some(OPEN_URI_IFACE),
                            "OpenFile",
                            &(parent.as_str(), zbus::zvariant::Fd::from(&file), options),
                        )
                        .await
                }
                Err(error) => {
                    lock_shared(shared).last_failure = Some((
                        DbusFailure::TransportFailure,
                        Some(error.kind().to_string()),
                    ));
                    return OpenUriReply::Failed;
                }
            },
            None => {
                connection
                    .call_method(
                        Some(portal_owner),
                        PORTAL_PATH,
                        Some(OPEN_URI_IFACE),
                        "OpenURI",
                        &(parent.as_str(), ashpd_uri.as_str(), options),
                    )
                    .await
            }
        };
        if let Err(error) = call {
            let (kind, raw_name) = Self::classify_call_error(&error);
            lock_shared(shared).last_failure = Some((kind, raw_name));
            return match kind {
                DbusFailure::NoPortalImplementation => OpenUriReply::Unavailable,
                DbusFailure::AccessDenied => OpenUriReply::Denied,
                DbusFailure::TransportFailure => OpenUriReply::Failed,
            };
        }

        use futures_util::StreamExt;
        let Some(signal) = responses.next().await else {
            return OpenUriReply::Failed;
        };
        let (code, _results): (u32, HashMap<String, OwnedValue>) = match signal.body().deserialize()
        {
            Ok(body) => body,
            Err(_) => return OpenUriReply::Failed,
        };
        match PortalResponse::from_code(code) {
            PortalResponse::Success => OpenUriReply::Success,
            PortalResponse::Cancelled => OpenUriReply::UserCancelled,
            PortalResponse::Other | PortalResponse::Unknown(_) => OpenUriReply::Failed,
        }
    }

    // Keep the live request path available for cancellation by separating the method call from response waiting.
    pub(super) async fn run_open_file(
        connection: &zbus::Connection,
        portal_owner: &str,
        request_path: &OwnedObjectPath,
        shared: &Arc<Mutex<SharedState>>,
        (member, file): (&str, &std::fs::File),
        parent: &ParentHandle,
        activation: Option<&OpenUriActivation>,
    ) -> OpenUriReply {
        let Ok(request_proxy) = zbus::Proxy::new(
            connection,
            portal_owner.to_owned(),
            request_path.clone(),
            REQUEST_IFACE,
        )
        .await
        else {
            return OpenUriReply::Failed;
        };
        let Ok(mut responses) = request_proxy.receive_signal("Response").await else {
            return OpenUriReply::Failed;
        };

        let handle_token = request_path.as_str().rsplit('/').next().unwrap_or_default();
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(handle_token));
        options.insert("ask", Value::from(false));
        if let Some(activation) = activation
            && let Ok(proxy) = zbus::Proxy::new(
                connection,
                portal_owner.to_owned(),
                PORTAL_PATH,
                OPEN_URI_IFACE,
            )
            .await
            && proxy.get_property::<u32>("version").await.unwrap_or(0) >= 4
            && std::time::Instant::now() < activation.expires_at
        {
            options.insert("activation_token", Value::from(activation.token.as_str()));
        }

        // Pass the caller's exact retained descriptor; a retained file is never
        // reopened from a pathname, so name substitution cannot launch different
        // bytes. The descriptor is read-only authority the user already holds.
        //
        // Use the underlying proxy because the high-level call returns only after Response.
        let call = connection
            .call_method(
                Some(portal_owner),
                PORTAL_PATH,
                Some(OPEN_URI_IFACE),
                member,
                &(parent.as_str(), zbus::zvariant::Fd::from(file), options),
            )
            .await;
        if let Err(error) = call {
            let (kind, raw_name) = Self::classify_call_error(&error);
            lock_shared(shared).last_failure = Some((kind, raw_name));
            return match kind {
                DbusFailure::NoPortalImplementation => OpenUriReply::Unavailable,
                DbusFailure::AccessDenied => OpenUriReply::Denied,
                DbusFailure::TransportFailure => OpenUriReply::Failed,
            };
        }

        use futures_util::StreamExt;
        let Some(signal) = responses.next().await else {
            return OpenUriReply::Failed;
        };
        let (code, _results): (u32, HashMap<String, OwnedValue>) = match signal.body().deserialize()
        {
            Ok(body) => body,
            Err(_) => return OpenUriReply::Failed,
        };
        match PortalResponse::from_code(code) {
            PortalResponse::Success => OpenUriReply::Success,
            PortalResponse::Cancelled => OpenUriReply::UserCancelled,
            PortalResponse::Other | PortalResponse::Unknown(_) => OpenUriReply::Failed,
        }
    }

    // Require a nonempty, correctly typed, in-bounds URI array for a successful chooser response.
    pub(super) fn file_chooser_reply(
        response_code: u32,
        results: &HashMap<String, OwnedValue>,
    ) -> BackendReply {
        Self::file_chooser_reply_with(response_code, results, |value| {
            let value = value.try_clone()?;
            Vec::<String>::try_from(value)
        })
    }

    pub(super) fn file_chooser_reply_with<E>(
        response_code: u32,
        results: &HashMap<String, OwnedValue>,
        decode_uris: impl FnOnce(&OwnedValue) -> Result<Vec<String>, E>,
    ) -> BackendReply {
        match PortalResponse::from_code(response_code) {
            PortalResponse::Success => match results
                .get("uris")
                .and_then(|value| decode_uris(value).ok().filter(|uris| !uris.is_empty()))
            {
                Some(uris) => {
                    let reply = BackendReply {
                        response: PortalResponse::Success,
                        uris,
                    };
                    // A reply over any URI bound is discarded whole; no partial
                    // grant list reaches the operation layer.
                    if reply.within_reply_limits() {
                        reply
                    } else {
                        Self::failed_reply()
                    }
                }
                None => Self::failed_reply(),
            },
            other => BackendReply {
                response: other,
                uris: vec![],
            },
        }
    }

    // Preserve the preparation token and current request path across PreparePrint and Print.
    pub(super) async fn run_print(
        connection: &zbus::Connection,
        portal_owner: &str,
        id: RequestId,
        prepare_path: &OwnedObjectPath,
        print_path: &OwnedObjectPath,
        shared: &Arc<Mutex<SharedState>>,
        request: PrintRequest,
    ) -> PortalOutcome<PrintReceipt> {
        let token = match request.token {
            Some(token) => token,
            None => match Self::run_prepare_print(connection, portal_owner, prepare_path, &request)
                .await
            {
                Ok(token) => token,
                Err(outcome) => return outcome,
            },
        };

        // Replace the active path before invoking Print so Close targets the current request.
        if let Some(active) = lock_shared(shared).active.get_mut(&id) {
            active.request_path = print_path.clone();
        }

        Self::run_print_document(connection, portal_owner, print_path, &request, token).await
    }

    pub(super) async fn run_prepare_print(
        connection: &zbus::Connection,
        portal_owner: &str,
        request_path: &OwnedObjectPath,
        request: &PrintRequest,
    ) -> Result<PrintToken, PortalOutcome<PrintReceipt>> {
        let request_proxy = zbus::Proxy::new(
            connection,
            portal_owner.to_owned(),
            request_path.clone(),
            REQUEST_IFACE,
        )
        .await
        .map_err(|_| PortalOutcome::Failed)?;
        let mut responses = request_proxy
            .receive_signal("Response")
            .await
            .map_err(|_| PortalOutcome::Failed)?;

        let handle_token = request_path.as_str().rsplit('/').next().unwrap_or_default();
        let settings: HashMap<&str, Value<'_>> = HashMap::new();
        let page_setup: HashMap<&str, Value<'_>> = HashMap::new();
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(handle_token));
        options.insert("modal", Value::from(true));
        options.insert(
            "supported_output_file_formats",
            Value::from(vec![request.format.portal_name()]),
        );

        connection
            .call_method(
                Some(portal_owner),
                PORTAL_PATH,
                Some(PRINT_IFACE),
                "PreparePrint",
                &(
                    request.parent.as_str(),
                    request.title.as_str(),
                    settings,
                    page_setup,
                    options,
                ),
            )
            .await
            .map_err(|_| PortalOutcome::Failed)?;

        use futures_util::StreamExt;
        let signal = responses.next().await.ok_or(PortalOutcome::Failed)?;
        let (code, results): (u32, HashMap<String, OwnedValue>) = signal
            .body()
            .deserialize()
            .map_err(|_| PortalOutcome::Failed)?;
        match PortalResponse::from_code(code) {
            PortalResponse::Success => results
                .get("token")
                .and_then(|v| v.try_clone().ok())
                .and_then(|v| u32::try_from(v).ok())
                .map(PrintToken::from_raw)
                .ok_or(PortalOutcome::Failed),
            PortalResponse::Cancelled => Err(PortalOutcome::Cancelled(CancelReason::User)),
            PortalResponse::Other | PortalResponse::Unknown(_) => Err(PortalOutcome::Failed),
        }
    }

    pub(super) async fn run_print_document(
        connection: &zbus::Connection,
        portal_owner: &str,
        request_path: &OwnedObjectPath,
        request: &PrintRequest,
        token: PrintToken,
    ) -> PortalOutcome<PrintReceipt> {
        let Ok(request_proxy) = zbus::Proxy::new(
            connection,
            portal_owner.to_owned(),
            request_path.clone(),
            REQUEST_IFACE,
        )
        .await
        else {
            return PortalOutcome::Failed;
        };
        let Ok(mut responses) = request_proxy.receive_signal("Response").await else {
            return PortalOutcome::Failed;
        };

        let handle_token = request_path.as_str().rsplit('/').next().unwrap_or_default();
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        options.insert("handle_token", Value::from(handle_token));
        options.insert("modal", Value::from(true));
        options.insert("token", Value::from(token.get()));
        options.insert(
            "supported_output_file_formats",
            Value::from(vec![request.format.portal_name()]),
        );
        let fd = ashpd::zvariant::Fd::from(request.document.as_fd());

        if connection
            .call_method(
                Some(portal_owner),
                PORTAL_PATH,
                Some(PRINT_IFACE),
                "Print",
                &(request.parent.as_str(), request.title.as_str(), fd, options),
            )
            .await
            .is_err()
        {
            return PortalOutcome::Failed;
        }

        use futures_util::StreamExt;
        let Some(signal) = responses.next().await else {
            return PortalOutcome::Failed;
        };
        let (code, _results): (u32, HashMap<String, OwnedValue>) = match signal.body().deserialize()
        {
            Ok(value) => value,
            Err(_) => return PortalOutcome::Failed,
        };
        PortalOutcome::from_response(PortalResponse::from_code(code), PrintReceipt { token })
    }

    // Encode `current_folder` as NUL-terminated `ay`; send `current_name` only for SaveFile.
    pub(super) fn apply_chooser_options(
        options: &mut HashMap<&'static str, Value<'_>>,
        chooser: &ChooserOptions,
        member: &str,
    ) {
        let filters = chooser.wire_filters();
        if !filters.is_empty() {
            options.insert("filters", Value::from(filters));
        }
        if let Some(current) = chooser.wire_current_filter() {
            options.insert("current_filter", Value::from(current));
        }
        if let Some(bytes) = chooser.current_folder_bytes() {
            options.insert("current_folder", Value::from(bytes));
        }
        if member == "SaveFile"
            && let Some(name) = chooser.name()
        {
            options.insert("current_name", Value::from(name.to_owned()));
        }
    }

    pub(super) async fn notification_action_stream(
        connection: &zbus::Connection,
    ) -> Result<zbus::MessageStream, zbus::Error> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .path(PORTAL_PATH)?
            .interface(NOTIFICATION_IFACE)?
            .member("ActionInvoked")?
            .build();
        zbus::MessageStream::for_match_rule(
            rule,
            connection,
            Some(NOTIFICATION_ACTION_QUEUE_CAPACITY),
        )
        .await
    }

    pub(super) fn publish_notification_action(
        shared: &Arc<Mutex<SharedState>>,
        signal: zbus::Message,
    ) {
        // Sender identity and encoded wire length gate deserialization so an
        // arbitrarily large or misdirected payload never allocates typed action
        // buffers on the reducer path.
        let header = signal.header();
        let sender = header.sender().map(|sender| sender.as_str().to_owned());
        let body_len = signal.primary_header().body_len() as usize;
        let (owner_generation, sender_matches, over_wire) = {
            let state = lock_shared(shared);
            let matches = sender.as_deref() == state.portal_owner.as_deref();
            (
                state.portal_generation,
                matches,
                body_len > MAX_NOTIFICATION_ACTION_WIRE_BYTES,
            )
        };
        if !sender_matches {
            Self::record_rejected_action(shared);
            return;
        }
        if over_wire {
            Self::record_rejected_action(shared);
            return;
        }
        let Ok((notification_id, action_id, parameters)) =
            signal
                .body()
                .deserialize::<(String, String, Vec<OwnedValue>)>()
        else {
            Self::record_rejected_action(shared);
            return;
        };
        if notification_id.is_empty()
            || notification_id.len() > MAX_NOTIFICATION_ID_BYTES
            || action_id.len() > MAX_NOTIFICATION_ID_BYTES
            || parameters.len() > MAX_NOTIFICATION_ACTION_PARAMETERS
        {
            Self::record_rejected_action(shared);
            return;
        }
        let mut target = None;
        let mut activation_token = None;
        let mut response = None;
        let mut malformed = false;
        let mut parameters = parameters.into_iter().peekable();
        if parameters.peek().is_some_and(|parameter| {
            parameter
                .try_clone()
                .and_then(HashMap::<String, OwnedValue>::try_from)
                .is_err()
        }) {
            target = parameters
                .next()
                .as_ref()
                .and_then(Self::notification_action_target);
            malformed |= target.is_none();
        }
        if let Some(parameter) = parameters.next() {
            match HashMap::<String, OwnedValue>::try_from(parameter) {
                Ok(platform) => match Self::notification_activation_token(&platform) {
                    Ok(token) => activation_token = token,
                    Err(crate::activation::InboundTokenError::TooLong) => {
                        Self::record_rejected_action(shared);
                        return;
                    }
                    Err(_) => malformed = true,
                },
                Err(_) => malformed = true,
            }
            if let Some(parameter) = parameters.next() {
                response = String::try_from(parameter).ok();
                malformed |= response.is_none();
            }
        }
        malformed |= parameters.next().is_some();
        if Self::action_field_over_limit(target.as_ref())
            || activation_token
                .as_ref()
                .is_some_and(|token| token.byte_len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES)
            || response
                .as_deref()
                .is_some_and(|value| value.len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES)
        {
            Self::record_rejected_action(shared);
            return;
        }

        let mut state = lock_shared(shared);
        // Portal ownership must still match the generation observed above.
        if state.portal_generation != owner_generation
            || sender.as_deref() != state.portal_owner.as_deref()
        {
            state.notification_rejected_actions =
                state.notification_rejected_actions.saturating_add(1);
            let completion = state.completion.clone();
            drop(state);
            completion.notify();
            return;
        }
        let action_count = state
            .notification_events
            .iter()
            .filter(|event| matches!(event, NotificationBackendEvent::Action(_)))
            .count();
        if action_count == NOTIFICATION_ACTION_QUEUE_CAPACITY {
            state.notification_dropped_actions =
                state.notification_dropped_actions.saturating_add(1);
            let completion = state.completion.clone();
            drop(state);
            completion.notify();
            return;
        }
        state
            .notification_events
            .push_back(NotificationBackendEvent::Action(
                NotificationBackendAction {
                    notification_id,
                    action_id,
                    target,
                    activation_token,
                    response,
                    malformed,
                    owner_generation,
                    received_at: std::time::Instant::now(),
                },
            ));
        let completion = state.completion.clone();
        drop(state);
        completion.notify();
    }

    pub(super) fn record_rejected_action(shared: &Arc<Mutex<SharedState>>) {
        let completion = {
            let mut state = lock_shared(shared);
            state.notification_rejected_actions =
                state.notification_rejected_actions.saturating_add(1);
            state.completion.clone()
        };
        completion.notify();
    }

    pub(super) fn action_field_over_limit(target: Option<&NotificationActionTarget>) -> bool {
        match target {
            Some(NotificationActionTarget::Text(value)) => {
                value.len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES
            }
            Some(NotificationActionTarget::Bytes(bytes)) => {
                bytes.len() > MAX_NOTIFICATION_ACTION_VALUE_BYTES
            }
            _ => false,
        }
    }

    pub(super) fn notification_activation_token(
        platform: &HashMap<String, OwnedValue>,
    ) -> Result<
        Option<crate::activation::InboundActivationToken>,
        crate::activation::InboundTokenError,
    > {
        crate::activation::inbound_platform_data(platform)
    }

    pub(super) fn notification_action_target(
        value: &OwnedValue,
    ) -> Option<NotificationActionTarget> {
        if let Some(value) = value
            .try_clone()
            .ok()
            .and_then(|value| String::try_from(value).ok())
        {
            return Some(NotificationActionTarget::Text(value));
        }
        if let Ok(value) = i64::try_from(value) {
            return Some(NotificationActionTarget::Integer(value));
        }
        if let Ok(value) = u64::try_from(value) {
            return Some(NotificationActionTarget::Unsigned(value));
        }
        if let Ok(value) = bool::try_from(value) {
            return Some(NotificationActionTarget::Boolean(value));
        }
        value
            .try_clone()
            .ok()
            .and_then(|value| Vec::<u8>::try_from(value).ok())
            .map(NotificationActionTarget::Bytes)
    }

    pub(super) async fn run_notification_command(
        connection: &zbus::Connection,
        portal_owner: &str,
        command: &NotificationBackendCommand,
        application_bound: bool,
    ) -> NotificationBackendOutcome {
        if let NotificationBackendCommand::Send(notification) = command
            && notification.has_exported_actions()
            && !application_bound
        {
            return NotificationBackendOutcome::Unsupported(
                UnsupportedNotificationFeature::ExportedAction,
            );
        }
        let proxy =
            match zbus::Proxy::new(connection, portal_owner, PORTAL_PATH, NOTIFICATION_IFACE).await
            {
                Ok(proxy) => proxy,
                Err(error) => return Self::notification_call_error(&error),
            };
        let version = match proxy.get_property::<u32>("version").await {
            Ok(version) => version,
            Err(error) => return Self::notification_call_error(&error),
        };
        match command {
            NotificationBackendCommand::Send(notification) => {
                let supported = if version >= 2 {
                    match proxy
                        .get_property::<HashMap<String, OwnedValue>>("SupportedOptions")
                        .await
                    {
                        Ok(options) => options,
                        Err(error) => return Self::notification_call_error(&error),
                    }
                } else {
                    HashMap::new()
                };
                if let Some(feature) =
                    Self::unsupported_notification_feature(notification, version, &supported)
                {
                    return NotificationBackendOutcome::Unsupported(feature);
                }
                let Ok(wire) = Self::wire_notification(notification, version) else {
                    return NotificationBackendOutcome::Failed;
                };
                match connection
                    .call_method(
                        Some(portal_owner),
                        PORTAL_PATH,
                        Some(NOTIFICATION_IFACE),
                        "AddNotification",
                        &(notification.id().as_str(), wire),
                    )
                    .await
                {
                    Ok(_) => NotificationBackendOutcome::Sent,
                    Err(error) => Self::notification_call_error(&error),
                }
            }
            NotificationBackendCommand::Withdraw(id) => match connection
                .call_method(
                    Some(portal_owner),
                    PORTAL_PATH,
                    Some(NOTIFICATION_IFACE),
                    "RemoveNotification",
                    &(id.as_str(),),
                )
                .await
            {
                Ok(_) => NotificationBackendOutcome::Withdrawn,
                Err(error) => Self::notification_call_error(&error),
            },
        }
    }

    pub(super) fn unsupported_notification_feature(
        notification: &DesktopNotification,
        version: u32,
        supported: &HashMap<String, OwnedValue>,
    ) -> Option<UnsupportedNotificationFeature> {
        if notification.reply_request().is_some()
            && (version < 2
                || notification
                    .notification_category()
                    .is_some_and(|category| category != "im.received")
                || !Self::notification_option_supported(supported, "category", "im.received")
                || !Self::notification_option_supported(
                    supported,
                    "button-purpose",
                    "im.reply-with-text",
                ))
        {
            return Some(UnsupportedNotificationFeature::InlineReply);
        }
        if version < 2 {
            if notification.markup().is_some() {
                return Some(UnsupportedNotificationFeature::MarkupBody);
            }
            if !notification.hints().is_empty() {
                return Some(UnsupportedNotificationFeature::DisplayHints);
            }
            if let Some(category) = notification.notification_category() {
                return Some(UnsupportedNotificationFeature::Category(
                    category.to_owned(),
                ));
            }
            if let Some(purpose) = notification
                .action_buttons()
                .iter()
                .find_map(|button| button.button_purpose())
            {
                return Some(UnsupportedNotificationFeature::ButtonPurpose(
                    purpose.as_str().to_owned(),
                ));
            }
            return None;
        }
        if let Some(category) = notification.notification_category() {
            let advertised = supported
                .get("category")
                .and_then(|value| value.try_clone().ok())
                .and_then(|value| Vec::<String>::try_from(value).ok())
                .is_some_and(|values| values.iter().any(|value| value == category));
            if !advertised {
                return Some(UnsupportedNotificationFeature::Category(
                    category.to_owned(),
                ));
            }
        }
        for purpose in notification
            .action_buttons()
            .iter()
            .filter_map(|button| button.button_purpose())
        {
            let advertised = supported
                .get("button-purpose")
                .and_then(|value| value.try_clone().ok())
                .and_then(|value| Vec::<String>::try_from(value).ok())
                .is_some_and(|values| values.iter().any(|value| value == purpose.as_str()));
            if !advertised {
                return Some(UnsupportedNotificationFeature::ButtonPurpose(
                    purpose.as_str().to_owned(),
                ));
            }
        }
        None
    }

    fn notification_option_supported(
        supported: &HashMap<String, OwnedValue>,
        option: &str,
        expected: &str,
    ) -> bool {
        supported
            .get(option)
            .and_then(|value| value.try_clone().ok())
            .and_then(|value| Vec::<String>::try_from(value).ok())
            .is_some_and(|values| values.iter().any(|value| value == expected))
    }

    pub(super) fn wire_notification(
        notification: &DesktopNotification,
        version: u32,
    ) -> std::io::Result<ashpd::desktop::notification::Notification> {
        use ashpd::desktop::notification::{Button, Category, DisplayHint, Notification, Priority};

        let mut wire = Notification::new(notification.title())
            .body(notification.plain_body())
            .markup_body(notification.markup())
            .priority(match notification.notification_priority() {
                NotificationPriority::Low => Priority::Low,
                NotificationPriority::Normal => Priority::Normal,
                NotificationPriority::High => Priority::High,
                NotificationPriority::Urgent => Priority::Urgent,
            });
        if let Some(icon) = notification.notification_icon() {
            wire = wire.icon(notification_icon::prepare(icon, version)?);
        }
        if let Some(category) = notification
            .notification_category()
            .or_else(|| notification.reply_request().map(|_| "im.received"))
        {
            wire = wire.category(Category::Other(category.to_owned()));
        }
        wire = wire.display_hint(notification.hints().iter().map(|hint| match hint {
            NotificationDisplayHint::Transient => DisplayHint::Transient,
            NotificationDisplayHint::Tray => DisplayHint::Tray,
            NotificationDisplayHint::Persistent => DisplayHint::Persistent,
            NotificationDisplayHint::HideOnLockScreen => DisplayHint::HideOnLockScreen,
            NotificationDisplayHint::HideContentOnLockScreen => {
                DisplayHint::HideContentOnLockScreen
            }
            NotificationDisplayHint::ShowAsNew => DisplayHint::ShowAsNew,
        }));
        if let Some(action) = notification.default_activation() {
            wire = wire.default_action(action.wire_name().as_ref());
            wire = Self::wire_default_target(wire, action.action_target());
        }
        for button in notification.action_buttons() {
            let mut wire_button = Button::new(button.label(), button.action().wire_name().as_ref());
            wire_button = Self::wire_button_target(wire_button, button.action().action_target());
            if let Some(purpose) = button.button_purpose() {
                wire_button = wire_button.purpose(
                    purpose
                        .as_str()
                        .parse::<ashpd::desktop::notification::ButtonPurpose>()
                        .ok(),
                );
            }
            wire = wire.button(wire_button);
        }
        if let Some(reply) = notification.reply_request() {
            let button = Button::new(reply.submit_label(), reply.action().wire_name().as_ref())
                .purpose(Some(
                    ashpd::desktop::notification::ButtonPurpose::ImReplyWithText,
                ));
            wire = wire.button(Self::wire_button_target(
                button,
                reply.action().action_target(),
            ));
        }
        Ok(wire)
    }

    pub(super) fn wire_default_target(
        notification: ashpd::desktop::notification::Notification,
        target: Option<&NotificationActionTarget>,
    ) -> ashpd::desktop::notification::Notification {
        match target {
            Some(NotificationActionTarget::Text(value)) => {
                notification.default_action_target(value.as_str())
            }
            Some(NotificationActionTarget::Integer(value)) => {
                notification.default_action_target(*value)
            }
            Some(NotificationActionTarget::Unsigned(value)) => {
                notification.default_action_target(*value)
            }
            Some(NotificationActionTarget::Boolean(value)) => {
                notification.default_action_target(*value)
            }
            Some(NotificationActionTarget::Bytes(value)) => {
                notification.default_action_target(value.as_slice())
            }
            None => notification,
        }
    }

    pub(super) fn wire_button_target(
        button: ashpd::desktop::notification::Button,
        target: Option<&NotificationActionTarget>,
    ) -> ashpd::desktop::notification::Button {
        match target {
            Some(NotificationActionTarget::Text(value)) => button.target(value.as_str()),
            Some(NotificationActionTarget::Integer(value)) => button.target(*value),
            Some(NotificationActionTarget::Unsigned(value)) => button.target(*value),
            Some(NotificationActionTarget::Boolean(value)) => button.target(*value),
            Some(NotificationActionTarget::Bytes(value)) => button.target(value.as_slice()),
            None => button,
        }
    }

    pub(super) fn notification_call_error(error: &zbus::Error) -> NotificationBackendOutcome {
        let name = match error {
            zbus::Error::MethodError(name, _, _) => Some(name.as_str()),
            _ => None,
        };
        match name {
            Some(
                "org.freedesktop.DBus.Error.UnknownInterface"
                | "org.freedesktop.DBus.Error.UnknownMethod"
                | "org.freedesktop.DBus.Error.UnknownProperty",
            ) => NotificationBackendOutcome::Unsupported(
                UnsupportedNotificationFeature::Notifications,
            ),
            Some("org.freedesktop.DBus.Error.AccessDenied") => NotificationBackendOutcome::Denied,
            Some(
                "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner",
            ) => NotificationBackendOutcome::Unavailable,
            _ => NotificationBackendOutcome::Failed,
        }
    }
}
