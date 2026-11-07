//! The GPU side: glyph atlases, per-terminal buffers and the draw.
//!
//! Per frame, a terminal whose screen and geometry did not change costs a
//! lock and two comparisons: its buffers stay on the GPU and are drawn as
//! they are. A change rebuilds only the damaged rows' instances and
//! background colours, rasterising a glyph only the first time it is seen.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use iced::Rectangle;
use iced_wgpu::graphics::Viewport;
use iced_wgpu::primitive::{Pipeline, Primitive};
use iced_wgpu::wgpu;

use crate::font::{CellMetrics, Rasterizer, TerminalFont};
use crate::screen::{Cell, CellFlags, CursorShape, Rgba, ScreenState, TerminalScreen};
use crate::{TerminalStyle, boxdraw};

/// What one terminal draws in one frame.
#[derive(Debug)]
pub(crate) struct TerminalPrimitive {
    pub screen: TerminalScreen,
    pub style: Arc<TerminalStyle>,
    pub focused: bool,
    /// Whether the cursor is in the visible half of its blink.
    pub cursor_on: bool,
    /// The rows drawn above the view's bounds, and how strongly they fade
    /// into the background towards their top (see
    /// [`crate::EnzTerminal::fade_rows_above`]).
    pub fade_above: (u16, f32),
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
struct Uniforms {
    viewport_origin: [f32; 2],
    viewport_size: [f32; 2],
    grid_origin: [f32; 2],
    cell: [f32; 2],
    grid: [u32; 2],
    default_bg: u32,
    /// Bit 0: extend edge cells' backgrounds to the widget's edge.
    flags: u32,
    /// The fade over the rows above the bounds: its top and bottom in
    /// framebuffer pixels, and its strength at the top; none at zero.
    fade: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
pub(crate) struct Instance {
    cell: u32,
    offset: u32,
    size: u32,
    uv: u32,
    color: u32,
    kind: u32,
}

const KIND_MASK: u32 = 0;
const KIND_COLOR: u32 = 1;
const KIND_SOLID: u32 = 2;

fn pack(a: u32, b: u32) -> u32 {
    (a & 0xffff) | (b << 16)
}

fn pack_signed(a: i32, b: i32) -> u32 {
    (a as u32 & 0xffff) | ((b as u32 & 0xffff) << 16)
}

impl Instance {
    fn solid(col: u16, row: u16, x: i32, y: i32, w: u32, h: u32, color: Rgba) -> Self {
        Self {
            cell: pack(u32::from(col), u32::from(row)),
            offset: pack_signed(x, y),
            size: pack(w, h),
            uv: 0,
            color: color.packed(),
            kind: KIND_SOLID,
        }
    }
}

/// A glyph image in an atlas, placed relative to its cell.
#[derive(Debug, Clone, Copy)]
struct Placed {
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    color: bool,
}

/// One 2D texture, packed with `etagere`, with a CPU copy so it can grow.
struct Atlas {
    format: wgpu::TextureFormat,
    bytes_per_pixel: u32,
    size: u32,
    allocator: etagere::BucketedAtlasAllocator,
    pixels: Vec<u8>,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

const ATLAS_START: u32 = 512;

impl Atlas {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, bytes_per_pixel: u32) -> Self {
        let (texture, view) = Self::texture(device, format, ATLAS_START);
        Self {
            format,
            bytes_per_pixel,
            size: ATLAS_START,
            allocator: etagere::BucketedAtlasAllocator::new(etagere::size2(
                ATLAS_START as i32,
                ATLAS_START as i32,
            )),
            pixels: vec![0; (ATLAS_START * ATLAS_START * bytes_per_pixel) as usize],
            texture,
            view,
        }
    }

    fn texture(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: u32,
    ) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("enzui terminal atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Stores `data`; `None` when the atlas is full and cannot grow.
    /// `grew` is set when the texture was replaced.
    fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
        grew: &mut bool,
    ) -> Option<(u32, u32)> {
        // One pixel of padding keeps neighbours apart.
        let size = etagere::size2(width as i32 + 1, height as i32 + 1);
        let allocation = loop {
            if let Some(allocation) = self.allocator.allocate(size) {
                break allocation;
            }
            let max = device.limits().max_texture_dimension_2d.min(8192);
            if self.size >= max {
                return None;
            }
            self.grow(device, queue, (self.size * 2).min(max));
            *grew = true;
        };
        let (x, y) = (
            allocation.rectangle.min.x as u32,
            allocation.rectangle.min.y as u32,
        );
        let bpp = self.bytes_per_pixel;
        for row in 0..height {
            let src = &data[(row * width * bpp) as usize..((row + 1) * width * bpp) as usize];
            let start = (((y + row) * self.size + x) * bpp) as usize;
            self.pixels[start..start + src.len()].copy_from_slice(src);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * bpp),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        Some((x, y))
    }

    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: u32) {
        let bpp = self.bytes_per_pixel;
        let mut pixels = vec![0; (size * size * bpp) as usize];
        for row in 0..self.size {
            let src = (row * self.size * bpp) as usize;
            let dst = (row * size * bpp) as usize;
            let len = (self.size * bpp) as usize;
            pixels[dst..dst + len].copy_from_slice(&self.pixels[src..src + len]);
        }
        self.allocator
            .grow(etagere::size2(size as i32, size as i32));
        let (texture, view) = Self::texture(device, self.format, size);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * bpp),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        self.size = size;
        self.pixels = pixels;
        self.texture = texture;
        self.view = view;
    }

