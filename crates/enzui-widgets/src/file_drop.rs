//! A region that takes files dropped from other applications.
//!
//! Wrap any part of the view in [`enz_file_drop_target`] and files dragged
//! onto it from a file manager or another app arrive as one message with
//! their paths. Nothing else to wire: `app_setup::run` installs the native
//! drag service (the same one that carries text drags), and while a target is
//! mounted the service takes file lists as files rather than as text. Text
//! fields keep taking dropped text.
//!
//! When targets nest, the innermost one under the drop point takes the
//! files. A drop that lands on no target is refused, and the source keeps its
//! files.
//!
//! [`enz_file_drop_target`] tells the source the drop succeeded as soon as
//! the target takes the paths. An app that copies or checks the files first
//! uses [`enz_file_drop_target_with_receipt`]: its message carries a
//! [`FileDrop`] whose [`finish`](FileDrop::finish) tells the source once the
//! files are safely the app's; dropping it unfinished (staging failed, the
//! compose window closed, a drop arrived while busy) refuses the drop.
//!
//! [`FileDrop::action`] says whether the user copied or moved the files. A
//! target that only ever takes copies (attaching, importing) sets
//! [`copy_only`](EnzFileDropTarget::copy_only): the drag service then offers
//! only Copy when this target is under the pointer, so a
//! modifier at release cannot turn the drop into a Move that lets the source
//! delete its files, and a Move that still arrives is refused unfinished.
//!
//! A drop belongs to the target under the pointer when the user released it,
//! even though the files arrive a moment later (the source may still be
//! writing them). If that target is gone by then, the drop is refused rather
//! than handed to whatever took its place. A target that stays mounted while
//! what it stands for changes (the page it attaches to, the open document)
//! says so with [`key`](EnzFileDropTarget::key): a drop released under one
//! key is refused once the target carries another.

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use enzui_clipboard::{MimeType, Pasted};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub use enzui_clipboard_runtime::native_drag::DragAction;

/// Theme appearance of an accepting drop area.
#[derive(Debug, Clone, Copy)]
pub struct DropStyle {
    /// Translucent fill above the content.
    pub fill: iced::Color,
    /// Outline and corner geometry.
    pub border: iced::Border,
}

/// Resolves native drop feedback for a theme.
pub trait Catalog {
    /// Appearance of the innermost accepting target under the drag.
    fn drop_style(&self) -> DropStyle;
}

impl Catalog for iced::Theme {
    fn drop_style(&self) -> DropStyle {
        let palette = crate::theme_lookup::palette(self);
        DropStyle {
            fill: palette
                .accent
                .scale_alpha(crate::theme_lookup::opacity(self).drop_target()),
            border: iced::Border {
                radius: enzui_theme::text_defaults::shape_radius(|shape| shape.row).into(),
                ..iced::Border::default()
            },
        }
    }
}

/// The source's acknowledgement of one drop, shared by the clones of a
/// [`FileDrop`]: the first [`finish`](Self::finish) reports success, and the
/// last clone dropped unfinished refuses the drop.
#[derive(Clone)]
pub(crate) struct Receipt(Arc<Mutex<Option<Settle>>>, Option<DragAction>, bool);

/// Tells the source the drop succeeded; dropping it unrun refuses the drop.
type Settle = Box<dyn FnOnce() + Send>;

impl Receipt {
    /// A receipt that runs `finish` on success and drops it otherwise.
    pub(crate) fn new(finish: impl FnOnce() + Send + 'static) -> Self {
        Self(Arc::new(Mutex::new(Some(Box::new(finish)))), None, false)
    }

    /// The receipt of a native drop; `None` for files with no source to tell.
    pub(crate) fn of(drop: Option<enzui_clipboard_runtime::native_drag::ReceivedDrop>) -> Self {
        match drop {
            Some(drop) => {
                let action = drop.action();
                Self::new(move || drop.finish()).with_action(action)
            }
            None => Self(Arc::new(Mutex::new(None)), None, false),
        }
    }

