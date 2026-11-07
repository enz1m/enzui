//! The render-side half: imports, their cache, and drawing.

use crate::import::{self, Imported};
use crate::{CACHED_ALLOCATIONS, ExternalFrame, IDLE_FRAMES, Refusal, Status, Support};
use iced::{ContentFit, Rectangle, Size, alignment};
use iced_wgpu::graphics::Viewport;
use iced_wgpu::primitive;
use iced_wgpu::wgpu;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// How long one draw waits for a new frame's producer to finish writing it
/// before deferring the frame to the next draw.
const WRITE_WAIT: std::time::Duration = std::time::Duration::from_millis(8);

/// Whether the producer has finished writing `frame`'s buffer. Tests decide
/// it through [`tests::READINESS`]; nothing else can skip the poll.
fn readiness(frame: &ExternalFrame) -> import::Readiness {
    #[cfg(test)]
    if let Some(readiness) = tests::READINESS
        .lock()
        .ok()
        .and_then(|forced| forced.clone())
    {
        return readiness;
    }
    import::wait_written(frame.buffer(), WRITE_WAIT)
}

/// One external image to draw.
#[derive(Debug)]
pub(crate) struct Primitive {
    /// The widget instance; each keeps its own uniforms.
    pub(crate) instance: u64,
    /// The frame to show, or `None` for a probe, which only makes sure the
    /// pipeline (and with it [`crate::support`]) exists.
    pub(crate) frame: Option<Arc<ExternalFrame>>,
    /// A source whose latest frame is shown, read when this is prepared, so
    /// a content-only redraw shows a newly published frame.
    pub(crate) source: Option<crate::ExternalFrameSource>,
    pub(crate) fit: ContentFit,
    pub(crate) alignment: (alignment::Horizontal, alignment::Vertical),
    pub(crate) opacity: f32,
}

struct Instance {
    frame: Arc<ExternalFrame>,
    uniforms: wgpu::Buffer,
    bind_group: Option<wgpu::BindGroup>,
    /// The import `bind_group` samples: a frame of the same allocation keeps
    /// the binding.
    bound: Option<Arc<Imported>>,
    /// What `uniforms` holds.
    written: Option<[f32; 8]>,
    seen: bool,
}

struct Cached {
    imported: Arc<Imported>,
    last_used: u64,
    /// The widget instance that showed it last.
    owner: u64,
}

/// Shared by every external image the renderer draws.
pub(crate) struct Pipeline {
    device: wgpu::Device,
    queue: wgpu::Queue,
    support: Option<Support>,
    render: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    imports: HashMap<u64, Cached>,
    instances: BTreeMap<u64, Instance>,
    /// Frames and imports the GPU may still be sampling: kept until the work
    /// submitted so far has finished.
    retiring: Vec<Arc<ExternalFrame>>,
    retiring_imports: Vec<Arc<Imported>>,
    frame: u64,
}

impl primitive::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let support = crate::import::support(device);
        crate::record_support(support.clone());
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("enzui external image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("enzui external image"),
            source: wgpu::ShaderSource::Wgsl(include_str!("external_image.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("enzui external image"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("enzui external image"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("enzui external image"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..wgpu::SamplerDescriptor::default()
        });
        Self {
            device: device.clone(),
            queue: queue.clone(),
            support,
            render,
            layout,
            sampler,
            imports: HashMap::new(),
            instances: BTreeMap::new(),
            retiring: Vec::new(),
            retiring_imports: Vec::new(),
            frame: 0,
        }
    }

    fn trim(&mut self) {
        // Widgets no longer drawn give up their frame.
        let gone: Vec<u64> = self
            .instances
            .iter()
            .filter(|(_, instance)| !instance.seen)
            .map(|(id, _)| *id)
            .collect();
        for id in &gone {
            if let Some(instance) = self.instances.remove(id) {
                self.retiring.push(instance.frame);
            }
        }
        for instance in self.instances.values_mut() {
            instance.seen = false;
        }
        self.retire_unused(&gone);
        self.frame += 1;
        // Retired imports were last sampled by work already submitted; their
        // references drop when this callback runs.
        if !self.retiring.is_empty() || !self.retiring_imports.is_empty() {
            let frames = std::mem::take(&mut self.retiring);
            let imports = std::mem::take(&mut self.retiring_imports);
            self.queue.on_submitted_work_done(move || {
                drop(frames);
                drop(imports);
            });
        }
    }
}

impl Drop for Pipeline {
    /// The device is going away: wait for its work, then let go of every
    /// frame so each producer gets its buffers back.
    fn drop(&mut self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        self.instances.clear();
        self.retiring.clear();
        self.retiring_imports.clear();
        self.imports.clear();
    }
}

impl Pipeline {
    /// Shows nothing for widget `instance`: its frame is retired once the
    /// work that sampled it has finished.
    fn release(&mut self, instance: u64) {
        if let Some(shown) = self.instances.remove(&instance) {
            self.retiring.push(shown.frame);
        }
    }

    /// Makes `frame` the one widget `instance` shows, retiring the frame it
    /// showed before once the GPU is done with it.
    fn adopt(
        &mut self,
        instance: u64,
        frame: &Arc<ExternalFrame>,
        uniforms: impl FnOnce() -> wgpu::Buffer,
    ) {
        match self.instances.get_mut(&instance) {
            Some(shown) => {
                let previous = std::mem::replace(&mut shown.frame, Arc::clone(frame));
                self.retiring.push(previous);
            }
            None => {
                let _ = self.instances.insert(
                    instance,
                    Instance {
                        frame: Arc::clone(frame),
                        uniforms: uniforms(),
                        bind_group: None,
                        bound: None,
                        written: None,
                        seen: true,
                    },
                );
            }
        }
    }

