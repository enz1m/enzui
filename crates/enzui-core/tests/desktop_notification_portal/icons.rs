use super::private_bus::PrivateBus;
use super::replies::{event, outcome, runtime};
use enzui_core::app_data::AppId;
use enzui_core::portal::dbus_backend::DbusBackend;
use enzui_core::portal::notification::*;
use rustix::fs::{SealFlags, fcntl_get_seals};
use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use zbus::zvariant::OwnedValue;

type Client = DesktopNotificationClient<DbusBackend>;
type Records = Arc<Mutex<Vec<Record>>>;

struct Record {
    kind: &'static str,
    bytes: Vec<u8>,
    names: Vec<String>,
    descriptor: Option<OwnedFd>,
    receiver_raw: Option<i32>,
    inode: Option<(u64, u64)>,
    cloexec: bool,
    sealed: bool,
    initial_offset: u64,
}

struct Server {
    version: u32,
    mode: u8,
    records: Records,
    release: Arc<tokio::sync::Notify>,
}

#[zbus::interface(name = "org.freedesktop.portal.Notification")]
impl Server {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.version
    }

    #[zbus(property, name = "SupportedOptions")]
    fn supported_options(&self) -> HashMap<String, OwnedValue> {
        HashMap::new()
    }

    async fn add_notification(
        &self,
        _id: &str,
        mut notification: HashMap<String, zbus::zvariant::Value<'_>>,
    ) -> zbus::fdo::Result<()> {
        let icon = notification.remove("icon");
        let receiver_raw = icon.as_ref().and_then(|value| {
            let structure = value.downcast_ref::<zbus::zvariant::Structure>().unwrap();
            if structure.fields()[0]
                .downcast_ref::<zbus::zvariant::Str>()
                .unwrap()
                == "file-descriptor"
            {
                Some(
                    structure.fields()[1]
                        .downcast_ref::<zbus::zvariant::Fd>()
                        .unwrap()
                        .as_raw_fd(),
                )
            } else {
                None
            }
        });
        let icon = icon.map(|icon| {
            ashpd::desktop::Icon::try_from(OwnedValue::try_from(icon).unwrap()).unwrap()
        });
        drop(notification);
        let mut record = inspect(icon);
        record.receiver_raw = receiver_raw;
        self.records.lock().unwrap().push(record);
        if self.mode == 2 {
            self.release.notified().await;
        }
        if self.mode == 1 {
            Err(zbus::fdo::Error::Failed("test rejection".into()))
        } else {
            Ok(())
        }
    }

    fn remove_notification(&self, _id: &str) {}
}

fn descriptors(inode: (u64, u64)) -> Vec<i32> {
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let metadata = std::fs::metadata(entry.path()).ok()?;
            if (metadata.dev(), metadata.ino()) == inode {
                entry.file_name().to_str()?.parse().ok()
            } else {
                None
            }
        })
        .collect()
}

#[allow(
    unsafe_code,
    reason = "F_GETFD observes descriptor flags without adopting ownership"
)]
fn cloexec(raw: i32) -> bool {
    // SAFETY: F_GETFD accepts any integer descriptor and reports EBADF if it has closed.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    flags >= 0 && flags & libc::FD_CLOEXEC != 0
}

fn inspect(icon: Option<ashpd::desktop::Icon>) -> Record {
    let mut record = Record {
        kind: "none",
        bytes: vec![],
        names: vec![],
        descriptor: None,
        receiver_raw: None,
        inode: None,
        cloexec: false,
        sealed: false,
        initial_offset: u64::MAX,
    };
    match icon {
        Some(ashpd::desktop::Icon::Bytes(bytes)) => {
            record.kind = "bytes";
            record.bytes = bytes;
        }
        Some(ashpd::desktop::Icon::Names(names)) => {
            record.kind = "themed";
            record.names = names;
        }
        Some(ashpd::desktop::Icon::FileDescriptor(fd)) => {
            record.kind = "file-descriptor";
            let mut file = std::fs::File::from(fd);
            let metadata = file.metadata().unwrap();
            let inode = (metadata.dev(), metadata.ino());
            let fds = descriptors(inode);
            assert!(
                fds.len() >= 2,
                "sender and receiver descriptors must both be live"
            );
            record.cloexec = fds.iter().all(|raw| cloexec(*raw));
            record.initial_offset = file.stream_position().unwrap();
            record.sealed = fcntl_get_seals(&file)
                .unwrap()
                .contains(SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL);
            file.read_to_end(&mut record.bytes).unwrap();
            assert!(file.write_all(b"mutate").is_err());
            assert!(file.set_len(0).is_err());
            record.inode = Some(inode);
            record.descriptor = Some(file.into());
        }
        None => {}
        Some(other) => panic!("unexpected icon: {other:?}"),
    }
    record
}

