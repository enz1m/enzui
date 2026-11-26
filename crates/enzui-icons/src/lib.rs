#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "this crate parses untrusted bytes; an `as` that loses data hides a bug"
)]
#![deny(missing_docs)]
//! Bundled icon assets and registry for enzUI.
//!
//! The catalog contains pinned Lucide icons and original enzUI artwork.
//! Use canonical names (`search`, `enzim-ai`) or semantic aliases
//! (`action.search`, `ai.assistant`).
//!
//! # Example
//! ```
//! use enzui_icons::{bundled_icon, bundled_icons};
//!
//! let icon = bundled_icon("action.save").expect("save icon");
//! assert_eq!(icon.name, "save");
//!
//! let all = bundled_icons();
//! assert!(all.len() >= 1_600);
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded_icons.rs"));
}

mod aliases;
pub mod app_icon;
pub mod file_type;
pub mod handle_cache;
pub mod icon_theme;
pub mod mime;
pub mod mime_icons;
mod registry_table;
pub mod resolver;
pub mod sniff;
pub mod theme_index;

/// Metadata for a single bundled icon asset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BundledIcon {
    /// Canonical icon name, for example `search` or `panel-left`.
    pub name: &'static str,
    /// Path of the SVG asset within the bundled catalog, unique across the
    /// registry.
    pub asset_path: &'static str,
    /// Source filename in the upstream checkout, or empty for original enzUI artwork.
    pub upstream_name: &'static str,
    /// SPDX license expression for the asset.
    pub license: &'static str,
}

/// An icon reference resolved against the bundled catalog.
///
/// Construction accepts either a canonical name (`"save"`) or an enzUI
/// semantic alias (`"action.save"`).
///
/// The icon retains the name it was constructed with ([`requested_name`]) even
/// when that name does not resolve, so diagnostics can report the original
/// string, while [`name`] yields the canonical name for a known icon and the
/// bundled `"fallback"` glyph for an unknown one.
///
/// App-owned icon identity is not supported through the standard renderers: an
/// unknown name always renders the fallback glyph. Use
/// `enzui_widgets::icon::enz_icon_from_handle` to supply an app-owned SVG
/// handle directly.
///
/// [`requested_name`]: Icon::requested_name
/// [`name`]: Icon::name
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Icon {
    /// The name the caller asked for (canonical, alias, or unknown).
    requested_name: &'static str,
    /// The canonical name if `requested_name` resolved, else `None`
    /// (unknown — renders the fallback glyph; retained for diagnostics only).
    canonical_name: Option<&'static str>,
}

impl Icon {
    /// Creates an icon from a `'static` name.
    ///
    /// An unknown name is retained rather than collapsed to `fallback`: the
    /// icon renders the fallback glyph while
    /// [`requested_name`](Icon::requested_name) preserves the original string.
    pub fn named(name: &'static str) -> Self {
        Self {
            requested_name: name,
            canonical_name: canonical_icon_name(name),
        }
    }

    /// Creates an icon only when the name resolves in the bundled catalog.
    pub fn try_named(name: &'static str) -> Option<Self> {
        canonical_icon_name(name).map(|canonical| Self {
            requested_name: name,
            canonical_name: Some(canonical),
        })
    }

    /// Returns the bundled fallback glyph (with `"fallback"` identity).
    pub fn fallback() -> Self {
        Self {
            requested_name: "fallback",
            canonical_name: canonical_icon_name("fallback"),
        }
    }

    /// Returns the name the caller originally requested — preserved even for
    /// unknown icons so renderers can diagnose typos.
    pub fn requested_name(self) -> &'static str {
        self.requested_name
    }

    /// Returns the canonical name accepted by the widget icon resolver.
    ///
    /// For an unresolved (unknown/custom) name this returns `"fallback"`, so
    /// the widget resolver always has a bundled glyph to draw. Use
    /// [`requested_name`](Icon::requested_name) to recover the original name
    /// and [`is_resolved`](Icon::is_resolved) to tell the two apart.
    pub fn name(self) -> &'static str {
        self.canonical_name.unwrap_or("fallback")
    }

    /// Returns whether the requested name resolved to a bundled icon.
    /// `false` means the icon is an unknown typo or an app-defined custom name
    /// that will render as the fallback glyph.
    pub fn is_resolved(self) -> bool {
        self.canonical_name.is_some()
    }

    /// Returns whether this icon renders the bundled fallback glyph — either
    /// because it *is* the explicit fallback or because its requested name did
    /// not resolve.
    pub fn is_fallback(self) -> bool {
        !self.is_resolved() || self.requested_name == "fallback"
    }
}

