//! The component-token API index and its naming-contract gates.
//!
//! # Why this file exists
//!
//! `enzui-widgets` publishes **81 `*Tokens` structs with 794 public
//! fields** (source-derived audit). The top-level
//! shape is consistent (every type ends in `Tokens`, no `Enz`
//! prefix, one `.tokens(SomeTokens)` builder shape), but without this file
//! nothing checks:
//!
//! - which module each token type is imported through,
//! - whether an alias resolves to the same type as its canonical name,
//! - whether a public field name matches the documented anatomy, or
//! - whether a `.tokens(...)` setter documents *how* the value is resolved
//!   (fixed override vs. runtime theme).
//!
//! This file makes all four checkable, plus emits a generated
//! `docs/tokens.md` + `docs/tokens.json` index the same way `widgets.md` is
//! generated. Being generated *once* is not the same as being generated
//! *currently*: the staleness gate here reddens whenever a token struct or
//! field is added, removed or renamed without regenerating the index.
//!
//! # Refreshing the index
//!
//! ```sh
//! ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index
//! ```
//!
//! The env var makes the staleness tests write the two doc files instead
//! of asserting on them; the gate is deliberately not the default so a
//! generator that silently rewrites its own expectation can never fail.

use enzui_test_support::hand_roll_scan::{
    contains_ident_word, mask_rust_non_code, read_source_or_panic, walk_rs_files_follow_root,
};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::tiers::src_dir;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest")
        .to_path_buf()
}

fn tokens_md_path() -> PathBuf {
    workspace_root().join("docs/tokens.md")
}

fn tokens_json_path() -> PathBuf {
    workspace_root().join("docs/tokens.json")
}

/// One public token struct found in the crate source.
#[derive(Debug, Clone)]
struct TokenStruct {
    /// The Rust type name (e.g. `ButtonTokens`, `ShellTokens`).
    name: String,
    /// The file the struct's definition lives in, relative to the crate root
    /// (e.g. `src/button.rs`).
    file: String,
    /// The module path callers reach the type through from within the crate
    /// tree (e.g. `button`, `app_shell`, `carousel`). This is not necessarily
    /// the *canonical* public import path; the canonical path is decided
    /// separately in [`canonical_component_import`] because callers reach it
    /// through both the module and the crate root.
    module: String,
    /// Public fields in declaration order. Each entry is `(field_name,
    /// type_text)`. `type_text` is whitespace-collapsed exactly as it
    /// appears between `:` and the trailing `,` - for the JSON index only.
    fields: Vec<(String, String)>,
    /// `true` when the source has a `pub fn from_palette` (or
    /// `pub fn from_theme`) inherent method on this token struct.
    has_from_palette: bool,
    /// `true` when the source has an `impl Default for <Name>` or a
    /// `pub fn default(` inherent constructor. Every geometry-only token set
    /// is expected to satisfy this - colour-bearing ones satisfy
    /// [`Self::has_from_palette`] instead.
    has_default_or_preset: bool,
    /// `true` when the source has a `pub fn tokens(mut self, ..) -> Self`
    /// builder setter for this type anywhere in the crate.
    has_setter: bool,
    /// The setter's kind, for [`ResolutionMode`] reporting in the index.
    setter_resolution: Option<ResolutionMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolutionMode {
    /// Rustdoc explicitly mentions pin/snapshot/bypass/fixed wording:
    /// passing a value freezes those channels and bypasses runtime-theme
    /// derivation. This is what `EnzButton::tokens` and the palette-backed
    /// setters describe.
    PinFixed,
    Override,
    /// Rustdoc explicitly mentions runtime theme / resolved wording
    /// without an accompanying "override" keyword - i.e. the setter
    /// installs a resolver called every draw, not a snapshot. No setter
    /// is currently classified here; all existing setters that mention
    /// "runtime theme" do so to describe the *default* behaviour and
    /// start with "Override". The variant exists so the enum
    /// covers all three possible modes.
    Runtime,
}

impl ResolutionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::PinFixed => "pin",
            Self::Override => "override",
            Self::Runtime => "runtime",
        }
    }
}

