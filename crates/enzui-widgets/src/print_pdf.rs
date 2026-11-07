//! Pages as PDF, for printing and export.
//!
//! The print portal (`enzui_core::portal::print`) and every printer take a
//! finished PDF. [`PdfDocument`] lays out headings, paragraphs and images on
//! pages of a [`PageSetup`] with the same text engine and fonts the app draws
//! with, and [`PdfDocument::render`] writes them as vector text: sharp at any
//! resolution, selectable and searchable in a viewer. Fonts are embedded.
//!
//! [`PdfDocument::print_request`] wraps the bytes for the print portal.
//!
//! ```no_run
//! use enzui_widgets::print_pdf::{PageSetup, PdfDocument};
//!
//! let mut document = PdfDocument::new(PageSetup::A4);
//! document.title("Shopping list");
//! document.heading("Shopping list");
//! document.paragraph("Bread, milk, and something for the weekend.");
//! let pdf: Vec<u8> = document.render().expect("fonts are available");
//! # let _ = pdf;
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;

use iced::Font;
use iced::advanced::graphics::text::cosmic_text;
use iced::font::Weight;

/// Points per inch: PDF measures in points.
const POINTS_PER_INCH: f32 = 72.0;

/// A sheet size and its margins, in points (1/72 inch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    /// The sheet's width.
    pub width: f32,
    /// The sheet's height.
    pub height: f32,
    /// The blank border on every side.
    pub margin: f32,
}

impl PageSetup {
    /// ISO A4 (210 × 297 mm) with 2 cm margins.
    pub const A4: Self = Self {
        width: 595.28,
        height: 841.89,
        margin: 56.69,
    };

    /// US Letter (8.5 × 11 in) with 1 in margins.
    pub const LETTER: Self = Self {
        width: 612.0,
        height: 792.0,
        margin: POINTS_PER_INCH,
    };

    fn content_width(&self) -> f32 {
        (self.width - 2.0 * self.margin).max(1.0)
    }

    fn content_height(&self) -> f32 {
        (self.height - 2.0 * self.margin).max(1.0)
    }
}

/// How a paragraph is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// The font, as the app names it.
    pub font: Font,
    /// The size in points.
    pub size: f32,
    /// The line height as a multiple of the size.
    pub line_height: f32,
    /// Space after the paragraph, in points.
    pub space_after: f32,
    /// The colour, as sRGB components from 0 to 1.
    pub color: [f32; 3],
}

impl TextStyle {
    /// Body text: 11 pt, 1.4 lines.
    pub const BODY: Self = Self {
        font: Font::DEFAULT,
        size: 11.0,
        line_height: 1.4,
        space_after: 8.0,
        color: [0.0, 0.0, 0.0],
    };

    /// A heading: 18 pt bold.
    pub const HEADING: Self = Self {
        font: Font {
            weight: Weight::Bold,
            ..Font::DEFAULT
        },
        size: 18.0,
        line_height: 1.25,
        space_after: 10.0,
        color: [0.0, 0.0, 0.0],
    };
}

#[derive(Debug, Clone)]
enum Block {
    Text(String, TextStyle),
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        display_width: f32,
    },
    Space(f32),
    PageBreak,
}

/// Why a document could not be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    /// A font the text was shaped with could not be read for embedding.
    FontUnavailable(String),
    /// An image's bytes do not match its size.
    BadImage,
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FontUnavailable(name) => write!(f, "the font {name} could not be embedded"),
            Self::BadImage => f.write_str("an image's bytes do not match its size"),
        }
    }
}

impl std::error::Error for PdfError {}

/// A document of flowing blocks. See the [module](self) documentation.
#[derive(Debug, Clone)]
pub struct PdfDocument {
    setup: PageSetup,
    title: Option<String>,
    blocks: Vec<Block>,
}

impl PdfDocument {
    /// An empty document on `setup` pages.
    #[must_use]
    pub fn new(setup: PageSetup) -> Self {
        Self {
            setup,
            title: None,
            blocks: Vec::new(),
        }
    }

    /// The title a viewer and the print queue show.
    pub fn title(&mut self, title: impl Into<String>) -> &mut Self {
        self.title = Some(title.into());
        self
    }

    /// A heading in [`TextStyle::HEADING`].
    pub fn heading(&mut self, text: impl Into<String>) -> &mut Self {
        self.text(text, TextStyle::HEADING)
    }

