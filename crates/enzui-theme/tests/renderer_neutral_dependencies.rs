//! The renderer-neutral dependency boundary: the token crate must stay renderer-neutral.
//!
//! `enzui-theme` is the foundational design-token crate. Depending on
//! `iced` would pull the **full iced facade plus the default renderer**
//! (`iced_wgpu`, `iced_winit`, `wgpu`, `winit`, `accesskit`, `arboard`),
//! giving a crate of colour constants a dependency graph of hundreds of
//! packages, and every consumer of a pure token would rebuild on any renderer
//! change.
//!
//! That matters here because compile memory is this workspace's tightest
//! limit, and a token crate that drags in wgpu would put the whole renderer
//! behind every crate that only needs a colour.
//!
//! The crate depends on `iced_core` directly, with the two OS
//! colour-scheme entry points (`current_system_theme`,
//! `system_theme_subscription`) gated behind a default-on `system-theme`
//! feature that adds only `iced_runtime` + `iced_futures` — both themselves
//! renderer-agnostic.
//!
//! # Why this is a test and not a comment
//!
//! The regression is invisible to every other check in the repo: adding
//! `iced.workspace = true` to `Cargo.toml` compiles fine, passes every
//! unit test, and changes no observable behaviour. It only shows up as build
//! time and build memory. A comment saying "do not depend on the facade"
//! cannot fail; this can.
//!
//! # What it screens for
//!
//! The manifest, not the resolved graph. `cargo tree` would be the stronger
//! probe, but it is not available from inside a test without shelling out to
//! cargo (which deadlocks on the package-cache lock while a test binary that
//! cargo itself launched is running). The manifest is where the regression
//! would be *written*, so it is the right thing to pin: any route back to the
//! renderer has to be spelled here first.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use std::path::Path;

/// Read this crate's own `Cargo.toml`.
fn manifest() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Only the `[dependencies]` table, so the prose in `[features]` comments
/// (which legitimately names `iced_runtime`, wgpu, etc.) cannot trip the
/// screen. Without this the check would report false positives, the usual
/// failure of name-based sweeps.
fn dependencies_table(manifest: &str) -> String {
    let mut out = String::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_deps = t == "[dependencies]";
            continue;
        }
        if in_deps {
            // Strip trailing comments; a `#` inside a dep line is prose.
            let code = t.split('#').next().unwrap_or("").trim();
            if !code.is_empty() {
                out.push_str(code);
                out.push('\n');
            }
        }
    }
    out
}

/// The dependency name on the left of `=`, for one manifest line.
fn dep_name(line: &str) -> &str {
    let lhs = line.split('=').next().unwrap_or("").trim();
    // `iced_runtime = { ... }` and `iced_core.workspace = true` both reduce to
    // the crate name by taking everything before the first `.`.
    lhs.split('.').next().unwrap_or("").trim()
}

#[test]
fn token_crate_does_not_depend_on_the_iced_facade() {
    let deps = dependencies_table(&manifest());
    let names: Vec<&str> = deps.lines().map(dep_name).collect();

    // An empty dependency table would make the absence check vacuous.
    assert!(
        names.contains(&"iced_core"),
        "parser calibration failed: `iced_core` is declared in [dependencies] \
         but the screen did not see it, so the assertions above are vacuous. \
         Parsed: {names:?}"
    );

    assert!(
        !names.contains(&"iced"),
        "`enzui-theme` must not depend on the `iced` facade: it pulls the \
         default renderer (iced_wgpu/wgpu) and the windowing + accessibility \
         stack (iced_winit/winit/accesskit/arboard) into a design-token crate, \
         taking the graph from 33 packages to 281. Every type this crate uses \
         (Color, Theme, theme::Mode, theme::palette::*, Shadow, time::Duration) \
         lives in `iced_core`. Found dependencies: {names:?}"
    );
}