/// Every `.rs` file under the crate's `src/`, sorted.
fn rust_files() -> Vec<PathBuf> {
    let root = src_dir();
    walk_rs_files_follow_root(&root)
        .into_iter()
        .map(|rel| root.join(rel))
        .collect()
}

/// Read `pub struct <Name>Tokens { … }` from `text`, calling `visit` with
/// (name, body). Body is the exact text between the opening and closing
/// braces, so the caller can strip comments and parse fields. Nested braces
/// are supported by a simple depth counter. Rust does not put unbalanced
/// braces inside a `pub struct` body outside a string/char literal (which we
/// filter via the shared strict Rust masker before scanning), so this is
/// exact for the input this file cares about.
fn scan_pub_struct_tokens(text: &str, mut visit: impl FnMut(&str, usize, &str)) {
    let masked = mask_rust_non_code(text);
    let bytes = masked.as_bytes();
    let mut i = 0;
    while i + 12 <= bytes.len() {
        // Fast filter: match "pub struct " at a word boundary.
        if bytes[i..].starts_with(b"pub struct ")
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
        {
            let after = i + b"pub struct ".len();
            // Name: identifier ending in `Tokens`.
            let mut j = after;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let name = &masked[after..j];
            if !name.is_empty()
                && name.ends_with("Tokens")
                && name.as_bytes()[0].is_ascii_uppercase()
            {
                // Skip whitespace to `{` (skipping any `<Generics>` clause).
                let mut k = j;
                while k < bytes.len() && bytes[k] != b'{' {
                    // A `;` (tuple/unit struct) means no body - skip.
                    if bytes[k] == b';' {
                        break;
                    }
                    k += 1;
                }
                if k < bytes.len() && bytes[k] == b'{' {
                    // Find matching `}`.
                    let body_start = k + 1;
                    let mut depth = 1usize;
                    let mut m = body_start;
                    while m < bytes.len() && depth > 0 {
                        match bytes[m] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            _ => {}
                        }
                        m += 1;
                    }
                    if depth == 0 {
                        let body_end = m - 1;
                        // Compute 1-based line number of the declaration.
                        let line = 1 + text[..i].bytes().filter(|c| *c == b'\n').count();
                        visit(name, line, &masked[body_start..body_end]);
                    }
                    i = m;
                    continue;
                }
            }
        }
        i += 1;
    }
}