    /// The same receipt, recording the negotiated `action`.
    pub(crate) fn with_action(mut self, action: DragAction) -> Self {
        self.1 = Some(action);
        self
    }

    pub(crate) fn with_temporary(mut self, temporary: bool) -> Self {
        self.2 = temporary;
        self
    }

    fn finish(&self) {
        let finish = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(finish) = finish {
            finish();
        }
    }
}

impl std::fmt::Debug for Receipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Receipt").finish_non_exhaustive()
    }
}

/// Files dropped on an [`enz_file_drop_target_with_receipt`], with the
/// source still waiting to hear whether the drop succeeded.
///
/// Call [`finish`](Self::finish) once the files are the app's (copied into
/// its store, checked, attached). Dropping every clone unfinished refuses
/// the drop. Temporary exports may be removed when the source completes or is
/// cancelled; a receiver must own its copies before settling the receipt.
#[derive(Debug, Clone)]
pub struct FileDrop {
    paths: Vec<PathBuf>,
    receipt: Receipt,
}

impl FileDrop {
    /// The dropped files.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Whether the source says these paths are temporary exports.
    /// Copy these into receiver-owned storage before finishing. Other toolkits may
    /// omit this hint; destinations that need durable paths should import all files.
    #[must_use]
    pub fn is_temporary(&self) -> bool {
        self.receipt.2
    }

    /// Tell the source the drop succeeded. Only the first call counts.
    pub fn finish(&self) {
        self.receipt.finish();
    }

    /// Whether the user copied or moved the files, as the source and the
    /// compositor negotiated it; `None` for files that came with no native
    /// source. Finishing a [`DragAction::Move`] lets the source delete its
    /// files, so a consumer that keeps only copies drops a Move unfinished
    /// (or mounts a [`copy_only`](EnzFileDropTarget::copy_only) target).
    #[must_use]
    pub fn action(&self) -> Option<DragAction> {
        self.receipt.1
    }
}

/// A bounded native payload with deferred destination completion.
///
/// Call [`Self::finish`] after committing the destination operation. Dropping
/// every clone unfinished refuses the transfer and preserves source ownership.
#[derive(Debug, Clone)]
pub struct DataDrop {
    data: Arc<Pasted>,
    receipt: Receipt,
}

impl DataDrop {
    /// Received MIME name and bytes. Text, URI and image interpretation is explicit.
    #[must_use]
    pub fn data(&self) -> &Pasted {
        &self.data
    }

    /// Compositor-selected copy/move action, if the transport supplied one.
    #[must_use]
    pub fn action(&self) -> Option<DragAction> {
        self.receipt.1
    }

    /// Confirm that the destination committed the data.
    pub fn finish(&self) {
        self.receipt.finish();
    }
}

enum OnDrop<'a, Message> {
    Data(Box<dyn Fn(DataDrop) -> Message + 'a>),
    /// The source hears success as soon as the paths are published.
    Paths(Box<dyn Fn(Vec<PathBuf>) -> Message + 'a>),
    /// The app settles the drop itself.
    Receipt(Box<dyn Fn(FileDrop) -> Message + 'a>),
}

/// A region of the view that turns dropped files into `on_drop(paths)`.
pub fn enz_file_drop_target<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_drop: impl Fn(Vec<PathBuf>) -> Message + 'a,
) -> EnzFileDropTarget<'a, Message, Theme, Renderer> {
    EnzFileDropTarget {
        content: content.into(),
        on_drop: OnDrop::Paths(Box::new(on_drop)),
        enabled: true,
        copy_only: false,
        preferences: crate::text_drag::file_preferences(),
        preferred: DragAction::Copy,
        highlight: true,
        key: 0,
    }
}

