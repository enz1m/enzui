#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::blocking_task::BlockingTaskOwner;
use enzui_resources::local_image::*;
use iced_core::{Size, image::Handle};
use std::{
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    task::{Context, Poll, Wake, Waker},
    time::{Duration, Instant},
};
struct ThreadWake(std::thread::Thread);
impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}
fn await_task<F: Future>(task: F) -> F::Output {
    let mut task = std::pin::pin!(task);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Poll::Ready(value) = task.as_mut().poll(&mut context) {
            return value;
        }
        std::thread::park_timeout(
            deadline
                .checked_duration_since(Instant::now())
                .expect("local worker deadline"),
        );
    }
}
fn service(registry: ProviderRegistry) -> LocalImages {
    LocalImages::new(
        BlockingTaskOwner::with_defaults("local-tests").unwrap(),
        registry,
    )
}
fn run(
    registry: ProviderRegistry,
    request: LocalImageRequest,
) -> Result<LocalImageResult, LocalImageError> {
    let service = service(registry);
    let settled = await_task(service.submit(19, request));
    assert_eq!(settled.generation(), 19);
    settled.into_result().unwrap()
}
struct FixtureProvider {
    available: bool,
    decode_count: Arc<AtomicUsize>,
    frames: usize,
    duration: Duration,
    invalid_storage: bool,
    output_width: u32,
    capabilities: ImageCapabilities,
    entered: Option<mpsc::SyncSender<()>>,
    release: Option<Mutex<mpsc::Receiver<()>>>,
}
impl FixtureProvider {
    fn normal() -> Self {
        Self {
            available: true,
            decode_count: Arc::new(AtomicUsize::new(0)),
            frames: 1,
            duration: Duration::from_millis(10),
            invalid_storage: false,
            output_width: 2,
            capabilities: ImageCapabilities {
                still_preview: true,
                flattened_raster: true,
                motion_preview: true,
            },
            entered: None,
            release: None,
        }
    }
}
impl LocalImageProvider for FixtureProvider {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            key: "fixture",
            label: "Explicit fixture raster",
            extensions: &["fixture"],
            kind: MediaKind::DevelopedRaw,
            capabilities: self.capabilities,
        }
    }
    fn availability(&self) -> DecoderAvailability {
        if self.available {
            DecoderAvailability::Available
        } else {
            DecoderAvailability::Unavailable("explicit fixture decoder absent")
        }
    }
    fn recognizes(&self, bytes: &[u8]) -> bool {
        bytes.starts_with(b"ENZR")
    }
    fn probe(
        &self,
        request: &LocalImageRequest,
        _: &dyn Fn() -> bool,
    ) -> Result<ProviderHeader, LocalImageError> {
        if let Some(entered) = &self.entered {
            entered.send(()).unwrap();
            self.release
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
        if request.bytes.len() != 13 {
            return Err(LocalImageError::Malformed("fixture"));
        }
        Ok(ProviderHeader {
            dimensions: Size::new(
                u32::from_be_bytes(request.bytes[4..8].try_into().unwrap()),
                u32::from_be_bytes(request.bytes[8..12].try_into().unwrap()),
            ),
            orientation: request.bytes[12],
        })
    }
    fn decode(
        &self,
        request: &LocalImageRequest,
        _: ProviderHeader,
        _: &dyn Fn() -> bool,
    ) -> Result<Vec<LocalFrame>, LocalImageError> {
        self.decode_count.fetch_add(1, Ordering::SeqCst);
        let target = match request.operation {
            LocalImageOperation::Preview(target)
            | LocalImageOperation::FlattenedRaster(target)
            | LocalImageOperation::Motion { target, .. } => target,
            LocalImageOperation::Probe => Size::new(2, 1),
        };
        assert_eq!(target, Size::new(2, 1));
        Ok((0..self.frames)
            .map(|_| LocalFrame {
                handle: Handle::from_rgba(
                    self.output_width,
                    1,
                    vec![
                        4;
                        if self.invalid_storage {
                            7
                        } else {
                            self.output_width as usize * 4
                        }
                    ],
                ),
                duration: self.duration,
            })
            .collect())
    }
}
fn encoded(width: u32, height: u32, orientation: u8) -> Arc<[u8]> {
    let mut bytes = b"ENZR".to_vec();
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.push(orientation);
    bytes.into()
}
fn registry(provider: FixtureProvider) -> ProviderRegistry {
    let mut registry = ProviderRegistry::default();
    registry.register(Arc::new(provider)).unwrap();
    registry
}
fn request() -> LocalImageRequest {
    LocalImageRequest::preview(encoded(4, 2, 6), 2, 1).unwrap()
}
#[test]
fn local_images_available_adapters_drive_filters_and_missing_is_distinct_from_corruption() {
    let mut provider = FixtureProvider::normal();
    provider.available = false;
    let absent = registry(provider);
    assert!(
        !absent
            .available_formats()
            .iter()
            .any(|format| format.extensions.contains(&"fixture"))
    );
    assert_eq!(
        run(absent, request()).unwrap_err(),
        LocalImageError::MissingDecoder {
            provider: "fixture",
            reason: "explicit fixture decoder absent"
        }
    );
    let available = registry(FixtureProvider::normal());
    assert!(
        available
            .available_formats()
            .iter()
            .any(|format| format.extensions.contains(&"fixture"))
    );
    assert_eq!(
        run(
            available.clone(),
            LocalImageRequest::preview(Arc::from(&b"ENZR"[..]), 2, 1).unwrap()
        )
        .unwrap_err(),
        LocalImageError::Malformed("fixture")
    );
    assert_eq!(
        run(
            available.clone(),
            LocalImageRequest::preview(Arc::from(&b"unknown.fixture"[..]), 2, 1).unwrap()
        )
        .unwrap_err(),
        LocalImageError::UnknownFormat
    );
    let mut duplicate = available;
    assert_eq!(
        duplicate.register(Arc::new(FixtureProvider::normal())),
        Err(LocalImageError::InvalidProvider)
    );
}
