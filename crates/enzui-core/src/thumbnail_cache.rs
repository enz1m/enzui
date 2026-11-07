//! The desktop's shared thumbnail cache (the freedesktop thumbnail spec):
//! `$XDG_CACHE_HOME/thumbnails`.
//!
//! File managers, image viewers and file choosers on the same desktop read
//! and write the same thumbnails, so a picture thumbnailed by one is not
//! decoded again by the next. [`ThumbnailCache::lookup`] finds a thumbnail
//! that is still valid for the file (its recorded URI and modification time
//! match); [`ThumbnailCache::store`] writes one an app made itself, atomically
//! and readable only by the user; [`ThumbnailCache::record_failure`] notes a
//! file the app could not thumbnail, so it does not try again until the file
//! changes.
//!
//! A thumbnail is named by the MD5 of the file's URI as GLib writes it, so
//! [`file_uri`] produces exactly that spelling.

use md5::{Digest as _, Md5};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// A thumbnail size from the spec, by its directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThumbnailSize {
    /// At most 128 pixels on the long side (`normal`).
    Normal,
    /// At most 256 pixels (`large`).
    Large,
    /// At most 512 pixels (`x-large`).
    XLarge,
    /// At most 1024 pixels (`xx-large`).
    XXLarge,
}

impl ThumbnailSize {
    /// Every size, smallest first.
    pub const ALL: [Self; 4] = [Self::Normal, Self::Large, Self::XLarge, Self::XXLarge];

    /// The longest side in pixels.
    #[must_use]
    pub const fn pixels(self) -> u32 {
        match self {
            Self::Normal => 128,
            Self::Large => 256,
            Self::XLarge => 512,
            Self::XXLarge => 1024,
        }
    }

    /// The directory under the cache root.
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Large => "large",
            Self::XLarge => "x-large",
            Self::XXLarge => "xx-large",
        }
    }

    /// The smallest size that holds `pixels` on the long side.
    #[must_use]
    pub fn for_pixels(pixels: u32) -> Self {
        Self::ALL
            .into_iter()
            .find(|size| size.pixels() >= pixels)
            .unwrap_or(Self::XXLarge)
    }
}

/// A `file://` URI for `path`, escaped as GLib's `g_filename_to_uri`
/// escapes it, which the cache's names depend on.
#[must_use]
pub fn file_uri(path: &Path) -> String {
    use std::fmt::Write as _;
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        let kept = byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'!' | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b'-'
                    | b'.'
                    | b'/'
                    | b':'
                    | b'='
                    | b'@'
                    | b'_'
                    | b'~'
            );
        if kept {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

/// RGBA pixels for [`ThumbnailCache::store`].
#[derive(Debug, Clone, Copy)]
pub struct ThumbnailImage<'a> {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, straight (not premultiplied) alpha.
    pub rgba: &'a [u8],
}

/// What [`ThumbnailCache::lookup`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedThumbnail {
    /// The PNG to show.
    pub path: PathBuf,
    /// The size it was stored at.
    pub size: ThumbnailSize,
}

/// The thumbnail cache at a root directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbnailCache {
    root: PathBuf,
}

impl ThumbnailCache {
    /// The cache at `root` (normally `$XDG_CACHE_HOME/thumbnails`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The user's cache: `$XDG_CACHE_HOME/thumbnails`, or
    /// `~/.cache/thumbnails`. `None` without either variable.
    #[must_use]
    pub fn user() -> Option<Self> {
        let cache = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
        Some(Self::new(cache.join("thumbnails")))
    }

    /// Where the thumbnail of `uri` at `size` lives.
    #[must_use]
    pub fn path_for(&self, uri: &str, size: ThumbnailSize) -> PathBuf {
        self.root.join(size.directory()).join(name(uri))
    }

    /// A valid thumbnail of `file` at `size` or, failing that, a larger one
    /// (to scale down). `None` when there is none, or the file changed since.
    #[must_use]
    pub fn lookup(&self, file: &Path, size: ThumbnailSize) -> Option<CachedThumbnail> {
        let uri = file_uri(file);
        let stamp = stamp(file)?;
        ThumbnailSize::ALL
            .into_iter()
            .filter(|candidate| *candidate >= size)
            .map(|candidate| (candidate, self.path_for(&uri, candidate)))
            .find(|(_, path)| is_valid(path, &uri, stamp))
            .map(|(size, path)| CachedThumbnail { path, size })
    }

