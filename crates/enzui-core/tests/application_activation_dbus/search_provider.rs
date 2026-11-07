//! `dev.enzui.SearchProvider1` over a private bus, called as the desktop's
//! Search calls it: bounded answers, one activation per choice with its
//! token, a newer query superseding a running one, and the deadline.

use super::{APP_ID, APP_PATH, PrivateBus, platform_data};
use enzui_core::activation::ActivationBroker;
use enzui_core::activation::dbus::ApplicationService;
use enzui_core::activation::dbus::search::{
    DEFAULT_SEARCH_LIMIT, MAX_SEARCH_RESULTS, SEARCH_INTERFACE, SearchInbox, SearchProvider,
    SearchRequest, SearchResult,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use zbus::zvariant::OwnedValue;

/// Thirty results for any query, but none for `slow` or `late` until they
/// have waited.
fn provider() -> SearchProvider {
    SearchProvider::new(|query| async move {
        match query.text.as_str() {
            "slow" => tokio::time::sleep(Duration::from_secs(10)).await,
            "late" => tokio::time::sleep(Duration::from_secs(3)).await,
            _ => {}
        }
        (0..30)
            .map(|i| {
                SearchResult::new(format!("msg-{i}"), format!("{} {i}", query.text))
                    .unwrap()
                    .subtitle("from Ada")
                    .icon("mail-unread")
                    .score(0.5)
            })
            .collect()
    })
}

struct Desktop {
    _bus: PrivateBus,
    service: ApplicationService,
    inbox: SearchInbox,
    runtime: tokio::runtime::Runtime,
    proxy: zbus::Proxy<'static>,
}

fn desktop() -> Desktop {
    let bus = PrivateBus::spawn();
    let service = ApplicationService::connect_to_address(APP_ID, &bus.address).unwrap();
    let inbox = service.serve_search(provider()).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let proxy = runtime.block_on(async {
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        zbus::Proxy::new_owned(connection, APP_ID, APP_PATH, SEARCH_INTERFACE)
            .await
            .unwrap()
    });
    Desktop {
        _bus: bus,
        service,
        inbox,
        runtime,
        proxy,
    }
}

/// Choosing a result reaches the app once, as an open request carrying the
/// token the desktop sent; a malformed choice is refused and never queued.
#[test]
fn a_chosen_result_reaches_the_app_once_with_its_token() {
    let desktop = desktop();
    let before = desktop.service.completion_signal().generation();
    desktop.runtime.block_on(async {
        desktop
            .proxy
            .call_method(
                "Activate",
                &("msg-7", "inbox", platform_data(Some("search-token-3224"))),
            )
            .await
            .unwrap();
        let refused = desktop
            .proxy
            .call_method("Activate", &("", "inbox", platform_data(None)))
            .await;
        assert!(refused.is_err(), "an empty id is refused");
    });
    assert!(
        desktop.service.completion_signal().generation() > before,
        "the app is woken"
    );

    let mut broker = ActivationBroker::default();
    let activations = desktop.inbox.drain(&mut broker, Instant::now(), 8);
    assert_eq!(activations.len(), 1, "{activations:?}");
    assert_eq!(
        activations[0].request,
        SearchRequest::Open {
            id: "msg-7".into(),
            query: "inbox".into()
        }
    );
    assert!(activations[0].context.is_some(), "the token came along");
    assert!(
        desktop
            .inbox
            .drain(&mut broker, Instant::now(), 8)
            .is_empty()
    );

    desktop.runtime.block_on(async {
        desktop
            .proxy
            .call_method("LaunchSearch", &("inbox", platform_data(None)))
            .await
            .unwrap();
    });
    let launched = desktop.inbox.drain(&mut broker, Instant::now(), 8);
    assert_eq!(
        launched[0].request,
        SearchRequest::Search {
            query: "inbox".into()
        }
    );
    assert!(launched[0].context.is_none());
}

fn proxy_for(desktop: &Desktop, interface: &'static str) -> zbus::Proxy<'static> {
    desktop.runtime.block_on(async {
        zbus::Proxy::new_owned(
            desktop.proxy.connection().clone(),
            APP_ID,
            APP_PATH,
            interface,
        )
        .await
        .unwrap()
    })
}

/// GNOME Shell asks for ids, then for the details of those it shows, and
/// opens one by id with the terms typed.
#[test]
fn gnome_shell_searches_the_same_provider() {
    let desktop = desktop();
    let shell = proxy_for(&desktop, "org.gnome.Shell.SearchProvider2");
    let (ids, metas): (Vec<String>, Vec<HashMap<String, OwnedValue>>) =
        desktop.runtime.block_on(async {
            let ids: Vec<String> = shell
                .call("GetInitialResultSet", &(vec!["in", "box"],))
                .await
                .unwrap();
            let metas = shell
                .call("GetResultMetas", &(vec![ids[2].as_str(), "unknown"],))
                .await
                .unwrap();
            (ids, metas)
        });
    assert_eq!(ids.len(), DEFAULT_SEARCH_LIMIT);
    assert_eq!(ids[0], "msg-0");
    assert_eq!(metas.len(), 1, "an unknown id has no details");
    let text = |key: &str| String::try_from(metas[0][key].try_clone().unwrap()).unwrap();
    assert_eq!(text("id"), "msg-2");
    assert_eq!(text("name"), "in box 2");
    assert_eq!(text("description"), "from Ada");
    assert_eq!(text("gicon"), "mail-unread");

    desktop.runtime.block_on(async {
        let _: () = shell
            .call("ActivateResult", &("msg-2", vec!["in", "box"], 0u32))
            .await
            .unwrap();
        let _: () = shell
            .call("LaunchSearch", &(vec!["in", "box"], 0u32))
            .await
            .unwrap();
    });
    let mut broker = ActivationBroker::default();
    let requests: Vec<SearchRequest> = desktop
        .inbox
        .drain(&mut broker, Instant::now(), 8)
        .into_iter()
        .map(|activation| activation.request)
        .collect();
    assert_eq!(
        requests,
        [
            SearchRequest::Open {
                id: "msg-2".into(),
                query: "in box".into()
            },
            SearchRequest::Search {
                query: "in box".into()
            }
        ]
    );
}

type KRunnerMatch = (
    String,
    String,
    String,
    i32,
    f64,
    HashMap<String, OwnedValue>,
);

/// KRunner gets matches with their text, icon and relevance, and running
/// one opens it with the query that found it.
#[test]
fn krunner_matches_and_runs_the_same_provider() {
    let desktop = desktop();
    let runner = proxy_for(&desktop, "org.kde.krunner1");
    let matches: Vec<KRunnerMatch> = desktop.runtime.block_on(async {
        let matches = runner.call("Match", &("draft",)).await.unwrap();
        let _: () = runner.call("Run", &("msg-4", "")).await.unwrap();
        matches
    });
    assert_eq!(matches.len(), MAX_SEARCH_RESULTS);
    let (id, text, icon, kind, relevance, properties) = &matches[4];
    assert_eq!(
        (id.as_str(), text.as_str(), icon.as_str()),
        ("msg-4", "draft 4", "mail-unread")
    );
    assert_eq!(
        (*kind, *relevance),
        (30, 0.5),
        "a possible match at its score"
    );
    assert_eq!(
        String::try_from(properties["subtext"].try_clone().unwrap()).unwrap(),
        "from Ada"
    );
    let mut broker = ActivationBroker::default();
    let opened = desktop.inbox.drain(&mut broker, Instant::now(), 8);
    assert_eq!(
        opened[0].request,
        SearchRequest::Open {
            id: "msg-4".into(),
            query: "draft".into()
        }
    );
}