/// A region of the view that turns dropped files into `on_drop(drop)`,
/// leaving the source waiting until the app calls [`FileDrop::finish`].
pub fn enz_file_drop_target_with_receipt<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_drop: impl Fn(FileDrop) -> Message + 'a,
) -> EnzFileDropTarget<'a, Message, Theme, Renderer> {
    EnzFileDropTarget {
        content: content.into(),
        on_drop: OnDrop::Receipt(Box::new(on_drop)),
        enabled: true,
        copy_only: false,
        preferences: crate::text_drag::file_preferences(),
        preferred: DragAction::Copy,
        highlight: true,
        key: 0,
    }
}

/// A target for caller-selected MIME types, sharing file-target routing and feedback.
///
/// The default action is Copy only. List preferences from most to least useful;
/// the source's wire spelling is retained. An empty list refuses all drops.
pub fn enz_data_drop_target<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    preferences: impl IntoIterator<Item = MimeType>,
    on_drop: impl Fn(DataDrop) -> Message + 'a,
) -> EnzFileDropTarget<'a, Message, Theme, Renderer> {
    EnzFileDropTarget {
        content: content.into(),
        on_drop: OnDrop::Data(Box::new(on_drop)),
        enabled: true,
        copy_only: true,
        preferences: preferences.into_iter().take(64).collect(),
        preferred: DragAction::Copy,
        highlight: true,
        key: 0,
    }
}

/// See [`enz_file_drop_target`].
pub struct EnzFileDropTarget<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_drop: OnDrop<'a, Message>,
    enabled: bool,
    copy_only: bool,
    preferences: Vec<MimeType>,
    preferred: DragAction,
    highlight: bool,
    /// A hash of the app's [`key`](Self::key).
    key: u64,
}

impl<Message, Theme, Renderer> EnzFileDropTarget<'_, Message, Theme, Renderer> {
    /// Enable this target while retaining the content's focus and widget state.
    /// Disabling it retires its released and pending drops; enabling it again
    /// cannot revive an earlier operation. Child targets remain independent.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Take only copies (attaching, importing): file lists are offered as
    /// Copy alone while this target is under the pointer, and a drop that
    /// still arrives as a [`DragAction::Move`] is refused unfinished, so the
    /// source keeps its files.
    #[must_use]
    pub fn copy_only(mut self, copy_only: bool) -> Self {
        self.copy_only = copy_only;
        self
    }

    /// Preferred operation when the source permits it; otherwise its allowed operation wins.
    #[must_use]
    pub fn preferred_action(mut self, action: DragAction) -> Self {
        self.preferred = action;
        self
    }

    /// Paint the framework's accepting-target feedback (enabled by default).
    #[must_use]
    pub fn highlight(mut self, enabled: bool) -> Self {
        self.highlight = enabled;
        self
    }

    /// What this target stands for: the page, document or generation its
    /// `on_drop` callback captures. A drop released while the target had one
    /// key is refused unfinished if the target has another when the files
    /// arrive, so the callback never pairs an earlier drop with a later
    /// state. Targets without a key are bound to their place in the view
    /// alone.
    #[must_use]
    pub fn key(mut self, key: impl Hash) -> Self {
        let mut hasher = std::hash::DefaultHasher::new();
        key.hash(&mut hasher);
        self.key = hasher.finish();
        self
    }
}

impl<Message, Theme, Renderer> std::fmt::Debug for EnzFileDropTarget<'_, Message, Theme, Renderer> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnzFileDropTarget").finish_non_exhaustive()
    }
}

/// A mounted target's state: files handed to it by the drag service, which
/// it publishes on its next update.
#[derive(Debug)]
pub(crate) struct State {
    pending: Option<(Value, Receipt)>,
    enabled: bool,
    copy_only: bool,
    preferences: Vec<MimeType>,
    preferred: DragAction,
    hovered: bool,
    /// This mounted target, distinct from every other ever mounted.
    instance: u64,
    /// The key of the view that built it last.
    key: u64,
}

#[derive(Debug)]
enum Value {
    Paths(Vec<PathBuf>),
    Data(Arc<Pasted>, Vec<PathBuf>),
}

