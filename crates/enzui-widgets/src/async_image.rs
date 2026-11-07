//! A snapshot-driven image slot with loading, ready and failure states.
//!
//! The caller owns requests, I/O and decode admission through
//! [`enzui_core::async_image::EnzAsyncImage`]. This view clones the supplied
//! state and performs no I/O. It fills the available slot and uses Cover fit by
//! default. Loading can pulse from caller-supplied elapsed seconds; the widget
//! does not schedule animation ticks. Idle has no sheen, and failures use a
//! danger-coloured glyph and border resolved from the live palette.
//!
//! Trimmed, nonblank alt text publishes an image leaf with loading or failure
//! annotations; blank text leaves the image decorative. The requested radius
//! applies to all slot backgrounds and the ready image.

use enzui_core::async_image::{FailureReason, ImageState};
use iced::widget::{container, image as iced_image, text};
use iced::{Border, Color, ContentFit, Element, Length};

/// How the decoded image is scaled within its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzAsyncImageFit {
    /// Fill the slot, cropping overflow (default).
    #[default]
    Cover,
    /// Fit entirely inside the slot, letterboxing.
    Contain,
    /// Stretch to fill the slot exactly.
    Fill,
    /// Draw at native size.
    None,
}

impl EnzAsyncImageFit {
    /// Convert to iced's [`ContentFit`].
    pub fn to_iced(self) -> ContentFit {
        match self {
            Self::Cover => ContentFit::Cover,
            Self::Contain => ContentFit::Contain,
            Self::Fill => ContentFit::Fill,
            Self::None => ContentFit::None,
        }
    }
}

const SHIMMER_SHEEN_STATIC: f32 = 0.35;
const SHIMMER_PERIOD_SECS: f32 = 1.8;

/// An owned image-state snapshot with slot geometry and presentation options.
#[must_use]
pub struct EnzAsyncImageView<H> {
    state: ImageState<H>,
    fit: EnzAsyncImageFit,
    radius: f32,
    alt_text: String,
    width: Length,
    height: Length,
    shimmer_phase: Option<f32>,
}

/// Clone the current state into a view with Fill dimensions, Cover fit and no alt text.
pub fn enz_async_image_view<H: Clone>(state: &ImageState<H>) -> EnzAsyncImageView<H> {
    EnzAsyncImageView {
        state: state.clone(),
        fit: EnzAsyncImageFit::Cover,
        radius: 0.0,
        alt_text: String::new(),
        width: Length::Fill,
        height: Length::Fill,
        shimmer_phase: None,
    }
}

impl<H> EnzAsyncImageView<H> {
    /// Set the fit mode for the decoded image. Defaults to
    /// [`EnzAsyncImageFit::Cover`].
    pub fn fit(mut self, f: EnzAsyncImageFit) -> Self {
        self.fit = f;
        self
    }

    /// Corner radius in px. Rounds both the image and the placeholder/failure
    /// background. Non-finite or negative input falls back to 0.
    pub fn radius(mut self, r: f32) -> Self {
        self.radius = enzui_core::metrics::FiniteNonNeg::new_or(r, 0.0).get();
        self
    }

    /// Publish an image semantics leaf using trimmed alt text and lifecycle annotations.
    /// Whitespace-only or empty text makes the image decorative.
    pub fn alt_text(mut self, t: impl Into<String>) -> Self {
        self.alt_text = t.into();
        self
    }

    /// Set the slot width. Defaults to [`Length::Fill`].
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Set the slot height. Defaults to [`Length::Fill`].
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Set elapsed seconds for a 1.8-second triangular loading pulse.
    /// Unset uses a static 0.35 sheen; the pulse ranges from 0 to 0.7.
    /// Supply finite, nonnegative time and schedule updates in the caller.
    pub fn shimmer_phase(mut self, phase: f32) -> Self {
        self.shimmer_phase = Some(phase);
        self
    }

    fn semantics(&self) -> Option<enzui_core::semantics::Semantics>
    where
        H: std::fmt::Debug,
    {
        use enzui_core::semantics::{Role, Semantics};
        let alt = self.alt_text.trim();
        if alt.is_empty() {
            return None;
        }
        let label = match &self.state {
            ImageState::Idle | ImageState::Ready(_) => alt.to_owned(),
            ImageState::Loading(_) => enzui_core::localized!("enzui-image-loading", "alt" => alt),
            ImageState::Failed(reason) => {
                enzui_core::localized!("enzui-image-unavailable", "alt" => alt, "reason" => reason.reason())
            }
        };
        Some(Semantics::new(Role::Image).label(label))
    }