    fn clear(&mut self, device: &wgpu::Device) {
        *self = Self::new(device, self.format, self.bytes_per_pixel);
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct GlyphKey {
    ch: char,
    combining: Option<Box<str>>,
    style: u8,
    span: u8,
}

/// A cell's glyph images; one image is the common case and is kept inline.
#[derive(Clone)]
enum Glyphs {
    None,
    One(Placed),
    Many(Arc<[Placed]>),
}

impl Glyphs {
    fn as_slice(&self) -> &[Placed] {
        match self {
            Self::None => &[],
            Self::One(placed) => std::slice::from_ref(placed),
            Self::Many(placed) => placed,
        }
    }
}

/// rustc's Fx hash: one rotate, xor and multiply per word. Glyph lookups
/// happen once per visible cell per changed row, so SipHash showed up in
/// profiles.
#[derive(Default, Clone, Copy)]
struct FxHasher(u64);

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }

    fn write_u8(&mut self, n: u8) {
        self.write_u64(u64::from(n));
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

type FxMap<K, V> = HashMap<K, V, std::hash::BuildHasherDefault<FxHasher>>;

/// Glyphs rasterised for one font at one size.
struct GlyphCache {
    /// Single ASCII characters, indexed by `ch | style << 7`.
    ascii: Vec<Option<Glyphs>>,
    glyphs: FxMap<GlyphKey, Glyphs>,
    curl: Option<Placed>,
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self {
            ascii: vec![None; 128 * 4],
            glyphs: FxMap::default(),
            curl: None,
        }
    }
}

impl GlyphCache {
    fn ascii_slot(cell: &Cell, style: u8, span: u32) -> Option<usize> {
        (cell.combining.is_none() && (cell.ch as u32) < 128 && span == 1)
            .then(|| cell.ch as usize | usize::from(style) << 7)
    }

    fn get(&self, cell: &Cell, style: u8, span: u32) -> Option<Glyphs> {
        if let Some(slot) = Self::ascii_slot(cell, style, span) {
            return self.ascii[slot].clone();
        }
        self.glyphs
            .get(&GlyphKey {
                ch: cell.ch,
                combining: cell.combining.clone(),
                style,
                span: span as u8,
            })
            .cloned()
    }

    fn insert(&mut self, cell: &Cell, style: u8, span: u32, glyphs: Glyphs) {
        if let Some(slot) = Self::ascii_slot(cell, style, span) {
            self.ascii[slot] = Some(glyphs);
        } else {
            self.glyphs.insert(
                GlyphKey {
                    ch: cell.ch,
                    combining: cell.combining.clone(),
                    style,
                    span: span as u8,
                },
                glyphs,
            );
        }
    }
}

#[derive(Clone, PartialEq)]
struct FontKey {
    font: TerminalFont,
    scale_bits: u32,
}

/// GPU state of one terminal.
struct TerminalGpu {
    uniforms: Uniforms,
    uniform_buffer: wgpu::Buffer,
    bg_buffer: wgpu::Buffer,
    bg_capacity: u64,
    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    instance_count: u32,
    bind_group: wgpu::BindGroup,
    bg: Vec<u32>,
    rows: Vec<Vec<Instance>>,
    /// Each row's first instance in the uploaded buffer, then the total;
    /// empty until the first upload.
    instance_starts: Vec<usize>,
    flat: Vec<Instance>,
    snapshot: Snapshot,
    grid: (u16, u16),
    metrics: Option<CellMetrics>,
    style: Option<Arc<TerminalStyle>>,
    focused: bool,
    cursor_on: bool,
    /// The atlas epoch the instances were built against.
    epoch: Option<u64>,
    used_frame: u64,
    /// The frame this screen was last prepared in, to catch a second view.
    prepared_in: Option<u64>,
    /// How many prepares in a row the buffers have been oversized.
    oversized_for: u32,
}

/// Prepares in a row a buffer must stay more than four times larger than its
/// contents before it shrinks: a burst (a full-screen redraw, a big resize)
/// keeps its room, a terminal that settled smaller gives it back.
const SHRINK_AFTER: u32 = 300;

/// The capacity a buffer holding `needed` units should have, given its
/// `capacity` and how long it has been oversized: grown to fit at once,
/// shrunk only after [`SHRINK_AFTER`] oversized prepares. `None` keeps it.
fn resized_capacity(needed: u64, capacity: u64, oversized_for: u32) -> Option<u64> {
    let fitting = needed.max(1).next_power_of_two();
    let grow = needed > capacity;
    let shrink = oversized_for >= SHRINK_AFTER && needed.saturating_mul(4) < capacity;
    ((grow || shrink) && fitting != capacity).then_some(fitting)
}

/// The rows whose instances an upload rewrites, given each row's first
/// instance from the last upload (`starts`, ending with the total) and the
/// damaged rows. Damaged rows that kept their instance counts rewrite only
/// their span; a changed count moves every later row, so the upload runs to
/// the last row. `None` when nothing was damaged.
fn instance_rows(
    starts: &[usize],
    rows: &[Vec<Instance>],
    damaged: &[u16],
) -> Option<std::ops::Range<usize>> {
    let first = usize::from(*damaged.iter().min()?);
    let last = usize::from(*damaged.iter().max()?);
    let kept = (first..=last).all(|y| {
        let (Some(row), Some(start), Some(end)) = (rows.get(y), starts.get(y), starts.get(y + 1))
        else {
            return false;
        };
        end - start == row.len()
    });
    Some(first.min(rows.len())..if kept { last + 1 } else { rows.len() })
}

/// Shared by every terminal in the window.
pub(crate) struct TerminalPipeline {
    bg_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    terminal_layout: wgpu::BindGroupLayout,
    atlas_layout: wgpu::BindGroupLayout,
    atlas_bind_group: wgpu::BindGroup,
    mask: Atlas,
    color: Atlas,
    rasterizer: Rasterizer,
    fonts: Vec<(FontKey, CellMetrics, GlyphCache)>,
    terminals: HashMap<u64, TerminalGpu>,
    frame: u64,
    /// Atlas resets since creation; every terminal rebuilds after one.
    atlas_epoch: u64,
}

impl std::fmt::Debug for TerminalPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalPipeline")
            .field("terminals", &self.terminals.len())
            .finish_non_exhaustive()
    }
}