impl State {
    fn new(copy_only: bool, key: u64, preferences: Vec<MimeType>, preferred: DragAction) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            pending: None,
            enabled: true,
            copy_only,
            preferences,
            preferred,
            hovered: false,
            instance: NEXT.fetch_add(1, Ordering::Relaxed),
            key,
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EnzFileDropTarget<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
    Theme: Catalog,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        let mut state = State::new(
            self.copy_only,
            self.key,
            self.preferences.clone(),
            self.preferred,
        );
        state.enabled = self.enabled;
        tree::State::new(state)
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.key != self.key || state.enabled != self.enabled {
            *state = State::new(
                self.copy_only,
                self.key,
                self.preferences.clone(),
                self.preferred,
            );
            state.enabled = self.enabled;
        }
        state.copy_only = self.copy_only;
        state.preferences.clone_from(&self.preferences);
        state.preferred = self.preferred;
        state.key = self.key;
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.custom(
            None,
            layout.bounds().intersection(viewport).unwrap_or_default(),
            tree.state.downcast_mut::<State>(),
        );
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.state.downcast_ref::<State>().pending.is_some()
            || crate::frame_routing::interest_opaque(tree)
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
        if let Some((value, receipt)) = tree.state.downcast_mut::<State>().pending.take()
            // A copy-only target refuses a Move: dropping the receipt
            // unfinished leaves the source its files.
            && !(self.copy_only && receipt.1 == Some(DragAction::Move))
        {
            match (&self.on_drop, value) {
                (OnDrop::Data(on_drop), Value::Data(data, _)) => {
                    shell.publish(on_drop(DataDrop { data, receipt }));
                }
                (on_drop, value) => {
                    let paths = match value {
                        Value::Paths(paths) => paths,
                        Value::Data(_, paths) => paths,
                    };
                    if !paths.is_empty() {
                        match on_drop {
                            OnDrop::Paths(on_drop) => {
                                shell.publish(on_drop(paths));
                                receipt.finish();
                            }
                            OnDrop::Receipt(on_drop) => {
                                shell.publish(on_drop(FileDrop { paths, receipt }));
                            }
                            OnDrop::Data(_) => {}
                        }
                    }
                }
            }
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
        if self.enabled && self.highlight && tree.state.downcast_ref::<State>().hovered {
            let style = theme.drop_style();
            if let Some(clip) = layout.bounds().intersection(viewport) {
                renderer.with_layer(clip, |renderer| {
                    renderer.fill_quad(
                        iced::advanced::renderer::Quad {
                            bounds: layout.bounds(),
                            border: style.border,
                            ..Default::default()
                        },
                        style.fill,
                    );
                });
            }
        }
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

impl<'a, Message, Theme, Renderer> From<EnzFileDropTarget<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: Catalog + 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(target: EnzFileDropTarget<'a, Message, Theme, Renderer>) -> Self {
        Element::new(target)
    }
}

/// The target a drop was released over: which mounted target, and its key
/// at that moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Binding {
    instance: u64,
    key: u64,
}

/// Finds the innermost file drop target under a point. Parents are visited
/// before their children, so the last target containing the point is the
/// innermost.
pub(crate) struct Locate {
    position: iced::Point,
    found: Option<Binding>,
    policy: Option<Policy>,
    scroll: enzui_core::scroll_stack::ScrollStack,
    offered: Option<(
        Vec<MimeType>,
        enzui_clipboard_runtime::native_drag::DragActions,
    )>,
}

impl Locate {
    pub(crate) fn new(position: iced::Point) -> Self {
        Self {
            position,
            found: None,
            policy: None,
            scroll: Default::default(),
            offered: None,
        }
    }

    /// The target under the point, if any.
    pub(crate) fn found(&self) -> Option<Binding> {
        self.found
    }
}

/// Negotiation belongs to the concrete target under the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    pub(crate) binding: Binding,
    pub(crate) preferences: Vec<MimeType>,
    pub(crate) copy_only: bool,
    pub(crate) preferred: DragAction,
}

