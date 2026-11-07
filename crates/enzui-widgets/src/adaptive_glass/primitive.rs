//! The GPU half of adaptive glass: a custom primitive drawn with the
//! renderer's own material shader and a silhouette lens.

use std::collections::HashMap;
use std::sync::Arc;

use iced::advanced::material::{Material, Uniform};
use iced::{Point, Rectangle};
use iced_wgpu::graphics::Viewport;
use iced_wgpu::{primitive, wgpu};

use super::GlassSilhouette;

/// Frames an unused instance or silhouette upload is kept.
const KEEP_FRAMES: u64 = 2;

/// One glass shape in one place this frame.
#[derive(Debug)]
pub(super) struct GlassPrimitive {
    pub silhouette: Arc<GlassSilhouette>,
    pub material: Material,
    pub alpha: f32,
    /// Keys this instance's uniforms and bind groups, so shapes drawn in
    /// several places in one frame keep their own.
    pub key: u64,
}

impl GlassPrimitive {
    /// The key for `silhouette` drawn at `bounds` as `material` at `alpha`:
    /// everything `prepare` writes into an instance's slot besides the
    /// frame-wide viewport. Two draws that share a key therefore write the
    /// same state, so sharing the slot is harmless; two that differ in any of
    /// it (say, equal shapes stacked with different alpha) get their own.
    pub(super) fn key_for(
        silhouette: &GlassSilhouette,
        bounds: Rectangle,
        material: &Material,
        alpha: f32,
    ) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        silhouette.id.hash(&mut hasher);
        for value in [bounds.x, bounds.y, bounds.width, bounds.height, alpha] {
            value.to_bits().hash(&mut hasher);
        }
        // Every field, present and future: float formatting round-trips, so
        // equal text is equal values.
        format!("{material:?}").hash(&mut hasher);
        hasher.finish()
    }
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

struct Instance {
    uniform: wgpu::Buffer,
    silhouette_uniform: wgpu::Buffer,
    filter_uniforms: [wgpu::Buffer; 2],
    composite: Option<wgpu::BindGroup>,
    filters: Option<[wgpu::BindGroup; 2]>,
    scissor: Rectangle<u32>,
    blur: bool,
    used: u64,
}

pub(super) struct Pipeline {
    format: wgpu::TextureFormat,
    material: wgpu::RenderPipeline,
    filter: wgpu::RenderPipeline,
    material_layout: wgpu::BindGroupLayout,
    filter_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The backdrop copy and the two blur passes, shared by every instance:
    /// each one's render copies the frame again before drawing.
    targets: Option<([Target; 3], (u32, u32))>,
    silhouettes: HashMap<u64, (Target, u64)>,
    instances: HashMap<u64, Instance>,
    frame: u64,
}

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("enzui adaptive glass uniform"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn write(queue: &wgpu::Queue, buffer: &wgpu::Buffer, floats: &[f32]) {
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(floats));
}

fn target(device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> Target {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("enzui adaptive glass target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    Target { texture, view }
}

fn render_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    layout: &wgpu::BindGroupLayout,
    source: &str,
    entry: (&str, &str),
    blend: bool,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("enzui adaptive glass"),
        source: wgpu::ShaderSource::Wgsl(
            format!("const ENCODED: bool = {};\n{source}", !format.is_srgb()).into(),
        ),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("enzui adaptive glass layout"),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("enzui adaptive glass"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some(entry.0),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(entry.1),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: blend.then_some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

impl primitive::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enzui adaptive glass material"),
            entries: &[
                texture_entry(0, true),
                texture_entry(1, true),
                texture_entry(2, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(4),
                texture_entry(5, false),
                uniform_entry(6),
            ],
        });
        let filter_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enzui adaptive glass filter"),
            entries: &[texture_entry(0, true), uniform_entry(1)],
        });
        let source = format!(
            "{}\n{}",
            iced_wgpu::shader::MATERIAL,
            include_str!("silhouette.wgsl")
        );
        Self {
            format,
            material: render_pipeline(
                device,
                format,
                &material_layout,
                &source,
                ("vs_region", "fs_silhouette"),
                true,
            ),
            filter: render_pipeline(
                device,
                format,
                &filter_layout,
                iced_wgpu::shader::MATERIAL_FILTER,
                ("vs_fullscreen", "fs_filter"),
                false,
            ),
            material_layout,
            filter_layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("enzui adaptive glass sampler"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..wgpu::SamplerDescriptor::default()
            }),
            targets: None,
            silhouettes: HashMap::new(),
            instances: HashMap::new(),
            frame: 0,
        }
    }

    fn trim(&mut self) {
        let frame = self.frame;
        self.instances
            .retain(|_, instance| frame.saturating_sub(instance.used) < KEEP_FRAMES);
        self.silhouettes
            .retain(|_, (_, used)| frame.saturating_sub(*used) < KEEP_FRAMES);
        self.frame += 1;
    }
}

