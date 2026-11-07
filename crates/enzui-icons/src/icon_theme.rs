//! Pure icon-theme inheritance traversal and freedesktop size matching.
//!
//! The [Icon Theme Specification][spec] lookup algorithm over an in-memory
//! description of themes: nothing here parses an `index.theme`, stats a
//! directory or reads a file. It answers which directories to search, in what
//! order, and which is the best size match; the disk-touching layer wraps it.
//!
//! Traversal visits a theme and then its `Inherits` parents depth-first, always
//! ending at `hicolor` even when no theme lists it, and visits each theme once
//! so an inheritance cycle terminates. [`DirEntry::matches_size`] and
//! [`DirEntry::size_distance`] implement `DirectoryMatchesSize` and
//! `DirectorySizeDistance` for `Fixed`, `Scalable` and `Threshold`
//! directories, including the scale factor.
//!
//! [spec]: https://specifications.freedesktop.org/icon-theme/latest/
//!
//! # Example
//! ```
//! use enzui_icons::icon_theme::{Theme, DirEntry, DirType, ThemeSet};
//!
//! let themes = ThemeSet::new(vec![
//!     Theme::new("Adwaita").inherits(["hicolor"]).with_dirs(vec![
//!         DirEntry::fixed("48x48/mimetypes", 48),
//!     ]),
//!     Theme::new("hicolor").with_dirs(vec![
//!         DirEntry::fixed("48x48/mimetypes", 48),
//!     ]),
//! ]);
//! // Search order always ends at hicolor.
//! let order = themes.search_order("Adwaita");
//! assert_eq!(order, vec!["Adwaita".to_string(), "hicolor".to_string()]);
//! ```

/// The `Type` of an icon directory, deciding how its size band is computed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirType {
    /// Icons are exactly `size`; only that exact requested size matches.
    Fixed,
    /// Icons can be rendered anywhere in `[min_size, max_size]`.
    Scalable,
    /// Icons match within `size ± threshold`.
    Threshold,
}

/// One icon directory within a theme (a `[subdir]` section of `index.theme`).
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct DirEntry {
    /// The subdirectory path relative to the theme root, e.g. `48x48/mimetypes`.
    pub dir: String,
    /// Nominal (unscaled) icon size.
    pub size: u16,
    /// Target scale (defaults to 1).
    pub scale: u16,
    /// The directory type.
    pub kind: DirType,
    /// Minimum size (Scalable only; defaults to `size`).
    pub min_size: u16,
    /// Maximum size (Scalable only; defaults to `size`).
    pub max_size: u16,
    /// Threshold (Threshold only; defaults to 2).
    pub threshold: u16,
}

impl DirEntry {
    /// A `Fixed` directory: matches only the exact `size` (at scale 1).
    pub fn fixed(dir: impl Into<String>, size: u16) -> Self {
        Self {
            dir: dir.into(),
            size,
            scale: 1,
            kind: DirType::Fixed,
            min_size: size,
            max_size: size,
            threshold: 2,
        }
    }

    /// A `Scalable` directory spanning `[min, max]` (at scale 1).
    pub fn scalable(dir: impl Into<String>, size: u16, min: u16, max: u16) -> Self {
        Self {
            dir: dir.into(),
            size,
            scale: 1,
            kind: DirType::Scalable,
            min_size: min,
            max_size: max,
            threshold: 2,
        }
    }

    /// A `Threshold` directory: matches within `size ± threshold` (scale 1).
    pub fn threshold(dir: impl Into<String>, size: u16, threshold: u16) -> Self {
        Self {
            dir: dir.into(),
            size,
            scale: 1,
            kind: DirType::Threshold,
            min_size: size,
            max_size: size,
            threshold,
        }
    }

    /// Builder: set the target scale.
    pub fn with_scale(mut self, scale: u16) -> Self {
        self.scale = scale.max(1);
        self
    }

    /// Implements `DirectoryMatchesSize`: whether this directory holds an icon
    /// usable at the requested `(size, scale)` without resizing beyond the
    /// directory's band. The scale must match exactly (per the spec).
    pub fn matches_size(&self, size: u16, scale: u16) -> bool {
        if self.scale != scale {
            return false;
        }
        match self.kind {
            DirType::Fixed => self.size == size,
            DirType::Scalable => self.min_size <= size && size <= self.max_size,
            DirType::Threshold => {
                let lo = self.size.saturating_sub(self.threshold);
                let hi = self.size.saturating_add(self.threshold);
                lo <= size && size <= hi
            }
        }
    }

    /// Implements `DirectorySizeDistance`: the (scaled) distance between the
    /// requested size and this directory's usable band. `0` when the size is
    /// inside the band. Used to pick the closest directory when no exact match
    /// exists.
    pub fn size_distance(&self, size: u16, scale: u16) -> u32 {
        let want = u32::from(size) * u32::from(scale);
        let s = u32::from(self.size) * u32::from(self.scale);
        match self.kind {
            DirType::Fixed => want.abs_diff(s),
            DirType::Scalable => {
                let min = u32::from(self.min_size) * u32::from(self.scale);
                let max = u32::from(self.max_size) * u32::from(self.scale);
                distance_to_band(want, min, max)
            }
            DirType::Threshold => {
                let min =
                    u32::from(self.size.saturating_sub(self.threshold)) * u32::from(self.scale);
                let max =
                    u32::from(self.size.saturating_add(self.threshold)) * u32::from(self.scale);
                distance_to_band(want, min, max)
            }
        }
    }
}

