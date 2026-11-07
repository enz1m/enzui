use super::{IconContext, IconError};
use enzui_icons::icon_theme::{DirEntry, Theme, ThemeSet};
use std::collections::{BTreeMap, HashSet};
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

const INDEX_BYTES: usize = 256 * 1024;
const TOTAL_INDEX_BYTES: usize = 4 * 1024 * 1024;
const MAX_THEMES: usize = 64;
const MAX_DIRECTORIES: usize = 1024;
const MAX_PROBES: usize = 32768;

pub(super) struct LoadedThemes {
    themes: Vec<Theme>,
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name.is_ascii()
        && !name
            .bytes()
            .any(|b| b.is_ascii_control() || b == b' ' || b == b',' || b == b'/' || b == b'\\')
        && name != "."
        && name != ".."
}

pub(super) fn read_regular(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "icon file exceeds the regular-file budget",
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "icon file grew beyond the byte budget",
        ));
    }
    Ok(bytes)
}

impl LoadedThemes {
    pub(super) fn load(context: &IconContext, selected: &str) -> Result<Self, IconError> {
        let mut themes = Vec::new();
        let mut pending = vec![selected.to_owned(), "hicolor".to_owned()];
        let mut seen = HashSet::new();
        let mut total_bytes = 0;
        while let Some(name) = pending.pop() {
            if !valid_name(&name) || !seen.insert(name.clone()) {
                continue;
            }
            if seen.len() > MAX_THEMES {
                return Err(IconError::Limit);
            }
            let mut index = None;
            for root in &context.roots {
                let path = root.join(&name).join("index.theme");
                match read_regular(&path, INDEX_BYTES) {
                    Ok(bytes) => {
                        index = Some(bytes);
                        break;
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => break,
                }
            }
            let Some(bytes) = index else {
                continue;
            };
            total_bytes += bytes.len();
            if total_bytes > TOTAL_INDEX_BYTES {
                return Err(IconError::Limit);
            }
            let Some(theme) = std::str::from_utf8(&bytes)
                .ok()
                .and_then(|text| parse_index(&name, text))
            else {
                continue;
            };
            pending.extend(theme.inherits.iter().rev().cloned());
            themes.push(theme);
        }
        Ok(Self { themes })
    }

    pub(super) fn find(
        &self,
        context: &IconContext,
        selected: &str,
        name: &str,
        size: u16,
        scale: u16,
    ) -> Result<Option<PathBuf>, IconError> {
        let mut probes = 0;
        let mut exists = |path: PathBuf| -> Result<Option<PathBuf>, IconError> {
            probes += 1;
            if probes > MAX_PROBES {
                return Err(IconError::Limit);
            }
            Ok(path.is_file().then_some(path))
        };
        let set = ThemeSet::new(self.themes.clone());
        for theme_name in set.search_order(selected) {
            let Some(theme) = self.themes.iter().find(|t| t.name == theme_name) else {
                continue;
            };
            for dir in &theme.directories {
                if !dir.matches_size(size, scale) {
                    continue;
                }
                for root in &context.roots {
                    for extension in ["png", "svg"] {
                        if let Some(path) = exists(
                            root.join(&theme_name)
                                .join(&dir.dir)
                                .join(format!("{name}.{extension}")),
                        )? {
                            return Ok(Some(path));
                        }
                    }
                }
            }
            let mut best = None;
            for dir in &theme.directories {
                let distance = dir.size_distance(size, scale);
                for root in &context.roots {
                    for extension in ["png", "svg"] {
                        if let Some(path) = exists(
                            root.join(&theme_name)
                                .join(&dir.dir)
                                .join(format!("{name}.{extension}")),
                        )? && best.as_ref().is_none_or(|(_, old)| distance < *old)
                        {
                            best = Some((path, distance));
                        }
                    }
                }
            }
            // A selected theme's artwork wins over a closer size in an inherited theme.
            if let Some((path, _)) = best {
                return Ok(Some(path));
            }
        }
        for root in &context.roots {
            for extension in ["png", "svg"] {
                if let Some(path) = exists(root.join(format!("{name}.{extension}")))? {
                    return Ok(Some(path));
                }
            }
        }
        Ok(None)
    }
}

fn unescape(value: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            match chars.next()? {
                's' => ' ',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                '\\' => '\\',
                _ => return None,
            }
        } else {
            c
        });
    }
    Some(out)
}

