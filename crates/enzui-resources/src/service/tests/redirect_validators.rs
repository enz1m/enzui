// Service integration tests exercising the transport/cache seam across a
// redirect. `include!`d into `service::tests`, so each test keeps its
// `service::tests::…` path and shares that module's imports and helpers
// (including `CacheDir`/`cache_config_at` from the `disk_cache` family). Do not
// add `use super::*;` here.
//
// @service-tests-family: redirect_validators
//
// The real redirect loop runs beneath the real `HttpCache` and `ImageService`,
// so the verdict is the original entry's on-disk validator after a cross-origin
// 304 attempt, not a transport-call count.

#[test]
fn a_cross_origin_304_during_revalidation_does_not_rewrite_the_original_cache_key() {
    const URL: &str = "https://origin.test/asset.png";

    struct Case {
        label: &'static str,
        populate: Vec<(String, String)>,
        attacker: Vec<(String, String)>,
        read: fn(&crate::http_cache::CachedResponse) -> Option<String>,
        original: &'static str,
        attacker_value: &'static str,
    }

    let etag_case = Case {
        label: "etag",
        populate: vec![
            ("cache-control".to_owned(), "max-age=0".to_owned()),
            ("etag".to_owned(), "\"v7\"".to_owned()),
        ],
        attacker: vec![
            ("cache-control".to_owned(), "max-age=3600".to_owned()),
            ("etag".to_owned(), "\"attacker\"".to_owned()),
        ],
        read: |entry| entry.etag.clone(),
        original: "\"v7\"",
        attacker_value: "\"attacker\"",
    };
    let last_modified_case = Case {
        label: "last-modified",
        populate: vec![
            ("cache-control".to_owned(), "max-age=0".to_owned()),
            (
                "last-modified".to_owned(),
                "Wed, 21 Oct 2015 07:28:00 GMT".to_owned(),
            ),
        ],
        attacker: vec![
            ("cache-control".to_owned(), "max-age=3600".to_owned()),
            (
                "last-modified".to_owned(),
                "Sun, 06 Nov 1994 08:49:37 GMT".to_owned(),
            ),
        ],
        read: |entry| entry.last_modified.clone(),
        original: "Wed, 21 Oct 2015 07:28:00 GMT",
        attacker_value: "Sun, 06 Nov 1994 08:49:37 GMT",
    };

    for case in [etag_case, last_modified_case] {
        let (dir, cache_cfg) = cache_config_at(&format!("xorigin-{}", case.label));

        {
            let transport = crate::curl_transport::scripted::ScriptedTransport::new(vec![(
                URL,
                crate::curl_transport::scripted::Canned::ok(png(9), case.populate.clone()),
            )]);
            let mut service = pool(ImageService::new_with_cache_root(
                transport,
                ServiceConfig {
                    worker_threads: 1,
                    http_cache: cache_cfg,
                    ..config()
                },
                dir.path(),
            ));
            let g = Generation::FIRST.next();
            service.request(URL, g).expect("valid url");
            let events = wait_terminal(&mut service, 1);
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ServiceEvent::Ready { .. })),
                "{}: the populate fetch must succeed; got {events:?}",
                case.label
            );
        }
        // A stale entry is required, or the second service serves a fresh hit
        // and never revalidates.
        std::thread::sleep(Duration::from_millis(1100));

        {
            let transport = crate::curl_transport::scripted::ScriptedTransport::new(vec![
                (
                    URL,
                    crate::curl_transport::scripted::Canned::redirect("https://cdn.test/asset.png"),
                ),
                (
                    "https://cdn.test/asset.png",
                    crate::curl_transport::scripted::Canned::not_modified(case.attacker.clone()),
                ),
            ]);
            let mut service = pool(ImageService::new_with_cache_root(
                transport,
                ServiceConfig {
                    worker_threads: 1,
                    http_cache: cache_cfg,
                    ..config()
                },
                dir.path(),
            ));
            let g = Generation::FIRST.next();
            service.request(URL, g).expect("valid url");
            let events = wait_terminal(&mut service, 1);
            // A cross-origin 304 arrives with no validator on the wire, so it is
            // unsolicited and the flight fails rather than revalidating.
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ServiceEvent::Failed { .. })),
                "{}: a cross-origin 304 with no validator on the wire must fail \
                 the flight, not revalidate; got {events:?}",
                case.label
            );
        }

        let cache = HttpCache::open_at(dir.path(), cache_cfg);
        let entry = cache
            .get(CacheKey::from(URL), &[])
            .unwrap_or_else(|| panic!("{}: the original entry must still exist", case.label));
        let stored = (case.read)(&entry);
        assert_eq!(
            stored.as_deref(),
            Some(case.original),
            "{}: a cross-origin 304 must not rewrite the original cache key's \
             validator; expected {:?} but the entry now holds {stored:?} \
             (destination offered {:?})",
            case.label,
            case.original,
            case.attacker_value,
        );
    }
}

