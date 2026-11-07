//! Dragging text: selected text out of fields and editors, and text dropped
//! into them, between widgets and to and from other applications.
//!
//! `app_setup::run` installs this on Wayland (through the native drag
//! runtime); applications do nothing. Press inside a selection and drag: the
//! selection leaves as a native text drag, and is removed from the field when
//! the destination moved it (copied with Ctrl, as the compositor decides).
//! Text dropped over a field or editor goes in where it lands, through the
//! field's ordinary edit callback, so undo and validation see a normal edit.
//! Moving text inside one field is one edit.
//!
//! Each window's view is wrapped in a host widget that carries out the edits
//! a drop asks for, one per frame, by giving the field the same pointer and
//! input-method events a user would.

mod selection_copy;
pub use selection_copy::offer_read_only_selection;

use enzui_clipboard::{Kind, MimeType, Pasted, Payload, SeatSelector};
use enzui_clipboard_runtime::native_drag::{
    DragAction, DragActions, DragEvent, DragId, DragOutcome, NativeDragRuntime, ReceivedDrop,
};
use enzui_core::app_runtime::ServiceMessage;
use iced::advanced::text::editor::Position;
use iced::advanced::widget::operation::{Focusable, Outcome, TextInput};
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Task, Vector, window};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long a request to the drag transport may take.
const BUDGET: Duration = Duration::from_secs(2);

/// A message of the text drag service, carried in a [`ServiceMessage`].
#[derive(Debug, Clone)]
pub(crate) enum Message {
    /// A window's Wayland surface, for mapping drag positions to windows.
    Surface(window::Id, Option<u32>),
    /// The native runtime connected.
    Connected(NativeDragRuntime),
    ConnectionFailed,
    #[cfg(feature = "portal-dbus")]
    PortalReady(Option<Arc<enzui_core::portal::file_transfer::FileTransferClient>>),
    Closed(window::Id),
    Route(DragId, Route),
    FileSourceFinished,
    /// An event from the native runtime.
    Native(DragEvent),
    /// A dropped offer's text arrived.
    Received(
        Arc<Mutex<Option<ReceivedDrop>>>,
        DragId,
        Option<Vec<std::path::PathBuf>>,
    ),
    /// Our own drag ended.
    SourceFinished(DragOutcome),
}

/// What our own drag carried and where it came from.
#[derive(Debug, Clone)]
struct Source {
    text: String,
    window: Option<window::Id>,
    widget: Option<Id>,
    /// The selection, in the source field's positions.
    selection: Option<(Position, Position)>,
    /// A drop already moved the text (inside the source field).
    handled: bool,
}

const TEMPORARY_FILES: &str = "application/x-enzui-temporary-files-v1";