fn parse_index(name: &str, text: &str) -> Option<Theme> {
    let mut groups: BTreeMap<&str, BTreeMap<&str, String>> = BTreeMap::new();
    let mut current = "";
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(group) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = group;
        } else if let Some((key, value)) = line.split_once('=') {
            groups
                .entry(current)
                .or_default()
                .insert(key.trim(), unescape(value.trim())?);
        }
    }
    let main = groups.get("Icon Theme")?;
    let directories = main.get("Directories")?;
    let mut dirs = Vec::new();
    let mut seen = HashSet::new();
    for dir in directories.split(',').chain(
        main.get("ScaledDirectories")
            .into_iter()
            .flat_map(|s| s.split(',')),
    ) {
        let dir = dir.trim();
        if dir.is_empty()
            || !Path::new(dir)
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
            || !seen.insert(dir)
        {
            continue;
        }
        if seen.len() > MAX_DIRECTORIES {
            return None;
        }
        let Some(section) = groups.get(dir) else {
            continue;
        };
        let number = |key, fallback| {
            section
                .get(key)
                .map_or(Some(fallback), |s| s.parse::<u16>().ok())
        };
        let Some(size) = number("Size", 0).filter(|s| *s > 0) else {
            continue;
        };
        let Some(scale) = number("Scale", 1).filter(|s| *s > 0) else {
            continue;
        };
        let entry = match section.get("Type").map_or("Threshold", String::as_str) {
            "Fixed" => DirEntry::fixed(dir, size),
            "Scalable" => {
                let (Some(min), Some(max)) = (number("MinSize", size), number("MaxSize", size))
                else {
                    continue;
                };
                if min == 0 || min > max {
                    continue;
                }
                DirEntry::scalable(dir, size, min, max)
            }
            "Threshold" => {
                let Some(threshold) = number("Threshold", 2) else {
                    continue;
                };
                DirEntry::threshold(dir, size, threshold)
            }
            _ => continue,
        }
        .with_scale(scale);
        dirs.push(entry);
    }
    let parents = main
        .get("Inherits")
        .into_iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| valid_name(s))
        .map(str::to_owned);
    Some(Theme::new(name).inherits(parents).with_dirs(dirs))
}

pub(super) fn decode_png(
    bytes: &[u8],
    file_limit: usize,
) -> Result<(iced_core::image::Handle, usize), IconError> {
    use enzui_core::decode_limits::DecodeLimits;
    use image::ImageDecoder;
    let limits = DecodeLimits {
        max_encoded_bytes: file_limit as u64,
        max_width: 4096,
        max_height: 4096,
        max_pixels: 4 * 1024 * 1024,
        max_rgba_bytes: 16 * 1024 * 1024,
    };
    let map_error = |error| match error {
        image::ImageError::Limits(_) => IconError::Limit,
        other => IconError::Decode(other),
    };
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(bytes), image::ImageFormat::Png);
    let mut header_limits = image::Limits::default();
    header_limits.max_alloc = Some(limits.max_rgba_bytes);
    reader.limits(header_limits);
    let decoder = reader.into_decoder().map_err(map_error)?;
    let (width, height) = decoder.dimensions();
    if limits.check(bytes.len() as u64, width, height).is_refused() {
        return Err(IconError::Limit);
    }
    let rgba = image::DynamicImage::from_decoder(decoder)
        .map_err(map_error)?
        .into_rgba8()
        .into_raw();
    let length = rgba.len();
    Ok((
        iced_core::image::Handle::from_rgba(width, height, rgba),
        length,
    ))
}
