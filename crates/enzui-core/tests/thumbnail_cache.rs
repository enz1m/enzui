//! The thumbnail cache: GLib's URI spelling and names, valid and stale
//! thumbnails, larger sizes as a fallback, failures, and the files' modes.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking"
)]
use enzui_core::thumbnail_cache::{ThumbnailCache, ThumbnailImage, ThumbnailSize, file_uri};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn scratch(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("enzui-thumbs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn picture(root: &Path) -> PathBuf {
    let file = root.join("My Pictures (1),+&é.jpg");
    std::fs::write(&file, b"not really a jpeg").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        .unwrap();
    file
}

const PIXELS: [u8; 16] = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 9, 9, 9, 128];

fn image() -> ThumbnailImage<'static> {
    ThumbnailImage {
        width: 2,
        height: 2,
        rgba: &PIXELS,
    }
}

#[test]
fn thumbnail_cache_validates_identity_freshness_limits_and_atomic_storage_1() {
    // uris_and_names_match_what_glib_writes
    {
        assert_eq!(
            file_uri(Path::new("/home/ada/My Pictures (1),+&é.jpg")),
            "file:///home/ada/My%20Pictures%20(1),+&%C3%A9.jpg"
        );
        assert_eq!(
            file_uri(Path::new("/tmp/a#b?c%d;e[f]\"g")),
            "file:///tmp/a%23b%3Fc%25d%3Be%5Bf%5D%22g"
        );
        // The name is the MD5 of this exact spelling, parentheses unescaped as
        // GLib leaves them.
        let cache = ThumbnailCache::new("/cache");
        assert_eq!(
            cache.path_for(
                &file_uri(Path::new("/home/user/Pictures/holiday(1).png")),
                ThumbnailSize::Normal
            ),
            Path::new("/cache/normal/019adf91da7ac9ab63f1e0619abd74c0.png")
        );
    }

    // a_stored_thumbnail_is_found_until_the_file_changes
    {
        let root = scratch("store");
        let file = picture(&root);
        let cache = ThumbnailCache::new(root.join("thumbnails"));
        assert_eq!(cache.lookup(&file, ThumbnailSize::Normal), None);

        let stored = cache
            .store(&file, ThumbnailSize::Large, image(), "enzUI test")
            .unwrap();
        assert_eq!(
            std::fs::metadata(&stored).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(stored.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        // Asked for normal, the large one is the next best.
        let found = cache.lookup(&file, ThumbnailSize::Normal).unwrap();
        assert_eq!(
            (found.path.as_path(), found.size),
            (stored.as_path(), ThumbnailSize::Large)
        );
        assert_eq!(
            cache.lookup(&file, ThumbnailSize::XLarge),
            None,
            "no smaller one stands in"
        );

        // The PNG carries the spec's keys and the pixels.
        let decoder = png::Decoder::new(std::io::BufReader::new(
            std::fs::File::open(&stored).unwrap(),
        ));
        let mut reader = decoder.read_info().unwrap();
        let text: Vec<(String, String)> = reader
            .info()
            .uncompressed_latin1_text
            .iter()
            .map(|chunk| (chunk.keyword.clone(), chunk.text.clone()))
            .collect();
        assert!(text.contains(&("Thumb::URI".into(), file_uri(&file))));
        assert!(text.contains(&("Thumb::MTime".into(), "1700000000".into())));
        assert!(text.contains(&("Thumb::Size".into(), "17".into())));
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        assert_eq!(pixels, PIXELS);

        // The file is edited: the thumbnail is stale.
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_100))
            .unwrap();
        assert_eq!(cache.lookup(&file, ThumbnailSize::Normal), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    // a_thumbnail_of_another_file_at_the_name_is_not_used
    {
        let root = scratch("other");
        let file = picture(&root);
        // Another file, modified at the same second: only the URI tells them apart.
        let other = root.join("other.jpg");
        std::fs::write(&other, b"x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
        let cache = ThumbnailCache::new(root.join("thumbnails"));
        let stored = cache
            .store(&other, ThumbnailSize::Normal, image(), "enzUI test")
            .unwrap();
        // Put the other file's thumbnail where this file's would be.
        let ours = cache.path_for(&file_uri(&file), ThumbnailSize::Normal);
        std::fs::rename(stored, &ours).unwrap();
        assert_eq!(cache.lookup(&file, ThumbnailSize::Normal), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    // a_failure_is_remembered_until_the_file_changes
    {
        let root = scratch("fail");
        let file = picture(&root);
        let cache = ThumbnailCache::new(root.join("thumbnails"));
        assert!(!cache.has_failed(&file, "enzui-notes"));
        cache.record_failure(&file, "enzui-notes").unwrap();
        assert!(cache.has_failed(&file, "enzui-notes"));
        assert!(!cache.has_failed(&file, "another-app"), "per application");
        std::fs::write(&file, b"now a real picture").unwrap();
        assert!(!cache.has_failed(&file, "enzui-notes"), "the file changed");
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn thumbnail_cache_validates_identity_freshness_limits_and_atomic_storage_2() {
    // pixels_that_do_not_fit_the_size_are_refused
    {
        let root = scratch("size");
        let file = picture(&root);
        let cache = ThumbnailCache::new(root.join("thumbnails"));
        let wrong = ThumbnailImage {
            width: 3,
            height: 2,
            rgba: &PIXELS,
        };
        assert!(
            cache
                .store(&file, ThumbnailSize::Normal, wrong, "x")
                .is_err()
        );
        assert_eq!(ThumbnailSize::for_pixels(200), ThumbnailSize::Large);
        assert_eq!(ThumbnailSize::for_pixels(4000), ThumbnailSize::XXLarge);
        let _ = std::fs::remove_dir_all(&root);
    }

    // a_same_second_rewrite_of_another_length_is_stale
    {
        let root = scratch("length");
        let file = picture(&root);
        let cache = ThumbnailCache::new(root.join("thumbnails"));
        let _ = cache
            .store(&file, ThumbnailSize::Normal, image(), "enzUI test")
            .unwrap();
        assert!(cache.lookup(&file, ThumbnailSize::Normal).is_some());

        std::fs::write(&file, b"a longer body, saved in the same second").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
        assert_eq!(cache.lookup(&file, ThumbnailSize::Normal), None);
        let _ = cache.record_failure(&file, "enzui-test");
        assert!(cache.has_failed(&file, "enzui-test"));
    }

    // storing_ignores_planted_links_and_leaves_no_temporaries
    {
        let root = scratch("links");
        let file = picture(&root);
        let cache = ThumbnailCache::new(root.join("thumbnails"));
        let target = cache.path_for(&file_uri(&file), ThumbnailSize::Normal);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        let victim = root.join("victim");
        std::fs::write(&victim, b"keep me").unwrap();
        let name = target.file_name().unwrap().to_string_lossy().into_owned();
        let planted = target.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &planted).unwrap();

        let _ = cache
            .store(&file, ThumbnailSize::Normal, image(), "enzUI test")
            .unwrap();
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
        let mut left: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        left.sort();
        let mut expected = vec![target, planted];
        expected.sort();
        assert_eq!(left, expected, "only the thumbnail and the planted link");
    }
}
