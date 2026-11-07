use super::*;

#[test]
fn filesystem_source_rejects_same_inode_mutation() {
    use std::io::Write;
    use std::os::unix::fs::MetadataExt;

    let sandbox = PoisonedFileFixture::beside_test_binary();
    let path = sandbox.root.join("mutable.txt");
    std::fs::write(&path, "before").unwrap();
    let original_metadata = std::fs::metadata(&path).unwrap();
    let original_modified = original_metadata.modified().unwrap();
    let source = FilePagedTextSource::open(&path).unwrap();

    let mut writer = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    writer.write_all(b"after!").unwrap();
    writer.sync_all().unwrap();
    writer
        .set_times(std::fs::FileTimes::new().set_modified(original_modified))
        .unwrap();
    let rewritten_metadata = writer.metadata().unwrap();
    assert_eq!(rewritten_metadata.dev(), original_metadata.dev());
    assert_eq!(rewritten_metadata.ino(), original_metadata.ino());
    assert_eq!(rewritten_metadata.len(), original_metadata.len());
    assert_eq!(rewritten_metadata.modified().unwrap(), original_modified);
    assert_ne!(
        (rewritten_metadata.ctime(), rewritten_metadata.ctime_nsec()),
        (original_metadata.ctime(), original_metadata.ctime_nsec())
    );

    let error = source.read_at(0, 6).unwrap_err();
    assert_eq!(error.kind(), PagedTextSourceErrorKind::Unreadable);
    assert_eq!(
        error.diagnostic(),
        "paged-text file changed after it was opened"
    );
}

struct PoisonedFileFixture {
    root: std::path::PathBuf,
}

impl PoisonedFileFixture {
    fn beside_test_binary() -> Self {
        let root = std::env::current_exe()
            .expect("current test executable")
            .parent()
            .expect("test executable directory")
            .join(format!(
                "enzui-paged-text-poison-{}-{}",
                std::process::id(),
                std::thread::current().name().unwrap_or("test")
            ));
        // Remove any stale fixture directory before taking ownership of the path.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        Self { root }
    }
}

impl Drop for PoisonedFileFixture {
    fn drop(&mut self) {
        // Ignore cleanup errors so `Drop` cannot panic during unwinding.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_character_cut_off_is_only_tolerated_in_lookahead_before_more_source() {
    let cut = b"ab\xE2\x82";
    // Cut off at the source's real end: invalid, at the cut character.
    assert!(matches!(
        decode_page(cut, 0, 0, 4, 0, true),
        Err(PagedTextOpenFailure::InvalidUtf8 { offset: 2 })
    ));
    // Cut off inside the page itself: invalid even with more source after.
    assert!(matches!(
        decode_page(cut, 0, 0, 4, 0, false),
        Err(PagedTextOpenFailure::InvalidUtf8 { offset: 2 })
    ));
    // Only in the lookahead past the page, with more to come: the page decodes.
    let page = decode_page(cut, 0, 0, 2, 0, false).expect("the page is whole");
    assert_eq!(page.byte_end, 2);
}
