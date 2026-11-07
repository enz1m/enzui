use super::*;

fn scope() -> BackgroundScope {
    BackgroundScope::with_overrides("enzui-transport-test", |_| None)
}

#[test]
fn cancelling_a_background_helper_ends_its_escaped_descendant() {
    if !matches!(placement(&scope()), Placement::Scope { .. }) {
        return;
    }
    let marker = marker("background-cancel");
    let script = r#"
        /usr/bin/setsid /bin/sh -c 'trap "" TERM; while :; do sleep 1; done' &
        printf '%s %s\n' "$$" "$!" > "$1"
        while :; do sleep 1; done
    "#;
    let request = sh(script, 4096, 4096, Duration::from_secs(20))
        .arg("fixture")
        .arg(marker.as_os_str())
        .background(scope());
    let token = CancelToken::new();
    let worker_token = token.share();
    let worker = thread::spawn(move || run(request, &worker_token));
    let (leader, escaped) = fixture_pids(&marker);
    // Group termination cannot discriminate scope cleanup unless this descendant escaped.
    let group = |pid: u32| {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(") ")?
                    .1
                    .split_whitespace()
                    .nth(2)
                    .map(str::to_owned)
            })
    };
    let deadline = Instant::now() + Duration::from_secs(2);
    while group(escaped).as_deref() == Some(&leader.to_string()) {
        assert!(
            Instant::now() < deadline,
            "the descendant never left the group"
        );
        thread::sleep(POLL_INTERVAL);
    }
    assert!(token.cancel(CancelCause::Cancelled));
    let result = worker.join().expect("transport worker joined");
    assert_eq!(
        result.expect_err("cancel wins").kind(),
        &ProcessErrorKind::Cancelled(CancelCause::Cancelled)
    );
    let gone = |pid: u32| proc_state(pid).is_none_or(|state| state == "Z" || state == "X");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !gone(escaped) {
        assert!(
            Instant::now() < deadline,
            "the escaped descendant {escaped} survived cancellation"
        );
        thread::sleep(POLL_INTERVAL);
    }
    assert!(gone(leader));
}
