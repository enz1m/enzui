use super::launcher::{Bus, Launcher, Summary};
use super::{Entry, MAX_JOBS, Shared};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::time::{Instant, sleep, sleep_until, timeout};

const IO_DEADLINE: Duration = Duration::from_secs(2);
const COMMAND_INTERVAL: Duration = Duration::from_millis(260);
const BATCH_INTERVAL: Duration = Duration::from_secs(1);
const HEARTBEAT: Duration = Duration::from_secs(30);
const LINGER: Duration = Duration::from_secs(4);
const MAX_FRAME: usize = 32768;

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ready {
        version: u32,
        lease_ms: u64,
        heartbeat_ms: u64,
        rate_per_second: u32,
        burst: u32,
    },
    Ack {
        revision: u64,
        changed: bool,
    },
    Error {
        code: String,
    },
}

struct Connection {
    stream: BufReader<UnixStream>,
    versions: BTreeMap<String, u64>,
    finished: BTreeMap<String, Instant>,
    next_command: Instant,
    next_batch: Instant,
    heartbeat: Instant,
}

pub(super) async fn run(shared: Arc<Shared>, app_id: String, socket: PathBuf, bus: Bus) {
    let mut backoff = Duration::from_millis(250);
    let mut next_batch = Instant::now();
    let mut launcher = Launcher::new(bus, &app_id);
    loop {
        discard_disconnected(&shared);
        if shared.lock().closed {
            launcher.hide().await;
            return;
        }
        if shared.lock().jobs.is_empty() {
            tokio::select! {
                () = launcher.hide() => {},
                () = shutdown(&shared) => return,
            }
            tokio::select! {
                () = shared.changed.notified() => {},
                () = shutdown(&shared) => return,
            }
            continue;
        }
        let connecting = async {
            let stream = timeout(IO_DEADLINE, UnixStream::connect(&socket)).await??;
            let mut stream = BufReader::new(stream);
            match exchange(
                &mut stream,
                &json!({"op":"publish", "version":1, "app_id":app_id}),
            )
            .await?
            {
                Reply::Ready {
                    version: 1,
                    lease_ms: 90000,
                    heartbeat_ms: 30000,
                    rate_per_second: 4,
                    burst: 8,
                } => {}
                _ => return Err(io::Error::other("unsupported activity protocol")),
            }
            let now = Instant::now();
            Ok(Connection {
                stream,
                versions: BTreeMap::new(),
                finished: BTreeMap::new(),
                next_command: now,
                next_batch,
                heartbeat: now + HEARTBEAT,
            })
        };
        let connection = tokio::select! {
            result = connecting => result,
            () = shutdown(&shared) => return,
        };
        let connected_now = connection.is_ok();
        if let Ok(mut connection) = connection {
            // The broker serves the jobs from here: a dock bar would show them twice.
            tokio::select! {
                () = launcher.hide() => {},
                () = shutdown(&shared) => return,
            }
            discard_disconnected(&shared);
            tokio::select! {
                _ = connected(&shared, &mut connection) => {},
                () = shutdown(&shared) => return,
            }
            next_batch = connection.next_batch;
            if !connection.versions.is_empty() {
                backoff = Duration::from_millis(250);
            }
        }
        discard_disconnected(&shared);
        if connected_now {
            tokio::select! {
                () = sleep(backoff) => {},
                () = shutdown(&shared) => return,
            }
        } else if !without_broker(&shared, &mut launcher, Instant::now() + backoff).await {
            launcher.hide().await;
            return;
        }
        backoff = (backoff * 2).min(Duration::from_secs(8));
    }
}

