//! One deterministic renderer for pixel gates.
//!
//! The checked-in render baselines were recorded with iced's tiny-skia
//! software renderer. `Headless::new(_, None)` instead tries wgpu first and
//! silently falls back to tiny-skia. That made a plain Cargo run use wgpu while
//! orchestrated runs used software, so identical source produced different
//! pixels. Pixel gates use [`renderer`] instead of adapter discovery.

use iced::advanced::renderer::Headless;

/// The renderer used by checked-in pixel baselines.
pub const PINNED_BACKEND: &str = "software";

/// Builds the renderer used by checked-in pixel baselines without probing an
/// adapter or consulting process state.
pub fn renderer() -> iced_test::renderer::Renderer {
    let renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some(PINNED_BACKEND),
        ),
    )
    .expect("the pinned software renderer must be available");

    assert_eq!(
        Headless::name(&renderer),
        "tiny-skia",
        "pixel gates must render with the pinned tiny-skia backend"
    );
    renderer
}
