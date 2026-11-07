use super::*;

#[test]
fn an_existing_utf8_ledger_still_loads() {
    let d = TmpDir::new("legacy-utf8");
    let state = d.path("downloads.state");
    let body = "id=3\tname=r%C3%A9sum%C3%A9.pdf\tattempt=0\tphase=paused\treceived=5\ttotal=10\tpartial=/tmp/caf\u{e9}%25.part\n";
    crate::atomic_state::save_atomic(&state, STATE_FORMAT_ID, STATE_VERSION, body)
        .expect("save legacy");

    let restored = load_state(&state);
    assert_eq!(restored.records.len(), 1, "legacy record must still load");
    let r = &restored.records[0];
    assert_eq!(r.id, DownloadId(3));
    assert_eq!(
        r.partial.as_deref(),
        Some(std::path::Path::new("/tmp/caf\u{e9}%.part")),
        "a legacy partial's `%25` must decode to a literal percent"
    );
}
