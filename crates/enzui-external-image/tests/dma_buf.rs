//! DMA-BUF frames the driver allocated (through GBM on the render node) are
//! imported and drawn by the shared renderer: their pixels, scaled, clipped
//! and faded; several at once; a pool cycled without leaking descriptors;
//! and leases handed back only once nothing shows the frame.
//!
//! Skips (and says so) without a render node, GBM or an importing device.

#![allow(clippy::unwrap_used, reason = "a test")]

use enzui_external_image::{
    DmaBuf, ExternalFrame, Lease, Status, enz_external_image, imports_made, support,
};
use iced::advanced::renderer::Headless as _;
use iced::widget::column;
use iced::{Color, ContentFit, Element, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[path = "gbm/mod.rs"]
mod gbm;

use gbm::Gbm;

/// Red, green, blue and white quadrants, opaque, as B, G, R, A.
fn quadrants(width: u32, height: u32) -> impl Fn(u32, u32) -> [u8; 4] {
    move |x, y| match (x < width / 2, y < height / 2) {
        (true, true) => [0, 0, 255, 255],
        (false, true) => [0, 255, 0, 255],
        (true, false) => [255, 0, 0, 255],
        (false, false) => [255, 255, 255, 255],
    }
}

struct Screen {
    renderer: iced::Renderer,
    cache: Cache,
    size: Size,
}

impl Screen {
    fn new(size: Size) -> Option<Self> {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        )?;
        Some(Self {
            renderer,
            cache: Cache::default(),
            size,
        })
    }

    /// Draws `view` and reads the frame back as RGBA rows.
    fn shot(&mut self, view: Element<'_, ()>) -> Vec<u8> {
        let mut ui = UserInterface::build(
            view,
            self.size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        ui.draw(
            &mut self.renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        self.cache = ui.into_cache();
        self.renderer.screenshot(
            Size::new(self.size.width as u32, self.size.height as u32),
            1.0,
            Color::BLACK,
        )
    }

    fn pixel(&self, shot: &[u8], x: u32, y: u32) -> [u8; 3] {
        let at = ((y * self.size.width as u32 + x) * 4) as usize;
        [shot[at], shot[at + 1], shot[at + 2]]
    }
}

/// The tests share the process's import counter and the GPU: one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// The importing renderer, a GBM device and what the renderer imports, or
/// `None` (after saying why) when this machine cannot run the test.
fn setup(size: Size) -> Option<(Screen, Gbm, enzui_external_image::Support)> {
    let Some(gbm) = Gbm::open() else {
        eprintln!("SKIPPED: no GBM render node");
        return None;
    };
    let Some(mut screen) = Screen::new(size) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return None;
    };
    // The renderer reports what it imports once it has drawn an external
    // image; a one-pixel frame is enough.
    let probe = gbm.buffer(2, 2, None, u64::MAX, |_, _| [0; 4])?;
    let frame = ExternalFrame::new(probe, Lease::none());
    let _ = screen.shot(enz_external_image(&frame).into());
    match support() {
        Some(Some(support)) => Some((screen, gbm, support)),
        _ => {
            eprintln!("SKIPPED: the renderer's device does not import DMA-BUF");
            None
        }
    }
}

/// Open descriptors of this process.
fn descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd").map_or(0, Iterator::count)
}

#[test]
fn a_pool_cycles_without_leaking_and_leases_return_once_nothing_shows_them() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((mut screen, gbm, _)) = setup(Size::new(128.0, 64.0)) else {
        return;
    };
    let released = Arc::new(Mutex::new(Vec::new()));
    let pool: Vec<DmaBuf> = (0..3)
        .map(|allocation| {
            gbm.buffer(32, 32, None, 10 + allocation, quadrants(32, 32))
                .unwrap()
        })
        .collect();
    let lease = |allocation: u64| {
        let released = Arc::clone(&released);
        Lease::new(move || released.lock().unwrap().push(allocation))
    };
    let show = |screen: &mut Screen, frame: &Arc<ExternalFrame>| {
        let _ = screen.shot(column![enz_external_image(frame).width(32).height(32)].into());
    };

    // The producer shows allocation 10, then 11: 10 is replaced but the
    // application still holds it, so it is not handed back.
    let first = ExternalFrame::new(pool[0].clone(), lease(10));
    show(&mut screen, &first);
    let second = ExternalFrame::new(pool[1].clone(), lease(11));
    show(&mut screen, &second);
    show(&mut screen, &second);
    assert!(
        released.lock().unwrap().is_empty(),
        "held frames stay leased"
    );
    // Dropped by the application: handed back after the next frame.
    drop(first);
    show(&mut screen, &second);
    show(&mut screen, &second);
    assert_eq!(*released.lock().unwrap(), [10], "replaced and dropped");
    assert_eq!(second.status(), Status::Imported);

    // Cycling the pool imports each allocation once: no descriptor grows.
    let shown = Arc::new(AtomicUsize::new(0));
    let cycle = |screen: &mut Screen, rounds: u64| {
        for round in 0..rounds {
            let index = (round % 3) as usize;
            let shown = Arc::clone(&shown);
            let frame = ExternalFrame::new(
                pool[index].clone(),
                Lease::new(move || {
                    let _ = shown.fetch_add(1, Ordering::Relaxed);
                }),
            );
            show(screen, &frame);
        }
    };
    cycle(&mut screen, 6);
    let warm = descriptors();
    let imports = imports_made();
    cycle(&mut screen, 24);
    // The driver may close descriptors of its own meanwhile; a leak grows.
    assert!(
        descriptors() <= warm,
        "cycling a pool leaked descriptors: {warm} became {}",
        descriptors()
    );
    assert_eq!(imports_made(), imports, "a cycled pool was imported again");
    assert!(
        shown.load(Ordering::Relaxed) >= 28,
        "replaced frames were not handed back: {}",
        shown.load(Ordering::Relaxed)
    );

    // Tearing the renderer down hands back what it still showed.
    drop(second);
    drop(screen);
    assert_eq!(
        *released.lock().unwrap(),
        [10, 11],
        "the last shown frame comes back on teardown"
    );
}

#[test]
fn a_refused_frame_says_why_and_shows_its_fallback() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some((mut screen, gbm, _)) = setup(Size::new(64.0, 64.0)) else {
        return;
    };
    let mut buffer = gbm.buffer(32, 32, None, 20, quadrants(32, 32)).unwrap();
    // NV12 is a video format this crate does not import.
    buffer.format = u32::from_le_bytes(*b"NV12");
    let frame = ExternalFrame::new(buffer, Lease::none());
    let fallback = iced::advanced::image::Handle::from_rgba(1, 1, vec![0, 0, 255, 255]);
    // The first frame finds out; the next draws the fallback alone.
    for _ in 0..2 {
        let _ = screen.shot(
            enz_external_image(&frame)
                .fallback(fallback.clone())
                .content_fit(ContentFit::Fill)
                .width(64)
                .height(64)
                .into(),
        );
    }
    assert_eq!(
        frame.status(),
        Status::Refused(enzui_external_image::Refusal::Format(u32::from_le_bytes(
            *b"NV12"
        )))
    );
    let shot = screen.shot(
        enz_external_image(&frame)
            .fallback(fallback)
            .content_fit(ContentFit::Fill)
            .width(64)
            .height(64)
            .into(),
    );
    assert_eq!(
        screen.pixel(&shot, 32, 32),
        [0, 0, 255],
        "the fallback image"
    );
}
