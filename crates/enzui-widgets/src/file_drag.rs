//! Files dragged from a row, thumbnail or document into another application.
//!
//! Wrap the source content in [`enz_file_drag_source`], passing the currently
//! selected paths. `app_setup::run` supplies the native Wayland transport.
//! Sources default to Copy: originals stay owned by the application. The
//! destination can finish asynchronously after copying or saving attachments.
//! Empty/invalid selections and unavailable transports retain ordinary clicks.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub use enzui_clipboard::{MimeType, Payload};
pub use enzui_clipboard_runtime::native_drag::{DragActions, DragIcon, DragOutcome};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, touch};

/// Wraps a file row or media item as a native Copy source.
pub fn enz_file_drag_source<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    paths: impl IntoIterator<Item = PathBuf>,
) -> EnzFileDragSource<'a, Message> {
    let mut paths: Vec<_> = paths.into_iter().take(129).collect();
    if paths.len() > 128 {
        paths.clear();
    }
    let key = source_key(&paths);
    EnzFileDragSource {
        key,
        content: content.into(),
        paths,
        captured_press: false,
        icon: None,
        on_finished: None,
        actions: DragActions::COPY,
        prepare: None,
        temporary: false,
        data: None,
    }
}

/// Wrap arbitrary ordinary MIME data as a native Copy source.
///
/// Supply a stable resource/session key. Payload clones share their buffers;
/// build large representations off the UI thread. Sensitive and deferred text
/// are refused. Use file sources for automatic file-portal exports.
pub fn enz_data_drag_source<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    key: impl Hash,
    payload: Payload,
) -> EnzFileDragSource<'a, Message> {
    let mut source = enz_file_drag_source(content, []).key(key);
    source.data = Some(payload);
    source
}

/// Files prepared off the UI thread, with an owner retained until the destination
/// finishes. A temporary directory can own named attachment exports here.
pub struct PreparedFiles {
    pub(crate) paths: Vec<PathBuf>,
    _owner: Box<dyn Send>,
}

impl PreparedFiles {
    /// Retain `owner` for the complete native source lifetime.
    #[must_use]
    pub fn owned(paths: Vec<PathBuf>, owner: impl Send + 'static) -> Self {
        Self {
            paths,
            _owner: Box::new(owner),
        }
    }
}

pub(crate) type Preparation = Arc<
    dyn Fn(enzui_core::cancellation::CancelToken) -> Result<PreparedFiles, String> + Send + Sync,
>;

/// Prepare temporary files only after a real drag gesture, on a worker.
///
/// Use this for in-memory or downloaded attachments. Check cancellation between
/// bounded I/O operations; return at most 128 absolute paths and an owner that
/// removes temporary exports when dropped. Preparation never runs while building
/// or hovering the view. These sources always offer Copy.
pub fn enz_file_drag_source_with<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    prepare: impl Fn(enzui_core::cancellation::CancelToken) -> Result<PreparedFiles, String>
    + Send
    + Sync
    + 'static,
) -> EnzFileDragSource<'a, Message> {
    let mut source = enz_file_drag_source(content, []);
    source.prepare = Some(Arc::new(prepare));
    source
}

/// A pointer/touch source using the desktop's drag threshold.
pub struct EnzFileDragSource<'a, Message> {
    key: u64,
    content: Element<'a, Message>,
    paths: Vec<PathBuf>,
    data: Option<Payload>,
    prepare: Option<Preparation>,
    temporary: bool,
    captured_press: bool,
    actions: DragActions,
    icon: Option<DragIcon>,
    on_finished: Option<Box<dyn Fn(DragOutcome) -> Message + 'a>>,
}

fn source_key(value: impl Hash) -> u64 {
    let mut hash = std::hash::DefaultHasher::new();
    value.hash(&mut hash);
    hash.finish()
}

impl<'a, Message> EnzFileDragSource<'a, Message> {
    /// Identify this source across view updates. Paths provide the default key;
    /// prepared attachments should supply their stable asset/session identity.
    /// Replacing a source cancels its pending gesture and retained child press.
    #[must_use]
    pub fn key(mut self, key: impl Hash) -> Self {
        self.key = source_key(key);
        self
    }

