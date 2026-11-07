//! Process driver with bounded response waits and forced cleanup on drop.
//!
//! Peers inherit an environment with X11 connection variables removed; supplied
//! overrides are applied afterward. Command writes and final process reaping
//! are blocking, so response budgets do not bound complete driver calls.

use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use super::compositor::TestCompositor;

pub struct Peer {
    label: String,
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
}

impl Peer {
    pub fn spawn(
        label: &str,
        compositor: &TestCompositor,
        runtime_dir: &std::path::Path,
        environment: &[(&str, String)],
    ) -> Self {
        let mut peer = Self::spawn_starting(label, compositor, runtime_dir, environment);
        let ready = peer.startup_announcement(Duration::from_secs(10));

        assert!(
            ready.starts_with("ready "),
            "peer {label} announced {ready:?} instead of readiness"
        );

        peer
    }

    pub fn spawn_starting(
        label: &str,
        compositor: &TestCompositor,
        runtime_dir: &std::path::Path,
        environment: &[(&str, String)],
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_enzui-clipboard-peer"));

        command
            .env("WAYLAND_DISPLAY", compositor.socket())
            .env("XDG_RUNTIME_DIR", runtime_dir)
            .env_remove("DISPLAY")
            .env_remove("XAUTHORITY")
            .env_remove("WAYLAND_SOCKET")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        for (key, value) in environment {
            command.env(key, value);
        }

        let mut child = command.spawn().expect("the peer program starts");
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, lines) = channel();

        std::thread::Builder::new()
            .name(format!("peer-{label}-reader"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };

                    if sender.send(line).is_err() {
                        break;
                    }
                }
            })
            .expect("the reader thread starts");

        Self {
            label: label.to_owned(),
            child,
            stdin,
            lines,
        }
    }

    pub fn startup_announcement(&mut self, budget: Duration) -> String {
        self.next_line(budget)
    }

    pub fn wait_for_exit(&mut self, budget: Duration) -> std::process::ExitStatus {
        self.wait(budget)
    }

    pub fn ask(&mut self, command: &str) -> String {
        self.send(command);
        self.next_line(Duration::from_secs(15))
    }

    pub fn send(&mut self, command: &str) {
        writeln!(self.stdin, "{command}").expect("the peer accepts commands");
        self.stdin.flush().expect("the peer reads its input");
    }

    pub fn answer(&mut self, budget: Duration) -> String {
        self.next_line(budget)
    }

    pub fn expect_ok(&mut self, command: &str) -> String {
        let answer = self.ask(command);

        assert!(
            answer.starts_with("ok"),
            "peer {}: `{command}` answered {answer:?}",
            self.label
        );

        answer.trim_start_matches("ok").trim().to_owned()
    }

    pub fn expect_err(&mut self, command: &str) -> String {
        let answer = self.ask(command);

        assert!(
            answer.starts_with("err"),
            "peer {}: `{command}` unexpectedly succeeded with {answer:?}",
            self.label
        );

        answer.trim_start_matches("err").trim().to_owned()
    }

    pub fn copy_text(&mut self, text: &str) {
        let _ = self.expect_ok(&format!("offer text {}", to_hex(text.as_bytes())));
    }

    pub fn paste_text(&mut self) -> String {
        let answer = self.expect_ok("paste text");
        let mut parts = answer.split_whitespace();
        let _mime = parts.next().expect("a negotiated type");
        let bytes = from_hex(parts.next().expect("payload bytes"));

        String::from_utf8(bytes).expect("the peer pasted UTF-8")
    }

    pub fn paste_bytes(&mut self, mime: &str) -> Vec<u8> {
        let answer = self.expect_ok(&format!("paste mime {mime}"));
        let mut parts = answer.split_whitespace();
        let _mime = parts.next().expect("a negotiated type");

        from_hex(parts.next().expect("payload bytes"))
    }

    pub fn quit(mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let status = self.wait(Duration::from_secs(10));

        assert!(status.success(), "peer {} exited with {status}", self.label);
    }

    fn wait(&mut self, budget: Duration) -> std::process::ExitStatus {
        let deadline = std::time::Instant::now() + budget;

        loop {
            match self.child.try_wait().expect("the peer status is readable") {
                Some(status) => return status,
                None if std::time::Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    return self.child.wait().expect("the killed peer is reaped");
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    fn next_line(&mut self, budget: Duration) -> String {
        match self.lines.recv_timeout(budget) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => {
                panic!("peer {} produced no answer within {budget:?}", self.label)
            }
            Err(RecvTimeoutError::Disconnected) => {
                panic!("peer {} exited before answering", self.label)
            }
        }
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }

    out
}

pub fn from_hex(text: &str) -> Vec<u8> {
    assert!(
        text.len().is_multiple_of(2),
        "the peer produced an odd hex string"
    );

    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).expect("hex digits"))
        .collect()
}
