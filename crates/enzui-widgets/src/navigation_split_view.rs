//! Lay navigation beside content or present it as an overlay drawer.
//!
//! The application owns [`NavSplitState`]. Presentation comes from
//! [`resolve_for_width`] using the assigned width and the supplied breakpoint scale;
//! [`EnzNavigationSplitView::forced`] overrides that choice. Pass custom theme scales
//! through [`EnzNavigationSplitView::breakpoints`]. Pane factories run on each layout
//! pass and must be repeatable and free of side effects.
//!
//! A hidden sidebar or closed drawer is absent from the widget tree. An open
//! drawer has an opaque sidebar over a pointer-blocking scrim; opening, it
//! slides in from its edge as the scrim fades in (at once under reduced
//! motion). Wire
//! [`EnzNavigationSplitView::on_dismiss_overlay`] to clear the application’s drawer
//! state on scrim clicks; without it the caller must supply another exit. This
//! widget’s scrim does not make background content inert for keyboard or
//! accessibility traversal. [`crate::app_shell`] supplies the full application shell
//! and its modal-background policy.
//!
//! The sidebar sits on the leading edge: the left in left-to-right text, the
//! right in right-to-left text ([`EnzNavigationSplitView::direction`]), inline
//! and as a drawer alike.
//!
//! [`EnzNavigationSplitView::sidebar_width`] and
//! [`EnzNavigationSplitView::content_minimum`] control pane geometry. For recursive
//! splits with user-draggable dividers, use [`crate::pane_grid`] instead.

use crate::exclusive_focus::exclusive_focus;
use crate::modal_host::scrim_color;
use crate::nav_split::{
    NavSplitGeometry, NavSplitState, SidebarPresentation, SidebarWidth, resolve_for_width,
};
use crate::scrim_click::ScrimClick;
use crate::semantics_container::SemanticsContainer;
use enzui_core::placement::{Direction, resolve_direction};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::BreakpointScale;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, overlay, renderer};
use iced::widget::{Space, container, row, stack};
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse};

/// A factory that builds one pane. Re-invoked on every layout pass.
pub type PaneFn<'a, Message> = Box<dyn Fn() -> Element<'a, Message> + 'a>;

/// English fallback label for the sidebar landmark.
pub const SIDEBAR_LABEL: &str = "Navigation";

/// The adaptive navigation shell. See the [module docs](self).
pub struct EnzNavigationSplitView<'a, Message> {
    sidebar: PaneFn<'a, Message>,
    content: PaneFn<'a, Message>,
    state: NavSplitState,
    breakpoints: BreakpointScale,
    sidebar_width: SidebarWidth,
    content_minimum: f32,
    on_dismiss_overlay: Option<Message>,
    forced: Option<SidebarPresentation>,
    sidebar_label: String,
    direction: Option<Direction>,
}

/// Build an adaptive navigation shell from a sidebar and a content factory.
///
/// The `state` is application-owned; this widget reads it and never writes it.
pub fn enz_navigation_split_view<'a, Message>(
    sidebar: impl Fn() -> Element<'a, Message> + 'a,
    content: impl Fn() -> Element<'a, Message> + 'a,
    state: NavSplitState,
) -> EnzNavigationSplitView<'a, Message> {
    EnzNavigationSplitView {
        sidebar: Box::new(sidebar),
        content: Box::new(content),
        state,
        breakpoints: BreakpointScale::default(),
        sidebar_width: SidebarWidth::default(),
        content_minimum: crate::nav_split::DEFAULT_CONTENT_MINIMUM,
        on_dismiss_overlay: None,
        forced: None,
        sidebar_label: enzui_core::localization::text("enzui-navigation-sidebar"),
        direction: None,
    }
}

impl<'a, Message> EnzNavigationSplitView<'a, Message> {
    /// Transport the live runtime theme's breakpoint catalog.
    ///
    /// Native `Responsive` receives a `Size`, not a `&Theme`, so the resolved
    /// scale must be passed explicitly at build time; the view rebuilds on a
    /// theme change. Without this the shell would fall back to the default
    /// scale and a custom theme's collapse point would be ignored.
    #[must_use]
    pub fn breakpoints(mut self, breakpoints: BreakpointScale) -> Self {
        self.breakpoints = breakpoints;
        self
    }

    /// Set the validated sidebar width.
    #[must_use]
    pub fn sidebar_width(mut self, width: SidebarWidth) -> Self {
        self.sidebar_width = width;
        self
    }

    /// Set the narrowest content pane worth rendering beside the sidebar.
    ///
    /// Validated by [`NavSplitGeometry::with_content_minimum`]: `0` is honoured
    /// literally (no minimum), non-finite falls back to the default.
    #[must_use]
    pub fn content_minimum(mut self, minimum: f32) -> Self {
        self.content_minimum = NavSplitGeometry::default()
            .with_content_minimum(minimum)
            .content_minimum;
        self
    }

    /// The message published when the drawer's scrim is clicked.
    ///
    /// Without this the scrim is inert: it still blocks clicks reaching the
    /// content behind it (which is the point of modality) but offers no
    /// dismissal, so the caller must provide another way out.
    #[must_use]
    pub fn on_dismiss_overlay(mut self, message: Message) -> Self {
        self.on_dismiss_overlay = Some(message);
        self
    }

    /// Override adaptive presentation, including the stored preference.
    #[must_use]
    pub fn forced(mut self, presentation: SidebarPresentation) -> Self {
        self.forced = Some(presentation);
        self
    }

