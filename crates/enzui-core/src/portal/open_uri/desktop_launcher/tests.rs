use std::path::Path;
use std::time::{Duration, Instant};

use enzui_test_support::fake_executable::write_executable;
use enzui_test_support::test_scratch::ScratchDir;

use super::*;
use crate::portal::open_uri::{
    OpenDirectoryRequest, OpenFileRequest, OpenUriRequest, RetainedFileError,
};
use crate::portal::{PortalOutcome, PortalParent};

/// A launcher script that records its arguments and environment beside it
/// and exits with `code`.
fn recording_launcher(dir: &Path, name: &str, code: i32) -> Launcher {
    let program = dir.join(name);
    write_executable(
        &program,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{log}/args\"\nenv > \"{log}/env\"\nexit {code}\n",
            log = dir.display()
        ),
    );
    Launcher::new(program, Vec::new())
}

fn wait_reply(backend: &mut DesktopLauncherBackend, id: RequestId) -> OpenUriReply {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(reply) = backend.poll_open_uri(id) {
            return reply;
        }
        assert!(Instant::now() < deadline, "the launcher never answered");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap_or_default()
}

#[test]
fn desktop_launcher_retains_targets_handles_replacement_and_reports_outcomes_1() {
    // route_follows_confinement
    {
        assert_eq!(
            OpenRoute::for_evidence(&SandboxEvidence::default()),
            OpenRoute::DesktopLauncher
        );
        let flatpak = SandboxEvidence {
            flatpak_info: true,
            ..SandboxEvidence::default()
        };
        assert_eq!(OpenRoute::for_evidence(&flatpak), OpenRoute::Portal);
    }

    // finds_xdg_open_before_gio
    {
        let scratch = ScratchDir::new("launcher-find");
        let (first, second) = (scratch.path().join("a"), scratch.path().join("b"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        write_executable(&first.join("gio"), "#!/bin/sh\n");
        assert_eq!(
            Launcher::find(&[first.clone(), second.clone()]),
            Some(Launcher::new(
                first.join("gio"),
                vec![OsString::from("open")]
            ))
        );
        write_executable(&second.join("xdg-open"), "#!/bin/sh\n");
        assert_eq!(
            Launcher::find(&[first, second.clone()]),
            Some(Launcher::new(second.join("xdg-open"), Vec::new())),
            "xdg-open anywhere on the path wins over gio"
        );
        assert_eq!(Launcher::find(&[PathBuf::from("relative")]), None);
    }

    // opens_a_uri_with_a_filtered_environment
    {
        let scratch = ScratchDir::new("launcher-uri");
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        let uri = OpenUri::parse("https://example.org/a?b=c").unwrap();
        let id = backend.start_open_uri(&uri, &ParentHandle::none()).unwrap();
        assert_eq!(wait_reply(&mut backend, id), OpenUriReply::Success);
        assert_eq!(read(scratch.path(), "args"), "https://example.org/a?b=c\n");
        let env = read(scratch.path(), "env");
        // Cargo sets CARGO_MANIFEST_DIR for every test process; it is not on the
        // allowlist, so the launcher must not see it.
        assert!(std::env::var_os("CARGO_MANIFEST_DIR").is_some());
        assert!(
            !env.contains("CARGO_MANIFEST_DIR="),
            "unlisted variables leak: {env}"
        );
        assert!(
            env.lines().any(|line| line.starts_with("PATH=")),
            "PATH is inherited: {env}"
        );
    }

    // exit_status_maps_to_the_reply
    {
        for (name, code, expected) in [
            ("xdg-open", 3, OpenUriReply::Unavailable),
            ("xdg-open", 4, OpenUriReply::Failed),
            ("gio", 3, OpenUriReply::Failed),
        ] {
            let scratch = ScratchDir::new("launcher-exit");
            let mut backend = DesktopLauncherBackend::with_launcher(Some(recording_launcher(
                scratch.path(),
                name,
                code,
            )));
            let uri = OpenUri::parse("mailto:someone@example.org").unwrap();
            let id = backend.start_open_uri(&uri, &ParentHandle::none()).unwrap();
            assert_eq!(
                wait_reply(&mut backend, id),
                expected,
                "{name} exiting {code}"
            );
        }
    }
}

#[test]
fn desktop_launcher_retains_targets_handles_replacement_and_reports_outcomes_2() {
    // without_a_launcher_the_request_is_unavailable
    {
        let mut backend = DesktopLauncherBackend::with_launcher(None);
        let request = OpenUriRequest::new(
            OpenUri::parse("https://example.org").unwrap(),
            PortalParent::None,
        );
        let outcome = request.start(&mut backend, |_| true).err();
        assert_eq!(outcome, Some(PortalOutcome::Unavailable));
    }

    // opens_a_retained_file_by_its_current_path
    {
        let scratch = ScratchDir::new("launcher-file");
        let document = scratch.path().join("report final.txt");
        std::fs::write(&document, "contents").unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        let file = RetainedFile::new(std::fs::File::open(&document).unwrap()).unwrap();
        let id = backend
            .start_open_file(&file, &ParentHandle::none(), None)
            .unwrap();
        assert_eq!(wait_reply(&mut backend, id), OpenUriReply::Success);
        let expected = OpenUri::from_file_path(&document).unwrap();
        assert_eq!(read(scratch.path(), "args"), format!("{expected}\n"));
    }

    // follows_the_retained_file_after_a_rename
    {
        let scratch = ScratchDir::new("launcher-replaced");
        let document = scratch.path().join("doc.txt");
        std::fs::write(&document, "original").unwrap();
        let file = RetainedFile::new(std::fs::File::open(&document).unwrap()).unwrap();
        let impostor = scratch.path().join("impostor.txt");
        std::fs::write(&impostor, "other bytes").unwrap();
        // Replace the name while keeping the original inode alive elsewhere, so the
        // descriptor's link resolves to the moved original, not the new name.
        std::fs::rename(&document, scratch.path().join("moved.txt")).unwrap();
        std::fs::rename(&impostor, &document).unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        let id = backend
            .start_open_file(&file, &ParentHandle::none(), None)
            .unwrap();
        assert_eq!(wait_reply(&mut backend, id), OpenUriReply::Success);
        let moved = OpenUri::from_file_path(&scratch.path().join("moved.txt")).unwrap();
        assert_eq!(
            read(scratch.path(), "args"),
            format!("{moved}\n"),
            "the launcher gets the file the descriptor holds, never the name's new occupant"
        );
    }

    // containing_folder_opens_the_directory_without_a_file_manager
    {
        let scratch = ScratchDir::new("launcher-folder");
        let folder = scratch.path().join("inner");
        std::fs::create_dir_all(&folder).unwrap();
        let document = folder.join("a.txt");
        std::fs::write(&document, "x").unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher))
            .with_file_manager_show_items(false);
        let request = OpenFileRequest::containing_folder(
            RetainedFile::new(std::fs::File::open(&document).unwrap()).unwrap(),
            PortalParent::None,
        );
        let mut pending = request.start(&mut backend, |_| true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let outcome = loop {
            if let Some(outcome) = pending.poll(&mut backend) {
                break outcome;
            }
            assert!(Instant::now() < deadline, "no outcome");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(outcome, PortalOutcome::Completed(()));
        let expected = OpenUri::from_file_path(&folder).unwrap();
        assert_eq!(read(scratch.path(), "args"), format!("{expected}\n"));
    }
}

#[test]
fn desktop_launcher_retains_targets_handles_replacement_and_reports_outcomes_3() {
    // closing_discards_a_late_answer
    {
        let scratch = ScratchDir::new("launcher-close");
        let program = scratch.path().join("xdg-open");
        let gate = scratch.path().join("go");
        write_executable(
            &program,
            &format!(
                "#!/bin/sh\nwhile [ ! -e \"{}\" ]; do sleep 0.01; done\nexit 0\n",
                gate.display()
            ),
        );
        let mut backend =
            DesktopLauncherBackend::with_launcher(Some(Launcher::new(program, Vec::new())));
        let id = backend
            .start_open_uri(
                &OpenUri::parse("https://example.org").unwrap(),
                &ParentHandle::none(),
            )
            .unwrap();
        backend.close_open_uri(id, CancelReason::Application);
        let signal = backend.open_uri_completion_signal().unwrap();
        let before = signal.generation();
        std::fs::write(&gate, "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while signal.generation() == before {
            assert!(Instant::now() < deadline, "the launcher never exited");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            backend.poll_open_uri(id),
            None,
            "a closed request reports nothing"
        );
    }

    // refuses_a_retained_file_that_lost_its_name
    {
        let scratch = ScratchDir::new("launcher-deleted");
        let document = scratch.path().join("gone.txt");
        std::fs::write(&document, "x").unwrap();
        let file = RetainedFile::new(std::fs::File::open(&document).unwrap()).unwrap();
        std::fs::remove_file(&document).unwrap();
        // Recreate the name with different bytes: a path-based launcher would
        // open this impostor.
        std::fs::write(&document, "impostor").unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        assert_eq!(
            backend.start_open_file(&file, &ParentHandle::none(), None),
            Err(BackendError::Failed)
        );
        assert_eq!(
            read(scratch.path(), "args"),
            "",
            "the launcher must not run"
        );
    }

    // an_application_task_can_hold_the_host_backend_across_an_await
    {
        fn send<T: Send>(_: &T) {}
        let task = async {
            let backend = super::host_open_uri_backend();
            std::future::ready(()).await;
            drop(backend);
            let routed = super::backend_for_route(super::OpenRoute::Portal);
            std::future::ready(()).await;
            drop(routed);
        };
        send(&task);
    }

    // opens_a_retained_directory_by_its_path
    {
        let scratch = ScratchDir::new("launcher-directory");
        let folder = scratch.path().join("Downloads folder");
        std::fs::create_dir_all(&folder).unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        let request = OpenDirectoryRequest::new(
            RetainedDirectory::new(std::fs::File::open(&folder).unwrap()).unwrap(),
            PortalParent::None,
        );
        let mut pending = request.start(&mut backend, |_| true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let outcome = loop {
            if let Some(outcome) = pending.poll(&mut backend) {
                break outcome;
            }
            assert!(Instant::now() < deadline, "no outcome");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(outcome, PortalOutcome::Completed(()));
        let expected = OpenUri::from_file_path(&folder).unwrap();
        assert_eq!(read(scratch.path(), "args"), format!("{expected}\n"));
    }
}

#[test]
fn desktop_launcher_retains_targets_handles_replacement_and_reports_outcomes_4() {
    // refuses_a_retained_directory_that_was_removed
    {
        let scratch = ScratchDir::new("launcher-directory-replaced");
        let folder = scratch.path().join("project");
        std::fs::create_dir_all(&folder).unwrap();
        let directory = RetainedDirectory::new(std::fs::File::open(&folder).unwrap()).unwrap();
        // The directory is removed and another takes its name: a path-based
        // launcher would open the newcomer.
        std::fs::remove_dir(&folder).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        assert_eq!(
            backend.start_open_directory(&directory, &ParentHandle::none(), None),
            Err(BackendError::Failed)
        );
        assert_eq!(
            read(scratch.path(), "args"),
            "",
            "the launcher must not run"
        );
    }

    // files_and_directories_are_retained_by_their_own_kinds
    {
        let scratch = ScratchDir::new("retained-kinds");
        let folder = scratch.path().join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let document = scratch.path().join("note.txt");
        std::fs::write(&document, "x").unwrap();
        assert_eq!(
            RetainedDirectory::new(std::fs::File::open(&document).unwrap()).err(),
            Some(RetainedFileError::NotDirectory)
        );
        assert_eq!(
            RetainedFile::new(std::fs::File::open(&folder).unwrap()).err(),
            Some(RetainedFileError::NotRegularFile)
        );
        assert!(RetainedDirectory::new(std::fs::File::open(&folder).unwrap()).is_ok());
    }

    // follows_the_retained_directory_after_a_rename
    {
        let scratch = ScratchDir::new("launcher-directory-renamed");
        let folder = scratch.path().join("project");
        std::fs::create_dir_all(&folder).unwrap();
        let directory = RetainedDirectory::new(std::fs::File::open(&folder).unwrap()).unwrap();
        let moved = scratch.path().join("project (old)");
        std::fs::rename(&folder, &moved).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        let launcher = recording_launcher(scratch.path(), "xdg-open", 0);
        let mut backend = DesktopLauncherBackend::with_launcher(Some(launcher));
        let id = backend
            .start_open_directory(&directory, &ParentHandle::none(), None)
            .unwrap();
        assert_eq!(wait_reply(&mut backend, id), OpenUriReply::Success);
        let expected = OpenUri::from_file_path(&moved).unwrap();
        assert_eq!(
            read(scratch.path(), "args"),
            format!("{expected}\n"),
            "the directory the descriptor holds, never the name's new occupant"
        );
    }
}
