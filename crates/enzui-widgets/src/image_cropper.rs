//! A controlled crop selection over the shared image viewer.
//!
//! Arrow keys move the crop; Shift+arrows resize its lower-right corner. Tab
//! reaches the viewer's zoom/pan controls. Drag outside the selection to pan;
//! wheel zoom keeps the existing viewer's constraints and boundary behavior.
//! Four accessible numeric controls expose source-pixel position and size.
//! Feed draft changes back through [`CropDraft::accept`], and keep committed
//! values separately for caller-owned confirm/cancel. Source replacement must
//! advance [`CropIdentity`] generation. Widgets accept resolved RGBA storage;
//! loading, encoding and destination publication remain application-owned.

use crate::image_viewer::{EnzImageViewer, enz_image_viewer};
use crate::value_action::{ValueIntent, ValueRequest};
use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, PressFocus, pointer_press_position, press_focus_from_cursor};
use enzui_core::image_crop::{CropDraft, CropGesture, CropHandle, CropIdentity};
use enzui_core::image_space::ImageProjection;
use enzui_core::image_transform::PixelCrop;
use enzui_core::semantics::{NumericValue, Role, Semantics};
use enzui_core::zoom_pan::{ViewTransform, ViewportGeometry, ZoomRange, clamp_transform};
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::tree::Interactive as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::widget::canvas::{Fill, Frame, Path, Stroke, fill, path};
use iced::{
    Color, ContentFit, Element, Event, Length, Point, Rectangle, Size, Vector, keyboard, touch,
};
use std::rc::Rc;

/// Controlled crop feedback and boundaries for one source revision.
#[derive(Debug, Clone, PartialEq)]
pub enum CropEvent {
    /// Begin one pointer, touch, keyboard or accessibility edit.
    GestureStarted(CropIdentity),
    /// Candidate draft; accept only while its source revision and aspect still match.
    DraftChanged(CropDraft),
    /// Complete one edit; confirmation and history remain caller-owned.
    GestureEnded(CropIdentity),
    /// Cancel one grab; the payload is its initial draft, suitable for explicit restoration.
    GestureCancelled(CropDraft),
    /// Shared viewer feedback; apply only while this source revision is current.
    TransformChanged(CropIdentity, ViewTransform),
}

/// Visual selection mask; encoded output always uses the rectangular crop.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum CropPreview {
    /// Reveal the selected rectangle.
    #[default]
    Rectangle,
    /// Reveal its inscribed circle, without changing the export recipe.
    Circle,
}

/// Why a cropper cannot use the supplied renderer storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CropperSourceError {
    /// A path or encoded handle would require loading or decoding.
    Unresolved,
    /// Actual RGBA dimensions disagree with the draft's source dimensions.
    Dimensions,
    /// Pixel storage is incomplete or its required length overflows.
    Storage,
}
impl std::fmt::Display for CropperSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unresolved => "cropper requires resolved RGBA pixels",
            Self::Dimensions => "cropper source dimensions differ from the draft",
            Self::Storage => "cropper pixel storage is incomplete",
        })
    }
}
impl std::error::Error for CropperSourceError {}

/// Theme-derived colours for the crop selection and focused controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageCropperTokens {
    /// Scrim over excluded pixels.
    pub scrim: Color,
    /// Rectangle, circular preview and handle outlines.
    pub outline: Color,
    /// Handle and focused-property caption interior.
    pub handle: Color,
    /// Focused-property caption text.
    pub caption: Color,
}
impl ImageCropperTokens {
    /// Track the active image canvas, accent and surface colors.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            scrim: Color {
                a: 0.62,
                ..p.image_canvas
            },
            outline: p.accent,
            handle: p.surface,
            caption: p.text,
        }
    }
}

/// A resolved image with application-owned crop and view transform.
pub struct EnzImageCropper<'a, Message> {
    handle: iced::widget::image::Handle,
    draft: &'a CropDraft,
    transform: ViewTransform,
    fit: ContentFit,
    range: ZoomRange,
    preview: CropPreview,
    label: String,
    width: Length,
    height: Length,
    on_event: Option<Rc<dyn Fn(CropEvent) -> Message + 'a>>,
    tokens: Option<ImageCropperTokens>,
}

