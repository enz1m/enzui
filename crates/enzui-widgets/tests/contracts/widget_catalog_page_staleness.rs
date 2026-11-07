use super::widget_catalog_page::*;

// ---------------------------------------------------------------------------
// Test 1 — the staleness gate
// ---------------------------------------------------------------------------

#[test]
fn docs_widgets_md_is_not_stale() {
    let model = read_model();
    let expected = render(&model);
    let path = widgets_md_path();

    if std::env::var_os("ENZUI_WRITE_WIDGET_DOC").is_some() {
        std::fs::write(&path, &expected).unwrap_or_else(|e| panic!("write {path:?}: {e}"));
        eprintln!("wrote {} ({} bytes)", path.display(), expected.len());
        // The JSON index is emitted by the same env var from the
        // same model, so `widgets.md` and `widgets.json` are always written
        // together and cannot fall out of step.
        let json = render_json(&model);
        let json_path = widgets_json_path();
        std::fs::write(&json_path, &json).unwrap_or_else(|e| panic!("write {json_path:?}: {e}"));
        eprintln!("wrote {} ({} bytes)", json_path.display(), json.len());
        return;
    }

    let actual = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}). It is generated; create it with\n  \
             ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts widget_catalog_page",
            path.display()
        )
    });

    if actual == expected {
        return;
    }

    // Report the first differing line, and the size of the drift, rather than
    // dumping two 200-line documents.
    let a: Vec<&str> = actual.lines().collect();
    let e: Vec<&str> = expected.lines().collect();
    let first = (0..a.len().max(e.len()))
        .find(|i| a.get(*i) != e.get(*i))
        .unwrap_or(0);
    panic!(
        "docs/widgets.md is STALE — it disagrees with the § Widget catalog in \
         crates/enzui-widgets/src/lib.rs.\n\n\
         Being generated once is not the same as being generated currently: \
         without this check the file rots exactly like the hand-typed list it \
         replaces.\n\n\
         First difference at line {}:\n  \
         on disk:   {:?}\n  \
         generated: {:?}\n\n\
         ({} lines on disk, {} generated.)\n\n\
         If you edited the catalog block in lib.rs, regenerate:\n  \
         ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts \
         widget_catalog_page\n\
         If you edited docs/widgets.md by hand, move the edit into the lib.rs \
         catalog block instead — the markdown is generated output and hand \
         edits are overwritten.",
        first + 1,
        a.get(first).unwrap_or(&"<end of file>"),
        e.get(first).unwrap_or(&"<end of file>"),
        a.len(),
        e.len(),
    );
}

// ---------------------------------------------------------------------------
// docs/widgets.json drift + validity gates
// ---------------------------------------------------------------------------

/// `docs/widgets.json` must be exactly what the generator emits from the live
/// catalog model — the same guarantee `docs_widgets_md_is_not_stale` gives the
/// markdown. Because both are rendered from the one [`read_model`], a hand-edit
/// of the JSON, or a catalog change that was regenerated into the `.md` but not
/// the `.json`, reddens here.
#[test]
fn docs_widgets_json_is_not_stale() {
    let model = read_model();
    let expected = render_json(&model);
    let path = widgets_json_path();

    if std::env::var_os("ENZUI_WRITE_WIDGET_DOC").is_some() {
        // Written by docs_widgets_md_is_not_stale in the same run; nothing to
        // assert in write mode.
        return;
    }

    let actual = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}). It is generated alongside docs/widgets.md; \
             create it with\n  ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts \
             widget_catalog_page",
            path.display()
        )
    });

    if actual == expected {
        return;
    }
    let a: Vec<&str> = actual.lines().collect();
    let e: Vec<&str> = expected.lines().collect();
    let first = (0..a.len().max(e.len()))
        .find(|i| a.get(*i) != e.get(*i))
        .unwrap_or(0);
    panic!(
        "docs/widgets.json is STALE — it disagrees with the catalog model in \
         crates/enzui-widgets/src/lib.rs.\n\n\
         It is generated from the SAME source as docs/widgets.md; if you \
         changed the catalog, regenerate BOTH with\n  \
         ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts \
         widget_catalog_page\n\
         If you hand-edited docs/widgets.json, move the edit into the lib.rs \
         catalog block instead.\n\n\
         First difference at line {}:\n  on disk:   {:?}\n  generated: {:?}\n\n\
         ({} lines on disk, {} generated.)",
        first + 1,
        a.get(first).unwrap_or(&"<end of file>"),
        e.get(first).unwrap_or(&"<end of file>"),
        a.len(),
        e.len(),
    );
}

// ---------------------------------------------------------------------------
// Catalog links must resolve on a fresh clone
// ---------------------------------------------------------------------------

/// Every source-relative link the generator emits into `docs/widgets.md` must
/// point at a file that exists in the repo. `../target/doc/...` links are dead
/// until someone runs `cargo doc`; this test makes a dead catalog
/// link a red build, so a module rename that moves its source file reddens
/// here instead of silently rotting the catalog.
#[test]
fn docs_widgets_links_resolve() {
    let md = std::fs::read_to_string(widgets_md_path())
        .unwrap_or_else(|e| panic!("read docs/widgets.md: {e}"));

    // Collect every `](../...)` link target that points into the source tree.
    // The markdown lives in `docs/`, so a link `../crates/...` resolves
    // relative to `docs/`, i.e. against the workspace root.
    let root = workspace_root();
    let mut checked = 0usize;
    let mut from = 0usize;
    let needle = "](../";
    while let Some(rel) = md[from..].find(needle) {
        let at = from + rel + needle.len();
        from = at;
        let tail = &md[at..];
        let Some(paren) = tail.find(')') else {
            continue;
        };
        let link = &tail[..paren]; // e.g. `crates/enzui-widgets/src/button.rs`
        // Skip anchors and non-source links (building-an-app.md is a sibling
        // in docs/, linked without `../`, so it will not appear here).
        let path_part = link.split('#').next().unwrap_or(link);
        // Only assert links that reach into the repo tree (crates/ or docs/);
        // an external http link, if one ever appears, is out of scope here.
        if !path_part.starts_with("crates/") && !path_part.starts_with("docs/") {
            continue;
        }
        let target = root.join(path_part);
        assert!(
            target.exists(),
            "docs/widgets.md links to `../{path_part}`, which does not exist \
             at {}. A catalog link that resolves to nothing is a dead entry \
             on the discovery page. If a module was renamed, the generator's \
             module_source_rel picked a stale path; regenerate the catalog.",
            target.display()
        );
        checked += 1;
    }

    assert!(
        checked > 100,
        "only {checked} source links were checked in docs/widgets.md — either \
         the file is missing or the link form changed and this test now \
         matches nothing (a silent pass). The generator emits one source link \
         per tier-1 module, so this floor must be in the hundreds."
    );
}
