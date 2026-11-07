//! Retain a subtree’s elements and layout while its inputs remain unchanged.
//!
//! Store a [`SubtreeHandle`] in application state and call
//! [`SubtreeHandle::invalidate`] when data captured by the view closure changes.
//! The closure must return an owned `Element<'static, ..>`. Changing a boundary’s
//! identity, revision, style generation or global font generation rebuilds it.
//! Renderer text defaults and layout limits participate in layout caching.
//! Draw, input, focus, accessibility and overlays still traverse the retained element.
//! Child layout or widget invalidation drops the retained layout entry.
//!
//! Attach nested handles to their containing boundary with [`SubtreeHandle::attach_to`]
//! so invalidation reaches warm ancestors. Unattached nesting increments
//! [`SubtreeCacheStats::unattached_nestings`] and panics in debug builds. Inherited
//! inputs such as theme, locale or image generation require explicit invalidation
//! or [`EnzCachedSubtree::style_generation`]; the latter does not invalidate ancestors.
//!
//! Each live boundary retains one element and one layout entry, released with its
//! widget tree. [`stats`] exposes retention counters. [`set_cache_bypass`] or
//! `ENZUI_CACHED_SUBTREE_BYPASS=1` disables cache hits. Paint is not retained;
//! [`crate::virtual_list`] and [`crate::virtual_grid`] separately bound realization.

use std::cell::{Cell, RefCell};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::{Rc, Weak};

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer, text};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// The stable identity of a cached subtree boundary.
///
/// Part of the cache key: a boundary that keeps its tree position but changes
/// its identity is a miss, never a stale hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubtreeId(u64);

impl SubtreeId {
    /// Names a boundary. Equal names are the same identity.
    pub fn new(name: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        name.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// The raw identity value (diagnostics).
    pub fn value(self) -> u64 {
        self.0
    }
}

// Bound ancestor traversal even if a malformed chain bypasses attachment checks.
const MAX_ANCESTOR_DEPTH: usize = 64;

#[derive(Debug)]
struct SubtreeNode {
    id: SubtreeId,
    revision: Cell<u64>,
    parent: RefCell<Weak<SubtreeNode>>,
}

impl Drop for SubtreeNode {
    fn drop(&mut self) {
        STATS.with(|s| s.live_handles.set(s.live_handles.get().saturating_sub(1)));
    }
}

/// The invalidation token of a cached subtree: application state, not view
/// state.
///
/// Cloning a handle shares the same revision (it is an `Rc`), so the copy the
/// app keeps and the copy `view` passes to [`enz_cached_subtree`] are the same
/// boundary.
#[derive(Debug, Clone)]
pub struct SubtreeHandle(Rc<SubtreeNode>);

impl SubtreeHandle {
    /// Creates a handle at revision 0.
    pub fn new(id: SubtreeId) -> Self {
        STATS.with(|s| s.live_handles.set(s.live_handles.get() + 1));

        Self(Rc::new(SubtreeNode {
            id,
            revision: Cell::new(0),
            parent: RefCell::new(Weak::new()),
        }))
    }

    /// The identity of this boundary.
    pub fn id(&self) -> SubtreeId {
        self.0.id
    }

    /// The current revision.
    pub fn revision(&self) -> u64 {
        self.0.revision.get()
    }

    /// Marks this subtree dirty and every ancestor it is attached to,
    /// so a pane that contains this one also rebuilds while unrelated
    /// siblings stay warm.
    pub fn invalidate(&self) {
        let mut node = Rc::clone(&self.0);
        let mut depth = 0;

        loop {
            node.revision.set(node.revision.get().wrapping_add(1));

            let parent = node.parent.borrow().upgrade();
            match parent {
                Some(parent) if depth < MAX_ANCESTOR_DEPTH => {
                    node = parent;
                    depth += 1;
                }
                _ => break,
            }
        }
    }

    /// Attaches this boundary to a containing one, so [`invalidate`] propagates
    /// upwards. Returns `false` (and changes nothing) if `parent` is this
    /// handle or already a descendant of it — a cycle would make invalidation
    /// non-terminating.
    ///
    /// [`invalidate`]: Self::invalidate
    pub fn attach_to(&self, parent: &SubtreeHandle) -> bool {
        if Rc::ptr_eq(&self.0, &parent.0) || self.is_ancestor_of(parent) {
            return false;
        }

        *self.0.parent.borrow_mut() = Rc::downgrade(&parent.0);
        true
    }

