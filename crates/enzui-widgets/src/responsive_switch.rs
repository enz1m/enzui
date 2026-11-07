//! Select wide or compact content using measured trailing-control width.
//!
//! Every layout measures the probe with unbounded limits. Compact content is
//! chosen only with trailing controls and a positive, finite available width
//! more than 0.01 px below the computed breakpoint. Other cases select wide.
//!
//! Normal paint, input, operations and overlays reach only the selected branch.
//! Branch changes also measure and operate the outgoing branch to release focus.
//! Named owners transfer to the first incoming focusable with the same ID;
//! anonymous owners transfer by ordinal among anonymous focusables. Callers must
//! preserve that anonymous ordering across arrangements or supply stable IDs.
//! Without a match, outgoing focus remains released. All children still receive diff.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::operation::Focusable;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

#[derive(Debug, Default)]
struct SwitchState {
    compact: bool,
}

#[derive(Debug, Clone)]
struct FocusOwner {
    id: Option<iced::advanced::widget::Id>,
    ordinal: usize,
}

#[derive(Debug, Default)]
struct FindFocusOwner {
    next: usize,
    owner: Option<FocusOwner>,
}

impl Operation for FindFocusOwner {
    fn focusable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if self.owner.is_none() && state.is_focused() {
            self.owner = Some(FocusOwner {
                id: id.cloned(),
                ordinal: self.next,
            });
        }
        if id.is_none() {
            self.next += 1;
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
}

struct InstallFocusOwner<'a> {
    owner: &'a FocusOwner,
    next: usize,
    match_id: bool,
    installed: bool,
}

impl Operation for InstallFocusOwner<'_> {
    fn focusable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        let matches = if self.match_id {
            self.owner
                .id
                .as_ref()
                .is_some_and(|target| id == Some(target))
        } else {
            // Named controls must not shift an anonymous owner’s matching ordinal.
            id.is_none() && self.next == self.owner.ordinal
        };

        if matches && !self.installed {
            state.focus();
            self.installed = true;
        } else {
            state.unfocus();
        }
        if id.is_none() {
            self.next += 1;
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
}

/// Builder for measured wide/compact branches with focus handoff.
pub struct ResponsiveSwitch<'a, Message> {
    children: Vec<Element<'a, Message>>,
    width: Length,
    has_trailing: bool,
    breakpoint: Box<dyn Fn(f32) -> f32 + 'a>,
}

impl<'a, Message: 'a> ResponsiveSwitch<'a, Message> {
    /// Own wide and compact branches plus a layout-only probe.
    /// The breakpoint maps intrinsic probe width to the minimum wide width.
    /// The probe is never drawn; false has_trailing selects wide regardless of its width.
    /// See the module for the branch threshold and focus-matching requirements.
    pub fn new(
        wide: Element<'a, Message>,
        compact: Element<'a, Message>,
        probe: Element<'a, Message>,
        width: Length,
        has_trailing: bool,
        breakpoint: impl Fn(f32) -> f32 + 'a,
    ) -> Self {
        Self {
            children: vec![wide, compact, probe],
            width,
            has_trailing,
            breakpoint: Box::new(breakpoint),
        }
    }

    fn active(tree: &Tree) -> usize {
        usize::from(tree.state.downcast_ref::<SwitchState>().compact)
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer>
    for ResponsiveSwitch<'a, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<SwitchState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(SwitchState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let probe_limits = layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY));
        let probe = self.children[2].as_widget_mut().measure(
            &mut tree.children[2],
            renderer,
            &probe_limits,
        );
        let breakpoint = (self.breakpoint)(probe.size().width);
        let limits = limits.width(self.width).height(Length::Shrink);
        let available = limits.max().width;
        let compact = self.has_trailing
            && available.is_finite()
            && available > 0.0
            && available + 0.01 < breakpoint;
        let previous = Self::active(tree);
        let active = usize::from(compact);
        let outgoing_owner = if active != previous {
            // Focus operations need geometry matching the outgoing branch’s diffed tree.
            let outgoing_node = self.children[previous].as_widget_mut().measure(
                &mut tree.children[previous],
                renderer,
                &limits,
            );
            let outgoing_layout = Layout::of(&outgoing_node);
            let mut find = FindFocusOwner::default();
            self.children[previous].as_widget_mut().operate(
                &mut tree.children[previous],
                outgoing_layout,
                &outgoing_layout.bounds(),
                renderer,
                &mut find,
            );
            let mut release = iced::advanced::widget::operation::focusable::unfocus::<()>();
            self.children[previous].as_widget_mut().operate(
                &mut tree.children[previous],
                outgoing_layout,
                &outgoing_layout.bounds(),
                renderer,
                &mut release,
            );
            find.owner
        } else {
            None
        };

        tree.state.downcast_mut::<SwitchState>().compact = compact;
        let node = self.children[active].as_widget_mut().measure(
            &mut tree.children[active],
            renderer,
            &limits,
        );
        if let Some(owner) = outgoing_owner {
            let incoming_layout = Layout::of(&node);
            let mut install = InstallFocusOwner {
                owner: &owner,
                next: 0,
                match_id: owner.id.is_some(),
                installed: false,
            };
            self.children[active].as_widget_mut().operate(
                &mut tree.children[active],
                incoming_layout,
                &incoming_layout.bounds(),
                renderer,
                &mut install,
            );
        }
        layout::Node::with_children(node.size(), vec![node])
            .with_placed([active])
            .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, active);
        self.children[active].as_widget_mut().operate(
            &mut tree.children[active],
            child_layout,
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
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, active);
        self.children[active].as_widget_mut().update(
            &mut tree.children[active],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.children[active], tree, active);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let active = Self::active(tree);
        tree.children
            .get(active)
            .is_some_and(Tree::has_frame_interest)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, active);
        crate::frame_routing::forward_frame_opaque(
            &mut self.children[active],
            tree,
            active,
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, active);
        self.children[active].as_widget().draw(
            &tree.children[active],
            renderer,
            theme,
            style,
            child_layout,
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
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, active);
        self.children[active].as_widget().mouse_interaction(
            &tree.children[active],
            child_layout,
            cursor,
            viewport,
            renderer,
        )
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
        let active = Self::active(tree);
        let child_layout = layout.child(&tree.children, active);
        self.children[active].as_widget_mut().overlay(
            &mut tree.children[active],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<ResponsiveSwitch<'a, Message>> for Element<'a, Message> {
    fn from(value: ResponsiveSwitch<'a, Message>) -> Self {
        Element::new(value)
    }
}
