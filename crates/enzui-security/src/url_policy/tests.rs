use super::*;

#[test]
fn http_is_refused_by_default_but_accepted_when_allowed() {
    let strict = UrlPolicy::DEFAULT;
    assert_eq!(
        strict.check_url("http://example.com/x.png"),
        Err(UrlVerdict::CleartextRefused),
        "cleartext http must be refused by default"
    );
    let lax = UrlPolicy {
        allow_http: true,
        ..UrlPolicy::DEFAULT
    };
    assert!(
        lax.check_url("http://example.com/x.png").is_ok(),
        "http must be accepted when allow_http is set"
    );
}

#[test]
fn non_http_schemes_with_authority_are_scheme_refused() {
    let p = UrlPolicy {
        allow_http: true,
        ..UrlPolicy::DEFAULT
    };
    for bad in [
        "file://host/etc/passwd",
        "ftp://host/x",
        "javascript://x",
        "gopher://host/",
    ] {
        assert_eq!(
            p.check_url(bad),
            Err(UrlVerdict::SchemeRefused {
                scheme: Scheme::Other
            }),
            "{bad} must be refused as a non-http scheme"
        );
    }
}

#[test]
fn opaque_non_authority_schemes_are_refused_as_malformed() {
    let p = UrlPolicy {
        allow_http: true,
        ..UrlPolicy::DEFAULT
    };
    for bad in [
        "data:image/png;base64,AAAA",
        "javascript:alert(1)",
        "file:///etc/passwd",
    ] {
        assert!(
            p.check_url(bad).is_err(),
            "{bad} must be refused (no fetchable authority)"
        );
    }
}