    /// A paragraph in [`TextStyle::BODY`]; it wraps and flows onto further
    /// pages.
    pub fn paragraph(&mut self, text: impl Into<String>) -> &mut Self {
        self.text(text, TextStyle::BODY)
    }

    /// A paragraph in `style`.
    pub fn text(&mut self, text: impl Into<String>, style: TextStyle) -> &mut Self {
        self.blocks.push(Block::Text(text.into(), style));
        self
    }

    /// An image from straight RGBA pixels, `display_width` points wide (at
    /// most the text width), keeping its aspect ratio.
    pub fn image(
        &mut self,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        display_width: f32,
    ) -> &mut Self {
        self.blocks.push(Block::Image {
            rgba,
            width,
            height,
            display_width,
        });
        self
    }

    /// Blank space, in points.
    pub fn space(&mut self, points: f32) -> &mut Self {
        self.blocks.push(Block::Space(points));
        self
    }

    /// Starts a new page.
    pub fn page_break(&mut self) -> &mut Self {
        self.blocks.push(Block::PageBreak);
        self
    }

    /// The rendered document as a print portal request with a dialog
    /// (`enzui_core::portal::print`); set its title and parent window there.
    ///
    /// # Errors
    /// The document cannot be rendered, or the in-memory file not made.
    pub fn print_request(
        &self,
    ) -> Result<enzui_core::portal::print::PrintRequest, Box<dyn std::error::Error + Send + Sync>>
    {
        let pdf = self.render()?;
        Ok(enzui_core::portal::print::PrintRequest::with_dialog_bytes(
            &pdf,
            enzui_core::portal::print::PrintFormat::Pdf,
        )?)
    }

    /// Lays the blocks out on pages and writes the PDF.
    ///
    /// # Errors
    /// A font cannot be embedded, or an image is malformed.
    pub fn render(&self) -> Result<Vec<u8>, PdfError> {
        crate::sans_serif_fallback::ensure_sans_serif_fallback();
        let mut font_system = iced::advanced::graphics::text::font_system()
            .write()
            .map_err(|_| PdfError::FontUnavailable("(font system poisoned)".to_owned()))?;
        let pages = layout(self, font_system.raw())?;
        write_pdf(self, &pages, font_system.raw())
    }
}

/// What one page shows.
#[derive(Debug, Default)]
struct Page {
    glyphs: Vec<PlacedGlyph>,
    images: Vec<PlacedImage>,
}

#[derive(Debug, Clone)]
struct PlacedGlyph {
    font: cosmic_text::fontdb::ID,
    glyph: u16,
    size: f32,
    /// The baseline origin, in PDF coordinates (from the bottom left).
    x: f32,
    y: f32,
    advance: f32,
    color: [f32; 3],
    /// The text the glyph stands for, empty for later glyphs of a cluster.
    text: String,
}