    /// Shows nothing for widget `instance` but `frame`'s fallback: `frame`
    /// is refused for `refusal`, held until the widget moves on like any
    /// shown frame, and never sampled.
    fn refuse(
        &mut self,
        instance: u64,
        frame: &Arc<ExternalFrame>,
        refusal: Refusal,
        uniforms: impl FnOnce() -> wgpu::Buffer,
    ) {
        frame.set_status(Status::Refused(refusal));
        let shows = self
            .instances
            .get(&instance)
            .is_some_and(|shown| Arc::ptr_eq(&shown.frame, frame));
        if !shows {
            self.adopt(instance, frame, uniforms);
        }
        if let Some(shown) = self.instances.get_mut(&instance) {
            shown.bind_group = None;
            shown.bound = None;
            shown.written = None;
        }
    }

    /// Releases imports no widget shows: those of widgets now `gone`, those
    /// unshown for [`IDLE_FRAMES`], then the least recently shown until the
    /// rest fit the unused-import budget. Each is released once the work
    /// already submitted, the last that could sample it, has finished.
    fn retire_unused(&mut self, gone: &[u64]) {
        let shown: Vec<u64> = self
            .instances
            .values()
            .map(|instance| instance.frame.buffer().allocation)
            .collect();
        // (allocation, last shown, bytes) of every import no widget shows.
        let mut unused: Vec<(u64, u64, u64)> = self
            .imports
            .iter()
            .filter(|(allocation, _)| !shown.contains(allocation))
            .map(|(allocation, cached)| (*allocation, cached.last_used, cached.imported.bytes))
            .collect();
        let mut release: Vec<u64> = Vec::new();
        unused.retain(|&(allocation, last_used, _)| {
            let owner_gone = self
                .imports
                .get(&allocation)
                .is_some_and(|cached| gone.contains(&cached.owner));
            let idle = self.frame.saturating_sub(last_used) >= IDLE_FRAMES;
            if owner_gone || idle {
                release.push(allocation);
            }
            !(owner_gone || idle)
        });
        // The least recently shown go first, until the rest fit the budget.
        unused.sort_by_key(|&(_, last_used, _)| last_used);
        let budget = crate::stats::unused_import_budget();
        let mut bytes: u64 = unused.iter().map(|&(_, _, bytes)| bytes).sum();
        for (allocation, _, size) in unused {
            if bytes <= budget {
                break;
            }
            release.push(allocation);
            bytes -= size;
        }
        for allocation in release {
            if let Some(cached) = self.imports.remove(&allocation) {
                self.retiring_imports.push(cached.imported);
            }
        }
    }

    /// The import of `frame`'s buffer for widget `instance`, from the cache
    /// or made now.
    fn import(&mut self, frame: &ExternalFrame, instance: u64) -> Result<Arc<Imported>, Refusal> {
        let support = self.support.as_ref().ok_or(Refusal::NoImport)?;
        let allocation = frame.buffer().allocation;
        if let Some(cached) = self.imports.get_mut(&allocation) {
            cached.last_used = self.frame;
            cached.owner = instance;
            return Ok(Arc::clone(&cached.imported));
        }
        let imported = Arc::new(import::import(&self.device, support, frame.buffer())?);
        if self.imports.len() >= CACHED_ALLOCATIONS {
            // Evict the least recently used import no widget is showing.
            let shown: Vec<u64> = self
                .instances
                .values()
                .map(|instance| instance.frame.buffer().allocation)
                .collect();
            let oldest = self
                .imports
                .iter()
                .filter(|(allocation, _)| !shown.contains(allocation))
                .min_by_key(|(_, cached)| cached.last_used)
                .map(|(allocation, _)| *allocation);
            if let Some(oldest) = oldest
                && let Some(evicted) = self.imports.remove(&oldest)
            {
                self.retiring_imports.push(evicted.imported);
            }
        }
        let _ = self.imports.insert(
            allocation,
            Cached {
                imported: Arc::clone(&imported),
                last_used: self.frame,
                owner: instance,
            },
        );
        Ok(imported)
    }
}

pub(crate) fn placement(
    fit: ContentFit,
    alignment: (alignment::Horizontal, alignment::Vertical),
    image: Size,
    bounds: Size,
) -> Rectangle {
    let fitted = fit.fit(image, bounds);
    let x = match alignment.0 {
        alignment::Horizontal::Left => 0.0,
        alignment::Horizontal::Center => (bounds.width - fitted.width) / 2.0,
        alignment::Horizontal::Right => bounds.width - fitted.width,
    };
    let y = match alignment.1 {
        alignment::Vertical::Top => 0.0,
        alignment::Vertical::Center => (bounds.height - fitted.height) / 2.0,
        alignment::Vertical::Bottom => bounds.height - fitted.height,
    };
    Rectangle::new(iced::Point::new(x, y), fitted)
}

fn quad(
    fit: ContentFit,
    alignment: (alignment::Horizontal, alignment::Vertical),
    image: Size,
    bounds: Size,
) -> [f32; 4] {
    let rect = placement(fit, alignment, image, bounds);
    let width = bounds.width.max(1.0);
    let height = bounds.height.max(1.0);
    [
        2.0 * rect.x / width - 1.0,
        1.0 - 2.0 * rect.y / height,
        2.0 * (rect.x + rect.width) / width - 1.0,
        1.0 - 2.0 * (rect.y + rect.height) / height,
    ]
}