impl Locate {
    pub(crate) fn for_offer(
        position: iced::Point,
        mimes: Vec<MimeType>,
        actions: enzui_clipboard_runtime::native_drag::DragActions,
    ) -> Self {
        let mut locate = Self::new(position);
        locate.offered = Some((mimes, actions));
        locate
    }

    pub(crate) fn policy(&self) -> Option<Policy> {
        self.policy.clone()
    }
}

pub(crate) struct Hover {
    pub(crate) active: Vec<Binding>,
    pub(crate) changed: bool,
}
impl Operation for Hover {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
    fn custom(
        &mut self,
        _id: Option<&iced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        if let Some(target) = state.downcast_mut::<State>() {
            let hovered = self.active.contains(&Binding {
                instance: target.instance,
                key: target.key,
            });
            self.changed |= hovered != target.hovered;
            target.hovered = hovered;
        }
    }
}

impl Operation for Locate {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        self.scroll.traverse_enter();
        operate(self);
        self.scroll.traverse_exit();
    }

    fn scrollable(
        &mut self,
        id: Option<&iced::widget::Id>,
        bounds: Rectangle,
        _content: Size,
        translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        self.scroll.note_scrollable(id, bounds, translation);
    }

    fn custom(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        let bounds = self.scroll.translate(bounds);
        if let Some(target) = state.downcast_ref::<State>()
            && target.enabled
            && self.offered.as_ref().is_none_or(|(mimes, actions)| {
                (actions.allows(DragAction::Copy)
                    || (!target.copy_only && actions.allows(DragAction::Move)))
                    && target
                        .preferences
                        .iter()
                        .any(|wanted| mimes.iter().any(|mime| wanted.matches(mime)))
            })
            && bounds.width > 0.0
            && bounds.height > 0.0
            && bounds.contains(self.position)
            && self
                .scroll
                .viewport()
                .is_none_or(|clip| clip.contains(self.position))
        {
            self.found = Some(Binding {
                instance: target.instance,
                key: target.key,
            });
            self.policy = self.found.map(|binding| Policy {
                binding,
                preferences: target.preferences.clone(),
                copy_only: target.copy_only,
                preferred: target.preferred,
            });
        }
    }
}

/// Hands dropped files to the target they were released over, if it is
/// still mounted with the same key; otherwise the receipt is dropped and the
/// drop refused.
pub(crate) struct Deliver {
    binding: Binding,
    paths: Option<(Value, Receipt)>,
}

impl Deliver {
    pub(crate) fn new(binding: Binding, paths: Vec<PathBuf>, receipt: Receipt) -> Self {
        Self {
            binding,
            paths: Some((Value::Paths(paths), receipt)),
        }
    }

    pub(crate) fn new_data(
        binding: Binding,
        data: Arc<Pasted>,
        paths: Vec<PathBuf>,
        receipt: Receipt,
    ) -> Self {
        Self {
            binding,
            paths: Some((Value::Data(data, paths), receipt)),
        }
    }

    /// Whether the bound target took the files.
    pub(crate) fn delivered(&self) -> bool {
        self.paths.is_none()
    }
}

impl Operation for Deliver {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn custom(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        if let Some(target) = state.downcast_mut::<State>()
            && target.enabled
            && target.instance == self.binding.instance
            && target.key == self.binding.key
            && self.paths.as_ref().is_some_and(|(value, _)| match value {
                Value::Paths(_) => true,
                Value::Data(data, _) => target
                    .preferences
                    .iter()
                    .any(|wanted| wanted.matches(data.mime())),
            })
            && let Some((paths, receipt)) = self.paths.take()
        {
            target.pending = Some((paths, receipt));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Deliver, FileDrop, Locate, Receipt, enz_file_drop_target, enz_file_drop_target_with_receipt,
    };
    use iced::advanced::shell::Bus;
    use iced::{Element, Event, Length, Point, Size, mouse, window};
    use iced_test::runtime::user_interface::{Cache, UserInterface};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Outer(Vec<PathBuf>),
        Inner(Vec<PathBuf>),
    }