impl std::fmt::Display for Icon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Counts the number of indexed lookups performed (test instrumentation).
/// Used to prove that lookups go through the index rather than scanning.
#[doc(hidden)]
pub static ICON_INDEX_LOOKUPS: AtomicUsize = AtomicUsize::new(0);

/// Read [`ICON_INDEX_LOOKUPS`].
#[doc(hidden)]
pub fn icon_index_lookups() -> usize {
    ICON_INDEX_LOOKUPS.load(Ordering::Relaxed)
}

/// Reset [`ICON_INDEX_LOOKUPS`] and return its previous value.
#[doc(hidden)]
pub fn reset_icon_index_lookups() -> usize {
    ICON_INDEX_LOOKUPS.swap(0, Ordering::Relaxed)
}

// The built registry plus the name and alias indexes. Built once, read after.
struct RegistryIndex {
    icons: Vec<BundledIcon>,
    /// canonical-name → index in `icons`
    name_to_idx: HashMap<&'static str, usize>,
    /// alias-or-synonym → canonical &'static str
    alias_to_canonical: HashMap<&'static str, &'static str>,
}

static REGISTRY: OnceLock<RegistryIndex> = OnceLock::new();

fn build_registry_index() -> RegistryIndex {
    const LICENSE: &str = "ISC AND MIT";

    let entries: &[(&str, &str, &str)] = registry_table::ICON_ENTRIES;

    let icons: Vec<BundledIcon> = entries
        .iter()
        .map(|(name, path, file)| BundledIcon {
            name,
            asset_path: path,
            upstream_name: file,
            license: LICENSE,
        })
        .chain(embedded::ENZUI_ICONS.iter().copied())
        .collect();

    let mut name_to_idx = HashMap::with_capacity(icons.len());
    for (i, icon) in icons.iter().enumerate() {
        name_to_idx.insert(icon.name, i);
    }

    let mut alias_to_canonical = HashMap::with_capacity(aliases::ALIASES.len());
    for &(alias, canonical) in aliases::ALIASES {
        alias_to_canonical.insert(alias, canonical);
    }

    RegistryIndex {
        icons,
        name_to_idx,
        alias_to_canonical,
    }
}

fn registry() -> &'static RegistryIndex {
    REGISTRY.get_or_init(build_registry_index)
}

/// Return the canonical name for a supported icon name or alias.
///
/// Lookups are O(1) via a `HashMap` index over canonical names and aliases.
pub fn canonical_icon_name(name: &str) -> Option<&'static str> {
    ICON_INDEX_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    let reg = registry();
    if let Some(&idx) = reg.name_to_idx.get(name) {
        return Some(reg.icons[idx].name);
    }
    reg.alias_to_canonical.get(name).copied()
}

/// Look up a bundled icon by canonical name or stable enzUI semantic alias.
///
/// Uses indexed resolution — O(1) via the name/alias `HashMap`.
pub fn bundled_icon(name: &str) -> Option<&'static BundledIcon> {
    let canonical = canonical_icon_name(name)?;
    let reg = registry();
    let &idx = reg.name_to_idx.get(canonical)?;
    Some(&reg.icons[idx])
}

/// Return compile-time embedded SVG bytes for a canonical name, semantic alias,
/// or the explicit `fallback` placeholder.
pub fn bundled_icon_svg(name: &str) -> Option<(&'static str, &'static [u8])> {
    let canonical = if name == "fallback" {
        "fallback"
    } else {
        canonical_icon_name(name)?
    };
    Some((canonical, embedded::icon_data(canonical)?))
}