#[derive(Debug, Clone)]
struct PlacedImage {
    index: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

fn layout(
    document: &PdfDocument,
    font_system: &mut cosmic_text::FontSystem,
) -> Result<Vec<Page>, PdfError> {
    let setup = document.setup;
    let mut pages = vec![Page::default()];
    // How far down the content area the next block starts.
    let mut cursor = 0.0_f32;
    let mut image_index = 0;
    for block in &document.blocks {
        match block {
            Block::Space(points) => cursor += points,
            Block::PageBreak => {
                pages.push(Page::default());
                cursor = 0.0;
            }
            Block::Image {
                rgba,
                width,
                height,
                display_width,
            } => {
                if *width == 0
                    || *height == 0
                    || rgba.len() as u64 != u64::from(*width) * u64::from(*height) * 4
                {
                    return Err(PdfError::BadImage);
                }
                let shown_width = display_width.clamp(1.0, setup.content_width());
                let shown_height = shown_width * (*height as f32 / *width as f32);
                if cursor > 0.0 && cursor + shown_height > setup.content_height() {
                    pages.push(Page::default());
                    cursor = 0.0;
                }
                let page = pages.last_mut().expect("a page");
                page.images.push(PlacedImage {
                    index: image_index,
                    x: setup.margin,
                    y: setup.height - setup.margin - cursor - shown_height,
                    width: shown_width,
                    height: shown_height,
                });
                image_index += 1;
                cursor += shown_height + TextStyle::BODY.space_after;
            }
            Block::Text(text, style) => {
                let metrics = cosmic_text::Metrics::new(style.size, style.size * style.line_height);
                let mut buffer = cosmic_text::Buffer::new(font_system, metrics);
                buffer.set_size(Some(setup.content_width()), None);
                let attributes = iced::advanced::graphics::text::to_attributes(style.font);
                buffer.set_text(text, &attributes, cosmic_text::Shaping::Advanced, None);
                buffer.shape_until_scroll(font_system, false);
                let mut paragraph_top = cursor;
                for run in buffer.layout_runs() {
                    if cursor > 0.0
                        && paragraph_top + run.line_top + run.line_height > setup.content_height()
                    {
                        pages.push(Page::default());
                        // The paragraph continues at the top of the new page.
                        paragraph_top = -run.line_top;
                    }
                    let page = pages.last_mut().expect("a page");
                    let baseline = setup.height - setup.margin - (paragraph_top + run.line_y);
                    let mut previous_cluster = None;
                    for glyph in run.glyphs {
                        let cluster = (glyph.start, glyph.end);
                        let text = if previous_cluster == Some(cluster) {
                            String::new()
                        } else {
                            run.text
                                .get(glyph.start..glyph.end)
                                .unwrap_or("")
                                .to_owned()
                        };
                        previous_cluster = Some(cluster);
                        page.glyphs.push(PlacedGlyph {
                            font: glyph.font_id,
                            glyph: glyph.glyph_id,
                            size: glyph.font_size,
                            x: setup.margin + glyph.x + glyph.x_offset,
                            y: baseline - glyph.y_offset,
                            advance: glyph.w,
                            color: style.color,
                            text,
                        });
                    }
                    cursor = paragraph_top + run.line_top + run.line_height;
                }
                cursor += style.space_after;
            }
        }
    }
    Ok(pages)
}

/// A font as embedded: its file, outline kind and the glyphs used.
struct Embedded {
    name: String,
    data: Vec<u8>,
    cff: bool,
    /// Glyph id to (advance in 1/1000 em, the text it stands for).
    glyphs: BTreeMap<u16, (i64, String)>,
}

struct Writer {
    out: Vec<u8>,
    offsets: Vec<usize>,
}

impl Writer {
    fn new() -> Self {
        let mut out = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n".to_vec();
        out.shrink_to_fit();
        Self {
            out,
            offsets: Vec::new(),
        }
    }

    /// Reserves the next object number.
    fn reserve(&mut self) -> usize {
        self.offsets.push(0);
        self.offsets.len()
    }

    fn object(&mut self, id: usize, body: &str) {
        self.offsets[id - 1] = self.out.len();
        let _ = write!(self.out, "{id} 0 obj\n{body}\nendobj\n");
    }

    fn stream(&mut self, id: usize, dictionary: &str, data: &[u8]) {
        let compressed = deflate(data);
        self.offsets[id - 1] = self.out.len();
        let _ = write!(
            self.out,
            "{id} 0 obj\n<< {dictionary} /Filter /FlateDecode /Length {} >>\nstream\n",
            compressed.len()
        );
        self.out.extend_from_slice(&compressed);
        self.out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn finish(mut self, root: usize, info: usize) -> Vec<u8> {
        let xref = self.out.len();
        let _ = write!(
            self.out,
            "xref\n0 {}\n0000000000 65535 f \n",
            self.offsets.len() + 1
        );
        for offset in &self.offsets {
            let _ = writeln!(self.out, "{offset:010} 00000 n ");
        }
        let _ = write!(
            self.out,
            "trailer\n<< /Size {} /Root {root} 0 R /Info {info} 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            self.offsets.len() + 1
        );
        self.out
    }
}

fn deflate(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    let _ = encoder.write_all(data);
    encoder.finish().unwrap_or_default()
}

/// A text string as PDF UTF-16BE hex.
fn pdf_text(text: &str) -> String {
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        let _ = write!(out, "{unit:04X}");
    }
    out.push('>');
    out
}

fn write_pdf(
    document: &PdfDocument,
    pages: &[Page],
    font_system: &cosmic_text::FontSystem,
) -> Result<Vec<u8>, PdfError> {
    // Collect the fonts and glyphs every page uses.
    let mut fonts: BTreeMap<cosmic_text::fontdb::ID, Embedded> = BTreeMap::new();
    for glyph in pages.iter().flat_map(|page| &page.glyphs) {
        let font = match fonts.entry(glyph.font) {
            std::collections::btree_map::Entry::Occupied(font) => font.into_mut(),
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(embed(font_system, glyph.font)?)
            }
        };
        let advance = (glyph.advance / glyph.size * 1000.0).round() as i64;
        let entry = font
            .glyphs
            .entry(glyph.glyph)
            .or_insert((advance, String::new()));
        if entry.1.is_empty() && !glyph.text.is_empty() {
            entry.1.clone_from(&glyph.text);
        }
    }
    let font_names: BTreeMap<cosmic_text::fontdb::ID, String> = fonts
        .keys()
        .enumerate()
        .map(|(index, id)| (*id, format!("F{index}")))
        .collect();