impl Pipeline for TerminalPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("enzui terminal shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let terminal_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enzui terminal layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enzui terminal atlas layout"),
            entries: &[texture_entry(0), texture_entry(1)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("enzui terminal pipeline layout"),
            bind_group_layouts: &[Some(&terminal_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });
        let target = [Some(wgpu::ColorTargetState {
            format,
            blend: Some(wgpu::BlendState::ALPHA_BLENDING),
            write_mask: wgpu::ColorWrites::ALL,
        })];
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..wgpu::PrimitiveState::default()
        };
        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("enzui terminal backgrounds"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("bg_vs"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("bg_fs"),
                targets: &target,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive,
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("enzui terminal glyphs"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("glyph_vs"),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Uint32, 1 => Uint32, 2 => Uint32, 3 => Uint32, 4 => Uint32, 5 => Uint32
                    ],
                }],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("glyph_fs"),
                targets: &target,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive,
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let mask = Atlas::new(device, wgpu::TextureFormat::R8Unorm, 1);
        let color = Atlas::new(device, wgpu::TextureFormat::Rgba8Unorm, 4);
        let atlas_bind_group = Self::atlas_bind_group(device, &atlas_layout, &mask, &color);
        Self {
            bg_pipeline,
            glyph_pipeline,
            terminal_layout,
            atlas_layout,
            atlas_bind_group,
            mask,
            color,
            rasterizer: Rasterizer::new(),
            fonts: Vec::new(),
            terminals: HashMap::new(),
            frame: 0,
            atlas_epoch: 0,
        }
    }

    fn trim(&mut self) {
        self.frame += 1;
        let frame = self.frame;
        // A terminal not drawn for a few seconds has left the window.
        self.terminals
            .retain(|_, terminal| frame.saturating_sub(terminal.used_frame) < 600);
    }
}

impl TerminalPipeline {
    fn atlas_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        mask: &Atlas,
        color: &Atlas,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("enzui terminal atlas"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mask.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color.view),
                },
            ],
        })
    }

    fn font_index(&mut self, font: &TerminalFont, scale: f32) -> usize {
        let key = FontKey {
            font: font.clone(),
            scale_bits: scale.to_bits(),
        };
        if let Some(index) = self.fonts.iter().position(|(known, ..)| *known == key) {
            return index;
        }
        let metrics = CellMetrics::measure(font, scale);
        self.fonts.push((key, metrics, GlyphCache::default()));
        self.fonts.len() - 1
    }

    /// The atlas images for one cell's glyph, rasterising on first use.
    #[allow(
        clippy::too_many_arguments,
        reason = "the GPU handles and the glyph key"
    )]
    fn glyph(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_index: usize,
        cell: &Cell,
        span: u32,
        deadline: std::time::Instant,
        atlas_full: &mut bool,
        atlas_grew: &mut bool,
    ) -> Option<Glyphs> {
        let bold = cell.flags.contains(CellFlags::BOLD);
        let italic = cell.flags.contains(CellFlags::ITALIC);
        let style = u8::from(bold) | u8::from(italic) << 1;
        if let Some(glyphs) = self.fonts[font_index].2.get(cell, style, span) {
            return Some(glyphs);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        let mut text = String::new();
        text.push(cell.ch);
        if let Some(combining) = &cell.combining {
            text.push_str(combining);
        }
        let (font_key, metrics, _) = &self.fonts[font_index];
        let (font, metrics) = (font_key.font.clone(), *metrics);
        let images = if cell.combining.is_none() && boxdraw::is_drawn(cell.ch) {
            boxdraw::draw(
                cell.ch,
                metrics.width * span,
                metrics.height,
                metrics.stroke,
            )
            .map(|data| crate::font::RasterGlyph {
                left: 0,
                top: 0,
                width: metrics.width * span,
                height: metrics.height,
                color: false,
                data,
            })
            .into_iter()
            .collect()
        } else {
            self.rasterizer
                .rasterize(&font, &metrics, &text, bold, italic, span)
        };
        let mut placed = Vec::with_capacity(images.len());
        for image in images {
            let atlas = if image.color {
                &mut self.color
            } else {
                &mut self.mask
            };
            match atlas.insert(
                device,
                queue,
                image.width,
                image.height,
                &image.data,
                atlas_grew,
            ) {
                Some((x, y)) => placed.push(Placed {
                    left: image.left,
                    top: image.top,
                    width: image.width,
                    height: image.height,
                    x,
                    y,
                    color: image.color,
                }),
                None => *atlas_full = true,
            }
        }
        let glyphs = match placed.len() {
            0 => Glyphs::None,
            1 => Glyphs::One(placed[0]),
            _ => Glyphs::Many(placed.into()),
        };
        if !*atlas_full {
            self.fonts[font_index]
                .2
                .insert(cell, style, span, glyphs.clone());
        }
        Some(glyphs)
    }

    /// A curly underline segment one cell wide, rasterised once per font.
    fn curl(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font_index: usize,
        atlas_grew: &mut bool,
    ) -> Option<Placed> {
        if let Some(curl) = self.fonts[font_index].2.curl {
            return Some(curl);
        }
        let metrics = self.fonts[font_index].1;
        let width = metrics.width;
        let height = (metrics.stroke * 4).max(3);
        let stroke = metrics.stroke as f32;
        let mut data = vec![0u8; (width * height) as usize];
        for y in 0..height {
            for x in 0..width {
                let mut hits = 0;
                for s in 0..16 {
                    let px = x as f32 + (s % 4) as f32 / 4.0 + 0.125;
                    let py = y as f32 + (s / 4) as f32 / 4.0 + 0.125;
                    let phase = px / width as f32 * std::f32::consts::TAU;
                    let centre =
                        (height as f32 - stroke) / 2.0 * (1.0 - phase.cos()) / 1.0 + stroke / 2.0;
                    if (py - centre.min(height as f32 - stroke / 2.0)).abs() <= stroke / 2.0 + 0.25
                    {
                        hits += 1;
                    }
                }
                data[(y * width + x) as usize] = (hits * 255 / 16) as u8;
            }
        }
        let (x, y) = self
            .mask
            .insert(device, queue, width, height, &data, atlas_grew)?;
        let curl = Placed {
            left: 0,
            top: metrics.underline_top as i32 - (height as i32 - metrics.stroke as i32) / 2,
            width,
            height,
            x,
            y,
            color: false,
        };
        self.fonts[font_index].2.curl = Some(curl);
        Some(curl)
    }

    fn new_terminal(&self, device: &wgpu::Device) -> TerminalGpu {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("enzui terminal uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg_buffer = Self::storage(device, 4);
        let instance_buffer = Self::instances(device, 1);
        let bind_group = self.terminal_bind_group(device, &uniform_buffer, &bg_buffer);
        TerminalGpu {
            uniforms: Uniforms::zeroed(),
            uniform_buffer,
            bg_buffer,
            bg_capacity: 4,
            instance_buffer,
            instance_capacity: 1,
            instance_count: 0,
            bind_group,
            bg: Vec::new(),
            rows: Vec::new(),
            instance_starts: Vec::new(),
            flat: Vec::new(),
            snapshot: Snapshot::default(),
            grid: (0, 0),
            metrics: None,
            style: None,
            focused: false,
            cursor_on: true,
            epoch: None,
            used_frame: 0,
            prepared_in: None,
            oversized_for: 0,
        }
    }

    fn storage(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("enzui terminal backgrounds"),
            size: bytes.max(4),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn instances(device: &wgpu::Device, count: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("enzui terminal glyphs"),
            size: count.max(1) * std::mem::size_of::<Instance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn terminal_bind_group(
        &self,
        device: &wgpu::Device,
        uniforms: &wgpu::Buffer,
        bg: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("enzui terminal"),
            layout: &self.terminal_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bg.as_entire_binding(),
                },
            ],
        })
    }
}