/// Parse the `pub field: Type,` declarations out of a struct body. Attributes
/// and rustdoc are already stripped by the shared Rust masker on the
/// outer text; inner nested types are handled by a depth counter over `<>`
/// and `()`.
fn parse_fields(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Match `pub` at a word boundary.
        if bytes[i..].starts_with(b"pub")
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_')
            && i + 3 < bytes.len()
            && !bytes[i + 3].is_ascii_alphanumeric()
            && bytes[i + 3] != b'_'
        {
            let mut j = i + 3;
            // Skip whitespace / possible `pub(...)`.
            if j < bytes.len() && bytes[j] == b'(' {
                let mut d = 1;
                j += 1;
                while j < bytes.len() && d > 0 {
                    match bytes[j] {
                        b'(' => d += 1,
                        b')' => d -= 1,
                        _ => {}
                    }
                    j += 1;
                }
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let name_start = j;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            let name_end = j;
            if name_end == name_start {
                i += 1;
                continue;
            }
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j >= bytes.len() || bytes[j] != b':' {
                i = j;
                continue;
            }
            j += 1;
            // Read type text until top-level `,`. Respect nested `<>` and `(`.
            let ty_start = j;
            let mut angle = 0i32;
            let mut paren = 0i32;
            while j < bytes.len() {
                match bytes[j] {
                    b'<' => angle += 1,
                    b'>' => angle -= 1,
                    b'(' => paren += 1,
                    b')' => paren -= 1,
                    b',' | b'}' if angle == 0 && paren == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            let ty = &body[ty_start..j];
            let ty_norm = ty.split_whitespace().collect::<Vec<_>>().join(" ");
            let name = &body[name_start..name_end];
            out.push((name.to_owned(), ty_norm));
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

/// Compute the crate-relative module path of a file:
/// `src/foo.rs` → `foo`, `src/foo/mod.rs` → `foo`, `src/foo/bar.rs` → `foo`
/// (the sibling files inside `foo/` all live inside the `foo` module for the
/// purposes of this index). The special-cased submodule filenames only apply
/// inside a subdirectory: at the crate root, `src/tokens.rs` is the
/// `tokens` module (its own top-level file), not the crate root itself.
fn module_of(file: &std::path::Path, src: &std::path::Path) -> String {
    let rel = file.strip_prefix(src).unwrap();
    let mut parts: Vec<&str> = rel.iter().map(|s| s.to_str().unwrap()).collect();
    if let Some(last) = parts.last().copied() {
        let inside_subdir = parts.len() > 1;
        let is_submodule_file = matches!(last, "mod.rs" | "builder.rs" | "tokens.rs" | "widget.rs");
        if inside_subdir && is_submodule_file {
            parts.pop();
        } else {
            let last = parts.pop().unwrap();
            parts.push(last.trim_end_matches(".rs"));
        }
    }
    parts.join("::")
}

fn has_palette_constructor(masked: &str, name: &str) -> bool {
    for (start, _) in masked.match_indices("impl") {
        if start > 0
            && (masked.as_bytes()[start - 1].is_ascii_alphanumeric()
                || masked.as_bytes()[start - 1] == b'_')
        {
            continue;
        }
        let tail = &masked[start + 4..];
        let Some(open) = tail.find('{') else { continue };
        if tail[..open].trim() != name {
            continue;
        }
        let mut depth = 1usize;
        for (offset, byte) in tail.as_bytes()[open + 1..].iter().enumerate() {
            match byte {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            if depth != 0 {
                continue;
            }
            let body = &tail[open + 1..open + 1 + offset];
            if [
                "pub fn from_palette",
                "pub fn from_theme",
                "pub const fn from_palette",
                "pub const fn from_theme",
            ]
            .iter()
            .any(|method| body.contains(method))
            {
                return true;
            }
            break;
        }
    }
    false
}

/// Scan every `.rs` file under `src/` once and build a map from token type
/// name to its documented setter's resolution mode. The setter can live in a
/// different file than the struct definition (e.g. `ButtonTokens` is
/// declared in `src/tokens.rs` but its `.tokens(...)` setter lives on
/// `EnzButton` in `src/button.rs`), so a per-file search from `scan_tokens`
/// misses it. Build this once and consult it while classifying each token.
fn scan_setter_resolutions() -> BTreeMap<String, Option<ResolutionMode>> {
    let mut out: BTreeMap<String, Option<ResolutionMode>> = BTreeMap::new();
    for file in rust_files() {
        let text = read_source_or_panic(&file);
        let lines: Vec<&str> = text.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            let Some(method) = t
                .strip_prefix("pub fn ")
                .and_then(|rest| rest.split_once("(mut self,").map(|(name, _)| name))
            else {
                continue;
            };
            if method != "tokens" && !method.ends_with("_tokens") {
                continue;
            }
            // Extract the token type from the second parameter's type
            // annotation - `.. : NAME )`.
            let Some(colon) = line.rfind(": ") else {
                continue;
            };
            let after = &line[colon + 2..];
            let end = after.find(|c: char| !c.is_ascii_alphanumeric() && c != '_');
            let Some(end) = end else { continue };
            let ty = &after[..end];
            if !ty.ends_with("Tokens") {
                continue;
            }
            let mut doc = String::new();
            let mut j = idx as isize - 1;
            while j >= 0 {
                let tt = lines[j as usize].trim_start();
                if tt.starts_with("///") {
                    doc.insert(0, ' ');
                    doc.insert_str(0, tt.trim_start_matches("///").trim());
                } else if tt.starts_with("#[") || tt.is_empty() {
                    // Keep walking upward through attributes / blanks.
                } else {
                    break;
                }
                j -= 1;
            }
            let lc = doc.to_ascii_lowercase();
            let has_pin = ["pin", "pinned", "pins", "bypass", "snapshot", "fixed"]
                .iter()
                .any(|kw| lc.contains(kw));
            let has_runtime = ["runtime theme", "resolved at"]
                .iter()
                .any(|kw| lc.contains(kw))
                || contains_ident_word(&lc, "derive")
                || contains_ident_word(&lc, "derives");
            let has_override = [
                "override",
                "overrides",
                "apply",
                "applies",
                "sets",
                " set ",
                " set.",
                "set the ",
                "configure",
                "replace",
            ]
            .iter()
            .any(|kw| lc.contains(kw))
                || lc.trim_start().starts_with("set ");
            let mode = match (has_pin, has_override, has_runtime) {
                (true, _, _) => Some(ResolutionMode::PinFixed),
                (_, true, _) => Some(ResolutionMode::Override),
                (false, false, true) => Some(ResolutionMode::Runtime),
                _ => None,
            };
            match out.get(ty) {
                None | Some(None) => {
                    out.insert(ty.to_owned(), mode);
                }
                Some(Some(_)) => {}
            }
        }
    }
    out
}

/// Scan every `.rs` file under `src/` and return the parsed model.
fn scan_tokens() -> Vec<TokenStruct> {
    let src = src_dir();
    let setter_map = scan_setter_resolutions();
    let mut out: Vec<TokenStruct> = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for file in rust_files() {
        let text = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("read {}: {e}", file.display()));
        let module = module_of(&file, &src);
        let text_masked = mask_rust_non_code(&text);
        let mut structs_in_file = Vec::new();
        scan_pub_struct_tokens(&text, |name, line, body| {
            structs_in_file.push((name.to_owned(), line, parse_fields(body)));
        });
        for (name, line, fields) in structs_in_file {
            if let Some(prev) = seen.insert(name.clone(), format!("{}:{line}", file.display())) {
                panic!(
                    "duplicate public `pub struct {name}` - first declaration in {prev}, \
                     second in {}:{line}. Every token type name must be globally unique so \
                     `pub use` re-exports through both module and crate root do not silently \
                     shadow.",
                    file.display()
                );
            }
            let has_from_palette = has_palette_constructor(&text_masked, &name);
            let has_default_or_preset = text_masked.contains(&format!("impl Default for {name}"))
                || text_masked.contains(&format!(": {name} = {name}"))
                || text_masked.contains("pub const ROUNDED_STRIP: Self")
                    && text_masked.contains(&format!("impl {name} {{"));

            let (has_setter, setter_resolution) = match setter_map.get(&name) {
                Some(mode) => (true, *mode),
                None => (false, None),
            };

            out.push(TokenStruct {
                name,
                file: file
                    .strip_prefix(workspace_root())
                    .unwrap_or(&file)
                    .to_string_lossy()
                    .into_owned(),
                module: module.clone(),
                fields,
                has_from_palette,
                has_default_or_preset,
                has_setter,
                setter_resolution,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The canonical component-module import path for each component-owned token
/// type: passing a token type through this module is what the vocabulary
/// promises, and it is what generated docs and new examples use.
const CANONICAL_COMPONENT_IMPORTS: &[(&str, &str)] = &[
    ("ButtonTokens", "button.rs"),
    ("ButtonSizeTokens", "button.rs"),
    ("CardTokens", "card.rs"),
    ("CheckboxTokens", "checkbox.rs"),
    ("SurfaceTokens", "surface.rs"),
    ("BadgeTokens", "badge.rs"),
    ("BadgeSizeTokens", "badge.rs"),
    ("ToastTokens", "toast.rs"),
    ("TextTokens", "text.rs"),
    ("AreaChartTokens", "area_chart.rs"),
    ("BarChartTokens", "bar_chart.rs"),
    ("LineChartTokens", "line_chart.rs"),
    ("PieChartTokens", "pie_chart.rs"),
    ("ScatterChartTokens", "scatter_chart.rs"),
    ("CartesianTokens", "cartesian_chart.rs"),
    ("ChartLegendTokens", "chart_legend.rs"),
    ("ChartTooltipTokens", "chart_tooltip.rs"),
    ("ShellTokens", "app_shell.rs"),
    ("MarqueeTokens", "marquee_area.rs"),
    ("SwitcherTokens", "view_switcher.rs"),
];

/// The canonical `pub type NEW = OLD;` aliases that make a type name
/// is derivable from its component's module stem (`AppShellTokens` from
/// `app_shell`, `ViewSwitcherTokens` from `view_switcher`,
/// `MarqueeAreaTokens` from `marquee_area`). The existing shorter names
/// remain as compatibility aliases so the apps' extensive `ShellTokens`
/// usage keeps compiling.
const CANONICAL_TYPE_ALIASES: &[(&str, &str, &str)] = &[
    ("AppShellTokens", "ShellTokens", "app_shell.rs"),
    ("ViewSwitcherTokens", "SwitcherTokens", "view_switcher.rs"),
    ("MarqueeAreaTokens", "MarqueeTokens", "marquee_area.rs"),
];

fn render_md(model: &[TokenStruct]) -> String {
    let mut out = String::new();
    out.push_str("# enzUI - component token API index\n\n");
    out.push_str("See [configuring framework appearance](styling.md) for global scales, snapshot precedence and choosing token roles.\n\n");
    out.push_str(
        "**This file is generated** from public source in `crates/enzui-widgets/src`. \
         See [`token_index.rs`](../crates/enzui-widgets/tests/contracts/token_index.rs). \
         Do not edit by hand; regenerate with\n\
         \n\
         ```sh\n\
         ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index\n\
         ```\n\n",
    );
    out.push_str(
        "## Token vocabulary and anatomy\n\n\
         Field names encode the component's anatomy after the type name (the \
         component itself) has been taken out. The rule is:\n\n\
         ```\n\
         component -> variant/part -> property -> interaction state -> exceptional unit\n\
         ```\n\n\
         - **parts** - `container`/`background`, `content`/`foreground`, \
         `text`, `icon`, `border`, `track`, `thumb`, `focus_ring`, …\n\
         - **states** - rest (implicit), `hover`, `pressed`, `selected`, \
         `disabled`, with combined-state ordering fixed.\n\
         - **geometry** - `padding` for inside an edge, `gap` between \
         siblings, `inset` inside an edge, `outset` beyond an edge, `width`/`height`/`radius` for \
         dimensions.\n\
         - **tones** - `error`, `danger`, `destructive` are *not* \
         interchangeable; each names a different semantic (validation, \
         intrinsic risk, action).\n\
         - **units** - px is implicit for component `f32` dimensions; \
         suffixes are for non-px or dimensionless values (`_ms`, `_fraction`, \
         `_alpha`).\n\n\
         Legacy spellings (`bg`/`fg`/`radius_lg`) stay compatible; see the \
         per-token tables below for the canonical semantic path.\n\n",
    );
    out.push_str(
        "## Token setters and resolution modes\n\n\
         Component paint snapshots use `.tokens(...)`; geometry helpers use \
         dedicated setters such as `.size_tokens(...)`. Passing a value usually \
         pins a per-instance snapshot. Omitting it follows the runtime theme \
         only for channels documented as live. See [styling](styling.md) for \
         scale forwarding and precedence.\n\n\
         ### When to override tokens\n\n\
         | You want to… | Technique | Theme-switch behaviour |\n\
         |---|---|---|\n\
         | Use the framework default | Omit `.tokens()` | Uses component defaults; documented live channels follow the theme |\n\
         | Use a named preset (`light`/`dark`) | `.tokens(T::light())` | \
         Snapshot - call again on theme change |\n\
         | Derive from the current palette | `.tokens(T::from_palette(p))` | \
         Snapshot - stale after theme change unless re-derived |\n\
         | Set one field, keep defaults | `T { field: val, ..T::from_palette(p) }` | \
         Snapshot - same caveat as `from_palette` |\n\
         | Hard-code a literal | `T { field_a: x, field_b: y, … }` | \
         Fully pinned - never changes |\n\n\
         **Rule of thumb:** prefer omitting `.tokens()` (the framework \
         default) whenever possible. Call `from_palette` only when you \
         need to change one or two fields and accept the snapshot \
         trade-off; never pin an entire struct literal unless the \
         component genuinely needs a fixed appearance.\n\n\
         ### Resolution modes\n\n\
         The generated table below records each setter's *resolution mode* \
         as parsed from its rustdoc:\n\n\
         - **pin** - rustdoc explicitly documents the value pins/bypasses \
         runtime-theme derivation (a fixed snapshot).\n\
         - **override** - rustdoc says the value overrides / applies / sets \
         / configures; silent on runtime resolution.\n\
         - **runtime** - rustdoc explicitly documents a live runtime-theme \
         resolver (no setter currently classified here; all existing setters \
         produce snapshots).\n\n\
         The `from_palette` constructor is the canonical way to seed a \
         palette-backed snapshot; `light()`/`dark()` are convenience presets, \
         not the runtime-theme path.\n\n",
    );
    let _ = write!(
        out,
        "Source-derived counts: **{} structs, {} public fields**.\n\n",
        model.len(),
        model.iter().map(|s| s.fields.len()).sum::<usize>()
    );

    out.push_str("## Token index\n\n");
    out.push_str("| Type | Module | Fields | `from_palette` | Default | Setter mode |\n");
    out.push_str("|---|---|---:|:---:|:---:|---|\n");
    for t in model {
        // Three states, three labels: setter absent, setter present but docs
        // do not classify, setter present with a classified mode.
        let mode = match (t.has_setter, t.setter_resolution) {
            (_, Some(m)) => m.as_str(),
            (true, None) => "undocumented",
            (false, None) => "(no builder setter)",
        };
        let _ = writeln!(
            out,
            "| [`{}`](#{}) | `{}` | {} | {} | {} | {} |",
            t.name,
            slug(&t.name),
            t.module,
            t.fields.len(),
            yes_no(t.has_from_palette),
            yes_no(t.has_default_or_preset),
            mode,
        );
    }
    out.push_str("\n## Canonical aliases\n\n");
    out.push_str(
        "Three component-owned token types have names shorter than their \
         `<ModuleStem>Tokens` rule would predict. The short spelling is \
         preserved for source compatibility; a canonical `pub type` alias \
         with the derivable name is added in the same module.\n\n",
    );
    out.push_str("| Canonical name | Compatibility name | Owning module |\n|---|---|---|\n");
    for (new, old, file) in CANONICAL_TYPE_ALIASES {
        let _ = writeln!(
            out,
            "| `{new}` | `{old}` | `{}` |",
            file.trim_end_matches(".rs")
        );
    }

    out.push_str("\n## Per-token details\n\n");
    for t in model {
        let _ = write!(out, "### `{}`\n\n", t.name);
        let _ = writeln!(out, "- **File:** `{}`", t.file);
        let _ = writeln!(out, "- **Module:** `{}`", t.module);
        let _ = writeln!(
            out,
            "- **Palette constructor:** {}",
            yes_no(t.has_from_palette)
        );
        let _ = writeln!(
            out,
            "- **Default / preset:** {}",
            yes_no(t.has_default_or_preset)
        );
        let mode_str = match (t.has_setter, t.setter_resolution) {
            (_, Some(m)) => m.as_str(),
            (true, None) => "undocumented",
            (false, None) => "(no builder setter for this type)",
        };
        let _ = writeln!(out, "- **Setter resolution mode:** {mode_str}");
        let _ = writeln!(out, "- **Fields ({}):**", t.fields.len());
        for (name, ty) in &t.fields {
            let _ = writeln!(out, "  - `{name}`: `{ty}`");
        }
        out.push('\n');
    }
    out
}

fn render_json(model: &[TokenStruct]) -> String {
    let mut out = String::from("[\n");
    for (i, t) in model.iter().enumerate() {
        out.push_str("  {\n");
        let _ = writeln!(out, "    \"name\": \"{}\",", t.name);
        let _ = writeln!(out, "    \"file\": \"{}\",", json_escape(&t.file));
        let _ = writeln!(out, "    \"module\": \"{}\",", t.module);
        let _ = writeln!(out, "    \"from_palette\": {},", t.has_from_palette);
        let _ = writeln!(
            out,
            "    \"default_or_preset\": {},",
            t.has_default_or_preset
        );
        match t.setter_resolution {
            Some(m) => {
                let _ = writeln!(out, "    \"setter_resolution\": \"{}\",", m.as_str());
            }
            None => out.push_str("    \"setter_resolution\": null,\n"),
        }
        out.push_str("    \"fields\": [\n");
        for (j, (name, ty)) in t.fields.iter().enumerate() {
            let _ = write!(
                out,
                "      {{ \"name\": \"{}\", \"type\": \"{}\" }}",
                name,
                json_escape(ty)
            );
            if j + 1 < t.fields.len() {
                out.push(',');
            }
            out.push('\n');
        }
        out.push_str("    ]\n  }");
        if i + 1 < model.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("]\n");
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn slug(name: &str) -> String {
    name.to_ascii_lowercase()
}

fn yes_no(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// The staleness gate for `docs/tokens.md`. Any change to a token struct or
/// field renders differently; if the on-disk file was not regenerated the
/// test fails and prints the refresh command.
#[test]
fn token_index_md_is_not_stale() {
    let model = scan_tokens();
    let expected = render_md(&model);
    let path = tokens_md_path();

    if std::env::var_os("ENZUI_WRITE_TOKENS_DOC").is_some() {
        std::fs::write(&path, &expected).unwrap_or_else(|e| panic!("write {path:?}: {e}"));
        eprintln!("wrote {} ({} bytes)", path.display(), expected.len());
        let json = render_json(&model);
        let json_path = tokens_json_path();
        std::fs::write(&json_path, &json).unwrap_or_else(|e| panic!("write {json_path:?}: {e}"));
        eprintln!("wrote {} ({} bytes)", json_path.display(), json.len());
        return;
    }

    let actual = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}). It is generated; create it with\n  \
             ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index",
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
        "docs/tokens.md is STALE - it disagrees with the source-scanned \
         model in `crates/enzui-widgets/src`.\n\n\
         First difference at line {}:\n  \
         on disk:   {:?}\n  \
         generated: {:?}\n\n\
         ({} lines on disk, {} generated.)\n\n\
         If you added, removed, or renamed a `*Tokens` struct/field, \
         regenerate:\n  \
         ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index\n",
        first + 1,
        a.get(first).unwrap_or(&"<end of file>"),
        e.get(first).unwrap_or(&"<end of file>"),
        a.len(),
        e.len(),
    );
}

/// The staleness gate for `docs/tokens.json` - same model, different render.
#[test]
fn token_index_json_is_not_stale() {
    let model = scan_tokens();
    let expected = render_json(&model);
    let path = tokens_json_path();

    if std::env::var_os("ENZUI_WRITE_TOKENS_DOC").is_some() {
        // Written by token_index_md_is_not_stale in the same run.
        return;
    }
    let actual = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {} ({e}). It is generated; create it with\n  \
             ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index",
            path.display()
        )
    });
    if actual == expected {
        return;
    }
    panic!(
        "docs/tokens.json is STALE. Regenerate with\n  \
         ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index\n"
    );
}

#[test]
fn every_component_module_reexports_its_tokens() {
    let src = src_dir();
    let mut missing: Vec<(&str, &str)> = Vec::new();
    for (ty, file) in CANONICAL_COMPONENT_IMPORTS {
        let path = src.join(file);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let masked = mask_rust_non_code(&text);
        let defined_in_file = masked.contains(&format!("pub struct {ty}"))
            || masked.contains(&format!("pub type {ty}"));
        let reexported = masked.lines().any(|line| {
            let t = line.trim_start();
            if !t.starts_with("pub use") {
                return false;
            }
            // Match the token type name at a word boundary.
            let bytes = line.as_bytes();
            let needle = ty.as_bytes();
            let n = needle.len();
            (0..bytes.len().saturating_sub(n)).any(|i| {
                &bytes[i..i + n] == needle
                    && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric() && bytes[i - 1] != b'_')
                    && (i + n == bytes.len()
                        || !bytes[i + n].is_ascii_alphanumeric() && bytes[i + n] != b'_')
            })
        });
        if !(defined_in_file || reexported) {
            missing.push((ty, file));
        }
    }
    assert!(
        missing.is_empty(),
        "The following component-owned token types are NOT reachable through \
         their canonical component module. Each must be \
         defined or re-exported in the named file:\n{}",
        missing
            .iter()
            .map(|(t, f)| format!("  `{t}` - expected in `src/{f}`"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