    let mut writer = Writer::new();
    let catalog = writer.reserve();
    let pages_id = writer.reserve();
    let info = writer.reserve();

    // Fonts: Type0 over a CIDFont with Identity-H, the font file and the
    // glyph-to-text map that makes the text searchable.
    let mut font_refs = String::new();
    for (id, font) in &fonts {
        let type0 = writer.reserve();
        let cid = writer.reserve();
        let descriptor = writer.reserve();
        let file = writer.reserve();
        let to_unicode = writer.reserve();
        let plain = font
            .name
            .replace([' ', '/', '(', ')', '[', ']', '<', '>', '{', '}', '%'], "");
        // Only TrueType outlines are cut down to the glyphs used; the tag says so.
        let subset = (!font.cff)
            .then(|| subset_truetype(&font.data, font.glyphs.keys().copied()))
            .flatten();
        let name = if subset.is_some() {
            format!("ENZUIA+{plain}")
        } else {
            plain
        };
        let data = subset.as_deref().unwrap_or(&font.data);
        writer.object(
            type0,
            &format!(
                "<< /Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H \
                 /DescendantFonts [{cid} 0 R] /ToUnicode {to_unicode} 0 R >>"
            ),
        );
        let mut widths = String::new();
        for (glyph, (advance, _)) in &font.glyphs {
            let _ = write!(widths, "{glyph} [{advance}] ");
        }
        let (subtype, file_key) = if font.cff {
            ("CIDFontType0", "FontFile3")
        } else {
            ("CIDFontType2", "FontFile2")
        };
        let map = if font.cff {
            ""
        } else {
            "/CIDToGIDMap /Identity"
        };
        writer.object(
            cid,
            &format!(
                "<< /Type /Font /Subtype /{subtype} /BaseFont /{name} \
                 /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                 /FontDescriptor {descriptor} 0 R /DW 1000 /W [{widths}] {map} >>"
            ),
        );
        writer.object(
            descriptor,
            &format!(
                "<< /Type /FontDescriptor /FontName /{name} /Flags 4 \
                 /FontBBox [-1000 -1000 2000 2000] /ItalicAngle 0 /Ascent 1000 /Descent -300 \
                 /CapHeight 700 /StemV 80 /{file_key} {file} 0 R >>"
            ),
        );
        let file_dictionary = if font.cff {
            "/Subtype /OpenType".to_owned()
        } else {
            format!("/Length1 {}", data.len())
        };
        writer.stream(file, &file_dictionary, data);
        writer.stream(to_unicode, "", to_unicode_cmap(font).as_bytes());
        let _ = write!(font_refs, "/{} {type0} 0 R ", font_names[id]);
    }

