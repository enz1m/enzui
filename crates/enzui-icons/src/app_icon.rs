//! Finding an installed application's icon file from its desktop entry.
//!
//! A `.desktop` file's `Icon=` key is either an absolute path or a name looked
//! up in the `hicolor` theme, where every application installs its icon, and
//! in the legacy `pixmaps` directory. [`find_app_icon`] answers which file to
//! load for a name at a requested size; [`data_roots`] gives the XDG data
//! directories to search, in precedence order.
//!
//! Each lookup is a handful of `stat` calls, so callers resolve once per
//! application and keep the result.

use std::path::{Path, PathBuf};

use crate::mime::xdg::{Globs2Env, globs2_candidates_os};

/// The fixed `hicolor` sizes applications install into, in pixels.
const FIXED_SIZES: [u16; 10] = [16, 22, 24, 32, 48, 64, 96, 128, 256, 512];

/// The XDG data directories, `$XDG_DATA_HOME` first and then each of
/// `$XDG_DATA_DIRS`, with the specification's defaults.
#[must_use]
pub fn data_roots() -> Vec<PathBuf> {
    let env = Globs2Env::from_env();
    globs2_candidates_os(
        env.data_home.as_deref(),
        env.data_dirs.as_deref(),
        env.home.as_deref(),
    )
    .into_iter()
    // Each candidate is `<root>/mime/globs2`.
    .filter_map(|globs| globs.parent()?.parent().map(Path::to_path_buf))
    .collect()
}

/// The icon file for a desktop entry's `Icon=` value, drawn at `size` pixels.
///
/// An absolute path is used as it is when it exists. A name is searched in
/// each root's `icons/hicolor`, preferring a scalable SVG, then the smallest
/// fixed size at least `size`, then the largest smaller one, and finally in
/// the root's `pixmaps`. Earlier roots win. A name with a path separator is
/// refused, so a desktop file cannot point the lookup outside the theme.
#[must_use]
pub fn find_app_icon(icon: &str, size: u16, roots: &[PathBuf]) -> Option<PathBuf> {
    let icon = icon.trim();
    if icon.is_empty() {
        return None;
    }
    let direct = Path::new(icon);
    if direct.is_absolute() {
        return direct.is_file().then(|| direct.to_path_buf());
    }
    if icon.contains('/') {
        return None;
    }
    let dirs = size_dirs(size);
    roots.iter().find_map(|root| {
        let hicolor = root.join("icons/hicolor");
        dirs.iter()
            .flat_map(|dir| {
                ["svg", "png"].map(|ext| hicolor.join(dir).join(format!("apps/{icon}.{ext}")))
            })
            .chain(["svg", "png"].map(|ext| root.join("pixmaps").join(format!("{icon}.{ext}"))))
            .find(|path| path.is_file())
    })
}

/// The `hicolor` size directories in the order [`find_app_icon`] tries them.
fn size_dirs(size: u16) -> Vec<String> {
    let larger = FIXED_SIZES.iter().filter(|fixed| **fixed >= size);
    let smaller = FIXED_SIZES.iter().rev().filter(|fixed| **fixed < size);
    std::iter::once("scalable".to_owned())
        .chain(
            larger
                .chain(smaller)
                .map(|fixed| format!("{fixed}x{fixed}")),
        )
        .collect()
}

#[cfg(test)]
mod tests;