    /// Add an ordinary representation, such as an app-specific row identifier.
    /// File sources retain their URI and portal representations; these reserved
    /// formats cannot be replaced. Keep metadata bounded and key it to its owner.
    #[must_use]
    pub fn with_data(mut self, mime: impl Into<MimeType>, bytes: impl Into<Arc<[u8]>>) -> Self {
        self.data = Some(self.data.take().unwrap_or_default().with(mime, bytes));
        self
    }

    /// Allow a clickable row to arm a drag after its child captures the press.
    /// Keep this disabled around selectable text or other drag gestures.
    #[must_use]
    pub fn allows_captured_press(mut self, enabled: bool) -> Self {
        self.captured_press = enabled;
        self
    }

    /// Advertise operations understood by the source. The default is Copy.
    /// Holding Control at drag start restricts a Copy-capable source to Copy.
    /// This widget never removes paths: file managers may perform filesystem
    /// moves at the destination; other source formats own their completion policy.
    #[must_use]
    pub fn actions(mut self, actions: DragActions) -> Self {
        self.actions = if self.prepare.is_some() || self.temporary {
            DragActions::COPY
        } else {
            actions
        };
        self
    }

    /// Mark session-owned exports that the receiver must copy before acknowledging.
    /// Prepared file sources set this automatically. Temporary exports offer Copy.
    #[must_use]
    pub fn temporary_files(mut self, temporary: bool) -> Self {
        self.temporary = temporary;
        if temporary {
            self.actions = DragActions::COPY;
        }
        self
    }

