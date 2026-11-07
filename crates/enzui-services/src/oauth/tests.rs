use super::*;

fn client() -> ClientConfig {
    ClientConfig::new(
        "https://accounts.example.test/o/authorize",
        "https://accounts.example.test/o/token",
        "app-client.apps.example",
    )
    .expect("client")
    .with_scopes(["openid", "https://mail.example.test/"])
    .expect("scopes")
}

#[test]
fn the_flow_refuses_what_it_does_not_own() {
    assert_eq!(
        client()
            .begin("http://127.0.0.1:1/cb", &[("state", "mine")])
            .err(),
        Some(OAuthError::ReservedParameter("state".into()))
    );
    assert_eq!(
        client().begin("http://127.0.0.1:1/cb#frag", &[]).err(),
        Some(OAuthError::Redirect(
            enzui_security::redirect_capture::RedirectCaptureError::QueryOrFragment
        ))
    );
    assert_eq!(
        ClientConfig::new(
            "http://idp.example.test/a",
            "https://idp.example.test/t",
            "c"
        )
        .err(),
        Some(OAuthError::InsecureEndpoint),
        "plain HTTP to a remote host"
    );
    assert!(
        ClientConfig::new("http://127.0.0.1:9/a", "http://[::1]:9/t", "c").is_ok(),
        "plain HTTP to loopback is a provider under test"
    );
    for (authorization, token) in [
        ("https://p.example/a", "http://127.0.0.1:x@evil.example/t"),
        ("http://localhost@evil.example/a", "https://p.example/t"),
    ] {
        assert_eq!(
            ClientConfig::new(authorization, token, "c").err(),
            Some(OAuthError::MalformedUrl),
            "a loopback-looking user name on a remote host: {authorization} {token}"
        );
    }
    assert_eq!(
        client().with_scopes(["two words"]).err(),
        Some(OAuthError::InvalidParameter)
    );
}