    /// An outer target filling 400×300 with an inner 100×100 target at its
    /// top-left corner; drops `paths` at `at` and returns what was published.
    fn drop_at(at: Point, paths: Vec<PathBuf>) -> (bool, Vec<Msg>) {
        let inner = enz_file_drop_target(
            iced::widget::Space::new().width(100).height(100),
            Msg::Inner,
        );
        let view: Element<'_, Msg> = enz_file_drop_target(
            iced::widget::container(inner)
                .width(Length::Fill)
                .height(Length::Fill),
            Msg::Outer,
        )
        .into();
        deliver(view, at, paths, Receipt::of(None))
    }

    /// Drops `paths` with `receipt` at `at` over `view`; returns whether a
    /// target was there and what it published.
    fn deliver<M: Clone>(
        view: Element<'_, M>,
        at: Point,
        paths: Vec<PathBuf>,
        receipt: Receipt,
    ) -> (bool, Vec<M>) {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .expect("headless renderer");
        let mut ui =
            UserInterface::build(view, Size::new(400.0, 300.0), Cache::new(), &mut renderer);
        let mut locate = Locate::new(at);
        ui.operate(&renderer, &mut locate);
        let Some(binding) = locate.found() else {
            return (false, vec![]);
        };
        let mut deliver = Deliver::new(binding, paths, receipt);
        ui.operate(&renderer, &mut deliver);
        assert!(deliver.delivered(), "the located target takes the files");
        let mut messages = Bus::new();
        let frame = Event::Window(window::Event::RedrawRequested(iced::time::Instant::now()));
        let _ = ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[frame],
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut messages,
        );
        (true, messages.messages().to_vec())
    }

    #[test]
    fn file_drop_delivers_to_the_target_with_the_negotiated_action_1() {
        // dropped_files_reach_the_innermost_target_under_the_point
        {
            let paths = vec![
                PathBuf::from("/home/user/a.png"),
                PathBuf::from("/home/user/b.png"),
            ];
            assert_eq!(
                drop_at(Point::new(50.0, 50.0), paths.clone()),
                (true, vec![Msg::Inner(paths.clone())]),
                "inside both: the inner target"
            );
            assert_eq!(
                drop_at(Point::new(300.0, 200.0), paths.clone()),
                (true, vec![Msg::Outer(paths.clone())]),
                "outside the inner one: the outer target"
            );
            assert_eq!(
                drop_at(Point::new(500.0, 500.0), paths),
                (false, vec![]),
                "outside both: no target, nothing published"
            );
        }

        // Negotiation follows visible, compatible targets after scrolling;
        // a nested image target must not swallow a URI drop for its parent.
        {
            use enzui_clipboard::{MimeType, Pasted};
            use enzui_clipboard_runtime::native_drag::DragActions;
            use iced::advanced::widget::{Id, operation};
            let mut renderer = iced_test::futures::futures::executor::block_on(
                <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                    iced::advanced::renderer::Settings::default(),
                    None,
                ),
            )
            .expect("headless renderer");
            let inner = super::enz_data_drop_target(
                iced::widget::Space::new().width(100).height(80),
                [MimeType::from("image/png")],
                |_| Msg::Inner(vec![]),
            )
            .key("image");
            let scrolled = iced::widget::scrollable(iced::widget::column![
                iced::widget::Space::new().height(200),
                inner,
                iced::widget::Space::new().height(400),
            ])
            .id("drop-scroll")
            .width(200)
            .height(100);
            let view: Element<'_, Msg> = enz_file_drop_target(
                iced::widget::container(scrolled)
                    .width(Length::Fill)
                    .height(Length::Fill),
                Msg::Outer,
            )
            .key("files")
            .into();
            let mut ui =
                UserInterface::build(view, Size::new(400.0, 300.0), Cache::new(), &mut renderer);
            ui.operate(
                &renderer,
                &mut operation::scrollable::scroll_to(
                    Id::new("drop-scroll"),
                    operation::scrollable::AbsoluteOffset {
                        x: None,
                        y: Some(200.0),
                    },
                    operation::Animation::Instant,
                ),
            );
            let locate = |ui: &mut UserInterface<'_, Msg, iced::Theme, iced::Renderer>,
                          at,
                          mime: &str,
                          actions| {
                let mut locate = Locate::for_offer(at, vec![MimeType::from(mime)], actions);
                ui.operate(&renderer, &mut locate);
                locate.policy()
            };
            let at = Point::new(20.0, 20.0);
            let image =
                locate(&mut ui, at, "image/png", DragActions::COPY).expect("visible image target");
            let files = locate(&mut ui, at, "text/uri-list", DragActions::BOTH)
                .expect("parent accepts files");
            assert_ne!(image.binding, files.binding);
            assert!(image.copy_only);
            assert!(!files.copy_only);
            assert!(
                locate(
                    &mut ui,
                    Point::new(20.0, 150.0),
                    "image/png",
                    DragActions::COPY
                )
                .is_none(),
                "offscreen image area is clipped"
            );
            assert!(
                locate(&mut ui, at, "image/png", DragActions::MOVE).is_none(),
                "copy-only child refuses Move"
            );
            assert!(
                locate(&mut ui, at, "text/uri-list", DragActions::NONE).is_none(),
                "no source operation means no accepting target"
            );
            let (receipt, _, _) = watched();
            let mut delivery = Deliver::new_data(
                image.binding,
                Arc::new(Pasted::new(
                    MimeType::from("text/uri-list"),
                    b"file:///tmp/image.png\r\n".to_vec(),
                )),
                vec!["/tmp/image.png".into()],
                receipt,
            );
            ui.operate(&renderer, &mut delivery);
            assert!(
                !delivery.delivered(),
                "late MIME changes cannot cross the bound target"
            );
        }

        // a_paths_target_tells_the_source_at_once
        {
            let (receipt, finished, _) = watched();
            let view: Element<'_, Vec<PathBuf>> = enz_file_drop_target(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
                |paths| paths,
            )
            .into();
            let (found, published) =
                deliver(view, Point::new(10.0, 10.0), vec!["/a".into()], receipt);
            assert!(found);
            assert_eq!(published, vec![vec![PathBuf::from("/a")]]);
            assert!(finished.load(Ordering::SeqCst));
        }

        // a_receipt_target_leaves_the_source_waiting_until_the_app_finishes
        {
            let target = || -> Element<'static, FileDrop> {
                enz_file_drop_target_with_receipt(
                    iced::widget::Space::new()
                        .width(Length::Fill)
                        .height(Length::Fill),
                    |drop| drop,
                )
                .into()
            };

            let (receipt, finished, _) = watched();
            let (_, published) =
                deliver(target(), Point::new(10.0, 10.0), vec!["/a".into()], receipt);
            let [drop] = published.as_slice() else {
                panic!("one drop published: {published:?}");
            };
            assert_eq!(drop.paths(), [PathBuf::from("/a")]);
            assert!(!finished.load(Ordering::SeqCst), "staging has not run yet");
            drop.clone().finish();
            assert!(finished.load(Ordering::SeqCst), "the app finished it");

            let (receipt, finished, released) = watched();
            let (_, published) =
                deliver(target(), Point::new(10.0, 10.0), vec!["/b".into()], receipt);
            assert!(!released.load(Ordering::SeqCst), "the app still holds it");
            drop_all(published);
            assert!(
                released.load(Ordering::SeqCst) && !finished.load(Ordering::SeqCst),
                "dropped unfinished: the source hears no success"
            );
        }

        // a_copy_only_target_refuses_a_move_and_takes_a_copy
        {
            use super::DragAction;
            let target = || -> Element<'static, FileDrop> {
                enz_file_drop_target_with_receipt(
                    iced::widget::Space::new()
                        .width(Length::Fill)
                        .height(Length::Fill),
                    |drop| drop,
                )
                .copy_only(true)
                .into()
            };
            let (receipt, finished, released) = watched();
            let (found, published) = deliver(
                target(),
                Point::new(10.0, 10.0),
                vec!["/a".into()],
                receipt.with_action(DragAction::Move),
            );
            assert!(found);
            assert!(
                published.is_empty(),
                "a moved drop is not handed to the app"
            );
            assert!(
                released.load(Ordering::SeqCst) && !finished.load(Ordering::SeqCst),
                "the move is refused unfinished, so the source keeps its files"
            );

            let (receipt, _, _) = watched();
            let (_, published) = deliver(
                target(),
                Point::new(10.0, 10.0),
                vec!["/a".into()],
                receipt.with_action(DragAction::Copy),
            );
            assert_eq!(published.len(), 1, "a copy is taken");
            assert_eq!(published[0].action(), Some(DragAction::Copy));
        }
    }

    /// A receipt that records whether the source heard success, and whether
    /// it was let go without it.
    fn watched() -> (Receipt, Arc<AtomicBool>, Arc<AtomicBool>) {
        struct Released(Arc<AtomicBool>);
        impl Drop for Released {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let finished = Arc::new(AtomicBool::new(false));
        let released = Arc::new(AtomicBool::new(false));
        let guard = Released(Arc::clone(&released));
        let on_finish = Arc::clone(&finished);
        let receipt = Receipt::new(move || {
            let _guard = guard;
            on_finish.store(true, Ordering::SeqCst);
        });
        (receipt, finished, released)
    }

    fn drop_all(published: Vec<FileDrop>) {
        drop(published);
    }

    /// A drop released over a target is refused if, by the time its files
    /// arrive, the target was replaced or carries another key; it reaches
    /// the target when neither changed.
    #[test]
    fn a_drop_reaches_only_the_target_it_was_released_over() {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .expect("headless renderer");
        let page = |key: u64, boxed: bool| -> Element<'static, (u64, FileDrop)> {
            let target = enz_file_drop_target_with_receipt(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
                move |drop| (key, drop),
            )
            .key(key);
            if boxed {
                iced::widget::container(target).into()
            } else {
                target.into()
            }
        };
        let at = Point::new(10.0, 10.0);
        let size = Size::new(400.0, 300.0);
        // Released over the target of page 0; by the time the files arrive
        // the app shows `then`. Returns what was published and whether the
        // source was let go unfinished.
        let mut drop_then = |then: Element<'static, (u64, FileDrop)>| {
            let mut ui = UserInterface::build(page(0, false), size, Cache::new(), &mut renderer);
            let mut locate = Locate::new(at);
            ui.operate(&renderer, &mut locate);
            let binding = locate.found().expect("a target under the point");
            let mut ui = UserInterface::build(then, size, ui.into_cache(), &mut renderer);
            let (receipt, finished, released) = watched();
            let mut deliver = Deliver::new(binding, vec!["/a".into()], receipt);
            ui.operate(&renderer, &mut deliver);
            drop(deliver);
            let mut messages = Bus::new();
            let frame = Event::Window(window::Event::RedrawRequested(iced::time::Instant::now()));
            let _ = ui.update(
                &iced::advanced::graphics::core::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[frame],
                mouse::Cursor::Unavailable,
                &mut renderer,
                &mut messages,
            );
            let published: Vec<u64> = messages.messages().iter().map(|(key, _)| *key).collect();
            let refused = released.load(Ordering::SeqCst) && !finished.load(Ordering::SeqCst);
            (published, refused)
        };
        assert_eq!(drop_then(page(0, false)), (vec![0], false), "unchanged");
        assert_eq!(
            drop_then(page(1, false)),
            (vec![], true),
            "same widget, another key: refused"
        );
        assert_eq!(
            drop_then(page(0, true)),
            (vec![], true),
            "replaced by another target with the same key: refused"
        );
    }
}