/// The rows a frame redraws, copied out of the screen so the emulator can
/// keep writing while they are laid out and rasterised.
#[derive(Default)]
struct Snapshot {
    cols: u16,
    cursor: crate::screen::Cursor,
    selection: Option<crate::screen::Selection>,
    /// Damaged rows' cells, by position in `rows`; buffers are reused.
    cells: Vec<Vec<Cell>>,
    rows: Vec<u16>,
    /// How many slots of `cells` hold a row: the ones past it are empty.
    filled: usize,
}

impl Snapshot {
    fn take(&mut self, state: &ScreenState, rows: &[u16]) {
        self.cols = state.cols();
        self.cursor = state.cursor();
        self.selection = state.selection();
        self.rows.clear();
        self.rows.extend_from_slice(rows);
        self.cells
            .resize_with(rows.len().max(self.cells.len()), Vec::new);
        for (slot, &y) in self.cells.iter_mut().zip(rows) {
            slot.clear();
            slot.extend_from_slice(state.row(y));
        }
        // Slots this frame no longer uses drop their cells (and any combining
        // text they own) once, keeping their capacity for reuse.
        for slot in &mut self.cells[rows.len()..self.filled.max(rows.len())] {
            slot.clear();
        }
        self.filled = rows.len();
    }
}

/// How long one frame may spend rasterising glyphs it has not seen before.
/// The rest wait for the next frames, so a burst of new glyphs (a page of
/// CJK, say) never stalls a frame.
const RASTER_BUDGET: std::time::Duration = std::time::Duration::from_millis(6);

/// What a row draws, given the screen and style.
pub(crate) struct RowInput<'a> {
    snapshot: &'a Snapshot,
    pub style: &'a TerminalStyle,
    pub metrics: &'a CellMetrics,
    pub focused: bool,
    pub cursor_on: bool,
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Rgba([
        lerp(a.0[0], b.0[0]),
        lerp(a.0[1], b.0[1]),
        lerp(a.0[2], b.0[2]),
        a.0[3].max(b.0[3]),
    ])
}

impl Primitive for TerminalPrimitive {
    type Pipeline = TerminalPipeline;

