//! Annotation drawing and keyed selection over an image.
//!
//! Match the underlying viewer's intrinsic image size, fit, and transform. The
//! overlay derives a local projection from its own layout bounds. Committed
//! annotations remain application-owned image-space data in paint order.
//!
//! Presses must start inside the overlay; moves and releases may occur outside.
//! Gestures capture their projection and stroke style at press time. Release uses
//! the last moved point. Escape, pointer loss, or window focus loss cancels a
//! gesture while the tool is armed and the current projection is valid.
//!
//! [`AnnotationTool::None`] disables event handling while committed shapes and
//! accessibility nodes remain. The overlay adds no focus stop; it publishes one
//! named list box and one keyed option per annotation. Undo/redo callbacks require
//! an armed tool and have no separate focus gate. The application owns history.
//!
//! [`Annotation`]: enzui_core::annotation::Annotation
//! [`Size`]: iced::Size
//! [`ContentFit`]: iced::ContentFit
//! [`ViewTransform`]: enzui_core::zoom_pan::ViewTransform
//! [`ImageProjection`]: enzui_core::image_space::ImageProjection
//! [`AnnotationTool::None`]: enzui_core::annotation::capture::AnnotationTool::None

use enzui_core::annotation::capture::{
    AnnotationTool, CaptureOutcome, CaptureSession, PreviewShape,
};
use enzui_core::annotation::export::StrokePath;
use enzui_core::annotation::hit::{DEFAULT_SCREEN_SLOP, hit_test_id, image_slop};
use enzui_core::annotation::{Annotation, AnnotationId, AnnotationIds, ShapeGeometry, StrokeStyle};
use enzui_core::collect::SemanticsNode;
use enzui_core::identity::OwnerId;
use enzui_core::image_space::ImageProjection;
use enzui_core::semantics::{Role, Semantics, SetPosition};
use enzui_core::zoom_pan::ViewTransform;
use enzui_theme::Palette;
use iced::advanced::widget::operation::Operation;
use iced::advanced::widget::{Id, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::keyboard;
use iced::widget::canvas::{Frame, Path, Stroke, path::Builder};
use iced::{Color, ContentFit, Element, Event, Length, Point, Rectangle, Size};

/// Dash length of the selection outline, in display pixels.
pub const SELECTION_DASH: f32 = 4.0;

/// Selection-outline width in logical display pixels, independent of image zoom.
pub const SELECTION_WIDTH: f32 = 1.5;

/// How far outside a selected shape's ink the outline sits, in display pixels.
pub const SELECTION_INSET: f32 = 3.0;

/// Default accessible name of the annotation collection; override with [`EnzAnnotationOverlay::label`].
pub const DEFAULT_LABEL: &str = "Annotations";

/// Selection-outline chrome; committed shapes and previews use their document stroke styles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnnotationOverlayTokens {
    /// Outline drawn around the selected annotation.
    pub selection: Color,
    /// Dash length in logical display pixels.
    pub selection_dash: f32,
    /// Outline thickness in logical display pixels.
    pub selection_width: f32,
    /// Gap outside the stroke extent in logical display pixels.
    pub selection_inset: f32,
}

impl AnnotationOverlayTokens {
    /// Derive from a runtime palette.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            selection: p.accent,
            selection_dash: SELECTION_DASH,
            selection_width: SELECTION_WIDTH,
            selection_inset: SELECTION_INSET,
        }
    }

    /// Dark preset.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light preset.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replaces non-finite or negative selection geometry with defaults.
    /// Zero geometry and the selection color are preserved.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        Self {
            selection_dash: FiniteNonNeg::new_or(self.selection_dash, SELECTION_DASH).get(),
            selection_width: FiniteNonNeg::new_or(self.selection_width, SELECTION_WIDTH).get(),
            selection_inset: FiniteNonNeg::new_or(self.selection_inset, SELECTION_INSET).get(),
            ..*self
        }
    }
}

impl Default for AnnotationOverlayTokens {
    fn default() -> Self {
        Self::dark()
    }
}