/// Distance from `want` to the closed band `[min, max]`: 0 inside, else the gap
/// to the nearer edge.
fn distance_to_band(want: u32, min: u32, max: u32) -> u32 {
    // Saturating so an out-of-order band cannot underflow.
    min.saturating_sub(want).max(want.saturating_sub(max))
}

/// A single icon theme: a name, its inherited parents, and its directories.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct Theme {
    /// The internal theme name (subdirectory name), case-sensitive.
    pub name: String,
    /// The `Inherits` list, in order.
    pub inherits: Vec<String>,
    /// The theme's icon directories.
    pub directories: Vec<DirEntry>,
}

impl Theme {
    /// A theme with a name, no inheritance, and no directories.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            inherits: Vec::new(),
            directories: Vec::new(),
        }
    }

    /// Builder: set the `Inherits` list.
    pub fn inherits<I, S>(mut self, parents: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inherits = parents.into_iter().map(Into::into).collect();
        self
    }

    /// Builder: set the directory list.
    pub fn with_dirs(mut self, dirs: Vec<DirEntry>) -> Self {
        self.directories = dirs;
        self
    }
}

/// The `hicolor` theme name, which every lookup must reach as its terminal.
pub const HICOLOR: &str = "hicolor";

/// A set of in-memory themes, keyed by name, over which lookups are performed.
#[derive(Clone, Debug, Default)]
pub struct ThemeSet {
    themes: Vec<Theme>,
}

impl ThemeSet {
    /// Build a theme set from a list of theme descriptions.
    pub fn new(themes: Vec<Theme>) -> Self {
        Self { themes }
    }

    fn by_name(&self) -> std::collections::HashMap<&str, &Theme> {
        let mut by_name = std::collections::HashMap::with_capacity(self.themes.len());
        for theme in &self.themes {
            by_name.entry(theme.name.as_str()).or_insert(theme);
        }
        by_name
    }

    /// The order in which themes are searched for `start`, following the
    /// freedesktop lookup: the start theme, then its parents (depth-first, in
    /// `Inherits` order), then `hicolor` last.
    ///
    /// Terminates on cycles: each theme is emitted at most once, so a theme
    /// that inherits itself (directly or through a loop) does not recurse
    /// forever. `hicolor` is always the final entry even when no theme lists it
    /// and even when the start theme is missing from the set.
    pub fn search_order(&self, start: &str) -> Vec<String> {
        Self::search_order_with(start, &self.by_name())
    }

    fn search_order_with(
        start: &str,
        themes_by_name: &std::collections::HashMap<&str, &Theme>,
    ) -> Vec<String> {
        let mut order = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut pending = vec![start.to_owned()];
        while let Some(name) = pending.pop() {
            if !visited.insert(name.clone()) {
                continue;
            }
            if let Some(theme) = themes_by_name.get(name.as_str()) {
                pending.extend(theme.inherits.iter().rev().cloned());
            }
            order.push(name);
        }

        // hicolor must always be reached, and must be last.
        if let Some(pos) = order.iter().position(|n| n == HICOLOR) {
            let h = order.remove(pos);
            order.push(h);
        } else {
            order.push(HICOLOR.to_owned());
        }
        order
    }

    /// Find the best directory for `(icon_size, scale)` across the inheritance
    /// chain, in the spec's two phases: any directory that matches the size
    /// exactly (`DirectoryMatchesSize`), and only if none does, the smallest
    /// size distance (`DirectorySizeDistance`).
    ///
    /// Returns the `(theme_name, DirEntry)` chosen, or `None` when no theme in
    /// the chain has any directory. Themes are considered in `search_order`, so
    /// a matching directory in an earlier theme beats a closer one in a later
    /// theme.
    pub fn best_directory(
        &self,
        start: &str,
        icon_size: u16,
        scale: u16,
    ) -> Option<(String, DirEntry)> {
        let themes_by_name = self.by_name();
        let order = Self::search_order_with(start, &themes_by_name);

        // Phase 1: exact size match, theme by theme in order.
        for theme_name in &order {
            if let Some(theme) = themes_by_name.get(theme_name.as_str())
                && let Some(dir) = theme
                    .directories
                    .iter()
                    .find(|d| d.matches_size(icon_size, scale))
            {
                return Some((theme_name.clone(), dir.clone()));
            }
        }

        // Phase 2: closest by size distance across the whole chain.
        let mut best: Option<(String, DirEntry, u32)> = None;
        for theme_name in &order {
            if let Some(theme) = themes_by_name.get(theme_name.as_str()) {
                for dir in &theme.directories {
                    let dist = dir.size_distance(icon_size, scale);
                    if best.as_ref().is_none_or(|(_, _, bd)| dist < *bd) {
                        best = Some((theme_name.clone(), dir.clone(), dist));
                    }
                }
            }
        }
        best.map(|(name, dir, _)| (name, dir))
    }
}