    /// The writing direction; by default the locale's. In right-to-left text
    /// the sidebar and its drawer sit at the right.
    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    fn mirrored(&self) -> bool {
        resolve_direction(self.direction) == Direction::RightToLeft
    }

    /// Override the sidebar landmark's accessible label.
    #[must_use]
    pub fn sidebar_label(mut self, label: impl Into<String>) -> Self {
        self.sidebar_label = label.into();
        self
    }

    /// The geometry this shell would resolve against for an assigned width.
    #[must_use]
    pub fn geometry_for(&self, available: f32) -> NavSplitGeometry {
        NavSplitGeometry::new(available)
            .with_sidebar(SidebarWidth::new(
                self.sidebar_width.resolve_for_available(available),
            ))
            .with_content_minimum(self.content_minimum)
    }

    /// Resolve presentation for an assigned width, honoring a forced override.
    #[must_use]
    pub fn presentation_for(&self, available: f32) -> SidebarPresentation {
        self.forced.unwrap_or_else(|| {
            resolve_for_width(
                self.state.preference,
                self.breakpoints,
                self.geometry_for(available),
            )
        })
    }
}

impl<'a, Message: Clone + 'a> EnzNavigationSplitView<'a, Message> {
    fn sidebar_pane(&self, width: iced::Length) -> Element<'a, Message> {
        SemanticsContainer::new(
            container((self.sidebar)())
                .width(width)
                .height(Length::Fill)
                .style(|theme: &iced::Theme| container::Style {
                    background: Some(crate::theme_lookup::palette(theme).surface_alt.into()),
                    ..container::Style::default()
                }),
            Semantics::new(Role::Group).label(self.sidebar_label.clone()),
        )
        .into()
    }

    fn inline_tree(&self, available: f32) -> Element<'a, Message> {
        let sidebar = self.sidebar_pane(Length::Fixed(
            self.sidebar_width.resolve_for_available(available),
        ));
        let content: Element<'a, Message> = container((self.content)())
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let panes = if self.mirrored() {
            row![content, sidebar]
        } else {
            row![sidebar, content]
        };
        panes.width(Length::Fill).height(Length::Fill).into()
    }

    fn content_only(&self) -> Element<'a, Message> {
        container((self.content)())
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn overlay_tree(&self) -> Element<'a, Message> {
        let scrim: Element<'a, Message> = {
            let body = container(Space::new().width(Length::Fill).height(Length::Fill))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|theme: &iced::Theme| container::Style {
                    background: Some(scrim_color(&crate::theme_lookup::palette(theme)).into()),
                    ..container::Style::default()
                });
            match &self.on_dismiss_overlay {
                // Pointer chrome only: assistive technology closes the drawer
                // through its toggle or Escape, as platform drawers do.
                Some(msg) => {
                    let msg = msg.clone();
                    ScrimClick::new(body, move || msg.clone())
                        .block_scroll()
                        .into()
                }
                // Block pointer input without advertising an unavailable dismissal action.
                None => ScrimClick::shield(body).into(),
            }
        };

        let drawer: Element<'a, Message> = container(exclusive_focus(
            self.sidebar_pane(Length::Fixed(self.sidebar_width.get())),
        ))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(if self.mirrored() {
            iced::alignment::Horizontal::Right
        } else {
            iced::alignment::Horizontal::Left
        })
        .into();

        // An opening drawer slides in from its edge over a fading scrim.
        let width = self.sidebar_width.get();
        let from = if self.mirrored() { width } else { -width };
        let scrim = crate::motion::animated(scrim).enters_from(Vector::ZERO, 0.0);
        let drawer = crate::motion::animated(drawer).enters_from(Vector::new(from, 0.0), 1.0);

        stack![self.content_only(), scrim, drawer]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// Lay out an adaptive navigation split using the parent’s assigned width.
pub struct NavigationSplitViewWidget<'a, Message> {
    shell: EnzNavigationSplitView<'a, Message>,
    content: Element<'a, Message>,
}

impl<'a, Message: Clone + 'a> NavigationSplitViewWidget<'a, Message> {
    fn build(&self, available: f32) -> Element<'a, Message> {
        let presentation = self.shell.presentation_for(available);
        match presentation {
            SidebarPresentation::Inline => self.shell.inline_tree(available),
            SidebarPresentation::Hidden => self.shell.content_only(),
            SidebarPresentation::Overlay => {
                if self.shell.state.effective_overlay_open(presentation) {
                    self.shell.overlay_tree()
                } else {
                    self.shell.content_only()
                }
            }
        }
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for NavigationSplitViewWidget<'a, Message>
{
    fn diff(&mut self, tree: &mut Tree) {
        // Layout builds and reconciles the panes; reconciling here would replace an
        // open drawer's tree on every rebuild.
        if tree.children.is_empty() {
            tree.diff_children(std::slice::from_mut(&mut self.content));
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fill,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(Length::Fill).height(Length::Fill);
        let available = limits.max().width;
        self.content = self.build(available);
        tree.diff_children(std::slice::from_mut(&mut self.content));
        let node =
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits.loose());
        let size = limits.resolve(Length::Fill, Length::Fill, node.size());
        layout::Node::with_children(size, vec![node]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().update(
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            viewport,
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: Clone + 'a> EnzNavigationSplitView<'a, Message> {
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: Clone + 'a> From<EnzNavigationSplitView<'a, Message>> for Element<'a, Message> {
    fn from(shell: EnzNavigationSplitView<'a, Message>) -> Self {
        let content = shell.content_only();
        Element::new(NavigationSplitViewWidget { shell, content })
    }
}