/// A hovering offer.
#[derive(Debug, Clone)]
struct Offer {
    surface: u32,
    position: Point,
    /// Whether the offer is taken as files.
    files: bool,
    mimes: Vec<MimeType>,
    actions: DragActions,
    route: Option<Route>,
    dropped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Route {
    Refused,
    Text,
    Target(crate::file_drop::Policy),
}

/// An edit a host carries out.
enum Edit {
    /// Put `text` where `position` is, then finish the drop.
    Insert {
        window: window::Id,
        position: Point,
        text: String,
        drop: Option<ReceivedDrop>,
        moving: bool,
    },
    /// Finish moving text inside its own field, once the click at the drop
    /// point has placed the caret (an editor's click goes through the
    /// application first).
    MoveWithin {
        window: window::Id,
        position: Point,
        drop: Option<ReceivedDrop>,
    },
    /// Remove the source's selection: our text was moved away.
    RemoveSource,
    /// Record which file drop target is at `position` as files are released
    /// there, before they arrive.
    Bind {
        window: window::Id,
        id: DragId,
        position: Point,
    },
    /// Hand dropped files, and the drop's receipt, to the file drop target
    /// they were released over.
    Files {
        window: window::Id,
        id: DragId,
        paths: Vec<std::path::PathBuf>,
        data: Option<Arc<Pasted>>,
        temporary: bool,
        drop: Option<ReceivedDrop>,
    },
}

#[derive(Default)]
struct Shared {
    runtime: Option<NativeDragRuntime>,
    #[cfg(feature = "portal-dbus")]
    file_portal: Option<Arc<enzui_core::portal::file_transfer::FileTransferClient>>,
    connecting: bool,
    surfaces: HashMap<u32, window::Id>,
    offers: HashMap<DragId, Offer>,
    source: Option<Source>,
    file_source_active: bool,
    /// Set by the drag hook; the host records the source on its next turn.
    started: Option<String>,
    edits: Vec<Edit>,
    /// The field a drop went into, refocused after the source is cleaned up.
    target: Option<Id>,
    /// The file drop target each released file drop belongs to; `None` when
    /// it was released over none.
    bindings: HashMap<DragId, Option<crate::file_drop::Binding>>,
    sender: Option<iced::futures::channel::mpsc::UnboundedSender<Message>>,
}

fn shared() -> MutexGuard<'static, Shared> {
    static SHARED: std::sync::OnceLock<Mutex<Shared>> = std::sync::OnceLock::new();
    SHARED
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn send(message: Message) {
    if let Some(sender) = &shared().sender {
        let _ = sender.unbounded_send(message);
    }
}

/// Wraps a service message.
fn service(message: Message) -> ServiceMessage {
    ServiceMessage::new(message)
}

/// The service's messages: its own (drag outcomes) and, once connected, the
/// native runtime's.
pub(crate) fn subscription() -> iced::Subscription<ServiceMessage> {
    struct Recipe;
    impl iced::advanced::subscription::Recipe for Recipe {
        type Output = ServiceMessage;

        fn hash(&self, state: &mut iced::advanced::subscription::Hasher) {
            use std::hash::Hash;
            "enzui-text-drag".hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: iced::advanced::subscription::EventStream,
        ) -> iced::advanced::graphics::futures::BoxStream<Self::Output> {
            use iced::futures::StreamExt as _;
            let (sender, receiver) = iced::futures::channel::mpsc::unbounded();
            shared().sender = Some(sender);
            Box::pin(receiver.map(service))
        }
    }
    let own = iced::advanced::subscription::from_recipe(Recipe);
    let native = shared().runtime.as_ref().map(|runtime| {
        runtime
            .subscription()
            .map(|event| service(Message::Native(event)))
    });
    let windows = iced::window::open_events().map(|window| service(Message::Surface(window, None)));
    let closed = iced::window::close_events().map(|window| service(Message::Closed(window)));
    iced::Subscription::batch([own, windows, closed].into_iter().chain(native))
}

/// Carries out `message`; returns the follow-up work.
pub(crate) fn update(message: &ServiceMessage) -> Task<ServiceMessage> {
    let Some(message) = message.downcast_ref::<Message>() else {
        return Task::none();
    };
    match message.clone() {
        Message::Surface(window, None) => iced::window::run(window, move |raw| {
            service(Message::Surface(
                window,
                enzui_clipboard_runtime::native_drag::surface_id(raw),
            ))
        }),
        Message::Surface(window, Some(surface)) => {
            let mut shared = shared();
            let _ = shared.surfaces.insert(surface, window);
            if shared.runtime.is_some() || shared.connecting {
                return Task::none();
            }
            shared.connecting = true;
            drop(shared);
            Task::future(NativeDragRuntime::connect(BUDGET)).then(|connected| match connected {
                Some(Ok(runtime)) => Task::done(service(Message::Connected(runtime))),
                Some(Err(error)) => {
                    log::info!(target: "enzui::text_drag", "no native text drag: {error}");
                    Task::done(service(Message::ConnectionFailed))
                }
                None => Task::done(service(Message::ConnectionFailed)),
            })
        }
        Message::ConnectionFailed => {
            shared().connecting = false;
            Task::none()
        }
        Message::Closed(window) => {
            let mut state = shared();
            let surfaces: Vec<_> = state
                .surfaces
                .iter()
                .filter_map(|(surface, owner)| (*owner == window).then_some(*surface))
                .collect();
            state.surfaces.retain(|_, owner| *owner != window);
            let ids: Vec<_> = state
                .offers
                .iter()
                .filter_map(|(id, offer)| surfaces.contains(&offer.surface).then_some(*id))
                .collect();
            for id in &ids {
                state.offers.remove(id);
                state.bindings.remove(id);
            }
            state.edits.retain(|edit| match edit {
                Edit::Insert { window: owner, .. }
                | Edit::MoveWithin { window: owner, .. }
                | Edit::Bind { window: owner, .. }
                | Edit::Files { window: owner, .. } => *owner != window,
                Edit::RemoveSource => true,
            });
            if state
                .source
                .as_ref()
                .is_some_and(|source| source.window == Some(window))
            {
                state.source = None;
            }
            let runtime = state.runtime.clone();
            drop(state);
            Task::batch(ids.into_iter().filter_map(|id| {
                runtime.clone().map(|runtime| {
                    Task::future(async move { runtime.reject(id, BUDGET).await }).discard()
                })
            }))
        }
        Message::FileSourceFinished => Task::none(),
        Message::Route(id, route) => {
            let (runtime, actions, ours) = {
                let state = shared();
                let Some(offer) = state
                    .offers
                    .get(&id)
                    .filter(|offer| offer.route.as_ref() == Some(&route))
                else {
                    return Task::none();
                };
                (state.runtime.clone(), offer.actions, state.source.is_some())
            };
            let Some(runtime) = runtime else {
                return Task::none();
            };
            let (preferences, allowed, preferred) = match route {
                Route::Refused => (
                    Kind::Uris.preferences(),
                    DragActions::NONE,
                    DragAction::Copy,
                ),
                Route::Text => (
                    Kind::Text.preferences(),
                    DragActions::BOTH,
                    if ours && actions.allows(DragAction::Move) {
                        DragAction::Move
                    } else {
                        DragAction::Copy
                    },
                ),
                Route::Target(policy) => (
                    policy.preferences,
                    if policy.copy_only {
                        DragActions::COPY
                    } else {
                        DragActions::BOTH
                    },
                    policy.preferred,
                ),
            };
            let preferred = if actions.allows(preferred) {
                preferred
            } else if actions.allows(DragAction::Copy) {
                DragAction::Copy
            } else {
                DragAction::Move
            };
            Task::future(async move {
                runtime
                    .accept(id, preferences, allowed, preferred, BUDGET)
                    .await
            })
            .discard()
        }
        #[cfg(feature = "portal-dbus")]
        Message::PortalReady(client) => {
            shared().file_portal = client;
            Task::none()
        }
        Message::Connected(runtime) => {
            shared().runtime = Some(runtime);
            iced::advanced::text::selection_drag::set_handler(Some(Arc::new(begin)));
            #[cfg(feature = "portal-dbus")]
            return Task::future(async {
                enzui_core::portal::file_transfer::FileTransferClient::connect()
                    .await
                    .ok()
            })
            .map(|client| service(Message::PortalReady(client)));
            #[cfg(not(feature = "portal-dbus"))]
            Task::none()
        }
        Message::Native(event) => native(event),
        Message::Received(drop, id, resolved) => {
            let Some(received) = drop
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            else {
                return Task::none();
            };
            #[cfg(feature = "portal-dbus")]
            if received.data().mime().as_str() == enzui_core::portal::file_transfer::MIME
                && resolved.is_none()
            {
                let client = shared().file_portal.clone();
                return Task::future(async move {
                    let paths = match client {
                        Some(client) => client.retrieve(received.data().bytes()).await.ok(),
                        None => None,
                    };
                    if let Some(paths) = paths {
                        Some(service(Message::Received(
                            Arc::new(Mutex::new(Some(received))),
                            id,
                            Some(paths),
                        )))
                    } else {
                        let mut state = shared();
                        state.offers.remove(&id);
                        state.bindings.remove(&id);
                        None
                    }
                })
                .then(|message| message.map_or_else(Task::none, Task::done));
            }
            let mut shared = shared();
            let offer = shared.offers.remove(&id);
            let paths = resolved.unwrap_or_else(|| received.data().file_paths());
            if offer.as_ref().is_some_and(|offer| offer.files) {
                let window = offer
                    .as_ref()
                    .and_then(|offer| shared.surfaces.get(&offer.surface).copied());
                if let Some(window) = window {
                    shared.edits.push(Edit::Files {
                        window,
                        id,
                        paths,
                        temporary: offer.as_ref().is_some_and(|offer| {
                            offer
                                .mimes
                                .iter()
                                .any(|mime| mime.as_str() == TEMPORARY_FILES)
                        }),
                        data: Some(Arc::new(received.data().clone())),
                        drop: Some(received),
                    });
                }
                return Task::none();
            }
            let text = received.data().text().map(str::to_owned);
            let window = offer
                .as_ref()
                .and_then(|offer| shared.surfaces.get(&offer.surface).copied());
            let (Some(text), Some(window), Some(offer)) = (text, window, offer) else {
                // Not text, or nowhere to put it: dropping the value cancels.
                return Task::none();
            };
            let moving = received.action() == DragAction::Move;
            shared.edits.push(Edit::Insert {
                window,
                position: offer.position,
                text,
                drop: Some(received),
                moving,
            });
            Task::none()
        }
        Message::SourceFinished(outcome) => {
            let mut shared = shared();
            let moved = matches!(outcome, DragOutcome::Finished(DragAction::Move));
            let handled = shared.source.as_ref().is_some_and(|source| source.handled);
            if moved && !handled && shared.source.is_some() {
                shared.edits.push(Edit::RemoveSource);
            } else {
                shared.source = None;
            }
            Task::none()
        }
    }
}

/// Answers the native runtime's destination events.
fn native(event: DragEvent) -> Task<ServiceMessage> {
    let Some(runtime) = shared().runtime.clone() else {
        return Task::none();
    };
    match event {
        DragEvent::Entered {
            id,
            surface,
            position,
            mimes,
            actions,
            ..
        } => {
            let _ = shared().offers.insert(
                id,
                Offer {
                    surface,
                    position: point(position),
                    files: false,
                    mimes,
                    actions,
                    route: None,
                    dropped: false,
                },
            );
            Task::none()
        }
        DragEvent::Motion { id, position } => {
            if let Some(offer) = shared().offers.get_mut(&id) {
                offer.position = point(position);
            }
            Task::none()
        }
        DragEvent::Dropped { id, .. } => {
            released(id);
            Task::future(async move { runtime.receive(id, BUDGET).await }).then(move |received| {
                match received {
                    Some(Ok(drop)) => Task::done(service(Message::Received(
                        Arc::new(Mutex::new(Some(drop))),
                        id,
                        None,
                    ))),
                    _ => {
                        let _ = shared().bindings.remove(&id);
                        Task::none()
                    }
                }
            })
        }
        DragEvent::Left { id } => {
            let mut state = shared();
            state.offers.remove(&id);
            state.bindings.remove(&id);
            Task::none()
        }
        DragEvent::Reset => {
            let mut state = shared();
            state.offers.clear();
            state.bindings.clear();
            state
                .edits
                .retain(|edit| matches!(edit, Edit::RemoveSource));
            Task::none()
        }
        _ => Task::none(),
    }
}

/// The offer `id` was released. Files belong to the target they are
/// released over, found on the host's next turn, before anything the app
/// does next can change the view.
fn released(id: DragId) {
    let mut shared = shared();
    let bind = shared.offers.get(&id).and_then(|offer| {
        let window = shared.surfaces.get(&offer.surface)?;
        offer.files.then_some(Edit::Bind {
            window: *window,
            id,
            position: offer.position,
        })
    });
    if let Some(bind) = bind {
        shared.edits.push(bind);
    }
    if let Some(offer) = shared.offers.get_mut(&id) {
        offer.dropped = true;
    }
}

#[allow(
    clippy::cast_possible_truncation,
    reason = "surface coordinates fit logical pixels"
)]
fn point(position: (f64, f64)) -> Point {
    Point::new(position.0 as f32, position.1 as f32)
}