    // Images, as RGB with the alpha as a soft mask.
    let mut image_ids = Vec::new();
    for block in &document.blocks {
        let Block::Image {
            rgba,
            width,
            height,
            ..
        } = block
        else {
            continue;
        };
        let image = writer.reserve();
        let mask = writer.reserve();
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect();
        let alpha: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|pixel| pixel[3])
            .collect();
        writer.stream(
            mask,
            &format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} \
                 /ColorSpace /DeviceGray /BitsPerComponent 8"
            ),
            &alpha,
        );
        writer.stream(
            image,
            &format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} \
                 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {mask} 0 R"
            ),
            &rgb,
        );
        image_ids.push(image);
    }
    let mut image_refs = String::new();
    for (index, id) in image_ids.iter().enumerate() {
        let _ = write!(image_refs, "/Im{index} {id} 0 R ");
    }

    let mut kids = String::new();
    for page in pages {
        let page_id = writer.reserve();
        let content = writer.reserve();
        let mut ops = String::new();
        for image in &page.images {
            let _ = writeln!(
                ops,
                "q {:.3} 0 0 {:.3} {:.3} {:.3} cm /Im{} Do Q",
                image.width, image.height, image.x, image.y, image.index
            );
        }
        if !page.glyphs.is_empty() {
            ops.push_str("BT\n");
            let mut current: Option<(cosmic_text::fontdb::ID, u32, [u32; 3])> = None;
            for glyph in &page.glyphs {
                let size_key = glyph.size.to_bits();
                let color_key = glyph.color.map(f32::to_bits);
                if current.is_none_or(|(font, size, _)| font != glyph.font || size != size_key) {
                    let _ = writeln!(ops, "/{} {:.2} Tf", font_names[&glyph.font], glyph.size);
                }
                if current.is_none_or(|(_, _, color)| color != color_key) {
                    let [r, g, b] = glyph.color;
                    let _ = writeln!(ops, "{r:.3} {g:.3} {b:.3} rg");
                }
                current = Some((glyph.font, size_key, color_key));
                let _ = writeln!(
                    ops,
                    "1 0 0 1 {:.3} {:.3} Tm <{:04X}> Tj",
                    glyph.x, glyph.y, glyph.glyph
                );
            }
            ops.push_str("ET\n");
        }
        writer.stream(content, "", ops.as_bytes());
        writer.object(
            page_id,
            &format!(
                "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {:.2} {:.2}] \
                 /Resources << /Font << {font_refs}>> /XObject << {image_refs}>> >> \
                 /Contents {content} 0 R >>",
                document.setup.width, document.setup.height
            ),
        );
        let _ = write!(kids, "{page_id} 0 R ");
    }
    writer.object(
        pages_id,
        &format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()),
    );
    writer.object(
        catalog,
        &format!("<< /Type /Catalog /Pages {pages_id} 0 R >>"),
    );
    let title = document
        .title
        .as_deref()
        .map(|title| format!("/Title {} ", pdf_text(title)))
        .unwrap_or_default();
    writer.object(info, &format!("<< {title}/Producer (enzUI) >>"));
    Ok(writer.finish(catalog, info))
}