    fn prepare(
        &self,
        pipeline: &mut TerminalPipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let scale = viewport.scale_factor();
        let font_index = pipeline.font_index(&self.style.font.resolved(), scale);
        let metrics = pipeline.fonts[font_index].1;
        let id = self.screen.id();
        if !pipeline.terminals.contains_key(&id) {
            let terminal = pipeline.new_terminal(device);
            pipeline.terminals.insert(id, terminal);
        }
        let frame = pipeline.frame;
        let epoch = pipeline.atlas_epoch;

        let mut state = self.screen.lock_for_frame();
        let grid = (state.cols(), state.rows());
        let padding = (self.style.padding * scale).round();
        self.screen.paint_state().rows = Some((metrics.height as f32 / scale, padding / scale));
        let viewport_rect = *bounds * scale;
        let uniforms = Uniforms {
            viewport_origin: [viewport_rect.x, viewport_rect.y],
            viewport_size: [viewport_rect.width.max(1.0), viewport_rect.height.max(1.0)],
            grid_origin: [
                (viewport_rect.x + padding).round(),
                (viewport_rect.y + padding).round(),
            ],
            cell: [metrics.width as f32, metrics.height as f32],
            grid: [u32::from(grid.0), u32::from(grid.1)],
            default_bg: self.style.background.packed(),
            flags: u32::from(self.style.extend_background),
            fade: {
                let (rows, strength) = self.fade_above;
                let bottom = viewport_rect.y + padding + f32::from(rows) * metrics.height as f32;
                [viewport_rect.y, bottom, strength.clamp(0.0, 1.0), 0.0]
            },
        };

        let terminal = pipeline.terminals.get_mut(&id).expect("inserted above");
        // One view per screen: two would share this GPU state and the screen's
        // damage, and both would draw wrong (see `TerminalScreen`).
        debug_assert!(
            terminal.prepared_in != Some(frame),
            "a TerminalScreen is shown by two views in one frame; a screen has one view"
        );
        terminal.prepared_in = Some(frame);
        terminal.used_frame = frame;
        if terminal.uniforms != uniforms {
            terminal.uniforms = uniforms;
            queue.write_buffer(&terminal.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));
        }
        let same_style = terminal
            .style
            .as_ref()
            .is_some_and(|style| Arc::ptr_eq(style, &self.style) || **style == *self.style);
        let full = terminal.grid != grid
            || terminal.metrics != Some(metrics)
            || !same_style
            || terminal.epoch != Some(epoch);
        if full {
            state.damage_all();
        } else if terminal.focused != self.focused || terminal.cursor_on != self.cursor_on {
            // Focus and blink change only how the cursor's own row draws.
            let row = state.cursor().row;
            state.damage_row(row);
        }
        let Some(damaged) = state.take_damage() else {
            return;
        };
        let mut snapshot = std::mem::take(&mut terminal.snapshot);
        snapshot.take(&state, &damaged);
        // Laying out and rasterising happen without the lock.
        drop(state);
        terminal.epoch = Some(epoch);
        terminal.grid = grid;
        terminal.metrics = Some(metrics);
        terminal.style = Some(Arc::clone(&self.style));
        terminal.focused = self.focused;
        terminal.cursor_on = self.cursor_on;

        let cells = usize::from(grid.0) * usize::from(grid.1);
        terminal.bg.resize(cells * 2, 0);
        terminal.rows.resize_with(usize::from(grid.1), Vec::new);

        // Rasterise and lay out each damaged row. The terminal is taken out
        // of the map so the pipeline's atlases can be borrowed while it is.
        let mut terminal = pipeline.terminals.remove(&id).expect("inserted above");
        let mut atlas_grew = false;
        let mut pending_rows = Vec::new();
        let deadline = std::time::Instant::now() + RASTER_BUDGET;
        // A second pass runs only when the atlases filled up and were reset:
        // rows laid out before the reset point at slots that are gone, so
        // every row is laid out again against the fresh atlases.
        for pass in 0..2 {
            let mut atlas_full = false;
            pending_rows.clear();
            for index in 0..snapshot.rows.len() {
                let y = snapshot.rows[index];
                let mut row = std::mem::take(&mut terminal.rows[usize::from(y)]);
                let pending = build_row(
                    pipeline,
                    device,
                    queue,
                    font_index,
                    &RowInput {
                        snapshot: &snapshot,
                        style: &self.style,
                        metrics: &metrics,
                        focused: self.focused,
                        cursor_on: self.cursor_on,
                    },
                    index,
                    &mut terminal.bg,
                    &mut row,
                    deadline,
                    &mut atlas_full,
                    &mut atlas_grew,
                );
                if pending {
                    pending_rows.push(y);
                }
                terminal.rows[usize::from(y)] = row;
            }
            if !atlas_full || pass == 1 {
                break;
            }
            pipeline.mask.clear(device);
            pipeline.color.clear(device);
            for (_, _, cache) in &mut pipeline.fonts {
                *cache = GlyphCache::default();
            }
            // Other terminals rebuild against the fresh atlases when they draw.
            pipeline.atlas_epoch += 1;
            terminal.epoch = Some(pipeline.atlas_epoch);
            atlas_grew = true;
            let state = self.screen.lock();
            if (state.cols(), state.rows()) != grid {
                // Resized since the first pass, which damaged every row: the
                // next frame lays them all out at the new size.
                break;
            }
            let all: Vec<u16> = (0..grid.1).collect();
            snapshot.take(&state, &all);
        }
        let damaged = std::mem::take(&mut snapshot.rows);
        terminal.snapshot = snapshot;
        if !pending_rows.is_empty() {
            // Glyphs the budget ran out for: draw those rows again next frame.
            self.screen.redraw_rows(&pending_rows);
        }
        if atlas_grew {
            pipeline.atlas_bind_group = TerminalPipeline::atlas_bind_group(
                device,
                &pipeline.atlas_layout,
                &pipeline.mask,
                &pipeline.color,
            );
        }

        // Backgrounds: upload the damaged rows, or everything after a resize.
        let bg_bytes = (cells * 8) as u64;
        let count: usize = terminal.rows.iter().map(Vec::len).sum();
        let oversized = bg_bytes.saturating_mul(4) < terminal.bg_capacity
            || (count as u64).saturating_mul(4) < terminal.instance_capacity;
        terminal.oversized_for = if oversized {
            terminal.oversized_for.saturating_add(1)
        } else {
            0
        };
        let oversized_for = terminal.oversized_for;
        if let Some(capacity) =
            resized_capacity(bg_bytes.max(4), terminal.bg_capacity, oversized_for)
        {
            terminal.bg_capacity = capacity;
            terminal.bg_buffer = TerminalPipeline::storage(device, terminal.bg_capacity);
            terminal.bind_group =
                pipeline.terminal_bind_group(device, &terminal.uniform_buffer, &terminal.bg_buffer);
            queue.write_buffer(&terminal.bg_buffer, 0, bytemuck::cast_slice(&terminal.bg));
        } else if let (Some(&first), Some(&last)) = (damaged.first(), damaged.last()) {
            // One write over the damaged span: each `write_buffer` stages its
            // own copy, so a call per row cost more than the bytes it saved.
            let cols = usize::from(grid.0);
            let (start, end) = (
                usize::from(first) * cols * 2,
                (usize::from(last) + 1) * cols * 2,
            );
            queue.write_buffer(
                &terminal.bg_buffer,
                (start * 4) as u64,
                bytemuck::cast_slice(&terminal.bg[start..end]),
            );
        }