    /// Detaches this boundary from its parent, if any.
    pub fn detach(&self) {
        *self.0.parent.borrow_mut() = Weak::new();
    }

    fn is_ancestor_of(&self, other: &SubtreeHandle) -> bool {
        let mut node = Rc::clone(&other.0);

        for _ in 0..MAX_ANCESTOR_DEPTH {
            if Rc::ptr_eq(&node, &self.0) {
                return true;
            }

            let parent = node.parent.borrow().upgrade();
            match parent {
                Some(parent) => node = parent,
                None => return false,
            }
        }

        true
    }
}

#[derive(Debug, Default)]
struct Counters {
    builds: Cell<u64>,
    build_hits: Cell<u64>,
    diff_skips: Cell<u64>,
    layout_builds: Cell<u64>,
    layout_hits: Cell<u64>,
    layout_drops: Cell<u64>,
    live_caches: Cell<u64>,
    live_handles: Cell<u64>,
    retained_nodes: Cell<u64>,
    unattached_nestings: Cell<u64>,
    bypass: Cell<Option<bool>>,
    nesting_check: Cell<Option<bool>>,
}

thread_local! {
    static STATS: Counters = Counters::default();

    // The stack of boundaries whose `view` closure is currently
    // executing on this thread. A nested [`enz_cached_subtree`]
    // constructed while this is non-empty was built inside the
    // boundary on top, which is the one datum the attach-to footgun
    // check needs: a child built inside a parent that is not attached
    // to it will be frozen when the parent is warm (the parent
    // short-circuits `diff`/`layout` and never descends), so the
    // child's own [`SubtreeHandle::invalidate`] reaches nothing and
    // the parent serves a stale copy of it. Recorded here rather than
    // inferred from the widget tree because the boundary has no view
    // of its ancestors.
    static BUILDING: RefCell<Vec<Rc<SubtreeNode>>> = const { RefCell::new(Vec::new()) };
}

// Pops the [`BUILDING`] stack even if the `view` closure unwinds, so
// a panic inside a build (including the footgun `debug_assert!`
// itself) cannot leave a stale parent on the stack to mis-attribute
// the next boundary.
struct BuildGuard;

impl Drop for BuildGuard {
    fn drop(&mut self) {
        BUILDING.with(|b| {
            b.borrow_mut().pop();
        });
    }
}

/// A snapshot of the cache counters for the current thread.
///
/// These are the numbers the performance gates assert on: they are mechanism
/// counts, not timings, so they are profile-independent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SubtreeCacheStats {
    /// Times a `view` closure was invoked (cold builds).
    pub builds: u64,
    /// Times a build was served from the retained element.
    pub build_hits: u64,
    /// Times reconciliation of the subtree was skipped entirely.
    pub diff_skips: u64,
    /// Times the subtree was laid out.
    pub layout_builds: u64,
    /// Times a layout was replayed from the retained geometry.
    pub layout_hits: u64,
    /// Times a retained layout was dropped by an event-driven invalidation.
    pub layout_drops: u64,
    /// Boundaries currently holding a retained element.
    pub live_caches: u64,
    /// Live [`SubtreeHandle`] identities.
    pub live_handles: u64,
    /// Total widget geometries whose layout live boundaries retain.
    pub retained_nodes: u64,
    /// Times a nested boundary was built inside another boundary without
    /// being [attached](SubtreeHandle::attach_to) to it. Every count is
    /// a latent stale-outer-pane bug: the child's invalidation cannot reach the
    /// parent, so a warm parent serves a frozen copy of the child. Also fired,
    /// in a debug build, as a `debug_assert!` at the moment of construction.
    pub unattached_nestings: u64,
}