async fn setup(
    bus: &PrivateBus,
    version: u32,
    mode: u8,
) -> (zbus::Connection, Client, Records, Arc<tokio::sync::Notify>) {
    let records = Arc::new(Mutex::new(Vec::new()));
    let release = Arc::new(tokio::sync::Notify::new());
    let server = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.portal.Desktop")
        .unwrap()
        .serve_at(
            "/org/freedesktop/portal/desktop",
            Server {
                version,
                mode,
                records: Arc::clone(&records),
                release: Arc::clone(&release),
            },
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let mut client = DesktopNotificationClient::new(
        AppId::new("org.enzui.IconTest").unwrap(),
        DbusBackend::with_address(Some(bus.address.clone())).unwrap(),
    );
    loop {
        if matches!(
            event(&mut client).await,
            NotificationEvent::ServiceChanged {
                available: true,
                ..
            }
        ) {
            break;
        }
    }
    (server, client, records, release)
}

fn notice(id: &str) -> DesktopNotification {
    DesktopNotification::new(NotificationId::new(id).unwrap(), "Icon")
}

fn raster(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
    let mut output = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(width, height)
        .write_to(&mut output, format)
        .unwrap();
    output.into_inner()
}

const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><rect width="32" height="32" fill="red"/></svg>"#;

async fn release_descriptor(records: &Records, release: Option<&tokio::sync::Notify>) {
    let (inode, descriptor, receiver_raw) = {
        let mut records = records.lock().unwrap();
        let record = records.last_mut().unwrap();
        (
            record.inode.unwrap(),
            record.descriptor.take().unwrap(),
            record.receiver_raw.unwrap(),
        )
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let fds = descriptors(inode);
            // zbus retains the receiver's incoming message until its method handler completes.
            if fds.iter().all(|raw| {
                *raw == descriptor.as_raw_fd() || (release.is_some() && *raw == receiver_raw)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "sender descriptor leaked: live={:?}, receiver={receiver_raw}, retained={}",
            descriptors(inode),
            descriptor.as_raw_fd()
        )
    });
    drop(descriptor);
    if let Some(release) = release {
        release.notify_one();
    }
    tokio::time::timeout(Duration::from_secs(3), async {
        while !descriptors(inode).is_empty() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("receiver descriptor leaked after handler completion");
}

#[test]
fn portal_icon_versions_transfer_contents_seals_and_lifetime() {
    for version in [1, 2] {
        let bus = PrivateBus::spawn();
        runtime().block_on(async {
            let (_server, mut client, records, _release) = setup(&bus, version, 0).await;
            for (index, bytes) in [
                raster(32, 32, image::ImageFormat::Png),
                raster(32, 32, image::ImageFormat::Jpeg),
                SVG.to_vec(),
            ]
            .into_iter()
            .enumerate()
            {
                client
                    .send(
                        notice(&format!("icon-{index}"))
                            .icon(NotificationIcon::Bytes(bytes.clone())),
                    )
                    .unwrap();
                assert_eq!(
                    outcome(&mut client).await,
                    NotificationOperationOutcome::Sent
                );
                {
                    let records = records.lock().unwrap();
                    let record = records.last().unwrap();
                    assert_eq!(record.bytes, bytes);
                    assert_eq!(
                        record.kind,
                        if version == 1 {
                            "bytes"
                        } else {
                            "file-descriptor"
                        }
                    );
                    if version == 2 {
                        assert!(
                            record.cloexec,
                            "all live copies, including sender, must be close-on-exec"
                        );
                        assert!(record.sealed);
                        assert_eq!(record.initial_offset, 0);
                    }
                }
                if version == 2 {
                    release_descriptor(&records, None).await;
                }
            }
            client
                .send(notice("themed").icon(NotificationIcon::Themed(vec![
                    "mail-unread".into(),
                    "mail".into(),
                ])))
                .unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Sent
            );
            let records = records.lock().unwrap();
            assert_eq!(records.last().unwrap().kind, "themed");
            assert_eq!(records.last().unwrap().names, ["mail-unread", "mail"]);
        });
    }
}

#[test]
fn portal_icon_rejects_malformed_dimensions_and_size_before_submission() {
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let (_server, mut client, records, _release) = setup(&bus, 2, 0).await;
        for (index, bytes) in [
            b"not an image".to_vec(),
            b"\x89PNG\r\n\x1a\n".to_vec(),
            raster(16, 32, image::ImageFormat::Png),
            raster(16, 32, image::ImageFormat::Jpeg),
            raster(513, 513, image::ImageFormat::Png),
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="32"/>"#.to_vec(),
            b"<svg>".to_vec(),
            [SVG, &vec![b' '; 4097]].concat(),
        ]
        .into_iter()
        .enumerate()
        {
            client
                .send(notice(&format!("invalid-{index}")).icon(NotificationIcon::Bytes(bytes)))
                .unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Failed
            );
        }
        for bytes in [vec![], vec![0; 256 * 1024 + 1]] {
            assert!(matches!(
                client.send(notice("oversized").icon(NotificationIcon::Bytes(bytes))),
                Err(NotificationError::InvalidPayload(
                    NotificationValidationError::InvalidIcon
                ))
            ));
        }
        client.send(notice("control")).unwrap();
        assert_eq!(
            outcome(&mut client).await,
            NotificationOperationOutcome::Sent
        );
        assert_eq!(records.lock().unwrap().len(), 1);
    });
}