/// Create a fill-sized, read-only crop surface; add `on_event` for interaction.
///
/// # Errors
/// Refuses unresolved handles, source-dimension mismatches and incomplete RGBA storage.
pub fn enz_image_cropper<'a, Message>(
    handle: iced::widget::image::Handle,
    draft: &'a CropDraft,
    transform: ViewTransform,
) -> Result<EnzImageCropper<'a, Message>, CropperSourceError> {
    let iced::widget::image::Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = &handle
    else {
        return Err(CropperSourceError::Unresolved);
    };
    if Size::new(*width, *height) != draft.source_dimensions() {
        return Err(CropperSourceError::Dimensions);
    }
    let required = u64::from(*width)
        .checked_mul(u64::from(*height))
        .and_then(|n| n.checked_mul(4));
    if required != Some(pixels.len() as u64) {
        return Err(CropperSourceError::Storage);
    }
    Ok(EnzImageCropper {
        handle,
        draft,
        transform,
        fit: ContentFit::Contain,
        range: ZoomRange::default(),
        preview: CropPreview::Rectangle,
        label: enzui_core::localization::text("enzui-image-cropper-crop-selection"),
        width: Length::Fill,
        height: Length::Fill,
        on_event: None,
        tokens: None,
    })
}

impl<'a, Message: Clone + 'a> EnzImageCropper<'a, Message> {
    /// Publish controlled drafts, gesture boundaries and shared viewer transforms.
    #[must_use]
    pub fn on_event(mut self, f: impl Fn(CropEvent) -> Message + 'a) -> Self {
        self.on_event = Some(Rc::new(f));
        self
    }
    /// Set the shared viewer's fitting policy; defaults to Contain.
    #[must_use]
    pub fn fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }
    /// Set the existing viewer's validated zoom range.
    #[must_use]
    pub fn zoom_range(mut self, range: ZoomRange) -> Self {
        self.range = range;
        self
    }
    /// Mask only the preview; the exported selection stays rectangular.
    #[must_use]
    pub fn preview(mut self, preview: CropPreview) -> Self {
        self.preview = preview;
        self
    }
    /// Accessible name of the crop selection.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
    /// Set width, defaulting to Fill.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    /// Set height, defaulting to Fill.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
    /// Pin selection paint instead of following the runtime palette.
    #[must_use]
    pub fn tokens(mut self, tokens: ImageCropperTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    fn viewer(&self) -> EnzImageViewer<'a, Message> {
        let identity = self.draft.identity().clone();
        let mut viewer = enz_image_viewer(self.handle.clone(), self.transform)
            .fit(self.fit)
            .zoom_range(self.range)
            .width(self.width)
            .height(self.height)
            .id(identity.owner().child("crop-viewer").id("image"))
            .alt_text(enzui_core::localization::text(
                "enzui-image-cropper-image-zoom-and-pan",
            ));
        if let Some(callback) = self.on_event.clone() {
            viewer = viewer
                .on_transform(move |t| callback(CropEvent::TransformChanged(identity.clone(), t)));
        }
        viewer
    }
    fn projection(&self, bounds: Rectangle) -> Option<ImageProjection> {
        let source = self.draft.source_dimensions();
        let geometry = ViewportGeometry::new(
            Size::new(source.width as f32, source.height as f32),
            bounds.size(),
            self.fit,
        );
        ImageProjection::from_geometry(
            bounds.position(),
            geometry,
            clamp_transform(self.transform, geometry, self.range),
        )
    }
    fn display_rect(&self, projection: ImageProjection) -> Rectangle {
        let rect = self.draft.image_rect();
        let near = projection.image_to_display(rect.position());
        let far =
            projection.image_to_display(Point::new(rect.x + rect.width, rect.y + rect.height));
        Rectangle::new(near, Size::new(far.x - near.x, far.y - near.y))
    }
    fn hit(&self, projection: ImageProjection, point: Point) -> Option<CropHandle> {
        let rect = self.display_rect(projection);
        for (handle, corner) in corners(rect) {
            if (point.x - corner.x).abs() <= 10.0 && (point.y - corner.y).abs() <= 10.0 {
                return Some(handle);
            }
        }
        rect.contains(point).then_some(CropHandle::Move)
    }
    fn publish(&self, state: &mut State, next: CropDraft, shell: &mut Shell<'_, Message>) {
        if next != *self.draft
            && let Some(callback) = &self.on_event
        {
            state.published = Some(next.pixels());
            shell.publish(callback(CropEvent::DraftChanged(next)));
            shell.request_redraw();
        }
    }
    fn discrete(&self, state: &mut State, next: CropDraft, shell: &mut Shell<'_, Message>) {
        if let Some(callback) = &self.on_event
            && next != *self.draft
        {
            shell.publish(callback(CropEvent::GestureStarted(
                self.draft.identity().clone(),
            )));
            self.publish(state, next, shell);
            shell.publish(callback(CropEvent::GestureEnded(
                self.draft.identity().clone(),
            )));
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Input {
    Mouse,
    Touch(touch::Finger),
}
#[derive(Debug)]
struct Grab {
    gesture: CropGesture,
    initial: CropDraft,
    projection: ImageProjection,
    input: Input,
    last: Point,
}
#[derive(Debug)]
struct State {
    seen: CropDraft,
    transform: ViewTransform,
    published: Option<PixelCrop>,
    grab: Option<Grab>,
    pan_finger: Option<touch::Finger>,
    revoke_viewer: bool,
    focus: FocusState,
    fields_focus: [FocusState; 4],
    window_key: u64,
    pending: Vec<(Field, ValueIntent)>,
}
impl tree::Interactive for State {
    fn cancel_interaction(&mut self) {
        self.grab = None;
        self.pending.clear();
        self.pan_finger = None;
        self.revoke_viewer = true;
    }
}
impl State {
    fn new(draft: &CropDraft, transform: ViewTransform) -> Self {
        Self {
            seen: draft.clone(),
            transform,
            published: None,
            grab: None,
            pan_finger: None,
            revoke_viewer: false,
            focus: FocusState::new(),
            fields_focus: [FocusState::new(); 4],
            window_key: enzui_core::focus::KEY_UNKEYED,
            pending: vec![],
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    X,
    Y,
    Width,
    Height,
}
impl Field {
    fn id_role(self) -> &'static str {
        match self {
            Self::X => "Crop x",
            Self::Y => "Crop y",
            Self::Width => "Crop width",
            Self::Height => "Crop height",
        }
    }

    fn label(self) -> String {
        match self {
            Self::X => enzui_core::localization::text("enzui-image-cropper-crop-x"),
            Self::Y => enzui_core::localization::text("enzui-image-cropper-crop-y"),
            Self::Width => enzui_core::localization::text("enzui-image-cropper-crop-width"),
            Self::Height => enzui_core::localization::text("enzui-image-cropper-crop-height"),
        }
    }
    fn numeric(self, draft: &CropDraft) -> NumericValue {
        let (x, y, w, h) = draft.pixels().bounds();
        let source = draft.source_dimensions();
        let maximum = draft
            .resize_to(
                CropHandle::SouthEast,
                Point::new(source.width as f32, source.height as f32),
            )
            .unwrap_or_else(|_| draft.clone())
            .pixels()
            .dimensions();
        let units = draft.aspect_units().unwrap_or(Size::new(1, 1));
        let (value, min, max, step) = match self {
            Self::X => (x, 0, source.width - w, 1),
            Self::Y => (y, 0, source.height - h, 1),
            Self::Width => (w, units.width, maximum.width, units.width),
            Self::Height => (h, units.height, maximum.height, units.height),
        };
        NumericValue {
            value: value.into(),
            min: min.into(),
            max: max.into(),
            step: Some(step.into()),
        }
    }
    fn change(self, draft: &CropDraft, intent: ValueIntent) -> Option<CropDraft> {
        let range = self.numeric(draft);
        let value = match intent {
            ValueIntent::Increment => range.value + range.step?,
            ValueIntent::Decrement => range.value - range.step?,
            ValueIntent::Set(s) => s.parse::<f64>().ok()?,
        };
        if !value.is_finite() {
            return None;
        }
        let value = value.clamp(range.min, range.max) as f32;
        let (x, y, w, h) = draft.pixels().bounds();
        match self {
            Self::X => draft.move_by(Vector::new(value - x as f32, 0.0)).ok(),
            Self::Y => draft.move_by(Vector::new(0.0, value - y as f32)).ok(),
            Self::Width | Self::Height => {
                let (nw, nh) = if let Some(units) = draft.aspect_units() {
                    let unit = if matches!(self, Self::Width) {
                        units.width
                    } else {
                        units.height
                    } as f32;
                    let count = (value / unit).round();
                    (count * units.width as f32, count * units.height as f32)
                } else if matches!(self, Self::Width) {
                    (value, h as f32)
                } else {
                    (w as f32, value)
                };
                draft
                    .resize_to(
                        CropHandle::SouthEast,
                        Point::new(x as f32 + nw, y as f32 + nh),
                    )
                    .ok()
            }
        }
    }
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for EnzImageCropper<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::interactive(State::new(self.draft, self.transform))
    }
    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        let replaced = state.seen.identity() != self.draft.identity()
            || state.seen.source_dimensions() != self.draft.source_dimensions();
        if replaced {
            *state = State::new(self.draft, self.transform);
            tree.children.clear();
        } else {
            if state.seen.aspect() != self.draft.aspect()
                || state.transform != self.transform
                || state.seen.pixels() != self.draft.pixels()
                    && state.published != Some(self.draft.pixels())
                || self.on_event.is_none()
            {
                state.grab = None;
                state.pending.clear();
            }
            state.seen = self.draft.clone();
            state.transform = self.transform;
            state.published = None;
            if self.on_event.is_none() {
                state.cancel_interaction();
                state.focus.unfocus();
                for focus in &mut state.fields_focus {
                    focus.unfocus();
                }
            }
        }
        tree.diff_children(&mut [Element::from(self.viewer())]);
    }
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let child = self
            .viewer()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child]).apply(tree);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let id = self.draft.identity().owner().id("crop");
        let state = tree.state.downcast_mut::<State>();
        if self.on_event.is_some() {
            operation.focusable(Some(&id), layout.bounds(), &mut state.focus);
        }
        let (x, y, w, h) = self.draft.pixels().bounds();
        let mut semantics = Semantics::new(Role::Group)
            .label(&self.label)
            .value(enzui_core::localized!("enzui-image-cropper-x-y-width-height-source-pixels", "x" => x, "y" => y, "w" => w, "h" => h))
            .description(enzui_core::localization::text("enzui-image-cropper-arrow-keys-move-shift-arrows-resize-tab-reaches-image-zoom-and-pan"))
            .focused(state.focus.is_focused());
        semantics.supports_focus = self.on_event.is_some();
        let mut node = SemanticsNode::group(semantics);
        operation.custom(Some(&id), layout.bounds(), &mut node);
        operation.container(Some(&id), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (index, field) in [Field::X, Field::Y, Field::Width, Field::Height]
                .into_iter()
                .enumerate()
            {
                let id = self.draft.identity().owner().id(field.id_role());
                let state = tree.state.downcast_mut::<State>();
                if self.on_event.is_some() {
                    operation.focusable(Some(&id), layout.bounds(), &mut state.fields_focus[index]);
                }
                let mut node = SemanticsNode::new(
                    Semantics::new(Role::SpinButton)
                        .label(field.label())
                        .numeric(field.numeric(self.draft))
                        .adjustable(self.on_event.is_some())
                        .editable(self.on_event.is_some())
                        .disabled(self.on_event.is_none())
                        .focused(state.fields_focus[index].is_focused()),
                );
                operation.custom(Some(&id), layout.bounds(), &mut node);
                if self.on_event.is_some() {
                    let mut request = ValueRequest::new(id.clone());
                    operation.custom(Some(&id), layout.bounds(), &mut request);
                    if let Some(intent) = request.take() {
                        let pending = &mut tree.state.downcast_mut::<State>().pending;
                        if let Some(existing) = pending.iter_mut().find(|(key, _)| *key == field) {
                            existing.1 = intent;
                        } else {
                            pending.push((field, intent));
                        }
                    }
                }
            }
            let child_layout = layout.child(&tree.children, 0);
            self.viewer().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
        });
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<State>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);
        let Some(callback) = self.on_event.clone() else {
            return;
        };
        let Some(projection) = self.projection(layout.bounds()) else {
            tree.state.downcast_mut::<State>().cancel_interaction();
            return;
        };
        let state = tree.state.downcast_mut::<State>();
        if state.revoke_viewer {
            state.revoke_viewer = false;
            let child_layout = layout.child(&tree.children, 0);
            self.viewer().update(
                &mut tree.children[0],
                &Event::Mouse(mouse::Event::CursorLeft),
                child_layout,
                mouse::Cursor::Unavailable,
                renderer,
                shell,
                viewport,
            );
        }
        if state
            .grab
            .as_ref()
            .is_some_and(|grab| grab.projection != projection)
            && let Some(grab) = state.grab.take()
        {
            shell.publish(callback(CropEvent::GestureCancelled(grab.initial)));
        }
        let mut candidate = self.draft.clone();
        for (field, intent) in std::mem::take(&mut state.pending) {
            if let Some(next) = field.change(&candidate, intent) {
                candidate = next;
            }
        }
        self.discrete(state, candidate, shell);
        let press = pointer_press_position(event, cursor);
        if press.is_some() {
            for focus in &mut state.fields_focus {
                focus.unfocus();
            }
            match press_focus_from_cursor(event, cursor, self.display_rect(projection)) {
                Some(PressFocus::Take) => state.focus.focus(),
                Some(PressFocus::Drop) => state.focus.unfocus(),
                Some(PressFocus::Leave) | None => {}
            }
        }
        let cancelled = matches!(
            event,
            Event::Window(iced::window::Event::Unfocused)
                | Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                })
        ) || matches!(event, Event::Mouse(mouse::Event::CursorLeft))
            && state
                .grab
                .as_ref()
                .is_some_and(|grab| grab.input == Input::Mouse);
        if matches!(event, Event::Window(iced::window::Event::Unfocused)) {
            state.focus.unfocus();
            for focus in &mut state.fields_focus {
                focus.unfocus();
            }
            state.pan_finger = None;
        }
        if cancelled && let Some(grab) = state.grab.take() {
            shell.publish(callback(CropEvent::GestureCancelled(grab.initial)));
            shell.capture_event();
            shell.request_redraw();
            return;
        }
        if let Some(grab) = &state.grab {
            let point = match (grab.input, event) {
                (Input::Mouse, Event::Mouse(mouse::Event::CursorMoved { position })) => {
                    Some((*position, false))
                }
                (Input::Mouse, Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))) => {
                    Some((cursor.position().unwrap_or(grab.last), true))
                }
                (
                    Input::Touch(expected),
                    Event::Touch(touch::Event::FingerMoved { id, position }),
                ) if expected == *id => Some((*position, false)),
                (
                    Input::Touch(expected),
                    Event::Touch(touch::Event::FingerLifted { id, position }),
                ) if expected == *id => Some((*position, true)),
                _ => None,
            };
            if let Some((point, end)) = point {
                if let Ok(next) = grab
                    .gesture
                    .drag(self.draft, grab.projection.display_to_image(point))
                {
                    self.publish(state, next, shell);
                }
                if end {
                    state.grab = None;
                    shell.publish(callback(CropEvent::GestureEnded(
                        self.draft.identity().clone(),
                    )));
                } else if let Some(grab) = &mut state.grab {
                    grab.last = point;
                }
                shell.capture_event();
                shell.request_redraw();
                return;
            }
            if matches!(event, Event::Touch(touch::Event::FingerLost { id, .. }) if grab.input == Input::Touch(*id))
            {
                let grab = state.grab.take().expect("a gesture is in progress");
                shell.publish(callback(CropEvent::GestureCancelled(grab.initial)));
                shell.capture_event();
                return;
            }
            if press.is_some() {
                shell.capture_event();
                return;
            }
        } else {
            let input = match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    Some(Input::Mouse)
                }
                Event::Touch(touch::Event::FingerPressed { id, .. }) => Some(Input::Touch(*id)),
                _ => None,
            };
            if state.pan_finger.is_none()
                && let (Some(input), Some(Some(point))) = (input, press)
                && layout.bounds().contains(point)
                && viewport.contains(point)
                && let Some(handle) = self.hit(projection, point)
                && let Ok(gesture) = self.draft.begin(handle, projection.display_to_image(point))
            {
                state.grab = Some(Grab {
                    gesture,
                    initial: self.draft.clone(),
                    projection,
                    input,
                    last: point,
                });
                state.focus.focus();
                for focus in &mut state.fields_focus {
                    focus.unfocus();
                }
                let child_layout = layout.child(&tree.children, 0);
                let mut release = iced::advanced::widget::operation::map(
                    enzui_core::focus::release_all_focus(),
                    |_| (),
                );
                self.viewer().operate(
                    &mut tree.children[0],
                    child_layout,
                    viewport,
                    renderer,
                    &mut release,
                );
                shell.publish(callback(CropEvent::GestureStarted(
                    self.draft.identity().clone(),
                )));
                shell.capture_event();
                return;
            }
            if (state.focus.is_focused() || state.fields_focus.iter().any(FocusState::is_focused))
                && let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
                && !modifiers.control()
                && !modifiers.alt()
                && !modifiers.logo()
            {
                let delta = match key {
                    keyboard::Key::Named(keyboard::key::Named::ArrowLeft) => {
                        Some(Vector::new(-1.0, 0.0))
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowRight) => {
                        Some(Vector::new(1.0, 0.0))
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowUp) => {
                        Some(Vector::new(0.0, -1.0))
                    }
                    keyboard::Key::Named(keyboard::key::Named::ArrowDown) => {
                        Some(Vector::new(0.0, 1.0))
                    }
                    _ => None,
                };
                if let Some(delta) = delta {
                    let field = state
                        .fields_focus
                        .iter()
                        .position(FocusState::is_focused)
                        .map(|index| [Field::X, Field::Y, Field::Width, Field::Height][index]);
                    let next = if let Some(field) = field {
                        field.change(
                            self.draft,
                            if delta.x > 0.0 || delta.y < 0.0 {
                                ValueIntent::Increment
                            } else {
                                ValueIntent::Decrement
                            },
                        )
                    } else if modifiers.shift() {
                        let field = if delta.x != 0.0 {
                            Field::Width
                        } else {
                            Field::Height
                        };
                        field.change(
                            self.draft,
                            if delta.x + delta.y > 0.0 {
                                ValueIntent::Increment
                            } else {
                                ValueIntent::Decrement
                            },
                        )
                    } else {
                        self.draft.move_by(delta).ok()
                    };
                    if let Some(next) = next {
                        self.discrete(state, next, shell);
                    }
                    shell.capture_event();
                    return;
                }
            }
        }
        if let Event::Touch(touch_event) = event {
            let adapted = match touch_event {
                touch::Event::FingerPressed { id, position }
                    if state.pan_finger.is_none()
                        && layout.bounds().contains(*position)
                        && viewport.contains(*position) =>
                {
                    state.pan_finger = Some(*id);
                    Some((
                        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                        mouse::Cursor::Available(*position),
                    ))
                }
                touch::Event::FingerMoved { id, position } if state.pan_finger == Some(*id) => {
                    Some((
                        Event::Mouse(mouse::Event::CursorMoved {
                            position: *position,
                        }),
                        mouse::Cursor::Available(*position),
                    ))
                }
                touch::Event::FingerLifted { id, position } if state.pan_finger == Some(*id) => {
                    state.pan_finger = None;
                    Some((
                        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                        mouse::Cursor::Available(*position),
                    ))
                }
                touch::Event::FingerLost { id, .. } if state.pan_finger == Some(*id) => {
                    state.pan_finger = None;
                    Some((
                        Event::Mouse(mouse::Event::CursorLeft),
                        mouse::Cursor::Unavailable,
                    ))
                }
                _ => None,
            };
            if let Some((event, cursor)) = adapted {
                let child_layout = layout.child(&tree.children, 0);
                self.viewer().update(
                    &mut tree.children[0],
                    &event,
                    child_layout,
                    cursor,
                    renderer,
                    shell,
                    viewport,
                );
            }
            return;
        }
        let child_layout = layout.child(&tree.children, 0);
        self.viewer().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if self.on_event.is_some()
            && let Some(point) = cursor.position_over(layout.bounds())
            && viewport.contains(point)
            && let Some(projection) = self.projection(layout.bounds())
            && let Some(handle) = self.hit(projection, point)
        {
            return match handle {
                CropHandle::Move => mouse::Interaction::Grab,
                CropHandle::NorthWest | CropHandle::SouthEast => {
                    mouse::Interaction::ResizingDiagonallyDown
                }
                _ => mouse::Interaction::ResizingDiagonallyUp,
            };
        }
        self.viewer().mouse_interaction(
            &tree.children[0],
            layout.child(&tree.children, 0),
            cursor,
            viewport,
            renderer,
        )
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        use iced::advanced::graphics::geometry::Renderer as _;
        self.viewer().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.child(&tree.children, 0),
            cursor,
            viewport,
        );
        let bounds = layout.bounds();
        let Some(projection) = self.projection(bounds) else {
            return;
        };
        let Some(cull) = bounds.intersection(viewport) else {
            return;
        };
        let tokens = self.tokens.unwrap_or_else(|| {
            crate::theme_lookup::resolve(theme, ImageCropperTokens::from_palette)
        });
        let rect = self.display_rect(projection);
        let local = rect - Vector::new(bounds.x, bounds.y);
        let mut frame = Frame::new(renderer, bounds.size());
        let mut mask = path::Builder::new();
        mask.rectangle(Point::ORIGIN, bounds.size());
        match self.preview {
            CropPreview::Rectangle => mask.rectangle(local.position(), local.size()),
            CropPreview::Circle => mask.circle(local.center(), local.width.min(local.height) / 2.0),
        }
        frame.fill(
            &mask.build(),
            Fill {
                style: tokens.scrim.into(),
                rule: fill::Rule::EvenOdd,
            },
        );
        frame.stroke(
            &Path::rectangle(local.position(), local.size()),
            Stroke::default().with_color(tokens.outline).with_width(
                if enzui_core::focus::focus_ring_visible_for(
                    tree.state.downcast_ref::<State>().focus.is_focused()
                        || tree
                            .state
                            .downcast_ref::<State>()
                            .fields_focus
                            .iter()
                            .any(FocusState::is_focused),
                    tree.state.downcast_ref::<State>().window_key,
                ) {
                    3.0
                } else {
                    2.0
                },
            ),
        );
        if self.preview == CropPreview::Circle {
            frame.stroke(
                &Path::circle(local.center(), local.width.min(local.height) / 2.0),
                Stroke::default().with_color(tokens.outline).with_width(2.0),
            );
        }
        for (_, corner) in corners(local) {
            let path = Path::rectangle(corner - Vector::new(4.0, 4.0), Size::new(8.0, 8.0));
            frame.fill(&path, tokens.handle);
            frame.stroke(
                &path,
                Stroke::default().with_color(tokens.outline).with_width(2.0),
            );
        }
        if let Some(index) = tree
            .state
            .downcast_ref::<State>()
            .fields_focus
            .iter()
            .position(FocusState::is_focused)
        {
            let field = [Field::X, Field::Y, Field::Width, Field::Height][index];
            let size = crate::theme_lookup::typography(theme).footnote_size;
            let caption = Rectangle::new(
                Point::new(8.0, (bounds.height - size - 18.0).max(0.0)),
                Size::new((bounds.width - 16.0).clamp(0.0, 200.0), size + 10.0),
            );
            frame.fill_rectangle(caption.position(), caption.size(), tokens.handle);
            frame.fill_text(iced::widget::canvas::Text {
                content: enzui_core::localized!("enzui-image-cropper-px", "field" => field.label(), "pixels" => field.numeric(self.draft).value),
                position: caption.position() + Vector::new(6.0, 5.0),
                color: tokens.caption,
                size: size.into(),
                font: crate::chart_geometry::chart_label_font(theme),
                ..Default::default()
            });
        }
        let geometry = frame.into_geometry();
        renderer.with_layer(cull, |renderer| {
            renderer.with_layer_rounded(
                bounds,
                enzui_theme::text_defaults::shape_radius(|s| s.media).into(),
                |renderer| {
                    renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
                        renderer.draw_geometry(geometry);
                    });
                },
            );
        });
    }
}

fn corners(rect: Rectangle) -> [(CropHandle, Point); 4] {
    [
        (CropHandle::NorthWest, rect.position()),
        (
            CropHandle::NorthEast,
            Point::new(rect.x + rect.width, rect.y),
        ),
        (
            CropHandle::SouthEast,
            Point::new(rect.x + rect.width, rect.y + rect.height),
        ),
        (
            CropHandle::SouthWest,
            Point::new(rect.x, rect.y + rect.height),
        ),
    ]
}
impl<'a, Message: Clone + 'a> From<EnzImageCropper<'a, Message>> for Element<'a, Message> {
    fn from(cropper: EnzImageCropper<'a, Message>) -> Self {
        Element::new(cropper)
    }
}

impl<'a, Message: Clone + 'a> EnzImageCropper<'a, Message> {
    /// Finish the builder as an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}