/// The icon under the pointer while text is dragged: a small page of lines
/// below and right of the pointer, clear of the caret where the text lands.
fn snippet_icon() -> Option<enzui_clipboard_runtime::native_drag::DragIcon> {
    const WIDTH: u32 = 36;
    const HEIGHT: u32 = 26;
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let edge = x == 0 || y == 0 || x == WIDTH - 1 || y == HEIGHT - 1;
            let corner = (x == 0 || x == WIDTH - 1) && (y == 0 || y == HEIGHT - 1);
            // Three lines of "text", the last one shorter.
            let line = (5..WIDTH - 5).contains(&x) && matches!(y, 6 | 7 | 12 | 13)
                || (5..WIDTH - 14).contains(&x) && matches!(y, 18 | 19);
            let pixel = if corner {
                [0, 0, 0, 0]
            } else if edge {
                [96, 96, 104, 230]
            } else if line {
                [120, 120, 128, 255]
            } else {
                [252, 252, 252, 235]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    enzui_clipboard_runtime::native_drag::DragIcon::from_rgba(&rgba, WIDTH, HEIGHT, (-14, -16), 1)
        .ok()
}

/// The fork's selection-drag hook: starts a native text drag. The source
/// field is recorded by its window's host right after.
fn begin(drag: iced::advanced::text::selection_drag::SelectionDrag) -> bool {
    let Some(runtime) = shared().runtime.clone() else {
        return false;
    };
    shared().started = Some(drag.text.clone());
    let spawned = std::thread::Builder::new()
        .name("enzui-text-drag".into())
        .spawn(move || {
            let outcome = iced::futures::executor::block_on(async {
                match runtime
                    .start_with_icon(
                        Payload::text(&drag.text),
                        DragActions::BOTH,
                        SeatSelector::default(),
                        snippet_icon(),
                        BUDGET,
                    )
                    .await
                {
                    Some(Ok(source)) => {
                        enzui_clipboard_runtime::native_drag::source_outcome(source)
                            .await
                            .unwrap_or(DragOutcome::Cancelled)
                    }
                    _ => DragOutcome::Cancelled,
                }
            });
            send(Message::SourceFinished(outcome));
        });
    spawned.is_ok()
}

pub(crate) fn file_preferences() -> Vec<MimeType> {
    let preferences = Kind::Uris.preferences();
    #[cfg(feature = "portal-dbus")]
    if shared().file_portal.is_some() {
        return std::iter::once(MimeType::from(enzui_core::portal::file_transfer::MIME))
            .chain(preferences)
            .collect();
    }
    preferences
}

pub(crate) fn file_source_active() -> bool {
    shared().file_source_active
}

struct FileSourceCompletion(Arc<Mutex<Option<DragOutcome>>>);
impl Drop for FileSourceCompletion {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert(DragOutcome::Cancelled);
        shared().file_source_active = false;
        send(Message::FileSourceFinished);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "Native startup captures payload ownership, cancellation, presentation and completion in one operation."
)]
pub(crate) fn begin_files(
    paths: &[std::path::PathBuf],
    prepare: Option<crate::file_drag::Preparation>,
    data: Option<Payload>,
    temporary: bool,
    cancel: enzui_core::cancellation::CancelToken,
    icon: Option<enzui_clipboard_runtime::native_drag::DragIcon>,
    actions: DragActions,
    outcome: Arc<Mutex<Option<DragOutcome>>>,
) -> bool {
    let paths = paths.to_vec();
    if paths.is_empty() && prepare.is_none() && data.as_ref().is_none_or(Payload::is_empty) {
        return false;
    }
    let mut state = shared();
    if state.file_source_active || state.source.is_some() {
        return false;
    }
    let Some(runtime) = state.runtime.clone() else {
        return false;
    };
    state.file_source_active = true;
    #[cfg(feature = "portal-dbus")]
    let portal = state.file_portal.clone();
    drop(state);
    let spawned = std::thread::Builder::new()
        .name("enzui-file-drag".into())
        .spawn(move || {
            let _completion = FileSourceCompletion(Arc::clone(&outcome));
            let result = (|| {
                let prepared = match prepare {
                    Some(prepare) => match prepare(cancel.share()) {
                        Ok(files) => Some(files),
                        Err(_) => return DragOutcome::Cancelled,
                    },
                    None => None,
                };
                let paths = prepared
                    .as_ref()
                    .map_or(paths.as_slice(), |files| files.paths.as_slice());
                let mut payload = Payload::uris(paths);
                if !paths.is_empty() && (temporary || prepared.is_some()) {
                    payload = payload.with(TEMPORARY_FILES, b"1".to_vec());
                }
                if cancel.is_terminal()
                    || (paths.is_empty() && data.is_none())
                    || paths.len() > 128
                    || paths.iter().any(|path| {
                        !path.is_absolute() || path.as_os_str().as_encoded_bytes().contains(&0)
                    })
                {
                    return DragOutcome::Cancelled;
                }
                if let Some(data) = data {
                    if !data.is_ordinary() {
                        return DragOutcome::Cancelled;
                    }
                    let formats = data.mime_types();
                    if formats.len() > 16 {
                        return DragOutcome::Cancelled;
                    }
                    let mut total = 0usize;
                    for mime in formats {
                        if !paths.is_empty()
                            && (Kind::Uris
                                .preferences()
                                .iter()
                                .any(|wanted| wanted.matches(&mime))
                                || mime.as_str() == "application/vnd.portal.filetransfer"
                                || mime.as_str() == TEMPORARY_FILES)
                        {
                            return DragOutcome::Cancelled;
                        }
                        let Some(bytes) = data.bytes_for(&mime) else {
                            return DragOutcome::Cancelled;
                        };
                        total = total.saturating_add(bytes.len());
                        if total > 32 * 1024 * 1024 {
                            return DragOutcome::Cancelled;
                        }
                        payload = payload.with(mime, bytes);
                    }
                }
                if payload.is_empty() {
                    return DragOutcome::Cancelled;
                }
                // Read-only exports are offered only for Copy sources. URI file
                // managers retain ownership of filesystem Move semantics.
                #[cfg(feature = "portal-dbus")]
                let export = if !paths.is_empty() && !actions.allows(DragAction::Move) {
                    portal.and_then(|portal| portal.export_blocking(paths).ok())
                } else {
                    None
                };
                #[cfg(not(feature = "portal-dbus"))]
                let _ = paths;
                #[cfg(feature = "portal-dbus")]
                let payload = if let Some(export) = &export {
                    payload.with(
                        enzui_core::portal::file_transfer::MIME,
                        export.key().as_bytes().to_vec(),
                    )
                } else {
                    payload
                };
                if cancel.is_terminal() {
                    return DragOutcome::Cancelled;
                }
                iced::futures::executor::block_on(async {
                    match runtime
                        .start_with_icon(
                            payload,
                            actions,
                            SeatSelector::default(),
                            icon.or_else(snippet_icon),
                            BUDGET,
                        )
                        .await
                    {
                        Some(Ok(source)) => {
                            enzui_clipboard_runtime::native_drag::source_outcome(source)
                                .await
                                .unwrap_or(DragOutcome::Cancelled)
                        }
                        _ => DragOutcome::Cancelled,
                    }
                })
            })();
            *outcome
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
        });
    if spawned.is_err() {
        shared().file_source_active = false;
        return false;
    }
    true
}