/// Reads the cache counters of the current thread.
pub fn stats() -> SubtreeCacheStats {
    STATS.with(|s| SubtreeCacheStats {
        builds: s.builds.get(),
        build_hits: s.build_hits.get(),
        diff_skips: s.diff_skips.get(),
        layout_builds: s.layout_builds.get(),
        layout_hits: s.layout_hits.get(),
        layout_drops: s.layout_drops.get(),
        live_caches: s.live_caches.get(),
        live_handles: s.live_handles.get(),
        retained_nodes: s.retained_nodes.get(),
        unattached_nestings: s.unattached_nestings.get(),
    })
}

/// Resets the per-frame counters (`builds`, `build_hits`, `diff_skips`,
/// `layout_*`). The liveness gauges (`live_caches`, `live_handles`,
/// `retained_nodes`) are not reset: they are levels, not events.
pub fn reset_stats() {
    STATS.with(|s| {
        s.builds.set(0);
        s.build_hits.set(0);
        s.diff_skips.set(0);
        s.layout_builds.set(0);
        s.layout_hits.set(0);
        s.layout_drops.set(0);
        s.unattached_nestings.set(0);
    });
}

/// Forces every boundary on this thread to miss, so an inspector, a debug
/// overlay or a pixel-diff control walks the real tree.
///
/// `ENZUI_CACHED_SUBTREE_BYPASS=1` does the same for a whole process run.
pub fn set_cache_bypass(bypass: bool) {
    STATS.with(|s| s.bypass.set(Some(bypass)));
}

fn cache_bypassed() -> bool {
    STATS.with(|s| match s.bypass.get() {
        Some(bypass) => bypass,
        None => {
            let from_env = std::env::var("ENZUI_CACHED_SUBTREE_BYPASS")
                .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
            s.bypass.set(Some(from_env));
            from_env
        }
    })
}

/// Enables or disables the nested-boundary attach-to check on this thread.
///
/// The check is on by default: constructing a nested
/// [`enz_cached_subtree`] that is not
/// [attached](SubtreeHandle::attach_to) to the boundary it is built
/// inside increments [`SubtreeCacheStats::unattached_nestings`] and,
/// in a debug build, trips a `debug_assert!`. Turning it off is for
/// the one caller that must observe the raw stale-outer-pane
/// behaviour the check exists to flag
/// (the nested-boundary defect gate), the same way [`set_cache_bypass`] lets a test see
/// the uncached tree. `ENZUI_CACHED_SUBTREE_NESTING_CHECK=0` does the same for
/// a whole process run.
pub fn set_nesting_check(enabled: bool) {
    STATS.with(|s| s.nesting_check.set(Some(enabled)));
}

fn nesting_check_enabled() -> bool {
    STATS.with(|s| match s.nesting_check.get() {
        Some(enabled) => enabled,
        None => {
            // Default ON; only an explicit `0`/`false` disables it, so a typo in
            // the variable name fails safe (the check stays on).
            let from_env = std::env::var("ENZUI_CACHED_SUBTREE_NESTING_CHECK")
                .map_or(true, |v| !(v == "0" || v.eq_ignore_ascii_case("false")));
            s.nesting_check.set(Some(from_env));
            from_env
        }
    })
}

// True when `ancestor` lies on `child`'s attach-to chain — i.e. an
// [`invalidate`](SubtreeHandle::invalidate) of `child` would reach
// `ancestor`. Bounded by [`MAX_ANCESTOR_DEPTH`] so a malformed chain
// cannot hang the check.
fn attaches_to(child: &Rc<SubtreeNode>, ancestor: &Rc<SubtreeNode>) -> bool {
    let mut node = Rc::clone(child);
    for _ in 0..MAX_ANCESTOR_DEPTH {
        let parent = node.parent.borrow().upgrade();
        match parent {
            Some(parent) => {
                if Rc::ptr_eq(&parent, ancestor) {
                    return true;
                }
                node = parent;
            }
            None => return false,
        }
    }
    false
}