/// Creates an annotation overlay borrowing the application’s committed image-space shapes.
pub fn enz_annotation_overlay<Message>(
    annotations: &[Annotation],
    image: Size,
) -> EnzAnnotationOverlay<'_, Message> {
    EnzAnnotationOverlay::new(annotations, image)
}

/// Draws borrowed annotations and publishes drawing or selection messages.
pub struct EnzAnnotationOverlay<'a, Message> {
    annotations: &'a [Annotation],
    image: Size,
    fit: ContentFit,
    transform: ViewTransform,
    tool: AnnotationTool,
    style: StrokeStyle,
    selected: Option<AnnotationId>,
    owner_key: String,
    label: String,
    screen_slop: f32,
    on_commit: Option<Box<dyn Fn(Box<Annotation>) -> Message + 'a>>,
    on_select: Option<Box<dyn Fn(Option<AnnotationId>) -> Message + 'a>>,
    on_preview: Option<Box<dyn Fn(Option<PreviewShape>) -> Message + 'a>>,
    on_undo: Option<Box<dyn Fn() -> Message + 'a>>,
    on_redo: Option<Box<dyn Fn() -> Message + 'a>>,
    width: Length,
    height: Length,
    tokens: Option<AnnotationOverlayTokens>,
}

impl<'a, Message> EnzAnnotationOverlay<'a, Message> {
    /// Creates a fill-sized overlay with Contain fit, identity transform, and no active tool.
    /// The default new-shape style is a magenta stroke two image units wide; the collection
    /// label is [`DEFAULT_LABEL`] and the default owner key is `enz-annotation-overlay`.
    pub fn new(annotations: &'a [Annotation], image: Size) -> Self {
        Self {
            annotations,
            image,
            fit: ContentFit::Contain,
            transform: ViewTransform::IDENTITY,
            tool: AnnotationTool::None,
            style: const {
                StrokeStyle::new(2.0, Color::from_rgb(0.95, 0.16, 0.86))
                    .expect("2.0 is a finite, non-negative width")
            },
            selected: None,
            owner_key: String::from("enz-annotation-overlay"),
            label: enzui_core::localization::text("enzui-annotation-label"),
            screen_slop: DEFAULT_SCREEN_SLOP,
            on_commit: None,
            on_select: None,
            on_preview: None,
            on_undo: None,
            on_redo: None,
            width: Length::Fill,
            height: Length::Fill,
            tokens: None,
        }
    }