impl Primitive {
    fn place(&self, pipeline: &mut Pipeline, queue: &wgpu::Queue, bounds: Size, opaque: bool) {
        let Some(instance) = pipeline.instances.get_mut(&self.instance) else {
            return;
        };
        let buffer = instance.frame.buffer();
        let rect = quad(
            self.fit,
            self.alignment,
            Size::new(buffer.width as f32, buffer.height as f32),
            bounds,
        );
        let opacity = if self.opacity.is_finite() {
            self.opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let values = [
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            opacity,
            if opaque { 1.0 } else { 0.0 },
            0.0,
            0.0,
        ];
        if instance.written != Some(values) {
            queue.write_buffer(&instance.uniforms, 0, bytemuck::cast_slice(&values));
            instance.written = Some(values);
            crate::stats::uniforms_written();
        }
    }
}

impl primitive::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        let resolved = match (&self.frame, &self.source) {
            (Some(frame), _) => Some(Arc::clone(frame)),
            (None, Some(source)) => {
                let Some(frame) = source.take_for_frame() else {
                    // A cleared source shows nothing.
                    pipeline.release(self.instance);
                    return;
                };
                Some(frame)
            }
            (None, None) => None,
        };
        let Some(frame) = resolved.as_ref() else {
            return;
        };
        let uniforms = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("enzui external image"),
                size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // A frame this widget has not shown yet is adopted only once its
        // producer has finished writing it; until then the widget keeps the
        // frame and binding it had.
        let is_new = match pipeline.instances.get_mut(&self.instance) {
            Some(instance) => {
                instance.seen = true;
                !Arc::ptr_eq(&instance.frame, frame)
            }
            None => true,
        };
        // A refused frame stays refused: it shows its fallback and is never
        // bound.
        if !is_new && matches!(frame.status(), Status::Refused(_)) {
            return;
        }
        let imported = match pipeline.import(frame, self.instance) {
            Ok(imported) => imported,
            Err(refusal) => {
                pipeline.refuse(self.instance, frame, refusal, uniforms);
                return;
            }
        };
        if is_new {
            match readiness(frame) {
                import::Readiness::Ready => {}
                import::Readiness::Unfinished if frame.writing_for() < crate::WRITE_WAIT_LIMIT => {
                    frame.set_status(Status::Writing);
                    let opaque = pipeline
                        .instances
                        .get(&self.instance)
                        .and_then(|instance| instance.bound.as_ref())
                        .is_some_and(|bound| bound.opaque);
                    self.place(pipeline, queue, bounds.size(), opaque);
                    return;
                }
                import::Readiness::Unfinished => {
                    let reason = format!(
                        "still being written after {} ms",
                        crate::WRITE_WAIT_LIMIT.as_millis()
                    );
                    pipeline.refuse(self.instance, frame, Refusal::Unwritten(reason), uniforms);
                    return;
                }
                import::Readiness::Failed(reason) => {
                    pipeline.refuse(self.instance, frame, Refusal::Unwritten(reason), uniforms);
                    return;
                }
            }
            pipeline.adopt(self.instance, frame, uniforms);
        }
        self.place(pipeline, queue, bounds.size(), imported.opaque);
        let Some(instance) = pipeline.instances.get_mut(&self.instance) else {
            return;
        };
        // A frame of the allocation already bound samples the same texture.
        let bound = instance
            .bound
            .as_ref()
            .is_some_and(|bound| Arc::ptr_eq(bound, &imported));
        if bound && instance.bind_group.is_some() {
            frame.set_status(Status::Imported);
            return;
        }
        crate::stats::binding_made();
        instance.bound = Some(Arc::clone(&imported));
        instance.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("enzui external image"),
            layout: &pipeline.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&imported.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&pipeline.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: instance.uniforms.as_entire_binding(),
                },
            ],
        }));
        frame.set_status(Status::Imported);
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        if self.frame.is_none() && self.source.is_none() {
            return true;
        }
        if let Some(instance) = pipeline.instances.get(&self.instance)
            && let Some(bind_group) = instance.bind_group.as_ref()
        {
            render_pass.set_pipeline(&pipeline.render);
            render_pass.set_bind_group(0, bind_group, &[]);
            render_pass.draw(0..6, 0..1);
            // The binding samples the frame this widget shows, which is the
            // one encoded: a newer frame still being written is not.
            instance.frame.encoded();
        }
        true
    }
}

#[cfg(test)]
mod tests {
    //! A frame is sampled only once its producer's writes are confirmed
    //! finished: drawn on the real importing renderer with the readiness the
    //! poll would report decided here.

    #![allow(clippy::unwrap_used, reason = "a test")]

    use crate::import::Readiness;
    use crate::{ExternalFrame, Lease, Refusal, Status, enz_external_image};
    use iced::advanced::renderer::Headless as _;
    use iced::{Color, ContentFit, Size, alignment, mouse};
    use iced_test::runtime::user_interface::{Cache, UserInterface};
    use std::sync::{Arc, Mutex};

    use crate::gbm;

    /// What the poll reports while set; `None` polls the buffer.
    pub(super) static READINESS: Mutex<Option<Readiness>> = Mutex::new(None);

    /// The tests share [`READINESS`] and the GPU: one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn serial() -> std::sync::MutexGuard<'static, ()> {
        SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn force(readiness: Option<Readiness>) {
        *READINESS.lock().unwrap() = readiness;
    }

    struct Screen(iced::Renderer, Cache);

    impl Screen {
        /// Draws `frame` filling 32x32 over `fallback`, and reads the centre
        /// pixel as B, G, R... in screenshot order (R, G, B).
        fn show(&mut self, frame: &Arc<ExternalFrame>) -> [u8; 3] {
            let fallback = iced::advanced::image::Handle::from_rgba(1, 1, vec![0, 0, 255, 255]);
            let view: iced::Element<'_, ()> = enz_external_image(frame)
                .fallback(fallback)
                .content_fit(ContentFit::Fill)
                .width(32)
                .height(32)
                .into();
            let mut ui = UserInterface::build(
                view,
                Size::new(32.0, 32.0),
                std::mem::take(&mut self.1),
                &mut self.0,
            );
            ui.draw(
                &mut self.0,
                &iced::Theme::Dark,
                &iced::advanced::renderer::Style {
                    text_color: Color::WHITE,
                },
                mouse::Cursor::Unavailable,
            );
            self.1 = ui.into_cache();
            let shot = self.0.screenshot(Size::new(32, 32), 1.0, Color::BLACK);
            let at = (16 * 32 + 16) * 4;
            [shot[at], shot[at + 1], shot[at + 2]]
        }
    }