        // Glyphs: the rows' instances in one buffer.
        let mut rewrite_all = terminal.instance_starts.len() != terminal.rows.len() + 1;
        if let Some(capacity) =
            resized_capacity(count as u64, terminal.instance_capacity, oversized_for)
        {
            terminal.instance_capacity = capacity;
            terminal.instance_buffer =
                TerminalPipeline::instances(device, terminal.instance_capacity);
            rewrite_all = true;
        }
        if oversized_for >= SHRINK_AFTER {
            // The CPU staging copies give their high-water room back too.
            terminal.oversized_for = 0;
            terminal.flat.shrink_to(count);
            terminal.bg.shrink_to(cells * 2);
        }
        let span = if rewrite_all {
            Some(0..terminal.rows.len())
        } else {
            instance_rows(&terminal.instance_starts, &terminal.rows, &damaged)
        };
        if let Some(span) = span {
            let starts = &mut terminal.instance_starts;
            starts.resize(terminal.rows.len() + 1, 0);
            let mut at = starts[span.start];
            for (y, row) in terminal.rows.iter().enumerate().skip(span.start) {
                starts[y] = at;
                at += row.len();
            }
            starts[terminal.rows.len()] = at;
            let mut flat = std::mem::take(&mut terminal.flat);
            flat.clear();
            for row in &terminal.rows[span.clone()] {
                flat.extend_from_slice(row);
            }
            if !flat.is_empty() {
                queue.write_buffer(
                    &terminal.instance_buffer,
                    (starts[span.start] * std::mem::size_of::<Instance>()) as u64,
                    bytemuck::cast_slice(&flat),
                );
            }
            terminal.flat = flat;
        }
        terminal.instance_count = count as u32;
        pipeline.terminals.insert(id, terminal);
    }

    /// One signature per row: its revision, and the cursor (with focus and
    /// blink) and selection on it; with the style, grid and bounds, which
    /// change every row.
    /// Each row's rectangle reaches half a cell into its neighbours for
    /// glyphs that overhang, and the first and last reach the edges.
    fn paint(&self, bounds: &Rectangle, area: &mut dyn FnMut(u64, Rectangle)) -> bool {
        use std::hash::{Hash as _, Hasher as _};
        let (generation, rows) = {
            let mut paint = self.screen.paint_state();
            let same = paint
                .style
                .as_ref()
                .is_some_and(|style| Arc::ptr_eq(style, &self.style) || **style == *self.style);
            if !same {
                paint.style = Some(Arc::clone(&self.style));
                paint.style_generation += 1;
            }
            (paint.style_generation, paint.rows)
        };
        // Until the grid has been drawn its rows are not known.
        let Some((cell, padding)) = rows else {
            return false;
        };
        self.screen.read(|state| {
            let mut global = std::collections::hash_map::DefaultHasher::new();
            generation.hash(&mut global);
            (state.cols(), state.rows(), state.modes()).hash(&mut global);
            for value in [
                bounds.x,
                bounds.y,
                bounds.width,
                bounds.height,
                cell,
                padding,
            ] {
                value.to_bits().hash(&mut global);
            }
            let global = global.finish();
            let cursor = state.cursor();
            let selection = state.selection();
            let bottom = bounds.y + bounds.height;
            let rows = state.rows();
            for y in 0..rows {
                let mut row = std::collections::hash_map::DefaultHasher::new();
                (global, y, state.row_revision(y)).hash(&mut row);
                if cursor.row == y {
                    (cursor, self.focused, self.cursor_on).hash(&mut row);
                }
                if let Some(selection) = selection.filter(|s| s.rows().contains(&y)) {
                    selection.hash(&mut row);
                }
                let top = if y == 0 {
                    bounds.y
                } else {
                    bounds.y + padding + f32::from(y) * cell
                };
                let end = if y + 1 == rows {
                    bottom
                } else {
                    bounds.y + padding + f32::from(y + 1) * cell
                };
                let top = (top - cell / 2.0).max(bounds.y);
                let end = (end + cell / 2.0).min(bottom);
                area(
                    row.finish(),
                    Rectangle {
                        x: bounds.x,
                        y: top,
                        width: bounds.width,
                        height: (end - top).max(0.0),
                    },
                );
            }
        });
        true
    }

    fn draw(&self, pipeline: &TerminalPipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(terminal) = pipeline.terminals.get(&self.screen.id()) else {
            return true;
        };
        pass.set_bind_group(0, &terminal.bind_group, &[]);
        pass.set_bind_group(1, &pipeline.atlas_bind_group, &[]);
        pass.set_pipeline(&pipeline.bg_pipeline);
        pass.draw(0..4, 0..1);
        if terminal.instance_count > 0 {
            pass.set_pipeline(&pipeline.glyph_pipeline);
            pass.set_vertex_buffer(0, terminal.instance_buffer.slice(..));
            pass.draw(0..4, 0..terminal.instance_count);
        }
        true
    }
}

