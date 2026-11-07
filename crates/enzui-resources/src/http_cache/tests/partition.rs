//! The complete request partition is one hashed identity, and no request-header
//! value ever reaches disk.

use super::*;

fn numbered_vary_names(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("x-vary-{index:02}"))
        .collect()
}

/// Revalidation applies the same 64-name production ceiling before rewriting an
/// existing file.
#[test]
fn revalidation_refuses_65_vary_names_before_rewriting() {
    let root = TempRoot::new("vary-revalidation-over-cap");
    let cache = cache_at(&root);
    let over_cap = Freshness {
        max_age: Some(3600),
        vary: numbered_vary_names(MAX_VARY_NAMES + 1),
        ..Freshness::default()
    };
    let material = canonical_material(&CacheKey::from(URL), &over_cap.vary, NO_REQ);
    let name = entry_filename_material(&material);
    let seeded = encode_entry(&StoredEntry {
        identity: identity_digest_material(&material),
        display_url: URL.to_owned(),
        body: BODY.to_vec(),
        etag: Some("\"seed\"".to_owned()),
        last_modified: None,
        lifetime: Some(1),
        initial_age: 0,
        no_cache: false,
        // Keep the crafted entry decodable so only the production input gate
        // prevents the rewrite; decode's independent 64-name defense remains.
        vary_names: Vec::new(),
        stored_at: SystemTime::now(),
    });
    fs::write(root.path().join(name), seeded).expect("seed over-cap key fixture");

    let window = disk_write_window();
    let writes_before = window.writes();
    assert!(!cache.mark_revalidated(URL, &over_cap, NO_REQ));
    assert_eq!(
        window.writes(),
        writes_before,
        "over-cap revalidation must be refused before an atomic rewrite"
    );
    drop(window);
}
