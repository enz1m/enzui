use super::{OwnedChild, PrivateWayland};
use std::io::Write as _;
use std::process::Stdio;
use std::time::{Duration, Instant};

const TEXT: &str = "text/plain;charset=utf-8";

/// Starts this binary as a clipboard keeper handed `text`, as
/// `keeper::hand_off` does: the payload encoded on stdin, which then closes.
fn keeper(wayland: &PrivateWayland, text: &str) -> OwnedChild {
    let mut stdin = Vec::new();
    stdin.extend_from_slice(&u32::try_from(TEXT.len()).unwrap().to_le_bytes());
    stdin.extend_from_slice(TEXT.as_bytes());
    stdin.extend_from_slice(&(text.len() as u64).to_le_bytes());
    stdin.extend_from_slice(text.as_bytes());
    let mut child = OwnedChild::spawn(
        wayland
            .command(std::env::current_exe().unwrap())
            .env(enzui_clipboard::keeper::KEEPER_ENV, "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        "keeper",
    );
    child
        .child_mut()
        .stdin
        .take()
        .unwrap()
        .write_all(&stdin)
        .unwrap();
    child
}

/// A `wl-copy` that owns the selection with `text` until it is replaced.
fn copier(wayland: &PrivateWayland, text: &str) -> OwnedChild {
    OwnedChild::spawn(
        wayland
            .command("wl-copy")
            .args(["--foreground", "--type", TEXT, text])
            .stdout(Stdio::null())
            .stderr(Stdio::null()),
        "wl-copy",
    )
}

fn paste(wayland: &PrivateWayland) -> String {
    let output = wayland
        .command("wl-paste")
        .args(["--no-newline", "--type", TEXT])
        .stderr(Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn wait_for_paste(wayland: &PrivateWayland, expected: &str, why: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let pasted = paste(wayland);
        if pasted == expected {
            return;
        }
        assert!(Instant::now() < deadline, "{why}: pasted {pasted:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_exit(child: &mut OwnedChild, why: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.child_mut().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "{why}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub fn never_replaces_a_newer_selection() {
    let wayland = PrivateWayland::start_for_activation();

    // An empty clipboard: the copy the application handed over is kept.
    let mut first = keeper(&wayland, "handed over");
    wait_for_paste(&wayland, "handed over", "an empty clipboard keeps the copy");

    // Another client copies: the keeper lets go.
    let newer = copier(&wayland, "copied since");
    wait_for_paste(&wayland, "copied since", "the newer copy is pasted");
    wait_for_exit(&mut first, "a replaced keeper exits");

    // A keeper that starts after the newer copy leaves it alone.
    let mut late = keeper(&wayland, "handed over");
    wait_for_exit(&mut late, "a keeper finding a newer copy exits");
    assert_eq!(paste(&wayland), "copied since", "the newer copy survives");

    // The owner exits and its copy goes with it: the clipboard is empty.
    drop(newer);
    wait_for_paste(&wayland, "", "the copy left with its owner");

    // A source offering exactly the handed-over types, as the application's
    // own does (`wl-copy` adds text aliases).
    let mut owner = keeper(&wayland, "still mine");
    wait_for_paste(&wayland, "still mine", "the owner's copy is pasted");

    // The same types with other bytes were copied since: left alone.
    let mut other = keeper(&wayland, "not the same");
    wait_for_exit(&mut other, "a keeper finding other bytes exits");
    assert_eq!(paste(&wayland), "still mine", "the other copy survives");

    // Exactly its copy, its application not yet gone: taken over.
    let _kept = keeper(&wayland, "still mine");
    wait_for_exit(&mut owner, "the keeper takes over its own copy");
    wait_for_paste(&wayland, "still mine", "the keeper serves its copy");
}