    /// Stores `image` as the thumbnail of `file` at `size`, written to a
    /// temporary file and renamed into place, readable only by the user.
    /// `software` names the app in the PNG.
    ///
    /// # Errors
    /// The file has no modification time, the pixels do not match the size,
    /// or the PNG could not be written.
    pub fn store(
        &self,
        file: &Path,
        size: ThumbnailSize,
        image: ThumbnailImage<'_>,
        software: &str,
    ) -> std::io::Result<PathBuf> {
        let uri = file_uri(file);
        let (mtime, length) = stamp(file).ok_or_else(no_mtime)?;
        let target = self.path_for(&uri, size);
        write_png(
            &target,
            image,
            &[
                ("Thumb::URI", uri.as_str()),
                ("Thumb::MTime", &mtime.to_string()),
                ("Thumb::Size", &length.to_string()),
                ("Software", software),
            ],
        )?;
        Ok(target)
    }

    /// Notes that `app` could not thumbnail `file`, under `fail/<app>`.
    ///
    /// # Errors
    /// The file has no modification time, or the note could not be written.
    pub fn record_failure(&self, file: &Path, app: &str) -> std::io::Result<()> {
        let uri = file_uri(file);
        let (mtime, length) = stamp(file).ok_or_else(no_mtime)?;
        let target = self.root.join("fail").join(app).join(name(&uri));
        write_png(
            &target,
            ThumbnailImage {
                width: 1,
                height: 1,
                rgba: &[0, 0, 0, 0],
            },
            &[
                ("Thumb::URI", uri.as_str()),
                ("Thumb::MTime", &mtime.to_string()),
                ("Thumb::Size", &length.to_string()),
            ],
        )
    }

    /// Whether `app` failed on `file` as it is now.
    #[must_use]
    pub fn has_failed(&self, file: &Path, app: &str) -> bool {
        let uri = file_uri(file);
        stamp(file).is_some_and(|stamp| {
            is_valid(
                &self.root.join("fail").join(app).join(name(&uri)),
                &uri,
                stamp,
            )
        })
    }
}

/// The cache file name for `uri`: its MD5 in hex, `.png`.
fn name(uri: &str) -> String {
    let digest = Md5::digest(uri.as_bytes());
    let mut name = String::with_capacity(36);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(name, "{byte:02x}");
    }
    name.push_str(".png");
    name
}

/// The file's modification time in whole seconds, and its length in bytes.
fn stamp(file: &Path) -> Option<(u64, u64)> {
    let metadata = std::fs::metadata(file).ok()?;
    let modified = metadata.modified().ok()?;
    Some((
        modified.duration_since(UNIX_EPOCH).ok()?.as_secs(),
        metadata.len(),
    ))
}

fn no_mtime() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "the file has no modification time",
    )
}

/// Whether the PNG at `path` is a thumbnail of `uri` as it is now: the same
/// modification second and, when the thumbnail records it, the same length
/// (so a same-second rewrite of different size is caught).
fn is_valid(path: &Path, uri: &str, (mtime, length): (u64, u64)) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let Ok(reader) = png::Decoder::new(BufReader::new(file)).read_info() else {
        return false;
    };
    let text = |key: &str| {
        reader
            .info()
            .uncompressed_latin1_text
            .iter()
            .find(|chunk| chunk.keyword == key)
            .map(|chunk| chunk.text.clone())
    };
    text("Thumb::URI").as_deref() == Some(uri)
        && text("Thumb::MTime").and_then(|value| value.parse::<u64>().ok()) == Some(mtime)
        && text("Thumb::Size").is_none_or(|value| value.parse::<u64>().ok() == Some(length))
}

/// Writes an RGBA PNG with `text` chunks to `target`, through a unique,
/// exclusively created temporary sibling (mode 0600, never following a
/// planted link) in a directory of mode 0700.
fn write_png(
    target: &Path,
    image: ThumbnailImage<'_>,
    text: &[(&str, &str)],
) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    let invalid =
        |message: &str| std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_owned());
    if u64::from(image.width) * u64::from(image.height) * 4 != image.rgba.len() as u64 {
        return Err(invalid("the pixels do not match the size"));
    }
    let directory = target.parent().ok_or_else(|| invalid("no directory"))?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)?;
    let mut stage = enzui_security::tempstage::TempStage::create(target)?;
    // Unbuffered, so a failed write is an error rather than lost on drop.
    let mut encoder = png::Encoder::new(stage.file_mut(), image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    for (key, value) in text {
        encoder
            .add_text_chunk((*key).to_owned(), (*value).to_owned())
            .map_err(std::io::Error::other)?;
    }
    let mut writer = encoder.write_header().map_err(std::io::Error::other)?;
    writer
        .write_image_data(image.rgba)
        .map_err(std::io::Error::other)?;
    writer.finish().map_err(std::io::Error::other)?;
    // A failure before this point drops the stage, which removes its temp.
    stage.commit()
}