/// Lays out row `y`: writes its background colours into `bg` (per cell, the
/// colour drawn and the cell's own, which alone reaches past the grid's edge)
/// and returns its glyph, decoration and cursor instances.
#[allow(
    clippy::too_many_arguments,
    reason = "the GPU handles and the row to build"
)]
fn build_row(
    pipeline: &mut TerminalPipeline,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    font_index: usize,
    input: &RowInput<'_>,
    index: usize,
    bg: &mut [u32],
    out: &mut Vec<Instance>,
    deadline: std::time::Instant,
    atlas_full: &mut bool,
    atlas_grew: &mut bool,
) -> bool {
    let RowInput {
        snapshot,
        style,
        metrics,
        focused,
        cursor_on,
    } = *input;
    let y = snapshot.rows[index];
    let cols = snapshot.cols;
    let frame = Frame {
        style,
        metrics,
        cursor: snapshot.cursor,
        selection: snapshot.selection,
        focused,
        cursor_on,
    };
    let mut pending = false;
    out.clear();
    let cells = &snapshot.cells[index];
    for (x, cell) in cells.iter().enumerate() {
        let x = x as u16;
        let look = frame.look(cell, x, y);
        let at = (usize::from(y) * usize::from(cols) + usize::from(x)) * 2;
        bg[at] = look.background.packed();
        bg[at + 1] = cell.bg.packed();
        if cell.has_glyph() {
            let Some(glyphs) = pipeline.glyph(
                device, queue, font_index, cell, look.span, deadline, atlas_full, atlas_grew,
            ) else {
                pending = true;
                continue;
            };
            for p in glyphs.as_slice() {
                out.push(Instance {
                    cell: pack(u32::from(x), u32::from(y)),
                    offset: pack_signed(p.left, p.top),
                    size: pack(p.width, p.height),
                    uv: pack(p.x, p.y),
                    color: look.foreground.packed(),
                    kind: if p.color { KIND_COLOR } else { KIND_MASK },
                });
            }
        }
        let curly = frame.decorations(cell, &look, |left, top, width, height, color| {
            out.push(Instance::solid(x, y, left, top, width, height, color));
        });
        if let Some(line) = curly
            && let Some(curl) = pipeline.curl(device, queue, font_index, atlas_grew)
        {
            for segment in 0..look.span {
                out.push(Instance {
                    cell: pack(u32::from(x), u32::from(y)),
                    offset: pack_signed((segment * metrics.width) as i32, curl.top),
                    size: pack(curl.width, curl.height),
                    uv: pack(curl.x, curl.y),
                    color: line.packed(),
                    kind: KIND_MASK,
                });
            }
        }
    }
    pending
}

/// What every cell of a frame is drawn against.
pub(crate) struct Frame<'a> {
    pub style: &'a TerminalStyle,
    pub metrics: &'a CellMetrics,
    pub cursor: crate::screen::Cursor,
    pub selection: Option<crate::screen::Selection>,
    pub focused: bool,
    pub cursor_on: bool,
}

/// One cell's colours once selection, the cursor and dimming are applied.
pub(crate) struct Look {
    /// `Rgba::NONE` for the terminal background.
    pub background: Rgba,
    pub foreground: Rgba,
    /// Cells the glyph covers: 2 for a wide character.
    pub span: u32,
    /// The cursor is drawn over this cell, but not as a focused block.
    pub cursor_over: bool,
}

impl Frame<'_> {
    pub(crate) fn look(&self, cell: &Cell, x: u16, y: u16) -> Look {
        let style = self.style;
        let cursor = self.cursor;
        let selected = self.selection.is_some_and(|s| s.contains(x, y));
        let cursor_here = cursor.visible && self.cursor_on && cursor.row == y && cursor.col == x;
        let block = cursor_here && self.focused && cursor.shape == CursorShape::Block;
        let mut background = cell.bg;
        let mut foreground = cell.fg;
        if selected {
            background = style.selection_background;
            if let Some(fg) = style.selection_foreground {
                foreground = fg;
            }
        }
        if block {
            background = style.cursor;
            foreground = style.cursor_text;
        }
        if cell.flags.contains(CellFlags::DIM) {
            let under = if background.is_none() {
                style.background
            } else {
                background
            };
            foreground = mix(foreground, under, 0.4);
        }
        Look {
            background,
            foreground,
            span: if cell.flags.contains(CellFlags::WIDE) {
                2
            } else {
                1
            },
            cursor_over: cursor_here && !block,
        }
    }

    /// Lines, strikethrough and a non-block cursor as rectangles relative to
    /// the cell's top-left, in device pixels. Returns the curly underline's
    /// colour, which each renderer draws its own way.
    pub(crate) fn decorations(
        &self,
        cell: &Cell,
        look: &Look,
        mut solid: impl FnMut(i32, i32, u32, u32, Rgba),
    ) -> Option<Rgba> {
        let metrics = self.metrics;
        let width = metrics.width * look.span;
        let line = if cell.underline.is_none() {
            look.foreground
        } else {
            cell.underline
        };
        let stroke = metrics.stroke;
        let top = metrics.underline_top as i32;
        let flags = cell.flags;
        if flags.contains(CellFlags::UNDERLINE) {
            solid(0, top, width, stroke, line);
        }
        if flags.contains(CellFlags::DOUBLE_UNDERLINE) {
            let second = (top + 2 * stroke as i32).min(metrics.height as i32 - stroke as i32);
            let first = (second - 2 * stroke as i32).max(0);
            solid(0, first, width, stroke, line);
            solid(0, second, width, stroke, line);
        }
        if flags.contains(CellFlags::DOTTED_UNDERLINE) {
            let mut dot = 0;
            while dot < width {
                solid(dot as i32, top, stroke, stroke, line);
                dot += stroke * 2;
            }
        }
        if flags.contains(CellFlags::DASHED_UNDERLINE) {
            let dash = (width / 3).max(1);
            solid(0, top, dash, stroke, line);
            solid((dash * 2) as i32, top, width - dash * 2, stroke, line);
        }
        if flags.contains(CellFlags::STRIKETHROUGH) {
            solid(0, metrics.strike_top as i32, width, stroke, look.foreground);
        }
        // A cursor other than a focused block draws over the cell.
        if look.cursor_over {
            let thick = (stroke * 2).max(2).min(metrics.width / 2).max(1);
            let h = metrics.height;
            let c = self.style.cursor;
            match (self.focused, self.cursor.shape) {
                (true, CursorShape::Beam) => solid(0, 0, thick, h, c),
                (true, CursorShape::Underline) => solid(0, (h - thick) as i32, width, thick, c),
                _ => {
                    let t = stroke.max(1);
                    solid(0, 0, width, t, c);
                    solid(0, (h - t) as i32, width, t, c);
                    solid(0, 0, t, h, c);
                    solid((width - t) as i32, 0, t, h, c);
                }
            }
        }
        flags.contains(CellFlags::CURLY_UNDERLINE).then_some(line)
    }
}

