//! Rasterising one frame of the document.

use std::sync::{Arc, OnceLock};

use resvg::{tiny_skia, usvg};

/// The system fonts SVG text draws with, loaded once on first use.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    Arc::clone(FONTS.get_or_init(|| {
        let mut database = usvg::fontdb::Database::new();
        database.load_system_fonts();
        Arc::new(database)
    }))
}

/// A parsed frame, ready to rasterise at any size.
pub(super) struct Parsed {
    tree: usvg::Tree,
}

/// Parse `text` (one frame written out by the scene).
pub(super) fn parse(text: &str) -> Option<Parsed> {
    let options = usvg::Options {
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_str(text, &options).ok()?;
    Some(Parsed { tree })
}

impl Parsed {
    /// The document's own size, in its user units.
    pub(super) fn size(&self) -> (f32, f32) {
        let size = self.tree.size();
        (size.width(), size.height())
    }

    /// The frame drawn into `width` × `height` pixels with `transform`
    /// mapping document units to pixels, as straight-alpha RGBA.
    pub(super) fn rasterise(
        &self,
        width: u32,
        height: u32,
        transform: [f32; 6],
    ) -> Option<Vec<u8>> {
        let mut pixmap = tiny_skia::Pixmap::new(width.max(1), height.max(1))?;
        let [sx, ky, kx, sy, tx, ty] = transform;
        let transform = tiny_skia::Transform::from_row(sx, ky, kx, sy, tx, ty);
        // A malformed document must not take the application down.
        let drawn = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            resvg::render(&self.tree, transform, &mut pixmap.as_mut());
        }));
        if drawn.is_err() {
            return None;
        }
        // The image pipeline premultiplies in its shader: hand it straight
        // alpha.
        let mut pixels = pixmap.take();
        for pixel in pixels.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            if alpha != 0 && alpha != 255 {
                for channel in &mut pixel[..3] {
                    *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        Some(pixels)
    }
}

impl Parsed {
    /// The box of the element with `id`, stroke included, in document
    /// units.
    pub(super) fn node_box(&self, id: &str) -> Option<iced::Rectangle> {
        let rect = self.tree.node_by_id(id)?.abs_stroke_bounding_box();
        Some(iced::Rectangle::new(
            iced::Point::new(rect.x(), rect.y()),
            iced::Size::new(rect.width(), rect.height()),
        ))
    }
}
