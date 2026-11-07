#![cfg(feature = "application-dbus")]
#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::activation::dbus::ApplicationService;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use zbus::zvariant::{OwnedValue, Value};

const APP_ID: &str = "org.enzui.ActivationFixture";
const APP_PATH: &str = "/org/enzui/ActivationFixture";

#[path = "support/private_bus.rs"]
mod private_bus;
use private_bus::PrivateBus;

fn platform_data(token: Option<&str>) -> HashMap<String, OwnedValue> {
    token
        .map(|token| {
            HashMap::from([(
                "activation-token".to_owned(),
                OwnedValue::try_from(Value::from(token)).unwrap(),
            )])
        })
        .unwrap_or_default()
}

fn lifecycle_reply(
    bus: &PrivateBus,
    method: &str,
    stop: Option<(&str, &str)>,
) -> zbus::Result<zbus::Message> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let connection = zbus::connection::Builder::address(bus.address.as_str())?
            .build()
            .await?;
        let interface = Some(enzui_core::activation::lifecycle::LIFECYCLE_INTERFACE);
        match stop {
            None => {
                connection
                    .call_method(Some(APP_ID), APP_PATH, interface, method, &())
                    .await
            }
            Some(body) => {
                connection
                    .call_method(Some(APP_ID), APP_PATH, interface, method, &body)
                    .await
            }
        }
    })
}

/// QueryStop's reply: the decision and its per-window rows.
type StopReply = (String, Vec<(String, String, String)>);

fn query_stop(bus: &PrivateBus) -> zbus::Result<StopReply> {
    lifecycle_reply(bus, "QueryStop", None)?
        .body()
        .deserialize()
}

fn request_stop(bus: &PrivateBus, choice: &str, revision: &str) -> zbus::Result<String> {
    lifecycle_reply(bus, "Stop", Some((choice, revision)))?
        .body()
        .deserialize()
}

/// Work's reply: known, active count, kinds, revision.
type WorkReply = (bool, u32, Vec<String>, u64);

fn query_work(bus: &PrivateBus) -> zbus::Result<WorkReply> {
    lifecycle_reply(bus, "Work", None)?.body().deserialize()
}

#[test]
fn a_caller_that_is_not_the_os_service_is_refused() {
    use enzui_core::activation::lifecycle::CallerPolicy;
    let bus = PrivateBus::spawn();
    let service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let stop = service
        .serve_lifecycle(CallerPolicy {
            executable: "/usr/lib/example/background-service".into(),
            bus_name: "org.example.Background1".into(),
            unit: "example-background.service".into(),
            witness: None,
        })
        .unwrap();
    let query = query_stop(&bus);
    assert!(
        matches!(&query, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied"),
        "{query:?}"
    );
    // The refusal names the first check the caller failed, so the OS
    // service's log says why.
    assert!(
        matches!(&query, Err(zbus::Error::MethodError(_, Some(message), _))
            if message == "not an admitted lifecycle caller: \
                           the caller does not own the service's bus name"),
        "{query:?}"
    );
    let stopped = request_stop(&bus, "cancel-and-exit", "1");
    assert!(stopped.is_err(), "{stopped:?}");
    assert_eq!(stop.take_stop(), None, "a refused caller queues nothing");
    stop.publish_work([enzui_core::activation::lifecycle::WorkKind::Copy]);
    let work = query_work(&bus);
    assert!(
        matches!(&work, Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.AccessDenied"),
        "a refused caller reads no work: {work:?}"
    );
}

#[path = "application_activation_dbus/search_provider.rs"]
mod search_provider;

#[path = "support/activation_child.rs"]
mod activation_child;
use activation_child::child_activation_environment;

fn send_activation_platform_data(bus: &PrivateBus, data: &HashMap<String, OwnedValue>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(3),
            connection.call_method(
                Some(APP_ID),
                APP_PATH,
                Some("org.freedesktop.Application"),
                "Activate",
                &(data,),
            ),
        )
        .await
        .unwrap()
        .unwrap();
    });
}

#[test]
fn activation_dbus_platform_keys_reach_only_the_matching_child_environment() {
    let bus = PrivateBus::spawn();
    let mut service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let mut broker = enzui_core::activation::ActivationBroker::default();
    for (key, expected) in [
        ("activation-token", "unset\nsame-opaque-value\n"),
        ("desktop-startup-id", "same-opaque-value\nunset\n"),
    ] {
        let data = HashMap::from([(
            key.to_owned(),
            OwnedValue::from(zbus::zvariant::Str::from("same-opaque-value")),
        )]);
        send_activation_platform_data(&bus, &data);
        let mut calls = service.drain(&mut broker, Instant::now(), 8);
        assert_eq!(calls.len(), 1);
        let call = calls.pop().unwrap();
        assert_eq!(call.context_error, None);
        let context = call.context.expect("the desktop platform context was lost");
        assert_eq!(child_activation_environment(context), expected);
    }
}