/// Waits until `until` while no broker is reachable, showing the live jobs as
/// launcher progress at most twice a second. Returns false on shutdown.
async fn without_broker(shared: &Shared, launcher: &mut Launcher, until: Instant) -> bool {
    loop {
        let notified = shared.changed.notified();
        discard_disconnected(shared);
        let summary = Summary::of(&shared.lock().jobs);
        if launcher.changes(summary) && launcher.ready_at().is_none() {
            tokio::select! {
                _ = launcher.show(summary) => {},
                () = shutdown(shared) => return false,
            }
            // A slow launcher must not keep the broker from being retried.
            if Instant::now() >= until {
                return true;
            }
            continue;
        }
        let reveal = shared
            .lock()
            .jobs
            .values()
            .filter(|entry| !entry.removed && entry.terminal.is_none())
            .filter_map(|entry| entry.visible_at)
            .map(Instant::from_std)
            .filter(|deadline| *deadline > Instant::now())
            .min();
        let wake = match launcher.ready_at() {
            Some(ready) if launcher.changes(summary) => ready.min(until),
            _ => until,
        };
        let wake = reveal.map_or(wake, |reveal| wake.min(reveal));
        tokio::select! {
            () = notified => {},
            () = sleep_until(wake) => {},
            () = shutdown(shared) => return false,
        }
        if Instant::now() >= until {
            return true;
        }
    }
}

async fn shutdown(shared: &Shared) {
    loop {
        let notified = shared.stopped.notified();
        if shared.lock().closed {
            return;
        }
        notified.await;
    }
}

fn discard_disconnected(shared: &Shared) {
    // Terminal records belong to one connection and must never be resurrected.
    shared
        .lock()
        .jobs
        .retain(|_, entry| !entry.removed && entry.terminal.is_none());
}

enum Pending {
    Batch(Vec<(String, Entry)>),
    Finish(String, super::Outcome),
    Remove(String),
    Heartbeat,
}

impl Pending {
    fn command(&self) -> Value {
        match self {
            Self::Batch(entries) => {
                json!({"op":"batch", "works":entries.iter().map(|(key, entry)| {
                let mut value =
                    serde_json::to_value(&entry.status).expect("a status always serialises");
                value["job"] = json!(key);
                // Omission preserves the v1 default for older brokers.
                if entry.visibility != super::Visibility::default() {
                    value["visibility"] = json!(entry.visibility);
                }
                value
            }).collect::<Vec<_>>()})
            }
            Self::Finish(key, outcome) => json!({"op":"finish", "job":key, "outcome":outcome}),
            Self::Remove(key) => json!({"op":"remove", "job":key}),
            Self::Heartbeat => json!({"op":"heartbeat"}),
        }
    }
}