    /// Sets image fit, defaulting to [`ContentFit::Contain`]; match the underlying viewer.
    #[must_use]
    pub fn fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }

    /// Sets the image zoom/pan transform, defaulting to identity; match the underlying viewer.
    #[must_use]
    pub fn transform(mut self, transform: ViewTransform) -> Self {
        self.transform = transform;
        self
    }

    /// Sets the active tool, defaulting to [`AnnotationTool::None`].
    /// A tool change cancels a retained gesture during tree diff without a preview callback.
    /// None disables event handling and pointer interaction, but keeps committed painting and semantics.
    #[must_use]
    pub fn tool(mut self, tool: AnnotationTool) -> Self {
        self.tool = tool;
        self
    }

    /// Sets the document-space stroke style for subsequent gestures.
    /// A live gesture retains the style captured at press time.
    #[must_use]
    pub fn style(mut self, style: StrokeStyle) -> Self {
        self.style = style;
        self
    }

    /// The annotation the application considers selected, drawn with a
    /// selection outline.
    #[must_use]
    pub fn selected(mut self, selected: Option<AnnotationId>) -> Self {
        self.selected = selected;
        self
    }

    /// Sets the document namespace for annotation and collection identities.
    /// Use distinct stable keys for independent documents; changing the key resets
    /// the capture session and reserves identities already in the supplied annotations.
    #[must_use]
    pub fn owner_key(mut self, key: impl Into<String>) -> Self {
        self.owner_key = key.into();
        self
    }

    /// Sets the list-box accessible name, defaulting to [`DEFAULT_LABEL`].
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Sets aiming slop in logical display pixels, defaulting to [`DEFAULT_SCREEN_SLOP`].
    /// Converted to image units for hit testing, in addition to half the stroke width.
    /// Negative or non-finite values fall back to the default; zero is preserved.
    #[must_use]
    pub fn screen_slop(mut self, slop: f32) -> Self {
        self.screen_slop = slop;
        self
    }

    /// Publishes one new image-space annotation on release of a drawing gesture.
    /// The application must add it to the committed list; the overlay does not retain it.
    #[must_use]
    pub fn on_commit(mut self, f: impl Fn(Box<Annotation>) -> Message + 'a) -> Self {
        self.on_commit = Some(Box::new(f));
        self
    }

    /// Publishes the topmost hit annotation ID, or None, on release of a Select click.
    /// Hit testing uses the press point and the current projection.
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(Option<AnnotationId>) -> Message + 'a) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Publishes the current preview on each cursor move while a gesture is captured.
    /// The value may remain unchanged or None. Release publishes None after commit or
    /// selection; explicit gesture cancellation also publishes None. Preview painting
    /// does not require this callback.
    #[must_use]
    pub fn on_preview(mut self, f: impl Fn(Option<PreviewShape>) -> Message + 'a) -> Self {
        self.on_preview = Some(Box::new(f));
        self
    }

    /// Publishes on Command/Control+Z while a tool is armed and the projection is valid.
    /// Cancels a live gesture and publishes its preview-clear message before undo.
    /// The application owns history; no overlay focus is required.
    #[must_use]
    pub fn on_undo(mut self, f: impl Fn() -> Message + 'a) -> Self {
        self.on_undo = Some(Box::new(f));
        self
    }

    /// Publishes on Command/Control+Shift+Z or Command/Control+Y under the same gate as [`Self::on_undo`].
    /// Cancels a live gesture before publishing redo; the application owns history.
    #[must_use]
    pub fn on_redo(mut self, f: impl Fn() -> Message + 'a) -> Self {
        self.on_redo = Some(Box::new(f));
        self
    }

    /// Sets width, defaulting to [`Length::Fill`].
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Height. Defaults to [`Length::Fill`].
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Pins selection chrome instead of tracking the runtime theme and sanitizes its geometry.
    #[must_use]
    pub fn tokens(mut self, tokens: AnnotationOverlayTokens) -> Self {
        self.tokens = Some(tokens.sanitized());
        self
    }

    fn container_id(&self) -> Id {
        OwnerId::of_kind("enz-annotation-overlay", &self.owner_key).id("list")
    }

    fn projection(&self, size: Size) -> Option<ImageProjection> {
        // Canvas geometry and pointer positions share widget-local coordinates.
        ImageProjection::new(
            Rectangle::new(Point::ORIGIN, size),
            self.image,
            self.fit,
            self.transform,
        )
    }
}

#[inline]
fn resolve_tokens(
    pinned: Option<AnnotationOverlayTokens>,
    theme: &iced::Theme,
) -> AnnotationOverlayTokens {
    pinned.unwrap_or_else(|| {
        crate::theme_lookup::resolve(theme, AnnotationOverlayTokens::from_palette)
    })
}

#[derive(Debug)]
struct OverlayState {
    session: CaptureSession,
    owner_key: String,
}

