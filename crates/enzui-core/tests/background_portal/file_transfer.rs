use super::{PrivateBus, Scratch};
use ashpd::{zbus, zvariant::OwnedValue};
use enzui_core::portal::file_transfer::FileTransferClient;
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Calls {
    bytes: Vec<Vec<u8>>,
    stopped: bool,
    retrieved: usize,
}
struct Portal(Arc<Mutex<Calls>>);

#[zbus::interface(name = "org.freedesktop.portal.FileTransfer")]
impl Portal {
    #[zbus(property)]
    fn version(&self) -> u32 {
        1
    }
    fn start_transfer(&self, options: HashMap<String, OwnedValue>) -> String {
        assert_eq!(
            options
                .get("writeable")
                .and_then(|v| bool::try_from(v).ok()),
            Some(false)
        );
        assert_eq!(
            options.get("autostop").and_then(|v| bool::try_from(v).ok()),
            Some(true)
        );
        "owned-export".into()
    }
    fn add_files(
        &self,
        key: &str,
        files: Vec<ashpd::zvariant::OwnedFd>,
        _options: HashMap<String, OwnedValue>,
    ) {
        assert_eq!(key, "owned-export");
        assert!(files.len() <= 16);
        for fd in files {
            let fd: std::os::fd::OwnedFd = fd.into();
            let mut file = std::fs::File::from(fd);
            let mut bytes = vec![];
            file.read_to_end(&mut bytes).unwrap();
            self.0.lock().unwrap().bytes.push(bytes);
        }
    }
    fn retrieve_files(&self, key: &str, _options: HashMap<String, OwnedValue>) -> Vec<String> {
        self.0.lock().unwrap().retrieved += 1;
        match key {
            "incoming" => vec!["/run/user/1000/doc/exported image.png".into()],
            "relative" => vec!["relative/image.png".into()],
            "too-many" => vec!["/tmp/example".into(); 129],
            _ => vec![],
        }
    }
    fn stop_transfer(&self, key: &str) {
        assert_eq!(key, "owned-export");
        self.0.lock().unwrap().stopped = true;
    }
}

#[test]
fn file_transfer_uses_read_only_descriptors_and_retires_export_capabilities() {
    let bus = PrivateBus::start();
    let files = Scratch::new();
    let source = files.path().join("image with spaces.png");
    std::fs::write(&source, b"source remains unchanged").unwrap();
    let calls = Arc::new(Mutex::new(Calls::default()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .name("org.freedesktop.portal.Documents")
            .unwrap()
            .serve_at(
                "/org/freedesktop/portal/documents",
                Portal(Arc::clone(&calls)),
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        let connection = zbus::connection::Builder::address(bus.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap();
        let client = FileTransferClient::with_connection(connection)
            .await
            .unwrap();
        assert_eq!(
            client.retrieve(b"incoming").await.unwrap(),
            [std::path::PathBuf::from(
                "/run/user/1000/doc/exported image.png"
            )]
        );
        for key in [
            b"relative".as_slice(),
            b"too-many",
            b"",
            b"invalid\0key",
            &[0xff],
        ] {
            assert!(client.retrieve(key).await.is_err());
        }
        assert_eq!(
            calls.lock().unwrap().retrieved,
            3,
            "malformed capabilities never reach the bus"
        );
        let worker = Arc::clone(&client);
        let path = source.clone();
        let export = tokio::task::spawn_blocking(move || worker.export_blocking(&[path]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            calls.lock().unwrap().bytes,
            [b"source remains unchanged".to_vec()]
        );
        assert!(
            !calls.lock().unwrap().stopped,
            "export lives until destination completion"
        );
        drop(export);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !calls.lock().unwrap().stopped {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let worker = Arc::clone(&client);
        let directory = files.path().to_owned();
        assert!(
            tokio::task::spawn_blocking(move || worker.export_blocking(&[directory]))
                .await
                .unwrap()
                .is_err()
        );
        drop(server);
    });
    assert_eq!(std::fs::read(source).unwrap(), b"source remains unchanged");
}
