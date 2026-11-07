use super::*;

#[derive(Clone, Default)]
struct CountingBackend {
    inner: InMemoryBackend,
    writes: Arc<std::sync::atomic::AtomicU64>,
}

impl CountingBackend {
    fn new() -> Self {
        Self::default()
    }
    fn writes(&self) -> u64 {
        self.writes.load(Ordering::Acquire)
    }
}

impl PersistBackend for CountingBackend {
    fn load_raw(&self, fmt: &str, ver: u32) -> BackendLoad {
        self.inner.load_raw(fmt, ver)
    }
    fn save_raw(&self, fmt: &str, ver: u32, body: &str) -> Result<(), PersistError> {
        self.writes.fetch_add(1, Ordering::AcqRel);
        self.inner.save_raw(fmt, ver, body)
    }
}

#[test]
fn gated_success_commits_token() {
    let backend = CountingBackend::new();
    let store = Store::<TestSettings>::new(backend.clone(), "gated-ok", 1);
    let mut gate = SaveGate::new(store);

    let op = gate
        .prepare_save_if_changed(1u64, TestSettings::default)
        .expect("prepare")
        .expect("first token yields an op");
    op.execute().expect("execute succeeds");
    assert_eq!(backend.writes(), 1);

    let op = gate
        .prepare_save_if_changed(1u64, || panic!("build must not run"))
        .expect("prepare same");
    assert!(op.is_none(), "committed token must be skipped");
}