// The attach-to footgun check, run when a boundary is constructed.
//
// If this boundary is being built inside another (the [`BUILDING`]
// stack is non-empty) and it is not attached to that enclosing
// boundary, its invalidation can never reach the enclosing one, so a
// warm parent will serve a stale copy of it. That is recorded as an
// [`unattached_nestings`](SubtreeCacheStats::unattached_nestings)
// count (so a release build and a test can both observe it) and, in
// a debug build, tripped as a `debug_assert!` with an actionable
// message.
fn check_nesting(handle: &SubtreeHandle) {
    if !nesting_check_enabled() {
        return;
    }
    let parent = BUILDING.with(|b| b.borrow().last().cloned());
    let Some(parent) = parent else {
        return; // top-level boundary: nothing encloses it.
    };
    if attaches_to(&handle.0, &parent) {
        return; // correctly attached: invalidation will propagate upward.
    }
    STATS.with(|s| s.unattached_nestings.set(s.unattached_nestings.get() + 1));
    debug_assert!(
        false,
        "cached subtree {:?} was built inside boundary {:?} but is not attached \
         to it. Call `child.attach_to(&parent)` in application state, or the \
         child's `invalidate()` will not reach the parent and a warm parent \
         will serve a stale copy of the child.",
        handle.id().value(),
        parent.id.value(),
    );
}

/// The widget geometries a retained layout keeps: the laid out subtree's
/// trees, which hold it since layout moved into the widget tree.
fn count_nodes(tree: &Tree) -> u64 {
    1 + tree.children.iter().map(count_nodes).sum::<u64>()
}

// The global font generation: every font load bumps it, and every
// retained layout depends on it because text measurement can change. The
// published text defaults (families, weights, desktop size) and the text
// scale are read when the subtree's elements are built, so a change to
// either also rebuilds it; otherwise a list cached before the theme arrived
// kept the fallback font.
fn font_generation() -> u64 {
    let version = iced::advanced::graphics::text::font_system()
        .read()
        .map(|system| system.version())
        .ok();

    let mut hasher = DefaultHasher::new();
    version.hash(&mut hasher);
    enzui_theme::text_defaults::generation().hash(&mut hasher);
    enzui_theme::text_scale::published()
        .factor()
        .to_bits()
        .hash(&mut hasher);
    hasher.finish()
}

// The inherited text inputs a subtree's layout depends on.
// `font: None` means "not observed yet" (there is no renderer in
// `Widget::state`), which never compares equal to an observed one,
// so the first `layout` always measures.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TextDefaults<Font> {
    font: Option<Font>,
    size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct BuildKey<Font> {
    id: SubtreeId,
    revision: u64,
    style_generation: u64,
    font_generation: u64,
    text: TextDefaults<Font>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct LayoutKey<Font> {
    build: BuildKey<Font>,
    limits: [u32; 4],
}

fn limits_key(limits: &layout::Limits) -> [u32; 4] {
    [
        limits.min().width.to_bits(),
        limits.min().height.to_bits(),
        limits.max().width.to_bits(),
        limits.max().height.to_bits(),
    ]
}

struct Cached<Message, Theme, Renderer: text::Renderer> {
    element: Element<'static, Message, Theme, Renderer>,
    key: BuildKey<iced::Font>,
    /// The key and size of the retained layout; the geometry below it stays
    /// in the retained subtree's tree.
    layout: Option<(LayoutKey<iced::Font>, Size)>,
    retained_nodes: u64,
}

impl<Message, Theme, Renderer: text::Renderer> Cached<Message, Theme, Renderer> {
    fn release_layout(&mut self) {
        if self.layout.take().is_some() {
            STATS.with(|s| {
                s.retained_nodes
                    .set(s.retained_nodes.get().saturating_sub(self.retained_nodes));
                s.layout_drops.set(s.layout_drops.get() + 1);
            });
            self.retained_nodes = 0;
        }
    }

    fn retain_layout(&mut self, key: LayoutKey<iced::Font>, size: Size, subtree: &Tree) {
        self.release_layout();

        let nodes = count_nodes(subtree);
        self.retained_nodes = nodes;
        self.layout = Some((key, size));
        STATS.with(|s| s.retained_nodes.set(s.retained_nodes.get() + nodes));
    }
}

impl<Message, Theme, Renderer: text::Renderer> Drop for Cached<Message, Theme, Renderer> {
    fn drop(&mut self) {
        STATS.with(|s| {
            s.live_caches.set(s.live_caches.get().saturating_sub(1));
            s.retained_nodes
                .set(s.retained_nodes.get().saturating_sub(self.retained_nodes));
        });
    }
}

/// A retained subtree boundary. Build it with [`enz_cached_subtree`].
#[must_use]
pub struct EnzCachedSubtree<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    handle: SubtreeHandle,
    view: Box<dyn Fn() -> Element<'static, Message, Theme, Renderer> + 'a>,
    style_generation: u64,
    size: Rc<Cell<Size<Length>>>,
}

