#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

//! The Secret portal credential store: credentials sealed in the app's own
//! file with the portal's master secret, a wrong or missing secret never
//! erasing them, and the portal chosen inside a sandbox.

use enzui_core::portal::sandbox::SandboxEvidence;
use enzui_security::secret::Secret;
use enzui_services::credential_store::portal::PortalSecretTransport;
use enzui_services::credential_store::{
    CredentialKey, CredentialOutcome, CredentialStore, TransportCredentialStore,
    system_transport_with,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "enzui-keyring-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn store(path: &std::path::Path, master: &[u8]) -> TransportCredentialStore {
    TransportCredentialStore::new(Arc::new(PortalSecretTransport::with_master_secret(
        "org.example.Mail",
        path,
        &Secret::new(master.to_vec()),
    )))
}

fn key(label: &str) -> CredentialKey {
    CredentialKey::new("org.example.Mail", "ada@example.org", label)
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn credentials_are_sealed_in_the_apps_file_and_read_back() {
    let root = scratch();
    let path = root.join("credentials.keyring");
    let first = store(&path, b"master secret from the portal");
    assert_eq!(
        run(first.set(key("imap"), Secret::new(b"hunter2".to_vec()))),
        CredentialOutcome::Stored
    );
    let on_disk = std::fs::read(&path).unwrap();
    for plaintext in [&b"hunter2"[..], b"imap", b"ada@example.org"] {
        assert!(
            !on_disk
                .windows(plaintext.len())
                .any(|window| window == plaintext),
            "{:?} is not readable in the file",
            String::from_utf8_lossy(plaintext)
        );
    }
    // Another process with the same master secret reads it back.
    let second = store(&path, b"master secret from the portal");
    assert_eq!(
        run(second.get(key("imap"))),
        CredentialOutcome::Retrieved(Secret::new(b"hunter2".to_vec()))
    );
    assert_eq!(run(second.get(key("smtp"))), CredentialOutcome::NotFound);
    assert_eq!(run(second.delete(key("imap"))), CredentialOutcome::Deleted);
    assert_eq!(run(first.get(key("imap"))), CredentialOutcome::NotFound);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_wrong_secret_or_damaged_file_never_erases_the_keyring() {
    let root = scratch();
    let path = root.join("credentials.keyring");
    let _ =
        run(store(&path, b"the right secret").set(key("imap"), Secret::new(b"hunter2".to_vec())));
    let sealed = std::fs::read(&path).unwrap();

    let wrong = store(&path, b"another secret");
    assert!(matches!(
        run(wrong.get(key("imap"))),
        CredentialOutcome::Failed(_)
    ));
    assert!(
        matches!(
            run(wrong.set(key("smtp"), Secret::new(b"x".to_vec()))),
            CredentialOutcome::Failed(_)
        ),
        "writing over a keyring it cannot read would erase it"
    );
    assert_eq!(
        std::fs::read(&path).unwrap(),
        sealed,
        "the file is untouched"
    );

    let mut damaged = sealed;
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    std::fs::write(&path, &damaged).unwrap();
    assert!(matches!(
        run(store(&path, b"the right secret").get(key("imap"))),
        CredentialOutcome::Failed(_)
    ));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn another_applications_key_is_refused() {
    let root = scratch();
    let path = root.join("credentials.keyring");
    let other = CredentialKey::new("org.example.Other", "ada@example.org", "imap");
    assert_eq!(
        run(store(&path, b"secret").get(other.clone())),
        CredentialOutcome::PermissionDenied
    );
    // The key is bound to the application: another application holding the
    // same master secret cannot open this keyring.
    let _ = run(store(&path, b"secret").set(key("imap"), Secret::new(b"hunter2".to_vec())));
    let impostor =
        TransportCredentialStore::new(Arc::new(PortalSecretTransport::with_master_secret(
            "org.example.Other",
            &path,
            &Secret::new(b"secret".to_vec()),
        )));
    assert!(matches!(
        run(impostor.get(other)),
        CredentialOutcome::Failed(_)
    ));
    let _ = std::fs::remove_dir_all(&root);
}

/// A private session bus with a fake `org.freedesktop.portal.Secret`.
struct Bus {
    child: std::process::Child,
    root: PathBuf,
    address: String,
}

impl Bus {
    fn new() -> Self {
        let root = scratch();
        let socket = root.join("bus");
        let address = format!("unix:path={}", socket.display());
        let child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile"])
            .arg(format!("--address={address}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("private dbus-daemon is required");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "private bus did not start");
            std::thread::sleep(Duration::from_millis(10));
        }
        Self {
            child,
            root,
            address,
        }
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct FakeSecretPortal {
    secret: Vec<u8>,
    calls: Arc<AtomicU64>,
}

#[zbus::interface(name = "org.freedesktop.portal.Secret")]
impl FakeSecretPortal {
    fn retrieve_secret(
        &self,
        fd: zbus::zvariant::OwnedFd,
        options: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> zbus::fdo::Result<zbus::zvariant::OwnedObjectPath> {
        use std::io::Write as _;
        let _ = self.calls.fetch_add(1, Ordering::Relaxed);
        let mut stream = std::fs::File::from(std::os::fd::OwnedFd::from(fd));
        stream.write_all(&self.secret).unwrap();
        drop(stream);
        let token: String = options
            .get("handle_token")
            .and_then(|value| String::try_from(value.clone()).ok())
            .unwrap_or_default();
        let sender = header
            .sender()
            .unwrap()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let connection = connection.clone();
        let response_path = path.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let results: std::collections::HashMap<String, zbus::zvariant::OwnedValue> =
                std::collections::HashMap::new();
            connection
                .emit_signal(
                    None::<&str>,
                    response_path.as_str(),
                    "org.freedesktop.portal.Request",
                    "Response",
                    &(0u32, results),
                )
                .await
                .unwrap();
        });
        Ok(zbus::zvariant::OwnedObjectPath::try_from(path).unwrap())
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
}

#[test]
fn inside_a_sandbox_the_store_asks_the_secret_portal() {
    let bus = Bus::new();
    let data_home = scratch();
    let calls = Arc::new(AtomicU64::new(0));
    run(async {
        let _portal = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.freedesktop.portal.Desktop")
            .unwrap()
            .serve_at(
                "/org/freedesktop/portal/desktop",
                FakeSecretPortal {
                    secret: b"per-application master secret".to_vec(),
                    calls: Arc::clone(&calls),
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let confined = SandboxEvidence {
            flatpak_info: true,
            ..SandboxEvidence::default()
        };
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let resolver =
            enzui_core::app_data::AppDataResolver::new(Some(&data_home), None::<PathBuf>);
        let transport = system_transport_with("org.example.Mail", &confined, connection, &resolver)
            .await
            .expect("the portal store");
        let store = TransportCredentialStore::new(transport);
        assert_eq!(
            store
                .set(key("imap"), Secret::new(b"hunter2".to_vec()))
                .await,
            CredentialOutcome::Stored
        );
        assert_eq!(
            calls.load(Ordering::Relaxed),
            1,
            "the master secret came from the portal"
        );
        let keyring = data_home
            .join("org.example.Mail")
            .join("credentials.keyring");
        assert!(keyring.is_file(), "in the app's data directory");
        // The same master secret, read again, opens the keyring.
        let reopened =
            TransportCredentialStore::new(Arc::new(PortalSecretTransport::with_master_secret(
                "org.example.Mail",
                &keyring,
                &Secret::new(b"per-application master secret".to_vec()),
            )));
        assert_eq!(
            reopened.get(key("imap")).await,
            CredentialOutcome::Retrieved(Secret::new(b"hunter2".to_vec()))
        );
    });
    let _ = std::fs::remove_dir_all(&data_home);
}

/// A FIFO planted where the keyring lives is refused at once rather than
/// blocking the worker on open.
#[test]
fn a_fifo_in_place_of_the_keyring_is_refused_without_blocking() {
    let root = scratch();
    let path = root.join("credentials.keyring");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &path,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(run(store(&path, b"secret").get(key("imap"))));
    });
    let outcome = finished
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the read returns instead of blocking");
    assert!(
        matches!(outcome, CredentialOutcome::Failed(_)),
        "{outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Two transports opened on one keyring take turns: concurrent stores through
/// both all land.
#[test]
fn separate_transports_on_one_keyring_lose_no_update() {
    let root = scratch();
    let path = root.join("credentials.keyring");
    std::thread::scope(|scope| {
        for n in 0..12 {
            let path = &path;
            scope.spawn(move || {
                let keyring = store(path, b"secret");
                assert_eq!(
                    run(keyring.set(key(&format!("account-{n}")), Secret::new(vec![n]))),
                    CredentialOutcome::Stored
                );
            });
        }
    });
    let keyring = store(&path, b"secret");
    for n in 0..12 {
        assert_eq!(
            run(keyring.get(key(&format!("account-{n}")))),
            CredentialOutcome::Retrieved(Secret::new(vec![n])),
            "account-{n} was lost"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
