//! Partial presentation must retire obsolete frames while retaining unchanged scene images and text.

use iced_wgpu::core::image::{Handle, Renderer as _};
use iced_wgpu::core::svg::{Handle as SvgHandle, Renderer as _};
use iced_wgpu::core::text::Renderer as _;
use iced_wgpu::core::{Color, Image, Point, Radians, Rectangle, Renderer as _, Size, Svg};
use iced_wgpu::graphics::Viewport;
use iced_wgpu::{Engine, ImageCachePolicy, Renderer, wgpu};

const EDGE: u32 = 256;
const SVG: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="#369"/></svg>"##;

fn rect(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
    Rectangle {
        x,
        y,
        width,
        height,
    }
}

fn read(device: &wgpu::Device, queue: &wgpu::Queue, target: &wgpu::Texture) -> Vec<u8> {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("partial cache readback"),
        size: u64::from(EDGE * EDGE * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(EDGE * 4),
                rows_per_image: Some(EDGE),
            },
        },
        wgpu::Extent3d {
            width: EDGE,
            height: EDGE,
            depth_or_array_layers: 1,
        },
    );
    let submission = queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(submission),
            timeout: Some(std::time::Duration::from_secs(10)),
        })
        .unwrap();
    receive
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap()
        .unwrap();
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}

fn draw(renderer: &mut Renderer, stream: Handle, steady: &Handle, svg: &SvgHandle, step: u32) {
    renderer.reset(Rectangle::with_size(Size::new(EDGE as f32, EDGE as f32)));
    let raster_bounds = rect(10., 10., 64., 64.);
    renderer.start_layer(raster_bounds);
    renderer.draw_image(Image::new(steady.clone()), raster_bounds, raster_bounds);
    renderer.end_layer();
    for bounds in [
        rect(96., 10., 64., 64.),
        rect(190., 80., 8. + step as f32, 20.),
    ] {
        renderer.start_layer(bounds);
        renderer.draw_svg(
            Svg {
                handle: svg.clone(),
                color: None,
                rotation: Radians(0.),
                opacity: 1.,
            },
            bounds,
            bounds,
        );
        renderer.end_layer();
    }
    let text_bounds = rect(180., 10., 76., 40.);
    renderer.start_layer(text_bounds);
    renderer.fill_text(
        iced_wgpu::core::text::Text {
            content: "KEEP".into(),
            bounds: text_bounds.size(),
            size: 20.0.into(),
            line_height: Default::default(),
            font: Default::default(),
            align_x: iced_wgpu::core::text::Alignment::Left,
            align_y: iced_wgpu::core::alignment::Vertical::Top,
            shaping: iced_wgpu::core::text::Shaping::Advanced,
            wrapping: iced_wgpu::core::text::Wrapping::None,
            ellipsis: Default::default(),
            hint_factor: None,
        },
        Point::new(180., 10.),
        Color::WHITE,
        text_bounds,
    );
    renderer.end_layer();
    let stream_bounds = rect(10., 150., 32., 32.);
    renderer.start_layer(stream_bounds);
    renderer.draw_image(Image::new(stream), stream_bounds, stream_bounds);
    renderer.end_layer();
}

fn region(pixels: &[u8], bounds: (u32, u32, u32, u32)) -> Vec<u8> {
    let (x, y, width, height) = bounds;
    (y..y + height)
        .flat_map(|row| {
            let start = ((row * EDGE + x) * 4) as usize;
            pixels[start..start + (width * 4) as usize].iter().copied()
        })
        .collect()
}