/// Wraps `view` in a retained subtree boundary owned by `handle`.
///
/// The closure returns owned elements and runs when the boundary is first built
/// or its identity, revision, style generation or global font generation changes.
/// Limits and renderer text defaults invalidate layout without rebuilding elements.
/// See the [module docs](self) for invalidation and nesting requirements.
pub fn enz_cached_subtree<'a, Message, Theme, Renderer>(
    handle: &SubtreeHandle,
    view: impl Fn() -> Element<'static, Message, Theme, Renderer> + 'a,
) -> EnzCachedSubtree<'a, Message, Theme, Renderer> {
    // If this boundary is being built inside another one, it must be
    // attached to it or its invalidation is lost behind the warm
    // parent with no visible failure. Checked here, at construction,
    // because this is the exact moment the enclosing boundary's
    // `view` closure runs (see `EnzCachedSubtree::build`).
    check_nesting(handle);
    EnzCachedSubtree {
        handle: handle.clone(),
        view: Box::new(view),
        style_generation: 0,
        size: Rc::new(Cell::new(Size::new(Length::Shrink, Length::Shrink))),
    }
}

impl<'a, Message, Theme, Renderer: text::Renderer> EnzCachedSubtree<'a, Message, Theme, Renderer> {
    /// Build the retained subtree element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer>
    where
        Message: 'static,
        Theme: 'static,
        Renderer: 'static,
    {
        Element::from(self)
    }

    /// Declares the generation of every inherited input the closure reads that
    /// the boundary cannot see for itself: the theme, an inherited text style,
    /// an image generation, a locale.
    ///
    /// Changing it invalidates this boundary only (it is not propagated to
    /// ancestors — use [`SubtreeHandle::invalidate`] for that).
    pub fn style_generation(mut self, generation: u64) -> Self {
        self.style_generation = generation;
        self
    }

    fn build_key(&self, text: TextDefaults<iced::Font>) -> BuildKey<iced::Font> {
        BuildKey {
            id: self.handle.id(),
            revision: self.handle.revision(),
            style_generation: self.style_generation,
            font_generation: font_generation(),
            text,
        }
    }

    fn build(&self, key: BuildKey<iced::Font>) -> Cached<Message, Theme, Renderer> {
        // Mark this boundary as the currently-building parent for the
        // duration of its `view` closure, so any nested `enz_cached_subtree`
        // constructed inside it can check it is attached. The guard pops the
        // stack even if the closure (or the footgun assert) unwinds.
        BUILDING.with(|b| b.borrow_mut().push(Rc::clone(&self.handle.0)));
        let _guard = BuildGuard;
        let element = (self.view)();
        self.size.set(element.as_widget().size());

        STATS.with(|s| {
            s.builds.set(s.builds.get() + 1);
            s.live_caches.set(s.live_caches.get() + 1);
        });

        Cached {
            element,
            key,
            layout: None,
            retained_nodes: 0,
        }
    }
}