impl OverlayState {
    fn new(owner_key: &str, existing: &[Annotation]) -> Self {
        let mut session = CaptureSession::new(AnnotationIds::new(owner_key));
        // Adopted IDs must be reserved before this session can mint another annotation.
        session.reserve_existing(existing);
        Self {
            session,
            owner_key: owner_key.to_owned(),
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzAnnotationOverlay<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<OverlayState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(OverlayState::new(&self.owner_key, self.annotations))
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<OverlayState>();
        // Document identity changes require a fresh ID deriver and gesture state.
        if state.owner_key != self.owner_key {
            *state = OverlayState::new(&self.owner_key, self.annotations);
        }
        state.session.set_tool(self.tool);
        state.session.set_style(self.style);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::Node::new(limits.width(self.width).height(self.height).resolve(
            self.width,
            self.height,
            Size::ZERO,
        ))
        .apply(tree);
    }

    fn operate(
        &mut self,
        _tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let bounds = layout.bounds();
        let container_id = self.container_id();
        let count = self.annotations.len();

        let mut container = Semantics::new(Role::ListBox).label(self.label.clone());
        if let Some(selected) = self.selected.as_ref()
            && self.annotations.iter().any(|a| a.id() == selected)
        {
            container = container.active_descendant(selected.widget_id().clone());
        }
        let mut node = SemanticsNode::group(container);
        operation.custom(Some(&container_id), bounds, &mut node);
        operation.container(Some(&container_id), bounds, viewport);

        operation.traverse(&mut |operation| {
            for (i, annotation) in self.annotations.iter().enumerate() {
                let option = Semantics::new(Role::ListBoxOption)
                    .label(annotation.describe())
                    .selected(self.selected.as_ref() == Some(annotation.id()))
                    .set_position(SetPosition {
                        index: i + 1,
                        count,
                    });
                let mut node = SemanticsNode::new(option);
                operation.custom(Some(annotation.id().widget_id()), bounds, &mut node);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if self.tool == AnnotationTool::None {
            return;
        }

        let bounds = layout.bounds();
        let Some(projection) = self.projection(bounds.size()) else {
            return;
        };
        let state = tree.state.downcast_mut::<OverlayState>();

        let local = |p: Point| Point::new(p.x - bounds.x, p.y - bounds.y);

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_in(bounds) else {
                    return;
                };
                if state.session.press(position, projection) {
                    shell.request_redraw();
                    shell.capture_event();
                }
            }

            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if !state.session.is_capturing() {
                    return;
                }
                // An inside press retains ownership when movement leaves the overlay bounds.
                if state.session.pointer_moved(local(*position)) {
                    if let Some(f) = &self.on_preview {
                        shell.publish(f(state.session.preview()));
                    }
                    shell.request_redraw();
                    shell.capture_event();
                }
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if !state.session.is_capturing() {
                    return;
                }
                match state.session.release() {
                    CaptureOutcome::Committed(annotation) => {
                        if let Some(f) = &self.on_commit {
                            shell.publish(f(annotation));
                        }
                    }
                    CaptureOutcome::Clicked(at) => {
                        if self.tool == AnnotationTool::Select
                            && let Some(f) = &self.on_select
                        {
                            let slop = image_slop(self.screen_slop, projection);
                            let hit = hit_test_id(
                                self.annotations,
                                projection.display_to_image(at),
                                slop,
                            );
                            shell.publish(f(hit));
                        }
                    }
                    CaptureOutcome::Nothing => {}
                }
                if let Some(f) = &self.on_preview {
                    shell.publish(f(None));
                }
                shell.request_redraw();
                shell.capture_event();
            }

            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Character(c),
                modifiers,
                ..
            }) if modifiers.command() => {
                let lower = c.to_lowercase();
                let redo = lower == "y" || (lower == "z" && modifiers.shift());
                let undo = lower == "z" && !modifiers.shift();
                let handler = if redo {
                    self.on_redo.as_ref()
                } else if undo {
                    self.on_undo.as_ref()
                } else {
                    None
                };
                if let Some(f) = handler {
                    if state.session.cancel()
                        && let Some(p) = &self.on_preview
                    {
                        shell.publish(p(None));
                    }
                    shell.publish(f());
                    shell.request_redraw();
                    shell.capture_event();
                }
            }

            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(keyboard::key::Named::Escape),
                ..
            }) => {
                let handles_event = state.session.cancel();
                if handles_event {
                    if let Some(f) = &self.on_preview {
                        shell.publish(f(None));
                    }
                    shell.request_redraw();
                    shell.capture_event();
                }
            }

            Event::Mouse(mouse::Event::CursorLeft)
            | Event::Window(iced::window::Event::Unfocused) => {
                // Keep cancellation’s state mutation in the handler rather than a match guard.
                #[allow(
                    clippy::collapsible_match,
                    reason = "the condition mutates state, so it cannot be a match guard"
                )]
                if state.session.cancel() {
                    if let Some(f) = &self.on_preview {
                        shell.publish(f(None));
                    }
                    shell.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if self.tool == AnnotationTool::None || !cursor.is_over(layout.bounds()) {
            return mouse::Interaction::None;
        }
        match self.tool {
            AnnotationTool::Select => mouse::Interaction::Pointer,
            _ => mouse::Interaction::Crosshair,
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::graphics::geometry::Renderer as _;

        let bounds = layout.bounds();
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return;
        }
        let Some(projection) = self.projection(bounds.size()) else {
            return;
        };
        let tokens = resolve_tokens(self.tokens, theme);
        let state = tree.state.downcast_ref::<OverlayState>();

        let mut frame = Frame::new(renderer, bounds.size());

        for annotation in self.annotations {
            let shape = annotation.to_display(projection);
            let path = path_of(&shape.geometry, shape.width);
            if let Some(fill) = shape.fill {
                frame.fill(&path, fill);
            }
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(shape.color)
                    .with_width(shape.width),
            );

            if self.selected.as_ref() == Some(annotation.id()) {
                draw_selection(&mut frame, &shape.geometry, shape.width, tokens);
            }
        }

        if let Some(preview) = state.session.preview() {
            let width = preview.screen_width();
            let path = path_of(preview.geometry(), width);
            if let Some(fill) = preview.style().fill() {
                frame.fill(&path, fill);
            }
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(preview.style().color())
                    .with_width(width),
            );
        }

        let geometry = frame.into_geometry();
        renderer.with_translation(iced::Vector::new(bounds.x, bounds.y), |renderer| {
            renderer.draw_geometry(geometry);
        });
    }
}