/// The CMap from glyph ids to the text they show.
fn to_unicode_cmap(font: &Embedded) -> String {
    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def \
         /CMapName /Adobe-Identity-UCS def /CMapType 2 def \
         1 begincodespacerange <0000> <FFFF> endcodespacerange\n",
    );
    let mapped: Vec<(&u16, &String)> = font
        .glyphs
        .iter()
        .filter(|(_, (_, text))| !text.is_empty())
        .map(|(glyph, (_, text))| (glyph, text))
        .collect();
    for chunk in mapped.chunks(100) {
        let _ = writeln!(cmap, "{} beginbfchar", chunk.len());
        for (glyph, text) in chunk {
            let units: String = text
                .encode_utf16()
                .map(|unit| format!("{unit:04X}"))
                .collect();
            let _ = writeln!(cmap, "<{glyph:04X}> <{units}>");
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap CMapName currentdict /CMap defineresource pop end end\n");
    cmap
}

fn embed(
    font_system: &cosmic_text::FontSystem,
    id: cosmic_text::fontdb::ID,
) -> Result<Embedded, PdfError> {
    let db = font_system.db();
    let name = db
        .face(id)
        .map_or_else(|| "Font".to_owned(), |face| face.post_script_name.clone());
    let data = db
        .with_face_data(id, single_face)
        .flatten()
        .ok_or_else(|| PdfError::FontUnavailable(name.clone()))?;
    let cff = table(&data, *b"CFF ").is_some() || table(&data, *b"CFF2").is_some();
    Ok(Embedded {
        name,
        data,
        cff,
        glyphs: BTreeMap::new(),
    })
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn read_u16(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

/// The table `tag` of a single-face font file.
fn table(data: &[u8], tag: [u8; 4]) -> Option<&[u8]> {
    let count = usize::from(read_u16(data, 4)?);
    (0..count).find_map(|index| {
        let record = 12 + index * 16;
        if data.get(record..record + 4)? != tag.as_slice() {
            return None;
        }
        let offset = read_u32(data, record + 8)? as usize;
        let length = read_u32(data, record + 12)? as usize;
        data.get(offset..offset + length)
    })
}

/// The tables a printed TrueType font keeps: outlines, metrics, hinting and
/// names. Shaping tables have done their work by the time a PDF is written.
const KEPT_TABLES: [&[u8; 4]; 11] = [
    b"head", b"hhea", b"maxp", b"loca", b"glyf", b"hmtx", b"cvt ", b"fpgm", b"prep", b"name",
    b"post",
];

/// `data` with only the outlines of `used` (and the glyphs composites draw
/// on) kept; every other glyph is emptied, so glyph ids stay as they were.
/// `None` when the font is not plain TrueType.
fn subset_truetype(data: &[u8], used: impl Iterator<Item = u16>) -> Option<Vec<u8>> {
    let head = table(data, *b"head")?;
    let long = read_u16(head, 50)? == 1;
    let glyph_count = usize::from(read_u16(table(data, *b"maxp")?, 4)?);
    let loca = table(data, *b"loca")?;
    let glyf = table(data, *b"glyf")?;
    let bounds = |glyph: usize| -> Option<(usize, usize)> {
        if long {
            Some((
                read_u32(loca, glyph * 4)? as usize,
                read_u32(loca, glyph * 4 + 4)? as usize,
            ))
        } else {
            Some((
                usize::from(read_u16(loca, glyph * 2)?) * 2,
                usize::from(read_u16(loca, glyph * 2 + 2)?) * 2,
            ))
        }
    };
    // The used glyphs, .notdef, and every component a composite refers to.
    let mut keep = vec![false; glyph_count];
    let mut pending: Vec<usize> = used.map(usize::from).chain([0]).collect();
    while let Some(glyph) = pending.pop() {
        if glyph >= glyph_count || keep[glyph] {
            continue;
        }
        keep[glyph] = true;
        let (start, end) = bounds(glyph)?;
        let outline = glyf.get(start..end)?;
        if outline.len() >= 10 && (read_u16(outline, 0)? as i16) < 0 {
            let mut at = 10;
            loop {
                let flags = read_u16(outline, at)?;
                pending.push(usize::from(read_u16(outline, at + 2)?));
                at += 4 + if flags & 0x0001 != 0 { 4 } else { 2 };
                at += match flags {
                    flags if flags & 0x0008 != 0 => 2,
                    flags if flags & 0x0040 != 0 => 4,
                    flags if flags & 0x0080 != 0 => 8,
                    _ => 0,
                };
                if flags & 0x0020 == 0 {
                    break;
                }
            }
        }
    }
    let mut new_glyf = Vec::new();
    let mut new_loca = Vec::with_capacity((glyph_count + 1) * 4);
    for (glyph, kept) in keep.iter().enumerate() {
        new_loca.extend_from_slice(&u32::try_from(new_glyf.len()).ok()?.to_be_bytes());
        if *kept {
            let (start, end) = bounds(glyph)?;
            new_glyf.extend_from_slice(glyf.get(start..end)?);
            while new_glyf.len() % 4 != 0 {
                new_glyf.push(0);
            }
        }
    }
    new_loca.extend_from_slice(&u32::try_from(new_glyf.len()).ok()?.to_be_bytes());
    let mut new_head = head.to_vec();
    // Long offsets, and no whole-file checksum to keep true.
    new_head
        .get_mut(50..52)?
        .copy_from_slice(&1u16.to_be_bytes());
    new_head
        .get_mut(8..12)?
        .copy_from_slice(&0u32.to_be_bytes());
    let tables: Vec<([u8; 4], Vec<u8>)> = KEPT_TABLES
        .iter()
        .filter_map(|tag| {
            let bytes = match *tag {
                b"head" => new_head.clone(),
                b"loca" => new_loca.clone(),
                b"glyf" => new_glyf.clone(),
                other => table(data, *other)?.to_vec(),
            };
            Some((**tag, bytes))
        })
        .collect();
    let count = u16::try_from(tables.len()).ok()?;
    let mut out = Vec::new();
    out.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    out.extend_from_slice(&count.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut offset = 12 + tables.len() * 16;
    let mut body = Vec::new();
    for (tag, bytes) in &tables {
        let checksum = bytes
            .chunks(4)
            .map(|chunk| {
                let mut word = [0u8; 4];
                word[..chunk.len()].copy_from_slice(chunk);
                u32::from_be_bytes(word)
            })
            .fold(0u32, u32::wrapping_add);
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&u32::try_from(offset).ok()?.to_be_bytes());
        out.extend_from_slice(&u32::try_from(bytes.len()).ok()?.to_be_bytes());
        body.extend_from_slice(bytes);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + tables.len() * 16 + body.len();
    }
    out.extend_from_slice(&body);
    Some(out)
}

/// The font file of face `index`: the file itself, or that face rebuilt as
/// a standalone font when the file is a collection (`ttcf`).
fn single_face(data: &[u8], index: u32) -> Option<Vec<u8>> {
    if data.get(0..4)? != b"ttcf" {
        return Some(data.to_vec());
    }
    let face = read_u32(data, 12 + 4 * index as usize)? as usize;
    let count = usize::from(read_u16(data, face + 4)?);
    let mut records = Vec::with_capacity(count);
    for record in 0..count {
        let at = face + 12 + record * 16;
        let tag: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        let checksum = read_u32(data, at + 4)?;
        let offset = read_u32(data, at + 8)? as usize;
        let length = read_u32(data, at + 12)? as usize;
        records.push((tag, checksum, data.get(offset..offset + length)?));
    }
    let mut out = data.get(face..face + 12)?.to_vec();
    let mut body_offset = 12 + count * 16;
    let mut body = Vec::new();
    for (tag, checksum, table) in &records {
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum.to_be_bytes());
        out.extend_from_slice(&u32::try_from(body_offset).ok()?.to_be_bytes());
        out.extend_from_slice(&u32::try_from(table.len()).ok()?.to_be_bytes());
        body.extend_from_slice(table);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        body_offset = 12 + count * 16 + body.len();
    }
    out.extend_from_slice(&body);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(paragraphs: usize) -> PdfDocument {
        let mut document = PdfDocument::new(PageSetup::A4);
        document.title("Test – Ünïcode");
        document.heading("Heading");
        for index in 0..paragraphs {
            document.paragraph(format!(
                "Paragraph {index}: the quick brown fox jumps over the lazy dog, \
                 again and again, until the line wraps onto the next one."
            ));
        }
        document
    }

    #[test]
    fn pdf_output_preserves_searchable_text_objects_fonts_and_image_admission_1() {
        // text_flows_onto_more_pages_and_stays_searchable
        {
            crate::sans_serif_fallback::ensure_sans_serif_fallback();
            let mut font_system = iced::advanced::graphics::text::font_system()
                .write()
                .unwrap();
            let short = layout(&document(2), font_system.raw()).unwrap();
            assert_eq!(short.len(), 1);
            let long = layout(&document(60), font_system.raw()).unwrap();
            assert!(long.len() > 2, "{} pages", long.len());
            for page in &long {
                for glyph in &page.glyphs {
                    assert!(
                        glyph.y >= PageSetup::A4.margin - 1.0,
                        "below the margin: {glyph:?}"
                    );
                    assert!(
                        glyph.y <= PageSetup::A4.height - PageSetup::A4.margin,
                        "{glyph:?}"
                    );
                }
            }
            let text: String = long[0]
                .glyphs
                .iter()
                .map(|glyph| glyph.text.as_str())
                .collect();
            assert!(text.starts_with("HeadingParagraph 0: the quick"), "{text}");
            drop(font_system);

            let pdf = document(60).render().unwrap();
            assert!(pdf.starts_with(b"%PDF-1.7"));
            assert!(pdf.ends_with(b"%%EOF\n"));
            let count = format!("/Count {}", long.len());
            assert!(
                pdf.windows(count.len())
                    .any(|window| window == count.as_bytes())
            );
        }

        // the_cross_reference_table_points_at_every_object
        {
            let pdf = document(3).render().unwrap();
            let find = |needle: &[u8]| {
                pdf.windows(needle.len())
                    .rposition(|window| window == needle)
                    .unwrap()
            };
            let start = find(b"startxref\n") + b"startxref\n".len();
            let end = start + pdf[start..].iter().position(|byte| *byte == b'\n').unwrap();
            let xref: usize = std::str::from_utf8(&pdf[start..end])
                .unwrap()
                .parse()
                .unwrap();
            let table = std::str::from_utf8(&pdf[xref..start]).unwrap();
            let entries: Vec<&str> = table
                .lines()
                .skip(3)
                .take_while(|line| line.ends_with(" n "))
                .collect();
            assert!(entries.len() > 10, "{table}");
            for (number, line) in entries.iter().enumerate() {
                let offset: usize = line.get(..10).unwrap().parse().unwrap();
                let expected = format!("{} 0 obj", number + 1);
                assert!(
                    pdf[offset..].starts_with(expected.as_bytes()),
                    "object {} is not at {offset}",
                    number + 1
                );
            }
        }

        // a_face_is_taken_out_of_a_collection
        {
            // A collection of two faces, each with one table.
            let mut ttc = b"ttcf\x00\x01\x00\x00\x00\x00\x00\x02".to_vec();
            ttc.extend_from_slice(&20u32.to_be_bytes());
            ttc.extend_from_slice(&48u32.to_be_bytes());
            for (tag, offset) in [(b"AAAA", 76u32), (b"BBBB", 80u32)] {
                ttc.extend_from_slice(b"\x00\x01\x00\x00\x00\x01\x00\x10\x00\x00\x00\x00");
                ttc.extend_from_slice(tag);
                ttc.extend_from_slice(&0u32.to_be_bytes());
                ttc.extend_from_slice(&offset.to_be_bytes());
                ttc.extend_from_slice(&4u32.to_be_bytes());
            }
            ttc.extend_from_slice(b"aaaabbbb");
            let second = single_face(&ttc, 1).unwrap();
            assert_eq!(table(&second, *b"BBBB"), Some(&b"bbbb"[..]));
            assert_eq!(table(&second, *b"AAAA"), None);
        }

        // a_subset_keeps_the_used_outlines_and_their_components
        {
            let mut document = PdfDocument::new(PageSetup::A4);
            document.paragraph("Á");
            let mut font_system = iced::advanced::graphics::text::font_system()
                .write()
                .unwrap();
            let pages = layout(&document, font_system.raw()).unwrap();
            let placed = pages[0].glyphs[0].clone();
            let font = embed(font_system.raw(), placed.font).unwrap();
            assert!(!font.cff, "the test font has TrueType outlines");
            let subset = subset_truetype(&font.data, [placed.glyph].into_iter()).unwrap();
            assert!(
                subset.len() < font.data.len() / 2,
                "most outlines are dropped"
            );
            let original = outline(&font.data, placed.glyph);
            assert_eq!(outline(&subset, placed.glyph), original);
            let glyph_count = read_u16(table(&subset, *b"maxp").unwrap(), 4).unwrap();
            let kept: Vec<u16> = (0..glyph_count)
                .filter(|glyph| !outline(&subset, *glyph).is_empty())
                .collect();
            if (read_u16(&original, 0).unwrap() as i16) < 0 {
                // A composite: its first component is kept as well.
                let component = read_u16(&original, 12).unwrap();
                assert!(
                    kept.contains(&component),
                    "component {component} kept: {kept:?}"
                );
            }
            assert!(
                kept.len() <= 4,
                "only the used glyphs, their parts and .notdef: {kept:?}"
            );
        }
    }

    /// The outline of `glyph` in a single-face TrueType file.
    fn outline(data: &[u8], glyph: u16) -> Vec<u8> {
        let long = read_u16(table(data, *b"head").unwrap(), 50).unwrap() == 1;
        let loca = table(data, *b"loca").unwrap();
        let glyph = usize::from(glyph);
        let (start, end) = if long {
            (
                read_u32(loca, glyph * 4).unwrap() as usize,
                read_u32(loca, glyph * 4 + 4).unwrap() as usize,
            )
        } else {
            (
                usize::from(read_u16(loca, glyph * 2).unwrap()) * 2,
                usize::from(read_u16(loca, glyph * 2 + 2).unwrap()) * 2,
            )
        };
        table(data, *b"glyf").unwrap()[start..end].to_vec()
    }

    #[test]
    fn a_malformed_image_is_refused() {
        let mut document = PdfDocument::new(PageSetup::LETTER);
        document.image(vec![0; 7], 2, 1, 100.0);
        assert_eq!(document.render(), Err(PdfError::BadImage));
    }
}