    /// Set the bounded native preview painted underneath the pointer.
    #[must_use]
    pub fn icon(mut self, icon: DragIcon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Observe destination completion or cancellation. Copy never removes files.
    #[must_use]
    pub fn on_finished(mut self, callback: impl Fn(DragOutcome) -> Message + 'a) -> Self {
        self.on_finished = Some(Box::new(callback));
        self
    }
}

#[derive(Default)]
struct State {
    key: u64,
    modifiers: iced::keyboard::Modifiers,
    press: Option<(Point, Option<touch::Finger>)>,
    outcome: Option<Arc<Mutex<Option<DragOutcome>>>>,
    preparing: Option<enzui_core::cancellation::CancelToken>,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(cancel) = &self.preparing {
            cancel.cancel(enzui_core::cancellation::CancelCause::Cancelled);
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzFileDragSource<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State {
            key: self.key,
            modifiers: iced::keyboard::Modifiers::default(),
            press: None,
            outcome: None,
            preparing: None,
        })
    }
    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.key != self.key {
            *state = State {
                key: self.key,
                modifiers: state.modifiers,
                press: None,
                outcome: None,
                preparing: None,
            };
            tree.children.clear();
        }
        tree.diff_children(std::slice::from_mut(&mut self.content));
        if self.paths.is_empty()
            && self.prepare.is_none()
            && self.data.as_ref().is_none_or(Payload::is_empty)
        {
            tree.state.downcast_mut::<State>().press = None;
        }
    }
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.state.downcast_ref::<State>().outcome.is_some()
            || crate::frame_routing::interest_opaque(tree)
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
        let state = tree.state.downcast_mut::<State>();
        match event {
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Window(iced::window::Event::Unfocused) => {
                state.modifiers = iced::keyboard::Modifiers::default();
            }
            _ => {}
        }
        let finished = state.outcome.as_ref().and_then(|outcome| {
            outcome
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        });
        if let Some(outcome) = finished {
            state.outcome = None;
            state.preparing = None;
            if let Some(callback) = &self.on_finished {
                shell.publish(callback(outcome));
            }
            shell.request_redraw();
        }
        let position = match event {
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                // Scrollable parents hide off-viewport cursors while the held grab still owns motion.
                cursor
                    .position()
                    .or_else(|| (!viewport.contains(*position)).then_some(*position))
                    .map(|position| (position, None))
            }
            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                Some((*position, Some(*id)))
            }
            _ => None,
        };
        if let (Some((position, finger)), Some((origin, owner))) = (position, state.press)
            && finger == owner
            && origin.distance(position) >= enzui_core::desktop_settings::input().drag_threshold
        {
            state.press = None;
            let outcome = Arc::new(Mutex::new(None));
            let cancel = enzui_core::cancellation::CancelToken::new();
            if crate::text_drag::begin_files(
                &self.paths,
                self.prepare.clone(),
                self.data.clone(),
                self.temporary,
                cancel.share(),
                self.icon.clone(),
                if state.modifiers.control()
                    && self.actions.allows(crate::file_drop::DragAction::Copy)
                {
                    DragActions::COPY
                } else {
                    self.actions
                },
                Arc::clone(&outcome),
            ) {
                state.outcome = Some(outcome);
                state.preparing = Some(cancel);
                let cancel = owner.map_or(
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                    |id| Event::Touch(touch::Event::FingerLost { id, position }),
                );
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    &cancel,
                    layout,
                    mouse::Cursor::Unavailable,
                    renderer,
                    shell,
                    viewport,
                );
                crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
                shell.capture_event();
                shell.request_redraw();
                return;
            }
        }
        let state = tree.state.downcast_mut::<State>();
        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                | Event::Touch(touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. })
        ) && let Some(cancel) = &state.preparing
        {
            cancel.cancel(enzui_core::cancellation::CancelCause::Cancelled);
        }
        match event {
            Event::Mouse(mouse::Event::ButtonReleased(_) | mouse::Event::CursorLeft)
            | Event::Window(iced::window::Event::Unfocused) => state.press = None,
            Event::Touch(
                touch::Event::FingerLifted { id, .. } | touch::Event::FingerLost { id, .. },
            ) if state.press.is_some_and(|(_, owner)| owner == Some(*id)) => state.press = None,
            _ => {}
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
        if (self.paths.is_empty()
            && self.prepare.is_none()
            && self.data.as_ref().is_none_or(Payload::is_empty))
            || (!self.captured_press && shell.is_event_captured())
        {
            return;
        }
        let state = tree.state.downcast_mut::<State>();
        if state.outcome.is_some() {
            return;
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                state.press = cursor
                    .position_over(layout.bounds().intersection(viewport).unwrap_or_default())
                    .map(|position| (position, None));
            }
            Event::Touch(touch::Event::FingerPressed { id, position })
                if layout.bounds().contains(*position) && viewport.contains(*position) =>
            {
                state.press = Some((*position, Some(*id)));
            }
            _ => {}
        }
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<EnzFileDragSource<'a, Message>> for Element<'a, Message> {
    fn from(source: EnzFileDragSource<'a, Message>) -> Self {
        Element::new(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_a_file_source_cannot_activate_the_new_item_with_an_old_press() {
        for (changed_path, changed_key) in [(true, false), (false, true), (false, false)] {
            let source = |changed: bool| -> Element<'static, bool> {
                let path = if changed && changed_path {
                    "/tmp/second"
                } else {
                    "/tmp/first"
                };
                let source = enz_file_drag_source(
                    iced::widget::button("File")
                        .on_press(changed)
                        .width(200)
                        .height(100),
                    [PathBuf::from(path)],
                )
                .allows_captured_press(true);
                if changed_key {
                    source.key(changed).into()
                } else {
                    source.into()
                }
            };
            let mut ui = iced_test::Simulator::new(source(false));
            ui.point_at(Point::new(40.0, 40.0));
            let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
            let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
            ui.simulate([press.clone()]);
            let mut ui = ui.rebuild(source(true));
            ui.simulate([release.clone(), press, release]);
            let expected = if changed_path || changed_key {
                vec![true]
            } else {
                vec![true, true]
            };
            assert_eq!(
                ui.into_messages().collect::<Vec<_>>(),
                expected,
                "retained source identity: path_changed={changed_path}, explicit_key_changed={changed_key}"
            );
        }
    }
}
