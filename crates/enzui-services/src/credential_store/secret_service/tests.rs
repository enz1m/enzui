use super::*;
use std::cell::Cell;

thread_local! {
    static PAYLOAD_WIPED: Cell<usize> = const { Cell::new(0) };
    static PAYLOAD_ZEROED: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn note_payload_wiped(n: usize, zeroed: usize) {
    PAYLOAD_WIPED.with(|c| c.set(c.get() + n));
    PAYLOAD_ZEROED.with(|c| c.set(c.get() + zeroed));
}

fn reset_payload() {
    PAYLOAD_WIPED.with(|c| c.set(0));
    PAYLOAD_ZEROED.with(|c| c.set(0));
}

fn payload_wiped() -> usize {
    PAYLOAD_WIPED.with(std::cell::Cell::get)
}

fn payload_zeroed() -> usize {
    PAYLOAD_ZEROED.with(std::cell::Cell::get)
}

#[test]
fn payload_guard_wipes_on_normal_drop() {
    reset_payload();
    let n = 41usize;
    let session = OwnedObjectPath::try_from("/").unwrap();
    let payload = DBusSecretPayload::new(session, vec![0xCDu8; n]);
    assert_eq!(payload_wiped(), 0, "counter must not move before the drop");
    drop(payload);
    assert_eq!(
        payload_wiped(),
        n,
        "Drop must cover all {n} plaintext bytes handed to D-Bus"
    );
    assert_eq!(
        payload_zeroed(),
        n,
        "all {n} plaintext bytes must actually be zero after Drop, not \
             merely reported"
    );
}

#[tokio::test]
async fn encrypted_attribute_search_unlocks_before_reporting_missing() {
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    struct Bus(std::process::Child);
    impl Drop for Bus {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    #[derive(Clone)]
    struct Provider {
        locked: Arc<AtomicBool>,
        searches: Arc<AtomicUsize>,
        unlocks: Arc<AtomicUsize>,
    }
    #[zbus::interface(name = "org.freedesktop.Secret.Service")]
    impl Provider {
        fn open_session(
            &self,
            _algorithm: &str,
            _input: Value<'_>,
        ) -> (OwnedValue, OwnedObjectPath) {
            (
                Value::new("").try_to_owned().unwrap(),
                OwnedObjectPath::try_from("/session").unwrap(),
            )
        }
        fn search_items(
            &self,
            attributes: HashMap<String, String>,
        ) -> (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) {
            self.searches.fetch_add(1, Ordering::SeqCst);
            if self.locked.load(Ordering::SeqCst)
                || attributes.get("label").is_some_and(|s| s == "absent")
            {
                (vec![], vec![])
            } else {
                (
                    vec![OwnedObjectPath::try_from("/collection/login/item").unwrap()],
                    vec![],
                )
            }
        }
        fn read_alias(&self, _name: &str) -> OwnedObjectPath {
            OwnedObjectPath::try_from("/collection/login").unwrap()
        }
        fn unlock(&self, objects: Vec<OwnedObjectPath>) -> (Vec<OwnedObjectPath>, OwnedObjectPath) {
            assert_eq!(
                objects,
                [OwnedObjectPath::try_from("/collection/login").unwrap()]
            );
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            self.locked.store(false, Ordering::SeqCst);
            (objects, OwnedObjectPath::try_from("/").unwrap())
        }
    }
    struct CollectionState(Arc<AtomicBool>);
    #[zbus::interface(name = "org.freedesktop.Secret.Collection")]
    impl CollectionState {
        fn create_item(
            &self,
            properties: HashMap<String, OwnedValue>,
            secret: SecretStruct,
            replace: bool,
        ) -> zbus::fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
            assert!(!self.0.load(Ordering::SeqCst));
            assert!(replace);
            assert!(properties.contains_key("org.freedesktop.Secret.Item.Attributes"));
            assert_eq!(secret.2, b"\xff\0fixture");
            if secret.3 != "application/octet-stream" {
                return Err(zbus::fdo::Error::InvalidArgs(
                    "credentials can contain arbitrary bytes".into(),
                ));
            }
            Ok((
                OwnedObjectPath::try_from("/collection/login/item")
                    .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?,
                OwnedObjectPath::default(),
            ))
        }
        #[zbus(property)]
        fn locked(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
    }
    struct StoredItem {
        locked: Arc<AtomicBool>,
        deletes: Arc<AtomicUsize>,
    }
    #[zbus::interface(name = "org.freedesktop.Secret.Item")]
    impl StoredItem {
        fn get_secret(&self, session: OwnedObjectPath) -> SecretStruct {
            assert!(!self.locked.load(Ordering::SeqCst));
            (
                session,
                vec![],
                b"encrypted fixture".to_vec(),
                CONTENT_TYPE.to_owned(),
            )
        }
        fn delete(&self) -> OwnedObjectPath {
            assert!(!self.locked.load(Ordering::SeqCst));
            self.deletes.fetch_add(1, Ordering::SeqCst);
            OwnedObjectPath::try_from("/").unwrap()
        }
    }
    let deletes = Arc::new(AtomicUsize::new(0));
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut address = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let _bus = Bus(child);
    let provider = Provider {
        locked: Arc::new(AtomicBool::new(true)),
        searches: Arc::default(),
        unlocks: Arc::default(),
    };
    let _server = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .serve_at(
            "/collection/login/item",
            StoredItem {
                locked: Arc::clone(&provider.locked),
                deletes: Arc::clone(&deletes),
            },
        )
        .unwrap()
        .name("org.freedesktop.secrets")
        .unwrap()
        .serve_at("/org/freedesktop/secrets", provider.clone())
        .unwrap()
        .serve_at(
            "/collection/login",
            CollectionState(Arc::clone(&provider.locked)),
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let conn = zbus::connection::Builder::address(address.trim())
        .unwrap()
        .method_timeout(Duration::from_secs(2))
        .build()
        .await
        .unwrap();
    let key = CredentialKey::new("test.App", "owner", "saved");
    assert_eq!(
        get_impl(conn.clone(), key.clone(), TransportCancel::inert()).await,
        TransportOutcome::Found(Secret::new(b"encrypted fixture".to_vec())),
        "a locked encrypted collection must not look empty"
    );
    assert_eq!(provider.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        provider.searches.load(Ordering::SeqCst),
        2,
        "search again after unlock"
    );
    assert_eq!(
        get_impl(
            conn.clone(),
            CredentialKey::new("test.App", "owner", "absent"),
            TransportCancel::inert()
        )
        .await,
        TransportOutcome::NotFound
    );
    assert_eq!(
        provider.unlocks.load(Ordering::SeqCst),
        1,
        "an unlocked collection needs no prompt"
    );
    assert_eq!(
        set_impl(
            conn.clone(),
            key.clone(),
            Secret::new(b"\xff\0fixture".to_vec()),
            TransportCancel::inert()
        )
        .await,
        TransportOutcome::Stored
    );
    provider.locked.store(true, Ordering::SeqCst);
    assert_eq!(
        delete_impl(conn, key, TransportCancel::inert()).await,
        TransportOutcome::Deleted
    );
    assert_eq!(
        deletes.load(Ordering::SeqCst),
        1,
        "deletion must reach the stored item"
    );
    assert_eq!(
        provider.unlocks.load(Ordering::SeqCst),
        2,
        "deletion unlocks before searching too"
    );
}