#[test]
fn activation_dbus_x11_context_validation_keeps_the_intent() {
    let bus = PrivateBus::spawn();
    let mut service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let mut broker = enzui_core::activation::ActivationBroker::default();
    use enzui_core::activation::InboundTokenError;
    for (value, error) in [
        (OwnedValue::from(17_u32), InboundTokenError::Malformed),
        (
            OwnedValue::from(zbus::zvariant::Str::from("")),
            InboundTokenError::Empty,
        ),
        (
            OwnedValue::from(zbus::zvariant::Str::from("x".repeat(4097))),
            InboundTokenError::TooLong,
        ),
    ] {
        let data = HashMap::from([("desktop-startup-id".to_owned(), value)]);
        send_activation_platform_data(&bus, &data);
        let mut calls = service.drain(&mut broker, Instant::now(), 8);
        assert_eq!(calls.len(), 1);
        let call = calls.pop().unwrap();
        assert_eq!(
            call.intent,
            enzui_core::activation::ApplicationIntent::Activate
        );
        assert!(call.context.is_none());
        assert_eq!(call.context_error, Some(error));
    }
}

#[test]
fn activation_dbus_typed_forwarding_and_installed_activation_keep_both_protocols() {
    use enzui_core::activation::dbus::ApplicationStart;
    use enzui_core::activation::{ActivationBroker, ApplicationIntent, InboundActivationToken};
    let bus = PrivateBus::spawn();
    let mut service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let mut broker = ActivationBroker::default();
    for (wayland, x11, expected) in [
        (Some("wayland-field"), None, "unset\nwayland-field\n"),
        (None, Some("x11-field"), "x11-field\nunset\n"),
        (
            Some("wayland-field"),
            Some("x11-field"),
            "x11-field\nwayland-field\n",
        ),
    ] {
        for installed in [false, true] {
            let mut token = InboundActivationToken::from_platform_data(
                wayland.map(str::to_owned),
                x11.map(str::to_owned),
            )
            .unwrap();
            if installed {
                ApplicationService::activate_installed_with_token_at_address(
                    APP_ID,
                    &bus.address,
                    &ApplicationIntent::Activate,
                    token,
                )
                .unwrap();
            } else {
                let result = ApplicationService::connect_or_forward_with_token_to_address(
                    APP_ID,
                    &bus.address,
                    Some(&ApplicationIntent::Activate),
                    &mut token,
                )
                .unwrap();
                assert!(matches!(result, ApplicationStart::Reused));
                assert!(token.is_none());
            }
            let mut calls = service.drain(&mut broker, Instant::now(), 8);
            assert_eq!(calls.len(), 1);
            let call = calls.pop().unwrap();
            assert_eq!(call.context_error, None);
            assert_eq!(
                child_activation_environment(call.context.unwrap()),
                expected
            );
        }
    }
}

#[test]
fn activation_dbus_combined_context_is_bounded_and_single_use() {
    use enzui_core::activation::{ActivationBroker, ActivationHandoffError, InboundTokenError};
    let bus = PrivateBus::spawn();
    let mut service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let mut broker = ActivationBroker::default();
    let data = HashMap::from([
        (
            "activation-token".to_owned(),
            OwnedValue::from(zbus::zvariant::Str::from("w".repeat(2049))),
        ),
        (
            "desktop-startup-id".to_owned(),
            OwnedValue::from(zbus::zvariant::Str::from("x".repeat(2049))),
        ),
    ]);
    send_activation_platform_data(&bus, &data);
    let mut calls = service.drain(&mut broker, Instant::now(), 8);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].context_error, Some(InboundTokenError::TooLong));
    assert!(calls[0].context.is_none());
    assert!(service.drain(&mut broker, Instant::now(), 8).is_empty());
    let fresh = platform_data(Some("positive-control"));
    send_activation_platform_data(&bus, &fresh);
    calls = service.drain(&mut broker, Instant::now(), 8);
    assert_eq!(calls.len(), 1);
    let context = calls.pop().unwrap().context.unwrap();
    assert!(matches!(
        context.into_token(Instant::now() + Duration::from_secs(11)),
        Err(ActivationHandoffError::Expired)
    ));
    assert!(service.drain(&mut broker, Instant::now(), 8).is_empty());
}

