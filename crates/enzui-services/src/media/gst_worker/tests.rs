use super::*;

#[test]
fn proxy_uri_equivalence_rejects_distinct_destinations() {
    assert!(!proxy_uris_equivalent(
        "http://127.0.0.1:49152",
        "http://127.0.0.1:49153"
    ));
    assert!(!proxy_uris_equivalent(
        "http://127.0.0.1:49152",
        "http://127.0.0.2:49152"
    ));
    assert!(!proxy_uris_equivalent(
        "http://127.0.0.1:49152",
        "https://127.0.0.1:49152"
    ));
    assert!(!proxy_uris_equivalent(
        "http://127.0.0.1:49152/",
        "http://127.0.0.1:49152/evil"
    ));
    assert!(!proxy_uris_equivalent(
        "http://127.0.0.1:49152",
        "http://127.0.0.1:49152/?x=1"
    ));
    assert!(!proxy_uris_equivalent(
        "http://127.0.0.1:49152",
        "http://user:pass@127.0.0.1:49152/"
    ));
}