/// Wraps a window's view with the host that carries out drop edits.
pub(crate) fn host<'a, Message: 'a, Theme: 'a, Renderer>(
    content: Element<'a, Message, Theme, Renderer>,
    window: window::Id,
) -> Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
{
    Element::new(Host { content, window })
}

struct Host<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    window: window::Id,
}

/// The focused text input: its id, bounds and text.
#[derive(Default)]
struct Focused {
    scroll: enzui_core::scroll_stack::ScrollStack,
    blocked: Vec<Rectangle>,
    focused: Vec<(Option<Id>, Rectangle)>,
    found: Option<(
        Option<Id>,
        Rectangle,
        Option<iced::advanced::widget::operation::text_input::AccessibleText>,
    )>,
}

impl Operation for Focused {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content: Size,
        translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.scroll.note_scrollable(id, bounds, translation);
    }
    fn custom(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &mut dyn std::any::Any) {
        if let Some(node) = value.downcast_ref::<enzui_core::collect::SemanticsNode>()
            && (node.semantics.disabled || node.semantics.read_only)
        {
            self.blocked.push(self.scroll.translate(bounds));
        }
    }
    fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
        let bounds = self.scroll.translate(bounds);
        if state.is_focused() {
            self.focused.push((id.cloned(), bounds));
        }
    }

    fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
        let bounds = self.scroll.translate(bounds);
        if self
            .blocked
            .iter()
            .any(|blocked| blocked.intersects(&bounds))
        {
            return;
        }
        if self
            .focused
            .iter()
            .any(|(focused, rect)| focused.as_ref() == id && *rect == bounds)
        {
            self.found = Some((id.cloned(), bounds, state.accessible_text()));
        }
    }
}

struct TextAt {
    position: Point,
    scroll: enzui_core::scroll_stack::ScrollStack,
    blocked: Vec<Rectangle>,
    found: bool,
}
impl Operation for TextAt {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
    }
    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _content: Size,
        translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.scroll.note_scrollable(id, bounds, translation);
    }
    fn custom(&mut self, _id: Option<&Id>, bounds: Rectangle, value: &mut dyn std::any::Any) {
        if let Some(node) = value.downcast_ref::<enzui_core::collect::SemanticsNode>()
            && (node.semantics.disabled || node.semantics.read_only)
        {
            self.blocked.push(self.scroll.translate(bounds));
        }
    }
    fn text_input(&mut self, _id: Option<&Id>, bounds: Rectangle, _state: &mut dyn TextInput) {
        let bounds = self.scroll.translate(bounds);
        if bounds.contains(self.position)
            && self
                .scroll
                .viewport()
                .is_none_or(|clip| clip.contains(self.position))
            && !self
                .blocked
                .iter()
                .any(|blocked| blocked.contains(self.position))
        {
            self.found = true;
        }
    }
}

/// Selects `start..end` in the text input `target`, or the focused one.
struct Select {
    target: Option<Id>,
    start: Position,
    end: Position,
}

impl Operation for Select {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text_input(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn TextInput) {
        if self.target.is_none() || self.target.as_ref() == id {
            state.select_range(self.start, self.end);
        }
    }
}

/// The byte offset of `position` in `lines` joined with line breaks.
fn offset(lines: &[String], position: Position) -> usize {
    lines
        .iter()
        .take(position.line)
        .map(|line| line.len() + 1)
        .sum::<usize>()
        + position.index
}

/// The position of byte `offset` in `lines` joined with line breaks.
fn position_at(lines: &[String], mut offset: usize) -> Position {
    for (line, text) in lines.iter().enumerate() {
        if offset <= text.len() {
            return Position {
                line,
                index: offset,
            };
        }
        offset -= text.len() + 1;
    }
    Position {
        line: lines.len().saturating_sub(1),
        index: lines.last().map_or(0, String::len),
    }
}