    /// A one-colour frame of `allocation` (B, G, R, A) whose lease records
    /// its release in `released`.
    fn frame(
        gbm: &gbm::Gbm,
        allocation: u64,
        bgra: [u8; 4],
        released: &Arc<Mutex<Vec<u64>>>,
    ) -> Arc<ExternalFrame> {
        let released = Arc::clone(released);
        ExternalFrame::new(
            gbm.buffer(32, 32, None, allocation, move |_, _| bgra)
                .unwrap(),
            Lease::new(move || released.lock().unwrap().push(allocation)),
        )
    }

    const RED: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];

    #[test]
    fn an_unwritten_frame_is_never_sampled_and_its_lease_still_comes_back() {
        let _serial = serial();
        let Some(gbm) = gbm::Gbm::open() else {
            eprintln!("SKIPPED: no GBM render node");
            return;
        };
        let Some(renderer) = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        ) else {
            eprintln!("SKIPPED: no wgpu adapter");
            return;
        };
        let mut screen = Screen(renderer, Cache::default());
        let released = Arc::new(Mutex::new(Vec::new()));
        force(None);
        let red = frame(&gbm, 1, RED, &released);
        let _ = screen.show(&red);
        if !matches!(crate::support(), Some(Some(_))) {
            eprintln!("SKIPPED: the renderer's device does not import DMA-BUF");
            return;
        }
        assert_eq!(screen.show(&red), [255, 0, 0], "a written frame is drawn");

        // Still being written: the red frame stays, the green one waits.
        force(Some(Readiness::Unfinished));
        let green = frame(&gbm, 2, GREEN, &released);
        for _ in 0..3 {
            assert_eq!(
                screen.show(&green),
                [255, 0, 0],
                "never sampled while unwritten"
            );
            assert_eq!(green.status(), Status::Writing);
        }
        // Written now: it replaces the red frame, which goes back once
        // nothing holds it.
        force(Some(Readiness::Ready));
        assert_eq!(screen.show(&green), [0, 255, 0]);
        assert_eq!(green.status(), Status::Imported);
        drop(red);
        let _ = screen.show(&green);
        let _ = screen.show(&green);
        assert_eq!(*released.lock().unwrap(), [1]);

        // A polling error refuses the frame: the fallback shows, not the
        // buffer, and its lease comes back once it is replaced and dropped.
        force(Some(Readiness::Failed("an error".into())));
        let broken = frame(&gbm, 3, RED, &released);
        // The first draw finds out, as for any refusal; the next shows the
        // fallback.
        assert_ne!(screen.show(&broken), [255, 0, 0], "never sampled");
        assert_eq!(
            screen.show(&broken),
            [0, 0, 255],
            "the fallback, not the buffer"
        );
        assert_eq!(
            broken.status(),
            Status::Refused(Refusal::Unwritten("an error".into()))
        );
        force(Some(Readiness::Ready));
        let _ = screen.show(&green);
        drop(broken);
        let _ = screen.show(&green);
        let _ = screen.show(&green);
        assert!(
            released.lock().unwrap().contains(&3),
            "{:?}",
            released.lock().unwrap()
        );

        // Unwritten past the limit: refused, never sampled.
        force(Some(Readiness::Unfinished));
        let late = frame(&gbm, 4, RED, &released);
        let _ = screen.show(&late);
        std::thread::sleep(crate::WRITE_WAIT_LIMIT);
        assert_ne!(screen.show(&late), [255, 0, 0], "never sampled");
        assert_eq!(screen.show(&late), [0, 0, 255]);
        assert!(matches!(
            late.status(),
            Status::Refused(Refusal::Unwritten(_))
        ));
        force(None);
    }

    /// Draws `view` in a `size` window.
    fn render(screen: &mut Screen, view: iced::Element<'_, ()>, size: Size) {
        let mut ui = UserInterface::build(view, size, std::mem::take(&mut screen.1), &mut screen.0);
        ui.draw(
            &mut screen.0,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        screen.1 = ui.into_cache();
        let _ = screen.0.screenshot(
            Size::new(size.width as u32, size.height as u32),
            1.0,
            Color::BLACK,
        );
    }

    /// A frame of `allocation` that counts its encoded callbacks in `count`
    /// and holds `guard` in the callback, so dropping it uncalled shows.
    fn counted(
        gbm: &gbm::Gbm,
        allocation: u64,
        count: &Arc<std::sync::atomic::AtomicUsize>,
        guard: &Arc<()>,
    ) -> Arc<ExternalFrame> {
        let (count, guard) = (Arc::clone(count), Arc::clone(guard));
        ExternalFrame::with_on_encoded(
            gbm.buffer(32, 32, None, allocation, |_, _| RED).unwrap(),
            Lease::none(),
            move || {
                let _held = &guard;
                let _ = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            },
        )
    }

    #[test]
    fn a_frame_reports_its_first_encoding_once_and_never_for_an_unencoded_frame() {
        let _serial = serial();
        use std::sync::atomic::{AtomicUsize, Ordering};
        let Some(gbm) = gbm::Gbm::open() else {
            eprintln!("SKIPPED: no GBM render node");
            return;
        };
        let Some(renderer) = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        ) else {
            eprintln!("SKIPPED: no wgpu adapter");
            return;
        };
        let mut screen = Screen(renderer, Cache::default());
        force(None);
        let probe = ExternalFrame::new(
            gbm.buffer(2, 2, None, 50, |_, _| RED).unwrap(),
            Lease::none(),
        );
        let _ = screen.show(&probe);
        if !matches!(crate::support(), Some(Some(_))) {
            eprintln!("SKIPPED: the renderer's device does not import DMA-BUF");
            return;
        }
        let count = |n: &Arc<AtomicUsize>| n.load(Ordering::Relaxed);

        // Prepared but not encoded: it reaches into its layer's clip by less
        // than a pixel, so the renderer prepares the layer and encodes
        // nothing for it.
        let (hidden_count, hidden_guard) = (Arc::new(AtomicUsize::new(0)), Arc::new(()));
        let hidden = counted(&gbm, 51, &hidden_count, &hidden_guard);
        render(
            &mut screen,
            iced::widget::container(iced::widget::row![
                iced::widget::Space::new().width(100.0),
                enz_external_image(&hidden).width(32).height(32),
            ])
            .width(100.3)
            .clip(true)
            .into(),
            Size::new(200.0, 64.0),
        );
        assert_eq!(hidden.status(), Status::Imported, "it was prepared");
        assert_eq!(count(&hidden_count), 0, "but never encoded");

        // Drawn: once, however often it is drawn again.
        let (shown_count, shown_guard) = (Arc::new(AtomicUsize::new(0)), Arc::new(()));
        let shown = counted(&gbm, 52, &shown_count, &shown_guard);
        for _ in 0..3 {
            let _ = screen.show(&shown);
        }
        assert_eq!(count(&shown_count), 1, "the first encoding, once");
        render(
            &mut screen,
            iced::widget::row![
                enz_external_image(&shown).width(32).height(32),
                enz_external_image(&shown).width(32).height(32),
            ]
            .into(),
            Size::new(64.0, 32.0),
        );
        assert_eq!(
            count(&shown_count),
            1,
            "another widget drawing it adds nothing"
        );

        // Still being written: the frame shown before keeps being drawn,
        // and the new one is not encoded until it is adopted and drawn.
        let _ = screen.show(&shown);
        force(Some(Readiness::Unfinished));
        let (writing_count, writing_guard) = (Arc::new(AtomicUsize::new(0)), Arc::new(()));
        let writing = counted(&gbm, 53, &writing_count, &writing_guard);
        let _ = screen.show(&writing);
        assert_eq!(writing.status(), Status::Writing);
        assert_eq!(
            count(&writing_count),
            0,
            "a frame still being written is not encoded"
        );
        force(Some(Readiness::Ready));
        let _ = screen.show(&writing);
        assert_eq!(
            count(&writing_count),
            1,
            "encoded once it is written and drawn"
        );

        // Refused: never encoded, and the callback goes with the frame.
        force(Some(Readiness::Failed("an error".into())));
        let (refused_count, refused_guard) = (Arc::new(AtomicUsize::new(0)), Arc::new(()));
        let refused = counted(&gbm, 54, &refused_count, &refused_guard);
        let _ = screen.show(&refused);
        let _ = screen.show(&refused);
        assert!(matches!(refused.status(), Status::Refused(_)));
        assert_eq!(count(&refused_count), 0, "a refused frame is never encoded");
        force(Some(Readiness::Ready));
        let _ = screen.show(&writing);
        drop(refused);
        let _ = screen.show(&writing);
        let _ = screen.show(&writing);
        assert_eq!(
            Arc::strong_count(&refused_guard),
            1,
            "the uncalled callback is dropped with the retired frame"
        );

        // Never shown: dropping it drops the callback at once.
        let (unused_count, unused_guard) = (Arc::new(AtomicUsize::new(0)), Arc::new(()));
        drop(counted(&gbm, 55, &unused_count, &unused_guard));
        assert_eq!(Arc::strong_count(&unused_guard), 1);
        assert_eq!(count(&unused_count), 0);
        drop((
            hidden,
            shown,
            writing,
            hidden_guard,
            shown_guard,
            writing_guard,
        ));
        force(None);
    }

    /// A frame of `allocation` in one colour (B, G, R, A), `size` square,
    /// whose lease records its release in `released`.
    fn colored(
        gbm: &gbm::Gbm,
        allocation: u64,
        size: u32,
        bgra: [u8; 4],
        released: &Arc<Mutex<Vec<u64>>>,
    ) -> Arc<ExternalFrame> {
        let released = Arc::clone(released);
        ExternalFrame::new(
            gbm.buffer(size, size, None, allocation, move |_, _| bgra)
                .unwrap(),
            Lease::new(move || released.lock().unwrap().push(allocation)),
        )
    }

    #[test]
    fn a_source_redraws_only_its_image_when_the_producer_publishes() {
        use iced_test::runtime::user_interface::State;
        let _serial = serial();
        let Some(gbm) = gbm::Gbm::open() else {
            eprintln!("SKIPPED: no GBM render node");
            return;
        };
        let Some(mut renderer) = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        ) else {
            eprintln!("SKIPPED: no wgpu adapter");
            return;
        };
        force(None);
        let released = Arc::new(Mutex::new(Vec::new()));
        let source = crate::ExternalFrameSource::new();
        // Published before anything shows the source.
        source.publish(colored(&gbm, 60, 32, RED, &released));

        let size = Size::new(32.0, 48.0);
        let fallback = iced::advanced::image::Handle::from_rgba(1, 1, vec![0, 0, 255, 255]);
        let view = |shown: bool| -> iced::Element<'_, ()> {
            // Browser-like chrome: a control that repaints when hovered.
            let chrome = iced::widget::button(iced::widget::Space::new())
                .width(32)
                .height(16)
                .on_press(())
                .style(|_: &iced::Theme, _| iced::widget::button::Style {
                    background: Some(Color::from_rgb(0.0, 0.0, 1.0).into()),
                    ..Default::default()
                });
            if shown {
                iced::widget::column![
                    chrome,
                    crate::enz_external_image_source(&source)
                        .fallback(fallback.clone())
                        .content_fit(ContentFit::Fill)
                        .width(32)
                        .height(32),
                ]
                .into()
            } else {
                iced::widget::column![chrome].into()
            }
        };
        let shot = |renderer: &mut iced::Renderer| {
            renderer.screenshot(Size::new(32, 48), 1.0, Color::BLACK)
        };
        let px = |pixels: &[u8], y: usize| {
            let at = (y * 32 + 16) * 4;
            [pixels[at], pixels[at + 1], pixels[at + 2]]
        };
        let chrome_rows = |pixels: &[u8]| pixels[..16 * 32 * 4].to_vec();
        let redraw = || {
            [iced::Event::Window(iced::window::Event::RedrawRequested(
                iced::time::Instant::now(),
            ))]
        };
        let update = |ui: &mut UserInterface<'_, (), iced::Theme, iced::Renderer>,
                      renderer: &mut iced::Renderer,
                      events: &[iced::Event],
                      cursor: mouse::Cursor|
         -> Option<bool> {
            match ui
                .update(
                    &iced::window::Headless,
                    &iced_test::core::shell::Waker::noop(),
                    events,
                    cursor,
                    renderer,
                    &mut iced::advanced::shell::Bus::new(),
                )
                .0
            {
                State::Updated {
                    is_content_only, ..
                } => Some(is_content_only),
                State::Outdated => None,
            }
        };
        let full = |renderer: &mut iced::Renderer, cache: Cache, shown: bool| {
            let mut ui = UserInterface::build(view(shown), size, cache, renderer);
            let _ = update(&mut ui, renderer, &redraw(), mouse::Cursor::Unavailable);
            ui.draw(
                renderer,
                &iced::Theme::Dark,
                &iced::advanced::renderer::Style {
                    text_color: Color::WHITE,
                },
                mouse::Cursor::Unavailable,
            );
            let pixels = shot(renderer);
            (ui.into_cache(), pixels)
        };

        let (cache, first) = full(&mut renderer, Cache::default(), true);
        if !matches!(crate::support(), Some(Some(_))) {
            eprintln!("SKIPPED: the renderer's device does not import DMA-BUF");
            return;
        }
        let (mut cache, first) = if px(&first, 32) == [255, 0, 0] {
            (cache, first)
        } else {
            full(&mut renderer, cache, true)
        };
        assert_eq!(px(&first, 32), [255, 0, 0], "the early publication shows");

        // A new frame of the same size: a content-only frame shows it, and
        // the chrome is as it was.
        source.publish(colored(&gbm, 61, 32, GREEN, &released));
        let mut ui = UserInterface::build(view(true), size, cache, &mut renderer);
        assert_eq!(
            update(
                &mut ui,
                &mut renderer,
                &[iced::Event::Waken],
                mouse::Cursor::Unavailable
            ),
            Some(true),
            "a publication alone asks for a content-only frame"
        );
        assert!(
            iced::advanced::Renderer::reuse_frame(&mut renderer),
            "the frame is presented again"
        );
        let second = shot(&mut renderer);
        assert_eq!(px(&second, 32), [0, 255, 0], "the new frame is drawn");
        assert_eq!(
            chrome_rows(&second),
            chrome_rows(&first),
            "the chrome is untouched"
        );
        // The chrome hovered in the same update: a full frame. A frame
        // passes first, as it does in the runtime, so the chrome knows how it
        // was drawn.
        let _ = update(
            &mut ui,
            &mut renderer,
            &redraw(),
            mouse::Cursor::Unavailable,
        );
        source.publish(colored(&gbm, 62, 32, RED, &released));
        assert_eq!(
            update(
                &mut ui,
                &mut renderer,
                &[
                    iced::Event::Waken,
                    iced::Event::Mouse(mouse::Event::CursorMoved {
                        position: iced::Point::new(16.0, 8.0),
                    }),
                ],
                mouse::Cursor::Available(iced::Point::new(16.0, 8.0)),
            ),
            Some(false),
            "a hover in the same update takes the full path"
        );
        cache = ui.into_cache();

        // Still being written: the frame shown before stays.
        let (next, _) = full(&mut renderer, cache, true);
        cache = next;
        force(Some(Readiness::Unfinished));
        source.publish(colored(&gbm, 63, 32, GREEN, &released));
        let mut ui = UserInterface::build(view(true), size, cache, &mut renderer);
        let _ = update(
            &mut ui,
            &mut renderer,
            &[iced::Event::Waken],
            mouse::Cursor::Unavailable,
        );
        assert!(iced::advanced::Renderer::reuse_frame(&mut renderer));
        assert_eq!(
            px(&shot(&mut renderer), 32),
            [255, 0, 0],
            "the previous frame stays"
        );
        force(Some(Readiness::Ready));
        assert_eq!(
            update(
                &mut ui,
                &mut renderer,
                &redraw(),
                mouse::Cursor::Unavailable
            ),
            Some(true),
            "the unfinished frame is tried again in a content-only frame"
        );
        assert!(iced::advanced::Renderer::reuse_frame(&mut renderer));
        assert_eq!(
            px(&shot(&mut renderer), 32),
            [0, 255, 0],
            "then it is drawn"
        );
        cache = ui.into_cache();

        // Refused while only the image was prepared: a full frame shows the
        // fallback.
        force(Some(Readiness::Failed("an error".into())));
        source.publish(colored(&gbm, 64, 32, RED, &released));
        let mut ui = UserInterface::build(view(true), size, cache, &mut renderer);
        let _ = update(
            &mut ui,
            &mut renderer,
            &[iced::Event::Waken],
            mouse::Cursor::Unavailable,
        );
        assert!(iced::advanced::Renderer::reuse_frame(&mut renderer));
        let _ = shot(&mut renderer);
        assert_eq!(
            update(
                &mut ui,
                &mut renderer,
                &redraw(),
                mouse::Cursor::Unavailable
            ),
            Some(false),
            "the refusal asks for a full frame"
        );
        cache = ui.into_cache();
        let (next, refused) = full(&mut renderer, cache, true);
        cache = next;
        assert_eq!(px(&refused, 32), [0, 0, 255], "the fallback shows");
        force(None);

        // A frame of another size needs layout. The view is laid out before
        // the publication, as the runtime's is.
        let mut ui = UserInterface::build(view(true), size, cache, &mut renderer);
        source.publish(colored(&gbm, 65, 16, GREEN, &released));
        assert_eq!(
            update(
                &mut ui,
                &mut renderer,
                &[iced::Event::Waken],
                mouse::Cursor::Unavailable
            ),
            Some(false),
            "a new size takes the full path"
        );
        cache = ui.into_cache();
        // The first frame finds the new frame imports; the next drops the
        // fallback it drew.
        let (next, _) = full(&mut renderer, cache, true);
        let (next, resized) = full(&mut renderer, next, true);
        cache = next;
        assert_eq!(px(&resized, 32), [0, 255, 0], "the resized frame shows");

        // Cleared: nothing to show, so the fallback.
        let mut ui = UserInterface::build(view(true), size, cache, &mut renderer);
        source.clear();
        assert_eq!(
            update(
                &mut ui,
                &mut renderer,
                &[iced::Event::Waken],
                mouse::Cursor::Unavailable
            ),
            Some(false),
            "an empty source has another size"
        );
        cache = ui.into_cache();
        let (next, cleared) = full(&mut renderer, cache, true);
        cache = next;
        assert_eq!(
            px(&cleared, 32),
            [0, 0, 255],
            "the fallback, with nothing to show"
        );

        // Hidden, then shown again: the frame is released, and the latest one
        // shows again.
        source.publish(colored(&gbm, 66, 32, RED, &released));
        let (next, _) = full(&mut renderer, cache, false);
        let (next, _) = full(&mut renderer, next, false);
        let (next, _) = full(&mut renderer, next, false);
        let (next, _) = full(&mut renderer, next, true);
        let (_, shown_again) = full(&mut renderer, next, true);
        assert_eq!(px(&shown_again, 32), [255, 0, 0], "shown again");

        // Every superseded frame came back; the latest is still held.
        let released = released.lock().unwrap().clone();
        for allocation in [60, 61, 62, 63, 64, 65] {
            assert!(
                released.contains(&allocation),
                "{allocation} released: {released:?}"
            );
        }
        assert!(!released.contains(&66), "the shown frame stays leased");
    }
    fn colored_box(pixels: &[u8], width: u32, rgb: [u8; 3]) -> Option<(u32, u32, u32, u32)> {
        let mut points = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, pixel)| pixel[..3] == rgb)
            .map(|(index, _)| (index as u32 % width, index as u32 / width));
        let (x, y) = points.next()?;
        Some(points.fold((x, y, x + 1, y + 1), |b, p| {
            (
                b.0.min(p.0),
                b.1.min(p.1),
                b.2.max(p.0 + 1),
                b.3.max(p.1 + 1),
            )
        }))
    }

    #[test]
    fn native_scale_fallback_uses_alignment_and_viewport_clipping() {
        let _serial = serial();
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("tiny-skia"),
            ),
        )
        .expect("software renderer");
        let mut screen = Screen(renderer, Cache::default());
        let source = crate::ExternalFrameSource::new();
        let fallback =
            iced::advanced::image::Handle::from_rgba(32, 24, [238, 0, 170, 255].repeat(32 * 24));
        for (alignment, expected) in [
            (None, (16, 12, 48, 36)),
            (
                Some((alignment::Horizontal::Left, alignment::Vertical::Top)),
                (0, 0, 32, 24),
            ),
            (
                Some((alignment::Horizontal::Right, alignment::Vertical::Bottom)),
                (32, 24, 64, 48),
            ),
        ] {
            for scale in [1.0, 2.0] {
                let mut image = crate::enz_external_image_source(&source)
                    .fallback(fallback.clone())
                    .content_fit(ContentFit::None)
                    .width(64)
                    .height(48);
                if let Some((x, y)) = alignment {
                    image = image.align_x(x).align_y(y);
                }
                let view: iced::Element<'_, ()> = image.into();
                let mut ui = UserInterface::build(
                    view,
                    Size::new(64.0, 48.0),
                    std::mem::take(&mut screen.1),
                    &mut screen.0,
                );
                ui.draw(
                    &mut screen.0,
                    &iced::Theme::Dark,
                    &iced::advanced::renderer::Style {
                        text_color: Color::WHITE,
                    },
                    mouse::Cursor::Unavailable,
                );
                screen.1 = ui.into_cache();
                let width = (64.0 * scale) as u32;
                let height = (48.0 * scale) as u32;
                let pixels = screen
                    .0
                    .screenshot(Size::new(width, height), scale, Color::BLACK);
                let scaled = |x: u32| (x as f32 * scale) as u32;
                assert_eq!(
                    colored_box(&pixels, width, [238, 0, 170]),
                    Some((
                        scaled(expected.0),
                        scaled(expected.1),
                        scaled(expected.2),
                        scaled(expected.3)
                    ))
                );
            }
        }
        let image = crate::enz_external_image_source(&source)
            .fallback(fallback)
            .content_fit(ContentFit::None)
            .align_x(alignment::Horizontal::Left)
            .align_y(alignment::Vertical::Top)
            .width(16)
            .height(12);
        let view: iced::Element<'_, ()> = image.into();
        let mut ui = UserInterface::build(
            view,
            Size::new(64.0, 48.0),
            std::mem::take(&mut screen.1),
            &mut screen.0,
        );
        ui.draw(
            &mut screen.0,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        screen.1 = ui.into_cache();
        let pixels = screen.0.screenshot(Size::new(64, 48), 1.0, Color::BLACK);
        assert_eq!(
            colored_box(&pixels, 64, [238, 0, 170]),
            Some((0, 0, 16, 12))
        );
    }
    #[test]
    #[ignore = "requires a DMA-BUF importing GPU; run this qualifier with --include-ignored"]
    fn native_scale_source_keeps_top_left_geometry_while_new_frame_is_writing() {
        let _serial = serial();
        let gbm = gbm::Gbm::open().expect("GBM device required for resize qualification");
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        )
        .expect("GPU renderer required for resize qualification");
        let mut screen = Screen(renderer, Cache::default());
        let source = crate::ExternalFrameSource::new();
        let old = ExternalFrame::new(
            gbm.buffer(32, 32, None, 991, |_, _| [170, 0, 238, 255])
                .expect("old buffer"),
            Lease::none(),
        );
        let next = ExternalFrame::new(
            gbm.buffer(48, 24, None, 992, |_, _| GREEN)
                .expect("next buffer"),
            Lease::none(),
        );
        source.publish(Arc::clone(&old));
        let mut show =
            |width: u32, height: u32, scale: f32, publish: Option<&Arc<ExternalFrame>>| {
                let view: iced::Element<'_, ()> = crate::enz_external_image_source(&source)
                    .content_fit(ContentFit::None)
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Top)
                    .width(iced::Length::Fill)
                    .height(iced::Length::Fill)
                    .into();
                let mut ui = UserInterface::build(
                    view,
                    Size::new(width as f32, height as f32),
                    std::mem::take(&mut screen.1),
                    &mut screen.0,
                );
                ui.draw(
                    &mut screen.0,
                    &iced::Theme::Dark,
                    &iced::advanced::renderer::Style {
                        text_color: Color::WHITE,
                    },
                    mouse::Cursor::Unavailable,
                );
                screen.1 = ui.into_cache();
                if let Some(frame) = publish {
                    source.publish(Arc::clone(frame));
                }
                screen.0.screenshot(
                    Size::new(
                        (width as f32 * scale) as u32,
                        (height as f32 * scale) as u32,
                    ),
                    scale,
                    Color::BLACK,
                )
            };
        force(Some(Readiness::Ready));
        assert_eq!(
            colored_box(&show(64, 48, 1.0, None), 64, [238, 0, 170]),
            Some((0, 0, 32, 32)),
            "native-size initial source"
        );
        assert!(
            matches!(crate::support(), Some(Some(_))),
            "DMA-BUF import must actually be exercised"
        );
        force(Some(Readiness::Unfinished));
        assert_eq!(
            colored_box(&show(96, 64, 1.0, Some(&next)), 96, [238, 0, 170]),
            Some((0, 0, 32, 32)),
            "retained buffer must keep its geometry during pending import"
        );
        assert_eq!(next.status(), Status::Writing);
        force(Some(Readiness::Ready));
        assert_eq!(
            colored_box(&show(96, 64, 1.0, None), 96, [0, 255, 0]),
            Some((0, 0, 48, 24)),
            "new source uses its actual buffer geometry"
        );
        assert_eq!(
            colored_box(&show(24, 16, 1.0, None), 24, [0, 255, 0]),
            Some((0, 0, 24, 16)),
            "shrinking clips native-size content"
        );
        assert_eq!(
            colored_box(&show(96, 64, 2.0, None), 192, [0, 255, 0]),
            Some((0, 0, 96, 48)),
            "native logical size follows output scale"
        );
        let before = crate::stats();
        let zero_control = show(96, 64, 2.0, None);
        assert_eq!(zero_control, show(96, 64, 2.0, None));
        let after = crate::stats();
        assert_eq!(before.imports_made, after.imports_made);
        assert_eq!(before.bindings_made, after.bindings_made);
        assert_eq!(before.uniform_writes, after.uniform_writes);
        let image = crate::enz_external_image_source(&source)
            .content_fit(ContentFit::Fill)
            .width(iced::Length::Fill)
            .height(iced::Length::Fill);
        let view: iced::Element<'_, ()> = enzui_widgets::surface::enz_surface(image)
            .radius(22.0)
            .clip(true)
            .padding(0)
            .width(96)
            .height(64)
            .into_element();
        let mut ui = UserInterface::build(
            view,
            Size::new(96.0, 64.0),
            std::mem::take(&mut screen.1),
            &mut screen.0,
        );
        ui.draw(
            &mut screen.0,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        screen.1 = ui.into_cache();
        let pixels = screen.0.screenshot(Size::new(96, 64), 1.0, Color::BLACK);
        let at = (32 * 96 + 48) * 4;
        assert_eq!(
            &pixels[at..at + 3],
            &[0, 255, 0],
            "the imported frame fills the panel"
        );
        for (x, y) in [(1, 1), (94, 1), (1, 62), (94, 62)] {
            let at = (y * 96 + x) * 4;
            assert_eq!(
                &pixels[at..at + 3],
                &[0, 0, 0],
                "an imported frame escapes its enclosing rounded panel at ({x},{y})"
            );
        }
        force(None);
    }
}
