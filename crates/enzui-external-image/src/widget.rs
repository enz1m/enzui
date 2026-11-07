//! The widget that shows an [`ExternalFrame`].

use crate::pipeline::Primitive;
use crate::{ExternalFrame, ExternalFrameSource, Refusal, Status, Support};
use iced::advanced::image::Renderer as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Tree, Widget};
use iced::{ContentFit, Element, Length, Rectangle, Size, alignment, mouse};
use iced_wgpu::primitive::Renderer as _;
use std::sync::Arc;

/// Shows `frame`, imported from GPU memory; see the [crate documentation](crate).
pub fn enz_external_image(frame: &Arc<ExternalFrame>) -> ExternalImage {
    image(Shown::Frame(Arc::clone(frame)))
}

/// Shows the latest frame of `source`, redrawing only the image when the
/// producer publishes one; see [`ExternalFrameSource`].
pub fn enz_external_image_source(source: &ExternalFrameSource) -> ExternalImage {
    image(Shown::Source(source.clone()))
}

fn image(shown: Shown) -> ExternalImage {
    ExternalImage {
        shown,
        fallback: None,
        fit: ContentFit::Contain,
        alignment: (alignment::Horizontal::Center, alignment::Vertical::Center),
        opacity: 1.0,
        width: Length::Fill,
        height: Length::Shrink,
    }
}

/// Shows an [`ExternalFrame`]; built by [`enz_external_image`] or
/// [`enz_external_image_source`].
#[derive(Debug)]
pub struct ExternalImage {
    shown: Shown,
    fallback: Option<iced::advanced::image::Handle>,
    fit: ContentFit,
    alignment: (alignment::Horizontal, alignment::Vertical),
    opacity: f32,
    width: Length,
    height: Length,
}

impl ExternalImage {
    /// What to show when the frame cannot be imported (the software
    /// renderer, a device without DMA-BUF import, a refused layout): usually
    /// the producer's copy of the same frame in CPU memory.
    #[must_use]
    pub fn fallback(mut self, image: iced::advanced::image::Handle) -> Self {
        self.fallback = Some(image);
        self
    }

    /// How the image fits the widget's bounds; [`ContentFit::Contain`] by
    /// default.
    #[must_use]
    pub fn content_fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }

    /// Align fitted content horizontally within the allocated viewport (center by default).
    #[must_use]
    pub fn align_x(mut self, alignment: alignment::Horizontal) -> Self {
        self.alignment.0 = alignment;
        self
    }

    /// Align fitted content vertically within the allocated viewport (center by default).
    #[must_use]
    pub fn align_y(mut self, alignment: alignment::Vertical) -> Self {
        self.alignment.1 = alignment;
        self
    }

    /// How opaque the image is drawn, `0.0..=1.0`.
    #[must_use]
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }

    /// The width; [`Length::Fill`] by default.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// The height; the image's aspect ratio at the width by default.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

/// What the widget shows.
#[derive(Debug)]
enum Shown {
    Frame(Arc<ExternalFrame>),
    Source(ExternalFrameSource),
}

impl Shown {
    /// The frame to show now.
    fn frame(&self) -> Option<Arc<ExternalFrame>> {
        match self {
            Self::Frame(frame) => Some(Arc::clone(frame)),
            Self::Source(source) => source.latest(),
        }
    }
}

/// One widget across views: its identity for its uniforms in the pipeline,
/// whether an imported frame is on screen in it, the image size it was laid
/// out for, and whether its last draw showed a refused frame.
struct Instance {
    id: u64,
    on_screen: std::cell::Cell<bool>,
    laid_out: Option<Size>,
    drawn_refused: std::cell::Cell<bool>,
}