async fn connected(shared: &Shared, connection: &mut Connection) -> io::Result<()> {
    loop {
        let notified = shared.changed.notified();
        let now = Instant::now();
        let entries = {
            let mut inner = shared.lock();
            inner.jobs.retain(|key, entry| {
                let linger_over = connection
                    .finished
                    .get(key)
                    .is_some_and(|at| now.duration_since(*at) >= LINGER);
                let unsent_removed = entry.removed && !connection.versions.contains_key(key);
                !(linger_over || unsent_removed)
            });
            inner.jobs.clone()
        };
        connection
            .finished
            .retain(|key, _| entries.contains_key(key));
        connection
            .versions
            .retain(|key, _| entries.contains_key(key));
        if entries.is_empty() {
            tokio::select! {
                () = notified => {},
                _ = connection.stream.fill_buf() => return Err(io::Error::other("activity connection ended")),
            }
            continue;
        }
        let active = entries
            .values()
            .any(|entry| !entry.removed && entry.terminal.is_none());
        let mut pending = None;
        if now >= connection.next_command {
            for (key, entry) in &entries {
                if connection.finished.contains_key(key) {
                    continue;
                }
                if entry.removed && connection.versions.contains_key(key) {
                    pending = Some(Pending::Remove(key.clone()));
                    break;
                }
                if let Some(outcome) = entry.terminal
                    && connection.versions.get(key) == Some(&entry.version)
                {
                    pending = Some(Pending::Finish(key.clone(), outcome));
                    break;
                }
            }
            if pending.is_none() && now >= connection.next_batch {
                let dirty = entries
                    .iter()
                    .filter(|(key, entry)| {
                        !entry.removed
                            && !connection.finished.contains_key(*key)
                            && connection.versions.get(*key) != Some(&entry.version)
                            && entry
                                .visible_at
                                .is_none_or(|deadline| deadline <= now.into_std())
                    })
                    .map(|(key, entry)| (key.clone(), entry.clone()))
                    .collect::<Vec<_>>();
                if !dirty.is_empty() {
                    debug_assert!(
                        dirty.len() <= MAX_JOBS,
                        "the dirty set outgrew the job table"
                    );
                    pending = Some(Pending::Batch(dirty));
                }
            }
            if pending.is_none() && active && now >= connection.heartbeat {
                pending = Some(Pending::Heartbeat);
            }
        }
        if let Some(pending) = pending {
            if matches!(pending, Pending::Batch(_)) {
                connection.next_batch = Instant::now() + BATCH_INTERVAL;
            }
            match exchange(&mut connection.stream, &pending.command()).await? {
                Reply::Ack { revision, changed } => {
                    let _ = (revision, changed);
                }
                Reply::Error { code } => {
                    // Retrying rejected state on one connection can exhaust its rate budget.
                    let _ = code;
                    return Err(io::Error::other("activity command rejected"));
                }
                _ => return Err(io::Error::other("unexpected activity response")),
            }
            let accepted = Instant::now();
            connection.next_command = accepted + COMMAND_INTERVAL;
            connection.heartbeat = accepted + HEARTBEAT;
            match pending {
                Pending::Batch(entries) => {
                    connection.next_batch = accepted + BATCH_INTERVAL;
                    for (key, entry) in entries {
                        connection.versions.insert(key, entry.version);
                    }
                }
                Pending::Finish(key, _) => {
                    connection.finished.insert(key, accepted);
                }
                Pending::Remove(key) => {
                    shared.lock().jobs.remove(&key);
                    connection.versions.remove(&key);
                }
                Pending::Heartbeat => {}
            }
            continue;
        }
        let mut deadline = now + HEARTBEAT;
        if entries.iter().any(|(key, entry)| {
            !connection.finished.contains_key(key)
                && (entry.removed && connection.versions.contains_key(key)
                    || entry.terminal.is_some()
                        && connection.versions.get(key) == Some(&entry.version))
        }) {
            deadline = deadline.min(connection.next_command.max(now));
        }
        if let Some(dirty_at) = entries
            .iter()
            .filter(|(key, entry)| {
                !entry.removed
                    && !connection.finished.contains_key(*key)
                    && connection.versions.get(*key) != Some(&entry.version)
            })
            .map(|(_, entry)| {
                entry
                    .visible_at
                    .map_or(now, Instant::from_std)
                    .max(now)
                    .max(connection.next_command)
                    .max(connection.next_batch)
            })
            .min()
        {
            deadline = deadline.min(dirty_at);
        }
        if active {
            deadline = deadline.min(connection.next_command.max(connection.heartbeat));
        }
        for at in connection.finished.values() {
            deadline = deadline.min(*at + LINGER);
        }
        tokio::select! {
            () = notified => {},
            () = sleep_until(deadline) => {},
            _ = connection.stream.fill_buf() => return Err(io::Error::other("unsolicited activity response or EOF")),
        }
    }
}

async fn exchange(stream: &mut BufReader<UnixStream>, command: &Value) -> io::Result<Reply> {
    let mut frame = serde_json::to_vec(command)?;
    if frame.len() > MAX_FRAME {
        return Err(io::Error::other("activity frame limit"));
    }
    frame.push(b'\n');
    timeout(IO_DEADLINE, async {
        stream.get_mut().write_all(&frame).await?;
        let mut response = Vec::new();
        loop {
            let buffer = stream.fill_buf().await?;
            if buffer.is_empty() {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
            }
            let newline = buffer.iter().position(|b| *b == b'\n');
            let count = newline.map_or(buffer.len(), |at| at + 1);
            if response.len() + count > MAX_FRAME + 1 {
                return Err(io::Error::other("activity response limit"));
            }
            response.extend_from_slice(&buffer[..count]);
            stream.consume(count);
            if newline.is_some() {
                return serde_json::from_slice(&response).map_err(io::Error::from);
            }
        }
    })
    .await?
}
