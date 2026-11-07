use super::*;

#[test]
fn dropping_an_active_backend_finishes_within_the_shutdown_bound() {
    let state = Arc::new(Mutex::new(MockState::default()));
    let (_runtime, (bus, _connection, address)) = require_mock!(Script::Hang, state);
    let mut backend = DbusBackend::with_address(Some(address)).expect("backend");
    let _id = start_when_ready(&mut backend, PortalOperation::OpenFile)
        .expect("active request must start");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while state.lock().unwrap().open_calls == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "request must reach mock"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let owner = backend.shutdown_owner.take().expect("owner thread");
    let started = std::time::Instant::now();
    drop(backend);
    owner.join().expect("shutdown owner must exit");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(7),
        "active shutdown must remain bounded: {:?}",
        started.elapsed()
    );
    drop(bus);
}

/// Opening a retained directory reaches the portal as `OpenFile` with the
/// directory's own descriptor, which the file manager opens; `OpenDirectory`
/// would open the folder containing it.
#[test]
fn a_retained_directory_opens_through_open_file_with_its_descriptor() {
    use crate::portal::PortalParent;
    use crate::portal::open_uri::{OpenDirectoryRequest, OpenUriBackend, RetainedDirectory};
    use std::os::unix::fs::MetadataExt;

    let state = Arc::new(Mutex::new(MockState::default()));
    let (_runtime, (bus, _connection, address)) = require_mock!(
        Script::Success {
            uris: Vec::new(),
            delay_ms: 0
        },
        state
    );
    let mut backend = DbusBackend::with_address(Some(address)).expect("backend");
    let scratch = enzui_test_support::test_scratch::ScratchDir::new("open-retained-directory");
    let folder = scratch.path().join("folder");
    std::fs::create_dir_all(&folder).unwrap();
    let directory = std::fs::File::open(&folder).unwrap();
    let metadata = directory.metadata().unwrap();
    let identity = (metadata.dev(), metadata.ino());
    let request = OpenDirectoryRequest::new(
        RetainedDirectory::new(directory).unwrap(),
        PortalParent::None,
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut pending = request
        .start(&mut backend, |_| true)
        .expect("request starts");
    let outcome = loop {
        if let Some(outcome) = pending.poll(&mut backend) {
            break outcome;
        }
        assert!(std::time::Instant::now() < deadline, "never answered");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(outcome, PortalOutcome::Completed(()));
    assert_eq!(
        state.lock().unwrap().open_uri_calls.last().copied(),
        Some(("OpenFile", Some(identity))),
        "{}",
        backend.describe_open_uri()
    );
    drop(bus);
}

/// "Show in folder" reaches the portal as `OpenDirectory` with the caller's
/// exact descriptor, and opening the same file stays `OpenFile`.
#[test]
fn containing_folder_calls_open_directory_with_the_retained_descriptor() {
    use crate::portal::PortalParent;
    use crate::portal::open_uri::{OpenFileRequest, OpenUriBackend, RetainedFile};
    use std::os::unix::fs::MetadataExt;

    let state = Arc::new(Mutex::new(MockState::default()));
    let (_runtime, (bus, _connection, address)) = require_mock!(
        Script::Success {
            uris: Vec::new(),
            delay_ms: 0
        },
        state
    );
    let mut backend = DbusBackend::with_address(Some(address)).expect("backend");
    let scratch = enzui_test_support::test_scratch::ScratchDir::new("open-directory");
    let path = scratch.path().join("shown.txt");
    std::fs::write(&path, "x").unwrap();
    let file = std::fs::File::open(&path).unwrap();
    let metadata = file.metadata().unwrap();
    let identity = (metadata.dev(), metadata.ino());
    let retained = RetainedFile::new(file).unwrap();

    for (request, member) in [
        (
            OpenFileRequest::containing_folder(retained.clone(), PortalParent::None),
            "OpenDirectory",
        ),
        (
            OpenFileRequest::new(retained, PortalParent::None),
            "OpenFile",
        ),
    ] {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut pending = request
            .start(&mut backend, |_| true)
            .expect("request starts");
        let outcome = loop {
            if let Some(outcome) = pending.poll(&mut backend) {
                break outcome;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{member} never answered"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(outcome, PortalOutcome::Completed(()), "{member}");
        assert_eq!(
            state.lock().unwrap().open_uri_calls.last().copied(),
            Some((member, Some(identity))),
            "{} must call {member} with the retained descriptor",
            backend.describe_open_uri()
        );
    }
    drop(bus);
}