#[cfg(test)]
mod snapshot_tests {}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn buffers_grow_at_once_and_shrink_only_after_staying_oversized() {
        assert_eq!(
            resized_capacity(100, 64, 0),
            Some(128),
            "grows to fit at once"
        );
        assert_eq!(
            resized_capacity(10, 1024, 0),
            None,
            "a burst keeps its room"
        );
        assert_eq!(resized_capacity(10, 1024, SHRINK_AFTER - 1), None);
        assert_eq!(
            resized_capacity(10, 1024, SHRINK_AFTER),
            Some(16),
            "then gives it back"
        );
        assert_eq!(
            resized_capacity(300, 1024, SHRINK_AFTER),
            None,
            "not oversized enough"
        );
        assert_eq!(resized_capacity(0, 1, SHRINK_AFTER), None);
    }

    #[test]
    fn an_upload_covers_damaged_rows_and_everything_a_count_change_moves() {
        let row = |n: usize| vec![Instance::zeroed(); n];
        // Rows of 2, 3 and 1 instances, as last uploaded.
        let starts = [0, 2, 5, 6];
        let same = [row(2), row(3), row(1)];
        assert_eq!(instance_rows(&starts, &same, &[]), None, "nothing damaged");
        assert_eq!(
            instance_rows(&starts, &same, &[1]),
            Some(1..2),
            "a row that kept its count rewrites only itself"
        );
        let grown = [row(3), row(3), row(1)];
        assert_eq!(
            instance_rows(&starts, &grown, &[0]),
            Some(0..3),
            "a row that gained an instance moves every row after it"
        );
    }
}

#[cfg(test)]
mod paint_tests {
    use super::*;
    use crate::screen::{Cell, Cursor, Selection};

    fn signatures(
        primitive: &TerminalPrimitive,
        bounds: Rectangle,
    ) -> Option<Vec<(u64, Rectangle)>> {
        let mut areas = Vec::new();
        primitive
            .paint(&bounds, &mut |signature, rect| {
                areas.push((signature, rect));
            })
            .then_some(areas)
    }

    fn changed(a: &[(u64, Rectangle)], b: &[(u64, Rectangle)]) -> Vec<usize> {
        a.iter()
            .zip(b)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, _)| index)
            .collect()
    }

    /// A terminal tells what each row paints: a written row, the cursor's
    /// blink and a selection change only their own rows; a new style (not
    /// an equal one) changes them all. The rows reach the view's edges.
    ///
    /// Sabotage: leave `row_revision` out of a row's signature -- the written
    /// row is not changed; hash the blink with every row -- it changes all.
    #[test]
    fn a_terminal_row_changes_only_its_own_signature() {
        let fg = Rgba::rgb(200, 200, 200);
        let screen = TerminalScreen::new(10, 4, fg);
        let style = Arc::new(TerminalStyle::default());
        let primitive = |style: &Arc<TerminalStyle>, cursor_on| TerminalPrimitive {
            screen: screen.clone(),
            style: Arc::clone(style),
            focused: true,
            cursor_on,
            fade_above: (0, 0.0),
        };
        let bounds = Rectangle::new(iced::Point::new(0.0, 0.0), iced::Size::new(200.0, 100.0));
        assert!(
            signatures(&primitive(&style, true), bounds).is_none(),
            "rows are unknown until the grid is drawn"
        );
        screen.paint_state().rows = Some((16.0, 4.0));
        let first = signatures(&primitive(&style, true), bounds).expect("rows are known");
        assert_eq!(first.len(), 4);
        assert_eq!(first[0].1.y, 0.0, "the first row reaches the top");
        let last = first[3].1;
        assert_eq!(
            last.y + last.height,
            100.0,
            "the last row reaches the bottom"
        );

        screen.update(|writer| writer.set_cell(3, 2, Cell::blank(Rgba::rgb(1, 2, 3))));
        let written = signatures(&primitive(&style, true), bounds).expect("rows are known");
        assert_eq!(changed(&first, &written), [2], "a written row");

        screen.update(|writer| {
            writer.set_cursor(Cursor {
                col: 1,
                row: 1,
                visible: true,
                ..Cursor::default()
            });
        });
        let moved = signatures(&primitive(&style, true), bounds).expect("rows are known");
        let blink = signatures(&primitive(&style, false), bounds).expect("rows are known");
        assert_eq!(changed(&moved, &blink), [1], "the cursor's blink");

        screen.update(|writer| {
            writer.set_selection(Some(Selection {
                start: (0, 3),
                end: (4, 3),
                block: false,
            }));
        });
        let selected = signatures(&primitive(&style, false), bounds).expect("rows are known");
        assert_eq!(changed(&blink, &selected), [3], "a selection");

        let equal = Arc::new(TerminalStyle::default());
        let same = signatures(&primitive(&equal, false), bounds).expect("rows are known");
        assert!(changed(&selected, &same).is_empty(), "an equal style");
        let padded = Arc::new(TerminalStyle {
            padding: 9.0,
            ..TerminalStyle::default()
        });
        let restyled = signatures(&primitive(&padded, false), bounds).expect("rows are known");
        assert_eq!(changed(&same, &restyled), [0, 1, 2, 3], "a new style");
    }
}
