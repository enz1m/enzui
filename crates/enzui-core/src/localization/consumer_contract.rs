use enzui_test_support::test_scratch::ScratchDir;
use std::path::Path;

#[test]
fn localization_standalone_consumer_resolves_without_the_iced_facade() {
    let scratch = ScratchDir::new("localization-consumer");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let manifest = format!(
        "[package]\nname = \"localization-consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
         [workspace]\n[dependencies]\nenzui-core = {{ path = {:?} }}\n",
        root.join("crates/enzui-core").to_str().unwrap()
    );
    std::fs::write(scratch.join("Cargo.toml"), manifest).unwrap();
    std::fs::create_dir(scratch.join("src")).unwrap();
    std::fs::write(
        scratch.join("src/lib.rs"),
        "pub use enzui_core::localization;\n",
    )
    .unwrap();
    let diagnostics = scratch.join("metadata.stderr");
    let mut child = std::process::Command::new(env!("CARGO"))
        .args([
            "metadata",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(scratch.join("Cargo.toml"))
        .stdout(std::process::Stdio::null())
        .stderr(std::fs::File::create(&diagnostics).unwrap())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("standalone Cargo metadata exceeded 30 seconds");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "{}",
        std::fs::read_to_string(diagnostics).unwrap()
    );
    let lock = std::fs::read_to_string(scratch.join("Cargo.lock"))
        .unwrap()
        .parse::<toml_edit::DocumentMut>()
        .unwrap();
    let packages = lock["package"].as_array_of_tables().unwrap();
    for name in ["enzui-core", "iced_core", "iced_futures", "iced_runtime"] {
        let matches: Vec<_> = packages
            .iter()
            .filter(|package| package["name"].as_str() == Some(name))
            .collect();
        assert_eq!(matches.len(), 1, "one patched dependency for {name}");
        if name == "enzui-core" {
            assert!(!matches[0].contains_key("source"), "local core source");
        } else {
            let workspace = std::fs::read_to_string(root.join("Cargo.toml"))
                .unwrap()
                .parse::<toml_edit::DocumentMut>()
                .unwrap();
            let dependency = &workspace["workspace"]["dependencies"][name];
            let git = dependency["git"].as_str().unwrap();
            let rev = dependency["rev"].as_str().unwrap();
            let expected = format!("git+{git}?rev={rev}#{rev}");
            assert_eq!(matches[0]["source"].as_str(), Some(expected.as_str()));
        }
    }
    assert!(
        packages
            .iter()
            .all(|package| package["name"].as_str() != Some("iced")),
        "localization needs no renderer facade"
    );
}