impl primitive::Primitive for GlassPrimitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let frame = pipeline.frame;
        let physical = viewport.physical_size();
        let scale = physical.width as f32 / viewport.logical_size().width;
        let size = (physical.width, physical.height);

        let silhouette = &self.silhouette;
        let entry = pipeline
            .silhouettes
            .entry(silhouette.id)
            .or_insert_with(|| {
                let texture = target(
                    device,
                    wgpu::TextureFormat::Rgba32Float,
                    silhouette.width,
                    silhouette.height,
                );
                queue.write_texture(
                    texture.texture.as_image_copy(),
                    &silhouette.texels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(silhouette.width * 16),
                        rows_per_image: Some(silhouette.height),
                    },
                    texture.texture.size(),
                );
                (texture, frame)
            });
        entry.1 = frame;

        if pipeline
            .targets
            .as_ref()
            .is_none_or(|(_, known)| *known != size)
        {
            pipeline.targets = Some((
                std::array::from_fn(|_| target(device, pipeline.format, size.0, size.1)),
                size,
            ));
        }

        // A key new this frame (a glass whose alpha or material is animating
        // has a new one each frame) takes over a slot no draw has used this
        // frame, rather than allocating buffers again.
        if !pipeline.instances.contains_key(&self.key)
            && let Some(idle) = pipeline
                .instances
                .iter()
                .find(|(_, instance)| instance.used < frame)
                .map(|(key, _)| *key)
            && let Some(recycled) = pipeline.instances.remove(&idle)
        {
            pipeline.instances.insert(self.key, recycled);
        }
        let instance = pipeline
            .instances
            .entry(self.key)
            .or_insert_with(|| Instance {
                uniform: buffer(device, Uniform::SIZE),
                silhouette_uniform: buffer(device, 16),
                filter_uniforms: [buffer(device, 32), buffer(device, 32)],
                composite: None,
                filters: None,
                scissor: Rectangle::default(),
                blur: false,
                used: frame,
            });
        instance.used = frame;

        let uniform = Uniform::new(
            &self.material,
            *bounds,
            viewport.logical_size(),
            Point::ORIGIN,
            scale,
            false,
        );
        write(queue, &instance.uniform, &uniform.floats());
        write(
            queue,
            &instance.silhouette_uniform,
            &[silhouette.width as f32, self.alpha, 0.0, 0.0],
        );

        // Blur only around the shape, padded for the blur and the lens reach.
        let sigma = (self.material.blur_sigma * scale).min(24.0);
        let radius = (sigma * 3.0).ceil().min(72.0);
        let padding = self.material.sample_padding(bounds.size()) * scale + radius + 4.0;
        let left = (bounds.x * scale - padding).floor().max(0.0) as u32;
        let top = (bounds.y * scale - padding).floor().max(0.0) as u32;
        let right = ((bounds.x + bounds.width) * scale + padding)
            .ceil()
            .min(size.0 as f32) as u32;
        let bottom = ((bounds.y + bounds.height) * scale + padding)
            .ceil()
            .min(size.1 as f32) as u32;
        instance.scissor = Rectangle {
            x: left,
            y: top,
            width: right.saturating_sub(left),
            height: bottom.saturating_sub(top),
        };
        instance.blur = sigma >= 0.5;
        for (buffer, axis) in instance
            .filter_uniforms
            .iter()
            .zip([[1.0, 0.0], [0.0, 1.0]])
        {
            write(
                queue,
                buffer,
                &[
                    axis[0],
                    axis[1],
                    sigma,
                    radius,
                    left as f32,
                    top as f32,
                    right.saturating_sub(1) as f32,
                    bottom.saturating_sub(1) as f32,
                ],
            );
        }

        let (targets, _) = pipeline.targets.as_ref().expect("created above");
        let silhouette_view = &pipeline.silhouettes[&silhouette.id].0.view;
        instance.filters = Some(std::array::from_fn(|i| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("enzui adaptive glass filter"),
                layout: &pipeline.filter_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&targets[i].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: instance.filter_uniforms[i].as_entire_binding(),
                    },
                ],
            })
        }));
        let blurred = if instance.blur {
            &targets[2].view
        } else {
            &targets[0].view
        };
        instance.composite = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("enzui adaptive glass composite"),
            layout: &pipeline.material_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&targets[0].view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(blurred),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(blurred),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: instance.uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(silhouette_view),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: instance.silhouette_uniform.as_entire_binding(),
                },
            ],
        }));
    }

    fn render(
        &self,
        pipeline: &Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        clip: &Rectangle<u32>,
    ) {
        let Some((targets, _)) = &pipeline.targets else {
            return;
        };
        let Some(instance) = pipeline.instances.get(&self.key) else {
            return;
        };
        let (Some(composite), Some(filters)) = (&instance.composite, &instance.filters) else {
            return;
        };
        // The frame so far is the backdrop: a material boundary around the
        // primitive makes it a copyable scene target.
        let source = output.texture();
        if !source.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            return;
        }
        let size = targets[0].texture.size();
        if source.size().width < size.width || source.size().height < size.height {
            return;
        }
        encoder.copy_texture_to_texture(
            source.as_image_copy(),
            targets[0].texture.as_image_copy(),
            size,
        );
        if instance.blur {
            for (i, group) in filters.iter().enumerate() {
                draw(
                    encoder,
                    &targets[i + 1].view,
                    &pipeline.filter,
                    group,
                    instance.scissor,
                    false,
                );
            }
        }
        draw(encoder, output, &pipeline.material, composite, *clip, true);
    }
}

fn draw(
    encoder: &mut wgpu::CommandEncoder,
    output: &wgpu::TextureView,
    pipeline: &wgpu::RenderPipeline,
    group: &wgpu::BindGroup,
    clip: Rectangle<u32>,
    load: bool,
) {
    if clip.width == 0 || clip.height == 0 {
        return;
    }
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("enzui adaptive glass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: output,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if load {
                    wgpu::LoadOp::Load
                } else {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    pass.set_scissor_rect(clip.x, clip.y, clip.width, clip.height);
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.draw(0..if load { 6 } else { 3 }, 0..1);
}
