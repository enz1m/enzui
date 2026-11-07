//! Tests for the handle cache and the MIME/theme resolution cache.

use super::*;
use std::path::Path;

fn fp(v: u64) -> Fingerprint {
    Fingerprint::new(v)
}

fn handle(id: u64) -> ResolvedHandle {
    ResolvedHandle::new(id, format!("mime/{id}"), format!("icon-{id}"))
}

fn rk(theme: &str, size: u16, scale: u16) -> ResolutionKey {
    ResolutionKey::of(theme, size, scale)
}

#[test]
fn each_resolution_input_is_part_of_the_key() {
    // Each input must invalidate independently: a key folding in only the theme
    // would pass the theme case and fail the other two.
    let base = rk("Adwaita", 48, 1);
    for (label, other) in [
        ("theme", rk("hicolor", 48, 1)),
        ("icon size", rk("Adwaita", 32, 1)),
        ("scale", rk("Adwaita", 48, 2)),
    ] {
        assert_ne!(base, other, "{label} must be part of the resolution key");

        let mut cache = HandleCache::new(16);
        let p = Path::new("/tmp/each.png");
        let f = fp(9);
        cache.set_resolution_key(base);
        cache.mark_in_flight(p);
        let g0 = cache.generation();
        cache.complete(p, g0, f.clone(), handle(1));
        cache.set_resolution_key(other);
        assert_eq!(
            cache.lookup(p, &f),
            HandleLookup::Miss,
            "a {label} change must invalidate the warm handle"
        );
    }
}

#[test]
fn only_a_marked_dispatch_completes_and_only_once() {
    let mut cache = HandleCache::new(16);
    let p = Path::new("/tmp/once.png");
    let f = fp(3);
    let g = cache.generation();
    assert!(
        !cache.complete(p, g, f.clone(), handle(1)),
        "never dispatched"
    );
    assert_eq!(cache.lookup(p, &f), HandleLookup::Miss);

    cache.set_resolution_key(rk("Adwaita", 48, 1));
    cache.mark_in_flight(p);
    assert!(cache.complete(p, g, f.clone(), handle(2)));
    assert!(!cache.complete(p, g, f.clone(), handle(3)), "a duplicate");
    assert_eq!(cache.lookup(p, &f), HandleLookup::Hit(handle(2)));

    // A replay after the inputs changed must not be stamped with the new key.
    cache.set_resolution_key(rk("hicolor", 48, 1));
    assert!(!cache.complete(p, g, f.clone(), handle(4)));
    assert_eq!(cache.lookup(p, &f), HandleLookup::Miss);
}
