use super::*;

#[test]
fn byte_preview_rejects_file_mutation_without_rendering_stale_data() {
    use std::io::Write as _;
    let scratch = enzui_test_support::test_scratch::ScratchDir::new("byte-preview");
    let path = scratch.path().join("source.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(b"initial bytes\x00").unwrap();
    file.flush().unwrap();
    let source = PagedByteSource::new(Arc::new(
        crate::paged_text::FilePagedTextSource::open(&path).unwrap(),
    ))
    .unwrap();
    assert!(source.read_at(0, source.len().unwrap() as usize).is_ok());
    file.set_len(1).unwrap();
    assert!(source.read_at(0, 1).is_err());
    assert!(source.read_at(u64::MAX, 1).is_err());
}
