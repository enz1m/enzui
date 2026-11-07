//! Table-driven tests for shared-mime-info glob precedence, certainty, and the
//! generic-icon fallback chain.
//!
//! The precedence tables are built so that a naive "first glob that matches
//! wins" implementation gives the wrong answer, which is what the ordering rules
//! are for. Literal priority is pinned by
//! [`literal_beats_longer_higher_weight_glob`], pattern length by
//! [`longer_suffix_beats_shorter`], the case-sensitivity tiebreak by
//! [`case_sensitive_c_and_cpp_are_distinct`], and
//! [`order_of_rules_in_table_does_not_change_result`] pins that precedence
//! rather than table order decides.

// ---- Certainty type ----

// ---- Icon candidate chain ----

// ==============================================================
// The bundled drawing path must never yield an unbundled name.
//
// `icon_candidates` returns the full freedesktop chain, whose early names
// (`image-png`, `text-x-generic`) are host-theme names and are absent from the
// bundled Lucide catalog. A consumer with no host theme must therefore use
// `bundled_icon_candidate`, which walks the chain to the first name
// `bundled_icon()` resolves. This test pins the class: any MIME class, or any
// new candidate branch, whose chain contains no bundled name fails here.
// ==============================================================

// ---------------------------------------------------------------------------
// globs2 parser
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// XDG discovery (mime::xdg) — layer 1 (pure path computation) and layer 2
// (thin I/O + search-order merge). See mime/xdg.rs for the design rationale.
// ---------------------------------------------------------------------------
mod xdg_tests {
    use super::super::xdg::load_globs2_from;

    use std::path::PathBuf;

    // ---- Layer 1: pure candidate-path computation -------------------------

    // ---- Relative-path rejection (spec: paths must be absolute) -----------

    // ---- Layer 2: thin I/O + search-order merge ---------------------------

    // A unique temp directory under the OS temp dir, cleaned up on drop. The
    // crate has no `tempfile` dev-dependency, so this is a small hand-rolled
    // version mirroring the one in `sniff/tests.rs`.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut base = std::env::temp_dir();
            let uniq = format!(
                "enzui-xdg-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            base.push(uniq);
            std::fs::create_dir_all(&base).expect("create temp dir");
            Self { path: base }
        }

        // Write a globs2 file `<dir>/<name>` and return its path.
        fn write(&self, name: &str, contents: &str) -> PathBuf {
            let p = self.path.join(name);
            std::fs::write(&p, contents).expect("write globs2 fixture");
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    // Calibration: the low-priority rule is actually chosen when it is the only
    // file present, before asserting that it gets overridden.

    #[test]
    fn rejected_counts_accumulate_across_files() {
        let dir = TempDir::new("reject-sum");
        let a = dir.write("a", "50:text/x-a:*.a\nbad-1\n");
        let b = dir.write("b", "50:text/x-b:*.b\nbad-2\nbad-3\n");
        let (parsed, files) = load_globs2_from(&[a, b]);
        assert_eq!(files, 2);
        assert_eq!(parsed.rules.len(), 2);
        assert_eq!(parsed.rejected, 3, "1 + 2 rejects summed across files");
    }
}

// ---------------------------------------------------------------------------
// GlobTable::resolve_ambiguous preserves equal-precedence alternatives
// instead of collapsing to the single tiebreak winner.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Resolution must not depend on the order rules were read
// ---------------------------------------------------------------------------

// The XDG snapshot must be byte-faithful: `$HOME`, `$XDG_DATA_HOME` and
// `$XDG_DATA_DIRS` are unix paths, that is bytes, and a `std::env::var(k).ok()`
// snapshot turns any non-UTF-8 value into "unset". That silent loss has three
// distinct consequences on the search order, each pinned below against a UTF-8
// positive control that shows the assertion is not vacuous.
