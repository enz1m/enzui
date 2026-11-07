use super::*;

struct TmpDir(PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "enzui-atomic-state-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const FMT: &str = "test-format";

#[test]
fn atomic_state_round_trips_both_envelopes_with_bounded_bodies() {
    // envelope_compatibility_accepts_both_prefixes_with_exact_raw_limits
    {
        let d = TmpDir::new("envelope-compatibility");
        let p = d.path("state");
        for prefix in ["enzui-iced", "enzui"] {
            let bytes = format!("{prefix} test-format v1\nbody\né\n");
            std::fs::write(&p, &bytes).expect("seed persisted envelope");
            let limit = u64::try_from(bytes.len()).expect("fixture fits u64");
            assert_eq!(
                load_bounded(&p, FMT, 1, limit),
                (StateLoad::Loaded("é\n".into()), None)
            );
            assert_eq!(
                load_bounded(&p, FMT, 1, limit - 1).1,
                Some(StateRefusal::TooLarge)
            );
            assert_eq!(
                load_bounded(&p, FMT, 2, limit).1,
                Some(StateRefusal::UnexpectedHeader)
            );
            assert_eq!(
                load_bounded(&p, "other", 1, limit).1,
                Some(StateRefusal::UnexpectedHeader)
            );
        }
        for bytes in [
            "enzui-iced test-format v1\n",
            "enzui test-format v1\nbody",
            "enzui-extra test-format v1\nbody\né\n",
        ] {
            std::fs::write(&p, bytes).expect("seed incompatible envelope");
            assert_eq!(
                load_with_reason(&p, FMT, 1).1,
                Some(StateRefusal::UnexpectedHeader)
            );
        }
    }

    // envelope_compatibility_writer_preserves_the_persisted_prefix
    {
        let d = TmpDir::new("envelope-writer");
        let p = d.path("state");
        save_atomic(&p, FMT, 1, "é\n").expect("save");
        let expected = "enzui-iced test-format v1\nbody\né\n";
        assert_eq!(std::fs::read_to_string(&p).expect("read frame"), expected);
        assert_eq!(framed_len(FMT, 1, "é\n"), Some(expected.len() as u64));
    }

    // a_saved_file_round_trips_through_load
    {
        let d = TmpDir::new("roundtrip");
        let p = d.path("state");
        save_atomic(&p, FMT, 1, "line one\nline two\n").expect("save");

        match load(&p, FMT, 1) {
            StateLoad::Loaded(body) => assert_eq!(body, "line one\nline two\n"),
            other => panic!("expected Loaded, got {other:?}"),
        }
    }
}
