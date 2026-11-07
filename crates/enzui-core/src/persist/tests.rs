use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, PartialEq)]
struct TestSettings {
    font_size: u32,
    dark_mode: bool,
    new_field: String,
}

impl Default for TestSettings {
    fn default() -> Self {
        Self {
            font_size: 14,
            dark_mode: false,
            new_field: String::new(),
        }
    }
}

impl Persistable for TestSettings {
    fn persist_serialize(&self) -> Result<String, String> {
        Ok(format!(
            "font_size={}\ndark_mode={}\nnew_field={}\n",
            self.font_size, self.dark_mode, self.new_field
        ))
    }

    fn persist_deserialize(body: &str) -> Result<Self, String> {
        let mut s = Self::persist_default();
        for line in body.lines() {
            if let Some((key, val)) = line.split_once('=') {
                match key.trim() {
                    "font_size" => {
                        s.font_size = val.trim().parse().unwrap_or(s.font_size);
                    }
                    "dark_mode" => {
                        s.dark_mode = val.trim() == "true";
                    }
                    "new_field" => {
                        s.new_field = val.trim().to_owned();
                    }
                    _ => {}
                }
            }
        }
        Ok(s)
    }

    fn persist_default() -> Self {
        Self::default()
    }
}

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::AtomicU32;
        static N: AtomicU32 = AtomicU32::new(0);

        // Cargo does not expose CARGO_TARGET_TMPDIR to unit-test targets.
        // Discover this test binary's `<target>/<profile>/deps` parent
        // instead, and refuse to guess a fallback outside the target tree.
        let executable = std::env::current_exe().expect("locate persistence test binary");
        let deps = executable
            .parent()
            .expect("persistence test binary has a parent");
        assert_eq!(
            deps.file_name().and_then(|name| name.to_str()),
            Some("deps"),
            "persistence test binary must live in a Cargo target deps directory: {}",
            executable.display()
        );
        let profile = deps.parent().expect("Cargo deps directory has a parent");
        let scratch = profile.join("test-scratch");
        std::fs::create_dir_all(&scratch).expect("create target-tree scratch root");
        let scratch = std::fs::canonicalize(&scratch).expect("resolve target-tree scratch root");
        let profile = std::fs::canonicalize(profile).expect("resolve Cargo profile directory");
        assert!(
            scratch.starts_with(&profile),
            "persistence scratch escaped Cargo target profile: {} not under {}",
            scratch.display(),
            profile.display()
        );

        loop {
            let p = scratch.join(format!(
                "enzui-persist-{}-{}-{tag}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&p) {
                Ok(()) => return Self(p),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create persistence scratch {}: {error}", p.display()),
            }
        }
    }
    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn save_op_is_dispatachable_off_thread() {
    let backend = InMemoryBackend::new();
    let store = Store::<TestSettings>::new(backend.clone(), "test", 1);

    let settings = TestSettings {
        font_size: 20,
        dark_mode: true,
        new_field: "hello".into(),
    };

    let op = store.prepare_save(&settings).expect("serialise");

    let done = AtomicBool::new(false);
    let done_ref = &done;
    std::thread::scope(|s| {
        s.spawn(|| {
            op.execute().expect("save");
            done_ref.store(true, Ordering::Release);
        });
    });

    assert!(
        done.load(Ordering::Acquire),
        "the save must have completed on the background thread"
    );
    assert!(
        backend.peek("test").is_some(),
        "the backend must hold the saved body"
    );
}

mod backend_parity_and_paths;
mod backend_safety;
mod reentry;

#[test]
fn envelope_compatibility_store_loads_and_saves_at_the_existing_body_limit() {
    let d = TmpDir::new("envelope-store");
    let p = d.file("settings");
    let body = "font_size=20\ndark_mode=true\nnew_field=é\n";
    let original = format!("enzui-iced settings v12\nbody\n{body}");
    let limit = RawByteLimit::for_body("settings", 12, body.len() as u64).expect("body limit");
    assert_eq!(limit.bytes(), original.len() as u64);
    for prefix in ["enzui-iced", "enzui"] {
        std::fs::write(&p, format!("{prefix} settings v12\nbody\n{body}"))
            .expect("seed saved settings");
        let store = Store::<TestSettings>::new(FileBackend::new(&p, limit), "settings", 12);
        let LoadOutcome::Loaded(settings) = store.load() else {
            panic!("existing settings must load")
        };
        assert_eq!(settings.font_size, 20);
        assert!(settings.dark_mode);
        assert_eq!(settings.new_field, "é");
        store
            .prepare_save(&settings)
            .expect("accepted state remains writable")
            .execute()
            .expect("save exact body limit");
        assert_eq!(
            std::fs::read_to_string(&p).expect("read settings"),
            original
        );
        let mut oversized = settings;
        oversized.new_field.push('x');
        assert!(store.prepare_save(&oversized).is_err());
        assert_eq!(
            std::fs::read_to_string(&p).expect("read preserved settings"),
            original
        );
    }
}
