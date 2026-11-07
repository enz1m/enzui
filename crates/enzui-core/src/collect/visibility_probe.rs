//! Pixel-level checks that semantic-node bounds contain rendered pixels distinct from the background.

use iced_core::{Color, Rectangle};

/// The result of a visibility check on a single node's bounds.
#[derive(Debug, Clone, PartialEq)]
pub enum VisibilityVerdict {
    /// At least one covered pixel differs from the background.
    Visible {
        /// Number of pixels inside the node's bounds that differ from
        /// the background colour.
        painted_pixels: usize,
    },
    /// The bounds occupy no screen space.
    ZeroArea,
    /// The node's bounds lie entirely outside the viewport.
    FullyClipped,
    /// The node's bounds have area and overlap the viewport, but every
    /// pixel inside them matches the background colour.
    InvisibleInk,
}

impl VisibilityVerdict {
    /// `true` only for [`VisibilityVerdict::Visible`].
    pub fn is_visible(&self) -> bool {
        matches!(self, VisibilityVerdict::Visible { .. })
    }
}

/// Tests semantic-node visibility against a rendered RGBA frame.
pub struct NodeVisibility<'a> {
    pixels: &'a [u8],
    width: u32,
    height: u32,
    bg: [u8; 4],
}

impl<'a> NodeVisibility<'a> {
    /// Creates a probe over `width * height` tightly packed RGBA pixels.
    ///
    /// `pixels` must contain exactly `width * height * 4` bytes.
    ///
    /// # Panics
    ///
    /// Panics in debug builds when the buffer length does not match those dimensions.
    pub fn new(pixels: &'a [u8], width: u32, height: u32, background: Color) -> Self {
        debug_assert_eq!(
            pixels.len(),
            (width as usize) * (height as usize) * 4,
            "pixel buffer size mismatch: expected {}x{}x4={}, got {}",
            width,
            height,
            (width as usize) * (height as usize) * 4,
            pixels.len()
        );
        Self {
            pixels,
            width,
            height,
            bg: rgba8(background),
        }
    }

    /// Checks whether the visible portion of `bounds` contains non-background pixels.
    pub fn check(&self, bounds: Rectangle) -> VisibilityVerdict {
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return VisibilityVerdict::ZeroArea;
        }

        let vp = Rectangle::new(
            iced_core::Point::ORIGIN,
            iced_core::Size::new(self.width as f32, self.height as f32),
        );
        let clipped = intersect(bounds, vp);
        if clipped.width <= 0.0 || clipped.height <= 0.0 {
            return VisibilityVerdict::FullyClipped;
        }

        let x0 = clipped.x.floor().max(0.0) as u32;
        let y0 = clipped.y.floor().max(0.0) as u32;
        let x1 = (clipped.x + clipped.width).ceil().min(self.width as f32) as u32;
        let y1 = (clipped.y + clipped.height).ceil().min(self.height as f32) as u32;

        let mut painted = 0usize;
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * self.width + x) * 4) as usize;
                let px = &self.pixels[i..i + 4];
                if px != self.bg.as_slice() {
                    painted += 1;
                }
            }
        }

        if painted > 0 {
            VisibilityVerdict::Visible {
                painted_pixels: painted,
            }
        } else {
            VisibilityVerdict::InvisibleInk
        }
    }
}

fn intersect(a: Rectangle, b: Rectangle) -> Rectangle {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rectangle {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0.0),
        height: (y1 - y0).max(0.0),
    }
}

fn rgba8(c: Color) -> [u8; 4] {
    let f = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    [f(c.r), f(c.g), f(c.b), f(c.a)]
}