#[test]
fn portal_icon_descriptor_closes_after_error_and_timeout() {
    for mode in [1, 2] {
        let bus = PrivateBus::spawn();
        runtime().block_on(async {
            let (_server, mut client, records, release) = setup(&bus, 2, mode).await;
            client
                .send(notice("error").icon(NotificationIcon::Bytes(SVG.to_vec())))
                .unwrap();
            assert_eq!(
                outcome(&mut client).await,
                NotificationOperationOutcome::Failed
            );
            release_descriptor(&records, (mode == 2).then_some(release.as_ref())).await;
        });
    }
}

#[test]
fn portal_icon_svg_preflight_does_not_open_linked_files() {
    use enzui_test_support::test_scratch::ScratchDir;
    use rustix::fs::inotify::{self, CreateFlags, WatchFlags};

    let root = ScratchDir::new_restricted("notification-icon");
    let linked = root.path().join("linked.png");
    std::fs::write(&linked, raster(32, 32, image::ImageFormat::Png)).unwrap();
    let watch = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK).unwrap();
    inotify::add_watch(&watch, &linked, WatchFlags::OPEN).unwrap();
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="32" height="32"><image xlink:href="{}" width="32" height="32"/></svg>"#,
        linked.display()
    );
    let bus = PrivateBus::spawn();
    runtime().block_on(async {
        let (_server, mut client, records, _release) = setup(&bus, 2, 0).await;
        client
            .send(notice("linked").icon(NotificationIcon::Bytes(svg.into_bytes())))
            .unwrap();
        assert_eq!(
            outcome(&mut client).await,
            NotificationOperationOutcome::Sent
        );
        let mut buffer = [0_u8; 512];
        assert_eq!(
            rustix::io::read(&watch, &mut buffer).unwrap_err(),
            rustix::io::Errno::AGAIN
        );
        let _control = std::fs::File::open(&linked).unwrap();
        assert!(rustix::io::read(&watch, &mut buffer).unwrap() > 0);
        release_descriptor(&records, None).await;
    });
}
