use super::*;

#[test]
fn symlinked_socket_path_is_refused_without_following() {
    let dir = std::env::temp_dir();
    let target = dir.join(format!("enzui-agent-target-{}", std::process::id()));
    let link = dir.join(format!("enzui-agent-link-{}", std::process::id()));
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_file(&link);
    let listener = std::os::unix::net::UnixListener::bind(&target).expect("bind real socket");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    let got = validate_socket_path(&link);
    drop(listener);
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&target);
    assert!(
        matches!(got, Err(SocketError::IsSymlink)),
        "a symlinked socket path must be refused, got {got:?}"
    );
}