fn next_instance() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ExternalImage {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<Instance>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(Instance {
            id: next_instance(),
            on_screen: std::cell::Cell::new(false),
            laid_out: None,
            drawn_refused: std::cell::Cell::new(false),
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let image = image_size(self.shown.frame().as_deref());
        tree.state.downcast_mut::<Instance>().laid_out = Some(image);
        let limits = limits.width(self.width).height(self.height);
        let max = limits.max();
        // The image's own size, shrunk to fit and kept in proportion.
        let scale = (max.width / image.width.max(1.0))
            .min(max.height / image.height.max(1.0))
            .min(1.0);
        let natural = Size::new(image.width * scale, image.height * scale);
        let size = limits.resolve(self.width, self.height, natural);
        layout::Node::new(size).apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let instance = tree.state.downcast_ref::<Instance>();
        let frame = self.shown.frame();
        if let Shown::Source(source) = &self.shown {
            source.register(instance.id, shell.waker());
            // A published frame of the size laid out needs only the image
            // prepared again; a new size needs layout.
            if source.is_pending() {
                if instance.laid_out == Some(image_size(frame.as_deref())) {
                    shell.request_content_redraw();
                } else {
                    shell.invalidate_layout();
                    shell.request_redraw();
                }
            }
        }
        if !matches!(
            event,
            iced::Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            return;
        }
        let status = frame.as_ref().map(|frame| frame.status());
        // A frame its producer is still writing is tried again next frame.
        if status == Some(Status::Writing) {
            match self.shown {
                Shown::Source(_) => shell.request_content_redraw(),
                Shown::Frame(_) => shell.request_redraw(),
            }
        }
        // A refusal found while only the image was prepared shows the
        // fallback in a full frame.
        if matches!(status, Some(Status::Refused(_))) && !instance.drawn_refused.get() {
            shell.request_redraw();
        }
        // A frame that imported after the fallback was drawn over it takes
        // the fallback away in a full frame.
        if status == Some(Status::Imported) && !instance.on_screen.get() {
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let frame = self.shown.frame();
        let supports_primitives = renderer.supports_primitives();
        if !supports_primitives {
            if let Some(frame) = &frame {
                frame.set_status(Status::Refused(Refusal::NoImport));
            }
            crate::record_no_import();
        }
        // The fallback shows while no imported frame is on screen: before the
        // first one, and for a refused frame. A new frame still pending or
        // being written leaves the one shown before in place.
        let instance = tree.state.downcast_ref::<Instance>();
        let status = frame.as_ref().map(|frame| frame.status());
        let on_screen = match status {
            Some(Status::Imported) => true,
            Some(Status::Refused(_)) | None => false,
            Some(Status::Pending | Status::Writing) => instance.on_screen.get(),
        };
        instance.on_screen.set(on_screen);
        instance
            .drawn_refused
            .set(matches!(status, Some(Status::Refused(_))));
        if !on_screen && let Some(fallback) = &self.fallback {
            let image = iced::advanced::image::Image {
                handle: fallback.clone(),
                filter_method: iced::advanced::image::FilterMethod::Linear,
                rotation: iced::Radians(0.0),
                border_radius: iced::border::Radius::default(),
                opacity: self.opacity,
            };
            let size = renderer.measure_image(fallback).unwrap_or_default();
            let placement = crate::pipeline::placement(
                self.fit,
                self.alignment,
                Size::new(size.width as f32, size.height as f32),
                bounds.size(),
            );
            let placed = placement + iced::Vector::new(bounds.x, bounds.y);
            renderer.draw_image(image, placed, bounds);
        }
        if !supports_primitives {
            return;
        }
        // A source's primitive reads the latest frame when it is prepared,
        // so it is drawn even while the source is empty.
        let (frame, source) = match &self.shown {
            Shown::Frame(frame) => (Some(Arc::clone(frame)), None),
            Shown::Source(source) => (None, Some(source.clone())),
        };
        renderer.draw_primitive(
            bounds,
            Primitive {
                instance: tree.state.downcast_ref::<Instance>().id,
                frame,
                source,
                fit: self.fit,
                alignment: self.alignment,
                opacity: self.opacity,
            },
        );
    }
}

/// The size of `frame`'s buffer, or nothing for no frame.
fn image_size(frame: Option<&ExternalFrame>) -> Size {
    frame.map_or(Size::ZERO, |frame| {
        let buffer = frame.buffer();
        Size::new(buffer.width as f32, buffer.height as f32)
    })
}

impl<'a, Message: 'a> From<ExternalImage> for Element<'a, Message> {
    fn from(image: ExternalImage) -> Self {
        Element::new(image)
    }
}

/// Learns what the renderer imports before any frame exists; see
/// [`ExternalImageProbe`].
pub fn enz_external_image_probe<'a, Message>() -> ExternalImageProbe<'a, Message> {
    ExternalImageProbe { on_support: None }
}

/// An invisible widget that makes the renderer find out what it imports, so
/// a producer can choose its first buffer's format and modifier before it
/// renders anything.
///
/// It has no size, and rows and columns skip drawing a child with no area,
/// so mount it with [`over`](Self::over) (a layer above other content) or as
/// a layer of a stack: a bare probe in a row or column never reports. On its
/// first draw a GPU renderer opens the
/// external-image pipeline on its own device and reads what that device
/// imports; the software renderer records that it imports nothing. It asks
/// for frames only until that is known, then reports it once through
/// [`on_support`](Self::on_support): `None` when the renderer imports
/// nothing, else what it imports. [`crate::support`] holds the same answer
/// afterwards.
pub struct ExternalImageProbe<'a, Message> {
    on_support: Option<Box<dyn Fn(Option<Support>) -> Message + 'a>>,
}

impl<'a, Message> ExternalImageProbe<'a, Message> {
    /// The message that carries what the renderer imports, sent once.
    #[must_use]
    pub fn on_support(mut self, on_support: impl Fn(Option<Support>) -> Message + 'a) -> Self {
        self.on_support = Some(Box::new(on_support));
        self
    }

    /// The probe as a layer above `content`, where it is drawn wherever
    /// `content` is placed.
    #[must_use]
    pub fn over(self, content: impl Into<Element<'a, Message>>) -> Element<'a, Message>
    where
        Message: 'a,
    {
        iced::widget::Stack::with_children([content.into(), Element::from(self)]).into()
    }
}

struct ProbeState {
    instance: u64,
    reported: bool,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ExternalImageProbe<'_, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<ProbeState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(ProbeState {
            instance: next_instance(),
            reported: false,
        })
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(0.0), Length::Fixed(0.0))
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, _limits: &layout::Limits) {
        layout::Node::new(Size::ZERO).apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if !matches!(
            event,
            iced::Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            return;
        }
        let state = tree.state.downcast_mut::<ProbeState>();
        if state.reported {
            return;
        }
        match crate::support() {
            Some(support) => {
                state.reported = true;
                if let Some(on_support) = &self.on_support {
                    shell.publish(on_support(support));
                }
            }
            // The renderer learns it while presenting the probe's first draw.
            None => shell.request_redraw(),
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        if !renderer.supports_primitives() {
            crate::record_no_import();
            return;
        }
        if crate::support().is_some() {
            return;
        }
        renderer.draw_primitive(
            Rectangle::new(layout.position(), Size::new(1.0, 1.0)),
            Primitive {
                instance: tree.state.downcast_ref::<ProbeState>().instance,
                frame: None,
                source: None,
                fit: ContentFit::None,
                alignment: (alignment::Horizontal::Center, alignment::Vertical::Center),
                opacity: 0.0,
            },
        );
    }
}

impl<'a, Message: 'a> From<ExternalImageProbe<'a, Message>> for Element<'a, Message> {
    fn from(probe: ExternalImageProbe<'a, Message>) -> Self {
        Element::new(probe)
    }
}