fn text_defaults<Renderer: text::Renderer>(renderer: &Renderer) -> TextDefaults<iced::Font> {
    TextDefaults {
        font: Some(renderer.font()),
        size: renderer.text_size().0.to_bits(),
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EnzCachedSubtree<'_, Message, Theme, Renderer>
where
    Message: 'static,
    Theme: 'static,
    Renderer: text::Renderer + 'static,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Cached<Message, Theme, Renderer>>()
    }

    fn state(&self) -> tree::State {
        // No renderer here: the text defaults enter the key at the first
        // `layout`, which is why `BuildKey::text` is checked there too.
        let key = self.build_key(TextDefaults {
            font: None,
            size: 0,
        });
        tree::State::new(self.build(key))
    }

    fn size(&self) -> Size<Length> {
        self.size.get()
    }

    fn diff(&mut self, tree: &mut Tree) {
        let Tree {
            state, children, ..
        } = tree;
        let cached = state.downcast_mut::<Cached<Message, Theme, Renderer>>();
        let key = self.build_key(cached.key.text);

        if cache_bypassed() || cached.key != key {
            cached.release_layout();
            let rebuilt = self.build(key);
            *cached = rebuilt;
            children.clear();
        } else {
            self.size.set(cached.element.as_widget().size());
            STATS.with(|s| s.build_hits.set(s.build_hits.get() + 1));

            if !children.is_empty() {
                // The retained element is the same object it was last frame, so
                // there is nothing to reconcile: this is the O(1) path, and it
                // is the one that makes an unchanged pane cost the dirty path
                // instead of its own descendant count.
                STATS.with(|s| s.diff_skips.set(s.diff_skips.get() + 1));
                return;
            }
        }

        children.push(Tree::new(cached.element.as_widget()));
        children[0].diff(cached.element.as_widget_mut());
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let Tree {
            state, children, ..
        } = tree;
        let cached = state.downcast_mut::<Cached<Message, Theme, Renderer>>();

        let key = LayoutKey {
            build: BuildKey {
                text: text_defaults(renderer),
                ..cached.key
            },
            limits: limits_key(limits),
        };

        if !cache_bypassed()
            && let Some((retained, size)) = &cached.layout
            && *retained == key
        {
            // The subtree's geometry is still in its tree.
            STATS.with(|s| s.layout_hits.set(s.layout_hits.get() + 1));
            tree.size = *size;
            return;
        }

        cached.key.text = key.build.text;

        cached
            .element
            .as_widget_mut()
            .layout(&mut children[0], renderer, limits);
        let size = children[0].size;

        STATS.with(|s| s.layout_builds.set(s.layout_builds.get() + 1));

        if cache_bypassed() {
            cached.release_layout();
        } else {
            cached.retain_layout(key, size, &children[0]);
        }

        tree.size = size;
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
        let cached = tree
            .state
            .downcast_ref::<Cached<Message, Theme, Renderer>>();

        cached.element.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.first().is_some_and(Tree::has_frame_interest)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let Tree {
            state, children, ..
        } = tree;
        let cached = state.downcast_mut::<Cached<Message, Theme, Renderer>>();

        cached.element.as_widget_mut().operate(
            &mut children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
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
        let Tree {
            state, children, ..
        } = tree;
        let cached = state.downcast_mut::<Cached<Message, Theme, Renderer>>();

        let ((), own) = shell.scoped_invalidation(|shell| {
            cached.element.as_widget_mut().update(
                &mut children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        });
        children[0].refresh_frame_interest(cached.element.as_widget());

        // The subtree asked the runtime for a new layout: the retained one is
        // stale by its own admission, whatever a sibling asked for first.
        if matches!(
            own,
            iced::advanced::shell::Invalidation::Layout(_)
                | iced::advanced::shell::Invalidation::Widgets
        ) {
            cached.release_layout();
        }
        tree.refresh_frame_child(0);
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let Tree {
            state, children, ..
        } = tree;
        let cached = state.downcast_mut::<Cached<Message, Theme, Renderer>>();

        let ((), own) = shell.scoped_invalidation(|shell| {
            cached.element.as_widget_mut().update_frame(
                &mut children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        });
        children[0].refresh_frame_interest(cached.element.as_widget());

        if matches!(
            own,
            iced::advanced::shell::Invalidation::Layout(_)
                | iced::advanced::shell::Invalidation::Widgets
        ) {
            cached.release_layout();
        }
        tree.refresh_frame_child(0);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        let cached = tree
            .state
            .downcast_ref::<Cached<Message, Theme, Renderer>>();

        cached.element.as_widget().mouse_interaction(
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
        let Tree {
            state, children, ..
        } = tree;
        let cached = state.downcast_mut::<Cached<Message, Theme, Renderer>>();

        let Some(child) = children.first_mut() else {
            return Vec::new();
        };

        cached.element.as_widget_mut().overlay(
            child,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<EnzCachedSubtree<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'static,
    Theme: 'static,
    Renderer: text::Renderer + 'static,
{
    fn from(subtree: EnzCachedSubtree<'a, Message, Theme, Renderer>) -> Self {
        Element::new(subtree)
    }
}
