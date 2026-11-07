//! One application identity generates desktop integration files that agree
//! with each other, and `check` finds an installed file that does not.

use enzui_core::packaging::{AppPackage, PackageError, Release, validate_app_id};
use std::path::{Path, PathBuf};

fn notes() -> AppPackage {
    AppPackage::new(
        "org.example.Notes",
        "Notes",
        "Write things down & find them again",
        "notes",
    )
    .expect("identity")
    .description(["Notes keeps short <formatted> notes."])
    .developer("org.example", "The Example Team")
    .categories(["Office", "Utility"])
    .keywords(["memo", "jot"])
    .mime_types(["text/markdown"])
    .homepage("https://example.org/notes")
    .bug_tracker("https://example.org/notes/issues")
    .project_license("GPL-3.0-or-later")
    .search_provider(true)
    .single_main_window(true)
    .release(Release {
        version: "1.0.0".into(),
        date: "2026-10-21".into(),
        notes: vec!["First release.".into()],
    })
    .translation("de", "Notizen", "Dinge aufschreiben und wiederfinden")
    .icon_svg("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"/>")
}

/// A fresh directory under the system temporary directory.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("enzui-packaging-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn package_identity_is_consistent_and_passes_reference_validators() {
    // application_ids_follow_the_shared_rules
    {
        for good in [
            "org.example.Notes",
            "io.github.some_one.App",
            "org.example.my-app",
        ] {
            assert_eq!(validate_app_id(good), Ok(()), "{good}");
        }
        for bad in [
            "notes",
            "org..Notes",
            "org.2fa.App",
            "org.example.Notes!",
            ".org.App",
            "",
        ] {
            assert!(
                matches!(validate_app_id(bad), Err(PackageError::InvalidId { .. })),
                "{bad} is refused"
            );
        }
        assert_eq!(
            AppPackage::new("org.example.Notes", " ", "s", "notes").err(),
            Some(PackageError::Empty("name"))
        );
    }

    // the_files_carry_one_identity
    {
        let package = notes();
        let desktop = package.desktop_entry();
        for line in [
            "Name=Notes",
            "Name[de]=Notizen",
            "Exec=notes %U",
            "Icon=org.example.Notes",
            "StartupWMClass=org.example.Notes",
            "MimeType=text/markdown;",
            "DBusActivatable=true",
            "SingleMainWindow=true",
        ] {
            assert!(desktop.lines().any(|l| l == line), "{line} in\n{desktop}");
        }
        let metainfo = package.metainfo();
        assert!(metainfo.contains("<id>org.example.Notes</id>"));
        assert!(
            metainfo
                .contains("<launchable type=\"desktop-id\">org.example.Notes.desktop</launchable>")
        );
        assert!(
            metainfo.contains("Write things down &amp; find them again"),
            "text is escaped for XML"
        );
        assert!(metainfo.contains("&lt;formatted&gt;"));
        let files = package.files(Path::new("/app"));
        let paths: Vec<_> = files.iter().map(|f| f.path.display().to_string()).collect();
        assert_eq!(
            paths,
            [
                "app/share/applications/org.example.Notes.desktop",
                "app/share/metainfo/org.example.Notes.metainfo.xml",
                "app/share/icons/hicolor/scalable/apps/org.example.Notes.svg",
                "app/share/dbus-1/services/org.example.Notes.service",
                "app/share/gnome-shell/search-providers/org.example.Notes.search-provider.ini",
                "app/share/krunner/dbusplugins/org.example.Notes.desktop",
            ]
        );
        let service = &files[3].contents;
        assert_eq!(
            service,
            "[D-BUS Service]\nName=org.example.Notes\nExec=/app/bin/notes\n"
        );
        assert!(
            files[4]
                .contents
                .contains("ObjectPath=/org/example/Notes\n")
        );
        // KRunner calls the same bus name and path.
        for line in [
            "X-Plasma-API=DBus\n",
            "X-Plasma-DBusRunner-Service=org.example.Notes\n",
            "X-Plasma-DBusRunner-Path=/org/example/Notes\n",
        ] {
            assert!(files[5].contents.contains(line), "{line}");
        }
    }

    // check_finds_what_disagrees_with_the_identity
    {
        let package = notes();
        let root = scratch("check");
        let prefix = Path::new("/usr");
        package.write(&root, prefix).expect("write");
        assert_eq!(
            package.check(&root, prefix),
            [],
            "freshly written files agree"
        );

        let share = root.join("usr/share");
        let desktop = share.join("applications/org.example.Notes.desktop");
        let text = std::fs::read_to_string(&desktop).expect("desktop");
        std::fs::write(
            &desktop,
            text.replace("Icon=org.example.Notes", "Icon=notes")
                .replace("Exec=notes %U", "Exec=/usr/bin/other %U"),
        )
        .expect("tamper");
        std::fs::remove_file(share.join("dbus-1/services/org.example.Notes.service")).expect("rm");
        std::fs::remove_file(share.join("icons/hicolor/scalable/apps/org.example.Notes.svg"))
            .expect("rm icon");
        let problems: Vec<String> = package
            .check(&root, prefix)
            .iter()
            .map(|mismatch| mismatch.problem.clone())
            .collect();
        assert_eq!(problems.len(), 4, "{problems:#?}");
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("Icon is Some(\"notes\")"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("Exec runs Some(\"other\")"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("missing for a D-Bus activatable"))
        );
        assert!(
            problems
                .iter()
                .any(|p| p.starts_with("no org.example.Notes.svg"))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// The generated files pass the reference validators. Needs
/// `desktop-file-validate` (desktop-file-utils) and `appstreamcli`
/// (AppStream).
#[test]
#[ignore = "needs desktop-file-validate and appstreamcli"]
fn the_files_pass_the_reference_validators() {
    let root = scratch("validators");
    let written = notes().write(&root, Path::new("/usr")).expect("write");
    let run = |program: &str, args: &[&str], file: &Path| {
        let output = std::process::Command::new(program)
            .args(args)
            .arg(file)
            .output()
            .unwrap_or_else(|error| panic!("{program}: {error}"));
        assert!(
            output.status.success(),
            "{program} {}:\n{}{}",
            file.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    let desktop = written
        .iter()
        .find(|p| p.extension().is_some_and(|e| e == "desktop"));
    run("desktop-file-validate", &[], desktop.expect("desktop"));
    let metainfo = written
        .iter()
        .find(|p| p.to_string_lossy().ends_with(".metainfo.xml"));
    run(
        "appstreamcli",
        &["validate", "--no-net", "--pedantic"],
        metainfo.expect("metainfo"),
    );
    let _ = std::fs::remove_dir_all(&root);
}