#[test]
fn activation_dbus_untyped_context_requires_selection_without_extending_its_deadline() {
    use activation_untyped::assert_untyped_context_needs_platform;
    use enzui_core::activation::dbus::{ApplicationServiceError, ApplicationStart};
    use enzui_core::activation::{
        ActivationBroker, ActivationHandoffError, ActivationPlatform, ApplicationIntent,
        InboundActivationToken,
    };
    let bus = PrivateBus::spawn();
    let mut service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let mut broker = ActivationBroker::default();
    let native = || InboundActivationToken::native("same-opaque-value".into());
    let mut untyped = Some(native());
    let refused = ApplicationService::connect_or_forward_with_token_to_address(
        APP_ID,
        &bus.address,
        Some(&ApplicationIntent::Activate),
        &mut untyped,
    );
    assert!(matches!(
        refused,
        Err(ApplicationServiceError::InvalidRequest)
    ));
    assert!(untyped.is_some());
    assert!(service.drain(&mut broker, Instant::now(), 8).is_empty());
    assert_untyped_context_needs_platform(broker.admit_token(native(), Instant::now()).unwrap());
    let selected = broker
        .admit_token(native(), Instant::now())
        .unwrap()
        .select_platform(ActivationPlatform::X11)
        .unwrap();
    let mut selected = Some(selected.into_token(Instant::now()).unwrap());
    let delivered = ApplicationService::connect_or_forward_with_token_to_address(
        APP_ID,
        &bus.address,
        Some(&ApplicationIntent::Activate),
        &mut selected,
    )
    .unwrap();
    assert!(matches!(delivered, ApplicationStart::Reused));
    assert!(selected.is_none());
    let mut calls = service.drain(&mut broker, Instant::now(), 8);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        child_activation_environment(calls.pop().unwrap().context.unwrap()),
        "same-opaque-value\nunset\n"
    );
    let now = Instant::now();
    let expired = broker
        .admit_token(native(), now - Duration::from_secs(11))
        .unwrap()
        .select_platform(ActivationPlatform::X11)
        .unwrap();
    assert!(matches!(
        expired.into_token(now),
        Err(ActivationHandoffError::Expired)
    ));
    let x11 = InboundActivationToken::for_platform(ActivationPlatform::X11, "x11".into());
    assert!(matches!(
        broker
            .admit_token(x11, now)
            .unwrap()
            .select_platform(ActivationPlatform::Wayland),
        Err(ActivationHandoffError::WrongPlatform)
    ));
}

#[path = "support/activation_untyped.rs"]
mod activation_untyped;

#[test]
fn activation_dbus_first_owner_retains_startup_context() {
    use enzui_core::activation::dbus::ApplicationStart;
    use enzui_core::activation::{
        ActivationBroker, ActivationPlatform, ApplicationIntent, InboundActivationToken,
    };
    let bus = PrivateBus::spawn();
    let mut token = Some(InboundActivationToken::for_platform(
        ActivationPlatform::X11,
        "first-owner-context".into(),
    ));
    let start = ApplicationService::connect_or_forward_with_token_to_address(
        APP_ID,
        &bus.address,
        Some(&ApplicationIntent::Activate),
        &mut token,
    )
    .unwrap();
    let ApplicationStart::Owner(mut service) = start else {
        panic!("missing primary owner")
    };
    assert_eq!(
        token.as_ref().unwrap().desktop_startup_id(),
        Some("first-owner-context")
    );
    let mut broker = ActivationBroker::default();
    assert!(service.drain(&mut broker, Instant::now(), 8).is_empty());
    let context = broker
        .admit_token(token.take().unwrap(), Instant::now())
        .unwrap();
    assert_eq!(
        child_activation_environment(context),
        "first-owner-context\nunset\n"
    );
}

#[test]
fn activation_dbus_a_second_launch_opens_its_files_in_the_running_instance() {
    use enzui_core::activation::dbus::ApplicationStart;
    use enzui_core::activation::{ActivationBroker, ApplicationIntent, launch_request};
    let bus = PrivateBus::spawn();
    let cwd = std::path::Path::new("/home/ada/notes");
    let first = launch_request(Vec::<String>::new(), cwd);
    let start = ApplicationService::connect_or_forward_to_address(
        APP_ID,
        &bus.address,
        Some(&first.intent),
        None,
    )
    .unwrap();
    let ApplicationStart::Owner(mut service) = start else {
        panic!("the first launch owns the application")
    };
    let second = launch_request(["today.md"], cwd);
    let forwarded = ApplicationService::connect_or_forward_to_address(
        APP_ID,
        &bus.address,
        Some(&second.intent),
        None,
    )
    .unwrap();
    assert!(
        matches!(forwarded, ApplicationStart::Reused),
        "the second launch forwards"
    );
    let mut broker = ActivationBroker::default();
    let deadline = Instant::now() + Duration::from_secs(5);
    let calls = loop {
        let calls = service.drain(&mut broker, Instant::now(), 8);
        if !calls.is_empty() || Instant::now() > deadline {
            break calls;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let intents: Vec<_> = calls.into_iter().map(|call| call.intent).collect();
    assert_eq!(
        intents,
        [ApplicationIntent::Open(vec![
            "file:///home/ada/notes/today.md".into()
        ])]
    );
}