/// The canonical glyph a directional semantic name draws in a right-to-left
/// locale (`navigation.forward` points left), or `None` when `name` looks the
/// same in both directions.
pub fn rtl_icon_name(name: &str) -> Option<&'static str> {
    aliases::RTL_MIRRORS
        .iter()
        .find(|(semantic, _)| *semantic == name)
        .map(|&(_, canonical)| canonical)
}

/// Return all canonical bundled icons. Aliases are intentionally omitted
/// so catalogs and pickers do not show duplicate entries.
pub fn bundled_icons() -> &'static [BundledIcon] {
    registry().icons.as_slice()
}

/// Return the canonical Lucide name for a legacy core icon identifier.
pub fn semantic_name_for_legacy(name: &str) -> Option<&'static str> {
    match name {
        "add" => Some("plus"),
        "clock" => Some("clock"),
        "close" => Some("x"),
        "delete" => Some("trash-2"),
        "edit" => Some("pencil"),
        "edit-delete" => Some("trash-2"),
        "go-home" => Some("house"),
        "save" => Some("save"),
        "settings" => Some("settings"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enzim_ai_icons_resolve_to_mit_assets() {
        for (requested, canonical) in [
            ("ai.assistant", "enzim-ai"),
            ("enzim-ai", "enzim-ai"),
            ("enzim-ai-chat", "enzim-ai-chat"),
        ] {
            let icon = bundled_icon(requested).expect("bundled AI icon");
            let (name, bytes) = bundled_icon_svg(requested).expect("embedded AI SVG");
            assert_eq!(name, canonical);
            assert_eq!(icon.name, canonical);
            assert_eq!(icon.license, "MIT");
            assert_eq!(
                bytes,
                std::fs::read(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(icon.asset_path)
                )
                .expect("AI artwork is readable"),
            );
        }
    }

    #[test]
    fn rtl_mirrors_name_bundled_glyphs_of_directional_aliases() {
        for &(semantic, rtl) in aliases::RTL_MIRRORS {
            let ltr = canonical_icon_name(semantic).expect("a mirrored name is an alias");
            assert!(bundled_icon_svg(rtl).is_some(), "{rtl} is bundled");
            assert_ne!(ltr, rtl, "{semantic} changes glyph");
            assert_eq!(rtl_icon_name(semantic), Some(rtl));
        }
        assert_eq!(rtl_icon_name("chevron-right"), None);
    }
    use sha2::{Digest, Sha256};

    fn lucide_payload_subjects() -> Vec<(&'static str, Vec<u8>)> {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let read = |rel: &str| std::fs::read(crate_dir.join(rel)).expect("bundled asset readable");
        let lucide_icons = bundled_icons()
            .iter()
            .filter(|icon| icon.asset_path.starts_with("assets/lucide/"));
        let mut subjects = vec![("fallback", read("assets/fallback.svg"))];
        subjects.extend(lucide_icons.map(|icon| (icon.name, read(icon.asset_path))));
        subjects
    }

    // Each blob is preceded by its byte length and the whole stream by the
    // subject count, so a byte moved across an entry boundary or a truncated
    // scan changes the digest.
    fn payload_digest(subjects: &[(&'static str, Vec<u8>)]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"enzui-icons lucide payload v1\0");
        hasher.update((subjects.len() as u64).to_le_bytes());
        for (label, bytes) in subjects {
            hasher.update((label.len() as u64).to_le_bytes());
            hasher.update(label.as_bytes());
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn manifest_payload_sha256() -> &'static str {
        let manifest = include_str!("../icon-manifest.toml");
        manifest
            .lines()
            .find_map(|line| line.trim().strip_prefix("payload_sha256 = "))
            .map(|value| value.trim().trim_matches('"'))
            .expect("icon-manifest.toml [source] must declare payload_sha256")
    }

    #[test]
    fn payload_digest_matches_pinned_source() {
        let subjects = lucide_payload_subjects();
        assert!(subjects.len() > 1_714, "catalog lost pinned Lucide icons");
        assert_eq!(
            payload_digest(&subjects),
            manifest_payload_sha256(),
            "bundled Lucide payload does not match the pinned commit digest in \
             icon-manifest.toml"
        );
    }
}