fn operate_overlay_at<Message, Theme, Renderer: iced::advanced::Renderer>(
    mut overlays: Vec<overlay::Element<'_, Message, Theme, Renderer>>,
    position: Point,
    renderer: &Renderer,
    operation: &mut dyn Operation,
) -> bool {
    overlays.sort_by(|a, b| {
        a.as_overlay()
            .index()
            .partial_cmp(&b.as_overlay().index())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for element in overlays.iter_mut().rev() {
        let surface = element.as_overlay_mut();
        if operate_overlay_at(surface.overlay(renderer), position, renderer, operation) {
            return true;
        }
        if surface.area().is_some_and(|area| area.contains(position))
            || surface.mouse_interaction(mouse::Cursor::Available(position), renderer)
                != mouse::Interaction::None
        {
            surface.operate(renderer, operation);
            return true;
        }
    }
    false
}

impl<Message, Theme, Renderer: iced::advanced::Renderer> Host<'_, Message, Theme, Renderer> {
    fn operate_everywhere(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        operation: &mut dyn Operation,
    ) {
        let bounds = *viewport;
        self.content
            .as_widget_mut()
            .operate(tree, layout, &bounds, renderer, operation);
        let overlays = self.content.as_widget_mut().overlay(
            tree,
            layout,
            renderer,
            &bounds,
            Vector::ZERO,
            bounds.size(),
        );
        if !overlays.is_empty() {
            overlay::Group::new(overlays).operate(renderer, operation);
        }
    }

    fn operate_at(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        position: Point,
        operation: &mut dyn Operation,
    ) {
        let bounds = *viewport;
        let overlays = self.content.as_widget_mut().overlay(
            tree,
            layout,
            renderer,
            &bounds,
            Vector::ZERO,
            bounds.size(),
        );
        // A surface above the document owns its covered area even when it has
        // no compatible target; a drop must never pass through a popup.
        if !operate_overlay_at(overlays, position, renderer, operation) {
            self.content
                .as_widget_mut()
                .operate(tree, layout, &bounds, renderer, operation);
        }
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the widget update arguments, passed through to the content"
    )]
    fn replay(
        &mut self,
        tree: &mut Tree,
        events: &[Event],
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for event in events {
            let mut bus = iced::advanced::shell::Bus::new();
            let mut local = shell.local(&mut bus);
            {
                let overlays = self.content.as_widget_mut().overlay(
                    tree,
                    layout,
                    renderer,
                    viewport,
                    Vector::ZERO,
                    viewport.size(),
                );
                if !overlays.is_empty() {
                    overlay::Group::new(overlays).update(event, cursor, renderer, &mut local);
                }
            }
            if !local.is_event_captured() {
                self.content
                    .as_widget_mut()
                    .update(tree, event, layout, cursor, renderer, &mut local, viewport);
            }
            shell.merge(local, |message| message);
        }
    }

    fn focused(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
    ) -> Focused {
        let mut focused = Focused::default();
        // Focus first, text second: one pass of each.
        self.operate_everywhere(tree, layout, renderer, viewport, &mut focused);
        self.operate_everywhere(tree, layout, renderer, viewport, &mut focused);
        focused
    }

    fn operate_all(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        mut operation: Box<dyn Operation>,
    ) {
        loop {
            self.operate_everywhere(tree, layout, renderer, viewport, operation.as_mut());
            match operation.finish() {
                Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }

    /// Carries out the next edit for this window, if any.
    fn edit(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let edit = {
            let mut shared = shared();
            let index = shared.edits.iter().position(|edit| match edit {
                Edit::Insert { window, .. }
                | Edit::MoveWithin { window, .. }
                | Edit::Files { window, .. } => *window == self.window,
                // Taken all at once by `bind`.
                Edit::Bind { .. } => false,
                Edit::RemoveSource => shared
                    .source
                    .as_ref()
                    .is_none_or(|source| source.window.is_none_or(|window| window == self.window)),
            });
            index.map(|index| shared.edits.remove(index))
        };
        let Some(edit) = edit else {
            return;
        };
        let commit =
            |text: String| Event::InputMethod(iced::advanced::input_method::Event::Commit(text));
        match edit {
            Edit::Bind { .. } => {}
            Edit::Files {
                id,
                paths,
                data,
                temporary,
                drop,
                ..
            } => {
                // Released over no target: dropping the receipt cancels.
                let Some(binding) = shared().bindings.remove(&id).flatten() else {
                    return;
                };
                let receipt = crate::file_drop::Receipt::of(drop).with_temporary(temporary);
                let mut deliver = match data {
                    Some(data) => {
                        crate::file_drop::Deliver::new_data(binding, data, paths, receipt)
                    }
                    None => crate::file_drop::Deliver::new(binding, paths, receipt),
                };
                self.operate_everywhere(tree, layout, renderer, viewport, &mut deliver);
                // The target settles the receipt: at once for paths, or
                // when the app finishes its `FileDrop`. A target replaced
                // or rekeyed since the release took nothing, and the
                // receipt dropped with `deliver` refuses the drop.
                if deliver.delivered() {
                    shell.request_redraw();
                }
            }
            Edit::Insert {
                position,
                text,
                drop,
                moving,
                ..
            } => {
                let at = mouse::Cursor::Available(position);
                let click = [
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                ];
                self.replay(tree, &click, layout, at, renderer, shell, viewport);
                let focused = self.focused(tree, layout, renderer, viewport);
                let Some((id, bounds, _)) = focused.found else {
                    return; // no field there: dropping `drop` cancels
                };
                if !bounds.contains(position) {
                    return;
                }
                let source = shared().source.clone();
                let same_field = moving
                    && source
                        .as_ref()
                        .is_some_and(|source| source.widget.is_some() && source.widget == id);
                if same_field {
                    // The caret is placed by the next frame.
                    shared().edits.insert(
                        0,
                        Edit::MoveWithin {
                            window: self.window,
                            position,
                            drop,
                        },
                    );
                    shell.request_redraw();
                    return;
                }
                self.replay(tree, &[commit(text)], layout, at, renderer, shell, viewport);
                shared().target = id;
                if let Some(drop) = drop {
                    drop.finish();
                }
                shell.request_redraw();
            }
            Edit::MoveWithin { position, drop, .. } => {
                let at = mouse::Cursor::Available(position);
                let focused = self.focused(tree, layout, renderer, viewport);
                let source = shared().source.clone();
                if let (Some((id, _, Some(accessible))), Some(source)) = (focused.found, source)
                    && let (Some((anchor, caret)), Some((drop_at, _))) =
                        (source.selection, accessible.selection)
                {
                    // One edit over the span between the selection and the
                    // drop point.
                    let lines: Vec<String> = accessible
                        .lines
                        .iter()
                        .map(|line| line.text.clone())
                        .collect();
                    let full = lines.join("\n");
                    let (a, b) = {
                        let (x, y) = (offset(&lines, anchor), offset(&lines, caret));
                        (x.min(y), x.max(y))
                    };
                    let c = offset(&lines, drop_at);
                    if (a..=b).contains(&c) {
                        return; // dropped onto itself: `drop` cancels
                    }
                    let (start, end, replacement) = if c < a {
                        (
                            c,
                            b,
                            format!("{}{}", source.text, full.get(c..a).unwrap_or_default()),
                        )
                    } else {
                        (
                            a,
                            c,
                            format!("{}{}", full.get(b..c).unwrap_or_default(), source.text),
                        )
                    };
                    self.operate_all(
                        tree,
                        layout,
                        renderer,
                        viewport,
                        Box::new(Select {
                            target: id,
                            start: position_at(&lines, start),
                            end: position_at(&lines, end),
                        }),
                    );
                    self.replay(
                        tree,
                        &[commit(replacement)],
                        layout,
                        at,
                        renderer,
                        shell,
                        viewport,
                    );
                    if let Some(source) = shared().source.as_mut() {
                        source.handled = true;
                    }
                    if let Some(drop) = drop {
                        drop.finish();
                    }
                    shell.request_redraw();
                }
            }
            Edit::RemoveSource => {
                let (source, target) = {
                    let mut shared = shared();
                    (shared.source.take(), shared.target.take())
                };
                let Some(Source {
                    text: dragged,
                    widget,
                    selection: Some((anchor, caret)),
                    ..
                }) = source
                else {
                    return;
                };
                if let Some(widget) = widget.clone() {
                    self.operate_all(
                        tree,
                        layout,
                        renderer,
                        viewport,
                        Box::new(iced::advanced::widget::operation::focusable::focus(widget)),
                    );
                }
                // The field may have changed while the drag was out: remove
                // only if the range still holds the dragged text, else the
                // move leaves the source as a copy would.
                let still_there = self
                    .focused(tree, layout, renderer, viewport)
                    .found
                    .and_then(|(_, _, accessible)| accessible)
                    .is_some_and(|accessible| {
                        let lines: Vec<String> =
                            accessible.lines.into_iter().map(|line| line.text).collect();
                        let (x, y) = (offset(&lines, anchor), offset(&lines, caret));
                        lines.join("\n").get(x.min(y)..x.max(y)) == Some(dragged.as_str())
                    });
                if !still_there {
                    return;
                }
                self.operate_all(
                    tree,
                    layout,
                    renderer,
                    viewport,
                    Box::new(Select {
                        target: widget,
                        start: anchor,
                        end: caret,
                    }),
                );
                let cursor = mouse::Cursor::Unavailable;
                self.replay(
                    tree,
                    &[commit(String::new())],
                    layout,
                    cursor,
                    renderer,
                    shell,
                    viewport,
                );
                if let Some(target) = target {
                    self.operate_all(
                        tree,
                        layout,
                        renderer,
                        viewport,
                        Box::new(iced::advanced::widget::operation::focusable::focus(target)),
                    );
                }
                shell.request_redraw();
            }
        }
    }

    /// Records, for each drop of files released over this window, the
    /// file drop target under the release point.
    fn bind(&mut self, tree: &mut Tree, layout: Layout, renderer: &Renderer, viewport: &Rectangle) {
        let binds: Vec<(DragId, Point)> = {
            let mut shared = shared();
            let mut binds = Vec::new();
            shared.edits.retain(|edit| match edit {
                Edit::Bind {
                    window,
                    id,
                    position,
                } if *window == self.window => {
                    binds.push((*id, *position));
                    false
                }
                _ => true,
            });
            binds
        };
        for (id, position) in binds {
            let offer = shared()
                .offers
                .get(&id)
                .map(|offer| (offer.mimes.clone(), offer.actions));
            let mut locate = offer.map_or_else(
                || crate::file_drop::Locate::new(position),
                |(mimes, actions)| crate::file_drop::Locate::for_offer(position, mimes, actions),
            );
            self.operate_at(tree, layout, renderer, viewport, position, &mut locate);
            let _ = shared().bindings.insert(id, locate.found());
        }
    }

    fn refresh_targets(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        shell: &mut Shell<'_, Message>,
    ) {
        let offers: Vec<_> = {
            let state = shared();
            state
                .offers
                .iter()
                .filter(|(_, offer)| {
                    !offer.dropped && state.surfaces.get(&offer.surface) == Some(&self.window)
                })
                .map(|(id, offer)| (*id, offer.position, offer.mimes.clone(), offer.actions))
                .collect()
        };
        let mut active = Vec::new();
        for (id, position, mimes, actions) in offers {
            let mut locate = crate::file_drop::Locate::for_offer(position, mimes.clone(), actions);
            self.operate_at(tree, layout, renderer, viewport, position, &mut locate);
            let route = if let Some(policy) = locate.policy().filter(|policy| {
                policy
                    .preferences
                    .iter()
                    .any(|wanted| mimes.iter().any(|mime| wanted.matches(mime)))
            }) {
                active.push(policy.binding);
                Route::Target(policy)
            } else {
                let mut text = TextAt {
                    position,
                    scroll: Default::default(),
                    blocked: Vec::new(),
                    found: false,
                };
                self.operate_at(tree, layout, renderer, viewport, position, &mut text);
                if text.found
                    && Kind::Text
                        .preferences()
                        .iter()
                        .any(|wanted| mimes.iter().any(|mime| wanted.matches(mime)))
                {
                    Route::Text
                } else {
                    Route::Refused
                }
            };
            let mut state = shared();
            if let Some(offer) = state.offers.get_mut(&id)
                && offer.route.as_ref() != Some(&route)
            {
                offer.files = matches!(route, Route::Target(_));
                offer.route = Some(route.clone());
                drop(state);
                send(crate::text_drag::Message::Route(id, route));
            }
        }
        let mut hover = crate::file_drop::Hover {
            active,
            changed: false,
        };
        self.operate_everywhere(tree, layout, renderer, viewport, &mut hover);
        if hover.changed {
            shell.request_redraw();
        }
    }

    /// Records the field a drag just started from.
    fn record_source(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
    ) {
        let Some(text) = shared().started.take() else {
            return;
        };
        let focused = self.focused(tree, layout, renderer, viewport);
        let (widget, selection) = match focused.found {
            Some((id, _, accessible)) => (id, accessible.and_then(|text| text.selection)),
            None => (None, None),
        };
        shared().source = Some(Source {
            text,
            window: Some(self.window),
            widget,
            selection,
            handled: false,
        });
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Host<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<selection_copy::State>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(selection_copy::State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Bind incoming drops before either gesture handling or content edits.
        self.bind(&mut tree.children[0], layout, renderer, viewport);
        let selection_gesture =
            self.copy_selection_event(tree, event, layout, cursor, renderer, shell, viewport);
        let content = &mut tree.children[0];
        if !selection_gesture {
            self.content
                .as_widget_mut()
                .update(content, event, layout, cursor, renderer, shell, viewport);
        }
        self.refresh_targets(content, layout, renderer, viewport, shell);
        self.record_source(content, layout, renderer, viewport);
        self.edit(content, layout, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
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
        renderer: &Renderer,
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
        renderer: &Renderer,
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_field::enz_text_field;
    use iced::advanced::graphics::core::shell::Waker;
    use iced_test::runtime::user_interface::{Cache, UserInterface};

    /// The shared state is process-wide: one test at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        First(String),
        Second(String),
    }

    struct Fields {
        popup: bool,
        drop_enabled: bool,
        first: String,
        second: String,
        renderer: iced_test::renderer::Renderer,
        cache: Cache,
        window: window::Id,
    }

    impl Fields {
        fn new(first: &str, second: &str) -> Self {
            let renderer = iced::futures::executor::block_on(
                <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                    iced::advanced::renderer::Settings::default(),
                    None,
                ),
            )
            .unwrap();
            *shared() = Shared::default();
            Self {
                popup: false,
                drop_enabled: true,
                first: first.to_owned(),
                second: second.to_owned(),
                renderer,
                cache: Cache::default(),
                window: window::Id::unique(),
            }
        }

        /// One frame of `events`, applying the fields' messages.
        fn frame(&mut self, events: &[Event], operation: Option<&mut dyn Operation>) {
            let second =
                enz_text_field("Second", "", &self.second, Msg::Second).id(Id::new("second"));
            let second: Element<'_, Msg> = if self.popup {
                let first = self.first.clone();
                crate::popover::enz_popover(
                    iced::widget::Space::new().width(100).height(20),
                    Some(second.into()),
                    move || Msg::First(first.clone()),
                )
                .width(300.0)
                .padding(0.0)
                .into()
            } else {
                second.into()
            };
            let view: Element<'_, Msg> = iced::widget::column![
                enz_text_field("First", "", &self.first, Msg::First).id(Id::new("first")),
                second,
            ]
            .spacing(20)
            .into();
            let unchanged = self.first.clone();
            let view = crate::file_drop::enz_file_drop_target(view, move |_| {
                Msg::First(unchanged.clone())
            })
            .enabled(self.drop_enabled)
            .key("field-drop-scope")
            .into();
            let mut ui = UserInterface::build(
                host(view, self.window),
                Size::new(400.0, 300.0),
                std::mem::take(&mut self.cache),
                &mut self.renderer,
            );
            if let Some(operation) = operation {
                ui.operate(&self.renderer, operation);
            }
            let mut messages = iced::advanced::shell::Bus::new();
            let _ = ui.update(
                &window::Headless,
                &Waker::noop(),
                events,
                mouse::Cursor::Unavailable,
                &mut self.renderer,
                &mut messages,
            );
            self.cache = ui.into_cache();
            for message in messages {
                match message {
                    Msg::First(text) => self.first = text,
                    Msg::Second(text) => self.second = text,
                }
            }
        }

        fn redraw(&mut self) {
            self.frame(
                &[Event::Window(window::Event::RedrawRequested(
                    std::time::Instant::now(),
                ))],
                None,
            );
        }

        /// Where the field `id`'s text ends, a little inside its box.
        fn end_of(&mut self, id: &'static str) -> Point {
            let mut found = Bounds(Id::new(id), None);
            self.frame(&[], Some(&mut found));
            let bounds = found.1.unwrap();
            Point::new(bounds.x + bounds.width - 4.0, bounds.center_y())
        }
    }

    struct Bounds(Id, Option<Rectangle>);

    impl Operation for Bounds {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, _state: &mut dyn TextInput) {
            if id == Some(&self.0) {
                self.1 = Some(bounds);
            }
        }
    }

    #[test]
    fn a_drop_inserts_where_it_lands() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for popup in [false, true] {
            let mut fields = Fields::new("Drag me", "Drop: ");
            fields.popup = popup;
            let at = fields.end_of("second");
            shared().edits.push(Edit::Insert {
                window: fields.window,
                position: at,
                text: "here".into(),
                drop: None,
                moving: false,
            });
            fields.redraw();
            assert_eq!(fields.second, "Drop: here", "popup={popup}");
            assert_eq!(shared().target, Some(Id::new("second")));
            fields.drop_enabled = false;
            fields.redraw();
            fields.drop_enabled = true;
            fields.redraw();
            fields.frame(
                &[Event::InputMethod(
                    iced::advanced::input_method::Event::Commit("!".into()),
                )],
                None,
            );
            assert_eq!(
                fields.second, "Drop: here!",
                "pausing a drop target must retain editor focus"
            );

            // Somewhere with no field: nothing changes.
            shared().edits.push(Edit::Insert {
                window: fields.window,
                position: Point::new(390.0, 290.0),
                text: "lost".into(),
                drop: None,
                moving: false,
            });
            fields.redraw();
            assert_eq!(
                (fields.first.as_str(), fields.second.as_str()),
                ("Drag me", "Drop: here!")
            );
        }
    }

    #[test]
    fn moving_out_removes_the_source_selection() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut fields = Fields::new("Drag me", "Drop: ");
        shared().source = Some(Source {
            text: "Drag ".into(),
            window: Some(fields.window),
            widget: Some(Id::new("first")),
            selection: Some((
                Position { line: 0, index: 0 },
                Position { line: 0, index: 5 },
            )),
            handled: false,
        });
        shared().edits.push(Edit::RemoveSource);
        fields.redraw();
        assert_eq!(fields.first, "me");
        assert!(shared().source.is_none());
    }

    #[test]
    fn a_source_changed_during_the_drag_is_left_alone() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // "Drag " was dragged out, then the field changed underneath.
        let mut fields = Fields::new("Keep everything", "Drop: ");
        shared().source = Some(Source {
            text: "Drag ".into(),
            window: Some(fields.window),
            widget: Some(Id::new("first")),
            selection: Some((
                Position { line: 0, index: 0 },
                Position { line: 0, index: 5 },
            )),
            handled: false,
        });
        shared().edits.push(Edit::RemoveSource);
        fields.redraw();
        assert_eq!(fields.first, "Keep everything", "nothing else was deleted");
        assert!(shared().source.is_none());
    }

    #[test]
    fn moving_within_a_field_is_one_edit() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut fields = Fields::new("Drag me far", "x");
        shared().source = Some(Source {
            text: "Drag ".into(),
            window: Some(fields.window),
            widget: Some(Id::new("first")),
            selection: Some((
                Position { line: 0, index: 0 },
                Position { line: 0, index: 5 },
            )),
            handled: false,
        });
        let at = fields.end_of("first");
        shared().edits.push(Edit::Insert {
            window: fields.window,
            position: at,
            text: "Drag ".into(),
            drop: None,
            moving: true,
        });
        fields.redraw(); // the click places the caret
        fields.redraw(); // the move
        assert_eq!(fields.first, "me farDrag ");
        assert!(
            shared()
                .source
                .as_ref()
                .is_some_and(|source| source.handled)
        );

        // The drag's own outcome then removes nothing more.
        let _ = update(&service(Message::SourceFinished(DragOutcome::Finished(
            DragAction::Move,
        ))));
        fields.redraw();
        assert_eq!(fields.first, "me farDrag ");
    }

    #[test]
    fn offsets_and_positions_round_trip_across_lines() {
        let lines = vec!["ab".to_owned(), String::new(), "cdé".to_owned()];
        for (offset_, position) in [
            (0, Position { line: 0, index: 0 }),
            (2, Position { line: 0, index: 2 }),
            (3, Position { line: 1, index: 0 }),
            (4, Position { line: 2, index: 0 }),
            (8, Position { line: 2, index: 4 }),
        ] {
            assert_eq!(offset(&lines, position), offset_);
            assert_eq!(position_at(&lines, offset_), position);
        }
    }

    /// Files released over a target reach it once they arrive, unless the
    /// app replaced what the target stands for in between.
    #[test]
    fn dropped_files_reach_the_target_they_were_released_over() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        #[derive(Debug, Clone, PartialEq)]
        enum Dropped {
            Files(u64, Vec<std::path::PathBuf>),
        }
        let mut renderer = iced::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .unwrap();
        let window = window::Id::unique();
        // Inside a column, as in a real view: the column routes frames only
        // to children that ask for them.
        let view = |page: u64| -> Element<'static, Dropped> {
            host(
                iced::widget::column![
                    iced::widget::Space::new().height(10),
                    crate::file_drop::enz_file_drop_target(
                        iced::widget::Space::new()
                            .width(Length::Fill)
                            .height(Length::Fill),
                        move |paths| Dropped::Files(page, paths),
                    )
                    .enabled(page != 99)
                    .key(if page == 99 { 0 } else { page }),
                ]
                .into(),
                window,
            )
        };
        let paths = vec![std::path::PathBuf::from("/home/user/photo.jpg")];
        let frame = || Event::Window(window::Event::RedrawRequested(std::time::Instant::now()));
        let moved = || {
            Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(10.0, 10.0),
            })
        };
        type Ui<'a> = UserInterface<'a, Dropped, iced::Theme, iced_test::renderer::Renderer>;
        fn run(
            ui: &mut Ui<'_>,
            renderer: &mut iced_test::renderer::Renderer,
            events: &[Event],
        ) -> Vec<Dropped> {
            let mut messages = iced::advanced::shell::Bus::new();
            let _ = ui.update(
                &window::Headless,
                &Waker::noop(),
                events,
                mouse::Cursor::Unavailable,
                renderer,
                &mut messages,
            );
            messages.messages().to_vec()
        }
        // Released over page 0's target; the app shows `then` before the
        // files arrive. Returns what the target published.
        let mut drop_then = |transitions: &[u64], delivery_first: bool| {
            *shared() = Shared::default();
            let id = DragId::detached(1);
            let _ = shared().surfaces.insert(7, window);
            let _ = shared().offers.insert(
                id,
                Offer {
                    surface: 7,
                    position: Point::new(200.0, 150.0),
                    files: true,
                    mimes: Kind::Uris.preferences(),
                    actions: DragActions::BOTH,
                    route: None,
                    dropped: false,
                },
            );
            // One interface per view, as the runtime keeps one: only its
            // first frame visits every widget.
            let mut ui = UserInterface::build(
                view(0),
                Size::new(400.0, 300.0),
                Cache::default(),
                &mut renderer,
            );
            assert_eq!(run(&mut ui, &mut renderer, &[frame()]), vec![]);
            released(id);
            // The host binds the drop on its next turn.
            assert_eq!(run(&mut ui, &mut renderer, &[moved()]), vec![]);
            assert!(shared().edits.is_empty(), "the host bound the drop");
            if !delivery_first {
                for page in transitions {
                    ui = UserInterface::build(
                        view(*page),
                        Size::new(400.0, 300.0),
                        ui.into_cache(),
                        &mut renderer,
                    );
                }
            }
            // The files arrive.
            let _ = shared().offers.remove(&id);
            shared().edits.push(Edit::Files {
                window,
                id,
                paths: paths.clone(),
                data: None,
                temporary: false,
                drop: None,
            });
            assert_eq!(run(&mut ui, &mut renderer, &[moved()]), vec![]);
            assert!(shared().edits.is_empty(), "the host took the drop");
            if delivery_first {
                for page in transitions {
                    ui = UserInterface::build(
                        view(*page),
                        Size::new(400.0, 300.0),
                        ui.into_cache(),
                        &mut renderer,
                    );
                }
            }
            // A target that took the files publishes them on the next frame.
            run(&mut ui, &mut renderer, &[frame()])
        };
        assert_eq!(
            drop_then(&[], false),
            vec![Dropped::Files(0, paths.clone())]
        );
        for delivery_first in [false, true] {
            for pages in [&[1][..], &[1, 0][..], &[99][..], &[99, 0][..]] {
                assert_eq!(
                    drop_then(pages, delivery_first),
                    vec![],
                    "retired scope must not consume a prior drop: pages={pages:?}, delivered={delivery_first}"
                );
            }
        }
        let overlay_view = |accepts: bool, open: bool| -> Element<'static, Dropped> {
            let panel: Element<'static, Dropped> = if accepts {
                crate::file_drop::enz_file_drop_target(
                    iced::widget::Space::new().width(160).height(100),
                    |paths| Dropped::Files(1, paths),
                )
                .key("popup-target")
                .into()
            } else {
                iced::widget::Space::new().width(160).height(100).into()
            };
            let popup = crate::popover::enz_popover(
                iced::widget::Space::new().width(80).height(32),
                open.then_some(panel),
                || Dropped::Files(99, vec![]),
            )
            .width(160.0)
            .padding(0.0);
            host(
                crate::file_drop::enz_file_drop_target(
                    iced::widget::container(popup)
                        .width(Length::Fill)
                        .height(Length::Fill),
                    |paths| Dropped::Files(0, paths),
                )
                .into(),
                window,
            )
        };
        for (accepts, stays_open) in [(true, true), (false, true), (true, false)] {
            *shared() = Shared::default();
            let id = DragId::detached(2);
            shared().surfaces.insert(7, window);
            shared().offers.insert(
                id,
                Offer {
                    surface: 7,
                    position: Point::new(60.0, 80.0),
                    files: true,
                    mimes: Kind::Uris.preferences(),
                    actions: DragActions::COPY,
                    route: None,
                    dropped: false,
                },
            );
            let mut ui = UserInterface::build(
                overlay_view(accepts, true),
                Size::new(400.0, 300.0),
                Cache::default(),
                &mut renderer,
            );
            run(&mut ui, &mut renderer, &[frame()]);
            released(id);
            run(&mut ui, &mut renderer, &[moved()]);
            let mut ui = UserInterface::build(
                overlay_view(accepts, stays_open),
                Size::new(400.0, 300.0),
                ui.into_cache(),
                &mut renderer,
            );
            shared().offers.remove(&id);
            shared().edits.push(Edit::Files {
                window,
                id,
                paths: paths.clone(),
                data: None,
                temporary: false,
                drop: None,
            });
            run(&mut ui, &mut renderer, &[moved()]);
            let received = run(&mut ui, &mut renderer, &[frame()]);
            assert_eq!(
                received,
                if accepts && stays_open {
                    vec![Dropped::Files(1, paths.clone())]
                } else {
                    vec![]
                },
                "popup admission and lifetime: accepts={accepts}, stays_open={stays_open}"
            );
        }
    }
}