    /// Return the snapshot label: "idle", "loading", "ready" or "failed".
    pub fn lifecycle(&self) -> &'static str {
        match &self.state {
            ImageState::Idle => "idle",
            ImageState::Loading(_) => "loading",
            ImageState::Ready(_) => "ready",
            ImageState::Failed(_) => "failed",
        }
    }

    /// The failure reason, if the state is `Failed`.
    pub fn failure_reason(&self) -> Option<&FailureReason> {
        self.state.failure()
    }

    /// Consume the builder and produce the widget tree.
    ///
    /// - `Idle` / `Loading`: a placeholder container with an
    ///   optional shimmer sheen (loading skeleton).
    /// - `Ready`: the decoded image inside a rounded,
    ///   background-filled container.
    /// - `Failed`: a placeholder with a failure indicator.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message>
    where
        H: Clone + Into<iced_image::Handle> + std::fmt::Debug,
    {
        let radius = self.radius;
        let width = self.width;
        let height = self.height;
        let a11y = self.semantics();

        let inner: Element<'a, Message> = match self.state {
            ImageState::Ready(handle) => {
                let handle: iced_image::Handle = handle.into();
                let img: Element<'a, Message> = iced_image(handle)
                    .content_fit(self.fit.to_iced())
                    .border_radius(radius)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into();

                container(img)
                    .width(width)
                    .height(height)
                    .clip(true)
                    .style(move |theme: &iced::Theme| {
                        let bg = crate::theme_lookup::palette(theme).surface_alt;
                        container::Style {
                            background: Some(bg.into()),
                            border: Border {
                                radius: radius.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    })
                    .into()
            }
            ImageState::Loading(_) => {
                let alpha = shimmer_alpha(self.shimmer_phase);
                Self::skeleton_element(radius, width, height, alpha)
            }
            ImageState::Idle => Self::skeleton_element(radius, width, height, 0.0),
            ImageState::Failed(reason) => {
                let label = reason.reason();
                Self::failure_element(radius, width, height, label)
            }
        };

        match a11y {
            Some(sem) => crate::semantics_container::SemanticsContainer::leaf(inner, sem).into(),
            None => inner,
        }
    }

    fn skeleton_element<'a, Message: 'a>(
        radius: f32,
        width: Length,
        height: Length,
        sheen_alpha: f32,
    ) -> Element<'a, Message> {
        container(
            iced::widget::Space::new()
                .width(Length::Fill)
                .height(Length::Fill),
        )
        .width(width)
        .height(height)
        .style(move |theme: &iced::Theme| {
            let p = crate::theme_lookup::palette(theme);
            let base = p.surface_alt;
            let sheen = Color {
                a: sheen_alpha,
                ..p.border
            };
            let bg = blend_over(base, sheen);
            container::Style {
                background: Some(bg.into()),
                border: Border {
                    radius: radius.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .into()
    }

    fn failure_element<'a, Message: 'a>(
        radius: f32,
        width: Length,
        height: Length,
        label: &'static str,
    ) -> Element<'a, Message> {
        let _ = label;
        let glyph = text(FAILURE_GLYPH)
            .size(enzui_theme::text_scale::ui_px(28.0))
            .style(|theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                text::Style {
                    color: Some(failure_glyph_color(&p)),
                }
            });

        container(glyph)
            .width(width)
            .height(height)
            // Centre without replacing the caller's fixed slot dimensions with Fill.
            .align_x(iced::alignment::Horizontal::Center)
            .align_y(iced::alignment::Vertical::Center)
            .style(move |theme: &iced::Theme| {
                let p = crate::theme_lookup::palette(theme);
                container::Style {
                    background: Some(p.surface_alt.into()),
                    border: Border {
                        radius: radius.into(),
                        width: 1.0,
                        color: failure_border_color(&p),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

pub(crate) const FAILURE_GLYPH: &str = "\u{2612}";

pub(crate) fn failure_border_color(p: &enzui_theme::Palette) -> Color {
    p.danger
}

pub(crate) fn failure_glyph_color(p: &enzui_theme::Palette) -> Color {
    p.danger
}

fn shimmer_alpha(phase: Option<f32>) -> f32 {
    match phase {
        Some(elapsed) => {
            let t = (elapsed % SHIMMER_PERIOD_SECS) / SHIMMER_PERIOD_SECS;
            let triangle = if t < 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
            triangle * SHIMMER_SHEEN_STATIC * 2.0
        }
        None => SHIMMER_SHEEN_STATIC,
    }
}

fn blend_over(bg: Color, fg: Color) -> Color {
    let a = fg.a;
    let inv = 1.0 - a;
    Color {
        r: fg.r * a + bg.r * inv,
        g: fg.g * a + bg.g * inv,
        b: fg.b * a + bg.b * inv,
        a: a + bg.a * inv,
    }
}
