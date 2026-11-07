//! Identity-bound cleanup under a bounded two-writer race.

use super::*;

/// A distinct, valid body a replacing writer publishes; never equal to [`BODY`].
const REPLACEMENT_BODY: &[u8] = b"\x89PNG\r\n\x1a\nnewer-replacement-body-2886";

/// Publish `bytes` at `path` as a new inode, modelling a second writer's atomic
/// `rename(2)` replacement.
fn replace_entry_atomically(path: &std::path::Path, bytes: &[u8]) {
    let tmp = path.with_extension("race-replacement");
    fs::write(&tmp, bytes).expect("stage the replacement bytes");
    fs::rename(&tmp, path).expect("atomically publish the replacement inode");
}

/// Valid on-disk bytes for a plain non-varying `GET` of `url`, produced by a
/// real `store` into a throwaway cache.
fn valid_base_entry_bytes(url: &str, body: &[u8], freshness: &Freshness) -> Vec<u8> {
    let scratch = TempRoot::new("race-base-src");
    let cache = cache_at(&scratch);
    assert!(
        cache.store(url, body, freshness, NO_REQ),
        "the replacement source store must succeed"
    );
    let path = cache.path_for(url).expect("replacement entry path");
    strict_read(&path)
}

/// A load that finds a corrupt inode cleans it up by identity: a newer entry
/// published between the read and the unlink is left intact.
#[test]
fn corrupt_load_cleanup_spares_a_concurrent_replacement() {
    let root = TempRoot::new("corrupt-load-race");
    let cache = cache_at(&root);

    assert!(cache.store(URL, BODY, &fresh_for(3600), NO_REQ));
    let path = cache.path_for(URL).expect("entry path");
    // Corrupting in place keeps the inode the load opens, so the miss and the
    // cleanup decision are about the inode the seam then replaces.
    let mut corrupt = strict_read(&path);
    let last = corrupt.len() - 1;
    corrupt[last] ^= 0xFF;
    fs::write(&path, &corrupt).expect("corrupt the body in place");

    let replacement = valid_base_entry_bytes(URL, REPLACEMENT_BODY, &fresh_for(3600));
    let seam_path = path.clone();
    let seam_bytes = replacement.clone();
    let _seam = cleanup_race::arm_corrupt(Box::new(move || {
        replace_entry_atomically(&seam_path, &seam_bytes);
    }));

    assert!(
        cache.get(URL, NO_REQ).is_none(),
        "the corrupt inode we read must be a miss on this pass"
    );
    assert!(
        !cleanup_race::armed(),
        "the seam must have fired inside the cleanup window, not leaked to a sibling test"
    );
    assert!(
        path.exists(),
        "the concurrent replacement must survive corrupt-load cleanup"
    );
    assert_eq!(
        strict_read(&path),
        replacement,
        "the replacement's bytes must be untouched by the identity-bound unlink"
    );
    let served = cache
        .get(URL, NO_REQ)
        .expect("the surviving replacement is a real, servable entry");
    assert_eq!(
        served.body, REPLACEMENT_BODY,
        "a second load serves the newer body the concurrent writer published"
    );
}