#[test]
fn partial_image_cache_retires_stream_frames_and_preserves_retained_ink() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::VULKAN,
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let (device, queue, adapter) = iced_test::futures::futures::executor::block_on(async {
        let adapter = instance
            .request_adapter(&Default::default())
            .await
            .expect("GPU adapter required");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .expect("GPU device");
        (device, queue, adapter)
    });
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let engine = Engine::new(
        &adapter,
        device.clone(),
        queue.clone(),
        format,
        None,
        iced_wgpu::graphics::Shell::headless(),
    );
    let mut renderer = Renderer::new(engine, Default::default());
    renderer.track_frame_damage(true);
    renderer.set_image_cache_policy(ImageCachePolicy::new(1, 0));
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("partial cache target"),
        size: wgpu::Extent3d {
            width: EDGE,
            height: EDGE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let viewport = Viewport::with_physical_size(
        Size::new(EDGE, EDGE),
        iced_wgpu::core::renderer::Scale {
            window: 1.,
            application: 1.,
        },
    );
    let steady = Handle::from_rgba(64, 64, [0, 200, 0, 255].repeat(64 * 64));
    let svg = SvgHandle::from_memory(SVG.to_vec());
    let mut stable = None;
    let mut initial_epoch = 0;
    let mut previous_stream = None;
    let mut last_stream = None;
    for step in 0..24 {
        let stream = Handle::from_rgba(32, 32, [step as u8 * 8, 50, 20, 255].repeat(32 * 32));
        last_stream = Some(stream.clone());
        draw(&mut renderer, stream, &steady, &svg, step);
        renderer.present_damaged(Color::BLACK, &target, &viewport, false);
        let pixels = read(&device, &queue, &target);
        let unchanged = [
            region(&pixels, (10, 10, 64, 64)),
            region(&pixels, (96, 10, 64, 64)),
            region(&pixels, (180, 10, 76, 40)),
        ];
        if let Some(stable) = &stable {
            assert_eq!(
                &unchanged, stable,
                "unchanged image/SVG/text lost at frame {step}"
            );
        } else {
            assert!(
                unchanged[0].as_chunks::<4>().0.iter().any(|p| p[1] > 100),
                "raster positive control"
            );
            assert!(
                unchanged[1].as_chunks::<4>().0.iter().any(|p| p[2] > 100),
                "SVG positive control"
            );
            assert!(
                unchanged[2]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[0] > 100)
                    .count()
                    > 20,
                "glyph positive control"
            );
            stable = Some(unchanged);
        }
        let moving = region(&pixels, (10, 150, 32, 32));
        if let Some(previous) = &previous_stream {
            assert_ne!(
                &moving, previous,
                "stream pixels must update at frame {step}"
            );
        }
        previous_stream = Some(moving);
        let statistics = renderer.image_cache_statistics();
        if step == 0 {
            initial_epoch = statistics.frame;
        }
        if step > 3 {
            assert!(
                renderer.partial_frames() >= u64::from(step - 1),
                "must exercise partial presentation"
            );
            assert!(
                statistics.frame > initial_epoch,
                "cache epoch stalled during partial frames"
            );
            assert_eq!(
                statistics.raster_entries, 2,
                "retain only the visible still image and latest frame: {statistics:?}"
            );
            assert_eq!(
                statistics.svg_raster_entries, 2,
                "retain only the two visible SVG sizes: {statistics:?}"
            );
            assert_eq!(statistics.svg_entries, 1, "visible SVG source evicted");
        }
    }
    assert!(
        renderer.image_cache_statistics().raster_evictions >= 20,
        "old streaming images must be evicted"
    );
    let before_repaint = renderer.image_cache_statistics();
    renderer.track_frame_damage(false);
    draw(
        &mut renderer,
        last_stream.expect("last frame"),
        &steady,
        &svg,
        23,
    );
    renderer.present_damaged(Color::BLACK, &target, &viewport, false);
    let repainted = read(&device, &queue, &target);
    let unchanged = [
        region(&repainted, (10, 10, 64, 64)),
        region(&repainted, (96, 10, 64, 64)),
        region(&repainted, (180, 10, 76, 40)),
    ];
    assert_eq!(
        Some(unchanged),
        stable,
        "full repaint must preserve retained image and glyph ink"
    );
    let after_repaint = renderer.image_cache_statistics();
    assert_eq!(
        after_repaint.raster_uploads, before_repaint.raster_uploads,
        "unchanged raster must remain cached"
    );
    assert_eq!(
        after_repaint.svg_uploads, before_repaint.svg_uploads,
        "unchanged SVG variants must remain cached"
    );
}