impl<'a, Message> EnzAnnotationOverlay<'a, Message>
where
    Message: 'a,
{
    /// Converts the configured overlay into an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzAnnotationOverlay<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(overlay: EnzAnnotationOverlay<'a, Message>) -> Self {
        Element::new(overlay)
    }
}

fn path_of(geometry: &ShapeGeometry, width: f32) -> Path {
    // Use a true canvas ellipse; other shapes share exporter StrokePath geometry.
    if let ShapeGeometry::Ellipse { center, radii } = geometry {
        let mut b = Builder::new();
        b.ellipse(iced::widget::canvas::path::arc::Elliptical {
            center: *center,
            radii: *radii,
            rotation: iced::Radians(0.0),
            start_angle: iced::Radians(0.0),
            end_angle: iced::Radians(std::f32::consts::TAU),
        });
        return b.build();
    }
    let stroke_path = StrokePath::of(geometry, width);
    let mut b = Builder::new();
    for contour in &stroke_path.contours {
        let mut iter = contour.points.iter();
        if let Some(first) = iter.next() {
            b.move_to(*first);
            for p in iter {
                b.line_to(*p);
            }
            if contour.closed {
                b.close();
            }
        }
    }
    b.build()
}

fn draw_selection(
    frame: &mut Frame,
    geometry: &ShapeGeometry,
    stroke_width: f32,
    tokens: AnnotationOverlayTokens,
) {
    let b = geometry.bounds();
    let pad = stroke_width / 2.0 + tokens.selection_inset;
    let path = Path::rectangle(
        Point::new(b.x - pad, b.y - pad),
        Size::new(b.width + pad * 2.0, b.height + pad * 2.0),
    );
    let dash = [tokens.selection_dash, tokens.selection_dash];
    let mut stroke = Stroke::default()
        .with_color(tokens.selection)
        .with_width(tokens.selection_width);
    stroke.line_dash = iced::widget::canvas::LineDash {
        segments: &dash,
        offset: 0,
    };
    frame.stroke(&path, stroke);
}
