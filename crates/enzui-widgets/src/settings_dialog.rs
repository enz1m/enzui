//! Settings-dialog preset with adaptive navigation, scrolling page bodies,
//! and an optional pinned action footer.
//!
//! The caller owns selection, drill state and messages. Automatic navigation
//! measures labels against the dialog width; zero or one page has no navigation.
//! Adaptive presentation uses a sheet for narrow or short windows.
//!
//! [`EnzSettingsDialog::headerless`] applies when navigation resolves to
//! [`SettingsNav::None`], including a forced mode.
//! [`EnzSettingsDialog::fit_content`] (on by default) sizes the dialog to its
//! content, up to the geometry rung's height cap, in every navigation mode. A
//! multi-page dialog takes the height of its tallest page, so it doesn't jump
//! when pages change. Sheets keep their fixed resting height unless the caller
//! asks for `fit_content(true)` explicitly and navigation resolves to None.
//! Use distinct [`EnzSettingsDialog::page_id_prefix`] values for coexisting dialogs.
//! [`EnzSettingsDialog::compact`] selects Small geometry and a Title3/Semibold title.
//! Space arbitrary body sections with a column gap (typically `SpaceToken::Md`);
//! the preset's `content_spacing` separates chrome, not children within page bodies.
//! Pair compact settings with [`crate::preferences_group::EnzPreferencesGroup::compact`]
//! for Body/Semibold group headings; page bodies own their typography.
//!
//!
//! The preset scrolls every page body: a body reports its natural height
//! (a column of [`crate::preferences_group::EnzPreferencesGroup`]s, not a
//! `Length::Fill` column) and holds no scroll view of its own, since a nested
//! one gets unbounded height and never scrolls. Dialog-wide commands such as
//! Save go in the pinned footer through [`EnzSettingsDialog::action`] and
//! [`settings_action`], which stays in view while the page scrolls; a header
//! or footer row inside the body scrolls away with it. A page that needs its
//! own viewport, such as a virtualized list, opts out with
//! [`SettingsPage::owns_scroll`]. The `settings_dialog` example shows both.
//!
//! [`EnzSettingsDialog::into_host_with`] gates the surface on `open` and handles
//! dismissal and focus events. Caller-hosted surfaces must gate existence and
//! select dialog or sheet content and placement themselves.

use crate::button::ButtonKind;
use crate::scroll_view::enz_scroll_view;
use crate::view_switcher::{
    SwitcherPage, SwitcherPresentation, SwitcherTokens, enz_view_switcher_unwired,
};
use enzui_core::dismissal::DismissPolicy;
use enzui_core::modal::DismissCause;
use iced::widget::{Space, column, container, row, text};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};

/// A settings page descriptor with a stable routing ID, title and optional icon.
/// Use unique IDs within a dialog; selection and navigation callbacks use them.
/// Avoid `back`, `close`, and `action:`-prefixed IDs, which share the control namespace.
#[derive(Debug, Clone)]
pub struct SettingsPage {
    id: String,
    title: String,
    icon: String,
    owns_scroll: bool,
}

impl SettingsPage {
    /// A page with a stable id and a visible title, no icon.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            icon: String::new(),
            owns_scroll: false,
        }
    }

    /// Whether the page body scrolls itself. By default the preset places the
    /// body in a scroll view, so the body reports its natural height. An
    /// owning body gets the pane's bounded size instead: give it
    /// `Length::Fill` height and a scrolling widget of its own, such as a
    /// virtualized list that only builds the rows in view.
    #[must_use]
    pub fn owns_scroll(mut self, yes: bool) -> Self {
        self.owns_scroll = yes;
        self
    }

    /// Builder-style leading-icon setter (resolved via `enzui-icons`).
    #[must_use]
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// The stable routing id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A dialog-wide command in the pinned footer.
/// Actions stay outside the scrolling pane and carry an activation callback,
/// not a setting value. Unwired or disabled actions render disabled.
pub struct SettingsAction<'a, Message> {
    id: String,
    label: String,
    icon: String,
    kind: ButtonKind,
    enabled: bool,
    on_activate: Option<Box<dyn Fn() -> Message + 'a>>,
}

impl<'a, Message> std::fmt::Debug for SettingsAction<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsAction")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("icon", &self.icon)
            .field("kind", &self.kind)
            .field("enabled", &self.enabled)
            .field("wired", &self.on_activate.is_some())
            .finish()
    }
}

/// Build a footer command whose message factory runs only on activation.
/// Defaults to [`ButtonKind::Secondary`].
pub fn settings_action<'a, Message>(
    id: impl Into<String>,
    label: impl Into<String>,
    on_activate: impl Fn() -> Message + 'a,
) -> SettingsAction<'a, Message> {
    SettingsAction {
        id: id.into(),
        label: label.into(),
        icon: String::new(),
        kind: ButtonKind::Secondary,
        enabled: true,
        on_activate: Some(Box::new(on_activate)),
    }
}

/// Build a disabled footer command without an activation callback.
pub fn settings_action_unwired<'a, Message>(
    id: impl Into<String>,
    label: impl Into<String>,
) -> SettingsAction<'a, Message> {
    SettingsAction {
        id: id.into(),
        label: label.into(),
        icon: String::new(),
        kind: ButtonKind::Secondary,
        enabled: true,
        on_activate: None,
    }
}

impl<'a, Message> SettingsAction<'a, Message> {
    /// Builder-style leading-icon setter (resolved via `enzui-icons`).
    #[must_use]
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// Override the footer button kind; defaults to [`ButtonKind::Secondary`].
    #[must_use]
    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    /// Whether the command is currently available. Defaults to `true`. A
    /// disabled action still publishes its accessible name, so an AT can read
    /// what is unavailable.
    #[must_use]
    pub fn enabled(mut self, yes: bool) -> Self {
        self.enabled = yes;
        self
    }

    /// The stable routing id from which this action's widget id is derived.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The visible label, which is also the published accessible name.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }
}

/// Dismissal and focus-navigation events from the settings modal host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsDialogEvent {
    /// Request dismissal with the originating cause.
    Dismissed(DismissCause),
    /// Tab inside the dialog: move focus onward, wrapping at the end.
    FocusNext,
    /// Shift+Tab inside the dialog.
    FocusPrevious,
}

impl SettingsDialogEvent {
    /// Funnel a [`ModalHostEvent`](crate::modal_host::ModalHostEvent) into the
    /// dialog's own vocabulary.
    #[must_use]
    pub const fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;
        match event {
            ModalHostEvent::Dismissed(cause) => Self::Dismissed(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// The focus operation this event calls for, scoped to the presentation
    /// that must receive it. Prefer this over the unscoped form: the
    /// scope id resolves against the rebuilt tree at traversal time, so a
    /// presentation since covered or dismissed declines structurally.
    ///
    /// Returns `None` for events that are not focus moves.
    #[must_use]
    pub fn scoped_focus_operation<T>(
        &self,
        scope: iced::advanced::widget::Id,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;
        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::scoped_focus_operation(host_event, scope)
    }

    /// The focus operation this event calls for, if it is a focus move.
    ///
    /// Delegates to [`crate::modal_host::focus_wrap_operation`]. Returns `None`
    /// for a dismissal, which is not a focus move.
    #[must_use]
    pub fn focus_operation<T>(&self) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;
        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }

    /// The focus-restore operation a dismissal calls for.
    ///
    /// Delegates to [`crate::modal_host::restore_focus_operation`]. Returns
    /// `None` for non-dismissal events or when no invoker was stored.
    #[must_use]
    pub fn restore_focus_operation<T>(
        &self,
        invoker: Option<&iced::advanced::widget::Id>,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;
        let host_event = match self {
            Self::FocusNext | Self::FocusPrevious => return None,
            Self::Dismissed(cause) => ModalHostEvent::Dismissed(*cause),
        };
        crate::modal_host::restore_focus_operation(host_event, invoker)
    }
}

/// Use the modal dismissal policy: Escape and scrim presses dismiss settings.
#[must_use]
pub const fn settings_dismiss_policy() -> DismissPolicy {
    DismissPolicy::modal()
}

include!("settings_dialog/geometry.rs");
include!("settings_dialog/navigation.rs");

// This namespace is part of stored control identity; changing it invalidates IDs.
const DEFAULT_SETTINGS_PAGE_ID_PREFIX: &str = "enz-settings-tab";

// Recheck each update: manual scrolling can also move the selected tab out of view.
struct RevealSelectedTab<'a, Message> {
    child: Element<'a, Message>,
    target: Option<iced::advanced::widget::Id>,
}

impl<'a, Message> RevealSelectedTab<'a, Message> {
    fn new(
        child: impl Into<Element<'a, Message>>,
        target: Option<iced::advanced::widget::Id>,
    ) -> Self {
        Self {
            child: child.into(),
            target,
        }
    }

    fn drive_reveal(&mut self, tree: &mut Tree, layout: Layout, renderer: &iced::Renderer) -> bool {
        use iced::advanced::widget::operation::Outcome;

        let Some(target) = self.target.clone() else {
            return false;
        };
        let mut moved = false;
        let mut op: Box<dyn Operation> = enzui_core::focus::reveal_id(target);
        // Bound operation chaining so an unexpected cycle cannot spin forever.
        for _ in 0..4 {
            self.child.as_widget_mut().operate(
                tree,
                layout,
                &layout.bounds(),
                renderer,
                op.as_mut(),
            );
            match op.finish() {
                Outcome::None | Outcome::Some(()) => return moved,
                Outcome::Chain(next) => {
                    moved = true;
                    op = next;
                }
            }
        }
        moved
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for RevealSelectedTab<'a, Message> {
    fn tag(&self) -> tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
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
        // Reveal before the scrollable commits the translation painted on redraw.
        let moved = self.drive_reveal(tree, layout, renderer);
        if moved
            && !matches!(
                event,
                Event::Window(iced::window::Event::RedrawRequested(_))
            )
        {
            shell.request_redraw();
        }

        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
}

impl<'a, Message: 'a> From<RevealSelectedTab<'a, Message>> for Element<'a, Message> {
    fn from(w: RevealSelectedTab<'a, Message>) -> Self {
        Element::new(w)
    }
}

enum SettingsPageBody<'a, Message> {
    Built(Element<'a, Message>),
    Lazy(Box<dyn FnOnce() -> Element<'a, Message> + 'a>),
}

impl<'a, Message> SettingsPageBody<'a, Message> {
    fn realize(self) -> Element<'a, Message> {
        match self {
            Self::Built(el) => el,
            Self::Lazy(make) => make(),
        }
    }
}

/// Builder for the Settings dialog preset. See [`enz_settings_dialog`].
#[must_use]
pub struct EnzSettingsDialog<'a, Message: Clone> {
    material: Option<enzui_theme::MaterialStyle>,
    grain: enzui_theme::GrainSpec,
    open: bool,
    window: Size,
    title: String,
    title_size: crate::enz_text::TextSize,
    title_weight: enzui_theme::WeightRung,
    title_typography: Option<enzui_theme::TypographyScale>,
    pages: Vec<(SettingsPage, SettingsPageBody<'a, Message>)>,
    selected: Option<String>,
    on_select: Option<std::rc::Rc<dyn Fn(String) -> Message + 'a>>,
    on_done: Option<Message>,
    surface_radius: f32,
    surface_border_width: f32,
    content_padding: f32,
    content_spacing: f32,
    nav: SettingsNav,
    size: DialogSize,
    drilled_in: bool,
    on_back: Option<Message>,
    /// The narrow-window drill-down wired by [`Self::stacked_navigation`]:
    /// whether a page is open, and the message that returns to the list.
    stacked_drilled_in: bool,
    stacked_back: Option<Message>,
    scrim: Option<Color>,
    surface_background: Option<Color>,
    surface_border: Option<Color>,
    text_color: Option<Color>,
    page_id_prefix: Option<String>,
    headerless: bool,
    actions: Vec<SettingsAction<'a, Message>>,
    fit_content: bool,
    /// Whether the caller set `fit_content` itself. Sheets fit their content
    /// only then: the default applies to dialogs, and a sheet keeps the
    /// framework's resting height (its bottom-sheet detent).
    fit_content_explicit: bool,
    invoker: Option<iced::advanced::widget::Id>,
    scope_key: Option<String>,
    presentation: EnzSettingsPresentation,
    presentation_open: Option<bool>,
    on_exit_settled: Option<Box<dyn Fn() -> Message + 'a>>,
}

/// Construct settings chrome for the supplied viewport.
/// Defaults to a centred dialog titled `"Settings"`, fixed rung height and
/// theme-resolved colours. Page bodies and lazy factories may borrow view data.
///
/// `open` gates [`EnzSettingsDialog::into_element`] and hosted surfaces.
/// Direct [`EnzSettingsDialog::into_surface`] and
/// [`EnzSettingsDialog::into_sheet_surface`] build regardless of `open`.
pub fn enz_settings_dialog<'a, Message: Clone>(
    open: bool,
    window: Size,
) -> EnzSettingsDialog<'a, Message> {
    EnzSettingsDialog {
        material: None,
        grain: enzui_theme::GrainSpec::None,
        open,
        window,
        title: enzui_core::localization::text("enzui-settings-dialog-settings"),
        title_size: crate::enz_text::TextSize::Title2,
        title_weight: enzui_theme::WeightRung::ExtraBold,
        title_typography: None,
        pages: Vec::new(),
        selected: None,
        on_select: None,
        on_done: None,
        surface_radius: enzui_theme::text_defaults::shape_radius(|s| s.dialog),
        surface_border_width: 1.0,
        content_padding: crate::modal_surface::DEFAULT_DIALOG_PADDING.px(),
        content_spacing: 12.0,
        nav: SettingsNav::default(),
        size: DialogSize::default(),
        drilled_in: false,
        on_back: None,
        stacked_drilled_in: false,
        stacked_back: None,
        scrim: None,
        surface_background: None,
        surface_border: None,
        text_color: None,
        page_id_prefix: None,
        headerless: false,
        actions: Vec::new(),
        fit_content: true,
        fit_content_explicit: false,
        invoker: None,
        scope_key: None,
        presentation: EnzSettingsPresentation::default(),
        presentation_open: None,
        on_exit_settled: None,
    }
}

impl<'a, Message: Clone + 'a> EnzSettingsDialog<'a, Message> {
    /// Set the centred dialog material; see [`crate::glass`].
    /// This option is not forwarded to sheet surfaces.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Lay grain on the dialog surface; see [`crate::grain`].
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Dialog title. Defaults to `"Settings"`.
    pub fn title(mut self, s: impl Into<String>) -> Self {
        self.title = s.into();
        self
    }

    /// Use Small geometry with a Title3/Semibold heading for short settings pages.
    /// Navigation remains adaptive and page height stays stable across selection.
    /// Later size and title setters override these choices; page bodies are unchanged.
    pub fn compact(mut self) -> Self {
        self.size = DialogSize::Small;
        self.title_size = crate::enz_text::TextSize::Title3;
        self.title_weight = enzui_theme::WeightRung::Semibold;
        self
    }

    /// Select the title's semantic size rung; defaults to Title2.
    pub fn title_size(mut self, size: crate::enz_text::TextSize) -> Self {
        self.title_size = size;
        self
    }

    /// Select the title's semantic weight; defaults to ExtraBold.
    pub fn title_weight(mut self, weight: enzui_theme::WeightRung) -> Self {
        self.title_weight = weight;
        self
    }

    /// Forward size and weight rungs for the title; page body typography is independent.
    /// Without a ramp, the title follows application text defaults and desktop scaling.
    pub fn title_typography(mut self, typography: enzui_theme::TypographyScale) -> Self {
        self.title_typography = Some(typography);
        self
    }

    /// Add a pane: its tab descriptor and its arbitrary body element.
    ///
    /// The body stays an [`Element`]: the preset owns the chrome, not the
    /// content. `enz_preferences_group` is the recommended default inside it,
    /// never a requirement. The preset already scrolls the body, so it reports
    /// its natural height and holds no scroll view or `Length::Fill` column;
    /// put commands such as Save in the pinned footer with [`Self::action`].
    /// [`SettingsPage::owns_scroll`] hands the pane's bounded size to a body
    /// that scrolls itself.
    pub fn page(mut self, page: SettingsPage, body: impl Into<Element<'a, Message>>) -> Self {
        self.pages
            .push((page, SettingsPageBody::Built(body.into())));
        self
    }

    /// Add a pane whose factory runs at most once per consumed builder.
    /// The body follows the same layout contract as [`Self::page`].
    /// Hidden panes and two-step Stacked navigation lists drop it uncalled.
    /// Closed hosted dialogs build no body; direct surface methods ignore `open`.
    /// Page descriptors remain eager for navigation.
    pub fn page_lazy<F>(mut self, page: SettingsPage, make: F) -> Self
    where
        F: FnOnce() -> Element<'a, Message> + 'a,
    {
        self.pages
            .push((page, SettingsPageBody::Lazy(Box::new(make))));
        self
    }

    /// The id of the pane to render. Defaults to the first page. An unknown id
    /// also falls back to the first page.
    pub fn selected(mut self, id: impl Into<String>) -> Self {
        self.selected = Some(id.into());
        self
    }

    /// Set the callback receiving the selected page's stable ID.
    /// Unset navigation is inert and announced disabled in every presentation.
    pub fn on_select<F>(mut self, f: F) -> Self
    where
        F: Fn(String) -> Message + 'a,
    {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }

    /// Set the dismissal message used by [`Self::into_element`] and `.into()`.
    /// Without it, those methods return an empty placeholder. Hosted constructors
    /// use their event callback instead; direct surfaces use their explicit callback.
    pub fn on_done(mut self, msg: Message) -> Self {
        self.on_done = Some(msg);
        self
    }

    /// Set the centred dialog corner radius in logical pixels.
    /// Negative or non-finite values fall back to 12. Sheets use their own radius.
    pub fn surface_radius(mut self, r: f32) -> Self {
        self.surface_radius =
            enzui_core::metrics::FiniteNonNeg::new_or(r, self.surface_radius).get();
        self
    }

    /// Inner padding in px. Non-finite/negative falls back to 12 (`Md`).
    pub fn content_padding(mut self, p: f32) -> Self {
        self.content_padding = enzui_core::metrics::FiniteNonNeg::new_or(
            p,
            crate::modal_surface::DEFAULT_DIALOG_PADDING.px(),
        )
        .get();
        self
    }

    /// Vertical spacing between header, tab strip and body, in px.
    /// Non-finite/negative falls back to 12.
    pub fn content_spacing(mut self, s: f32) -> Self {
        self.content_spacing = enzui_core::metrics::FiniteNonNeg::new_or(s, 12.0).get();
        self
    }

    /// Choose navigation; defaults to [`SettingsNav::Automatic`].
    /// With at least two pages, forced modes override label measurement; forced
    /// [`SettingsNav::None`] hides navigation. Zero or one page always resolves to None.
    /// Forced tabs remain scrollable when labels overflow.
    pub fn nav(mut self, mode: SettingsNav) -> Self {
        self.nav = mode;
        self
    }

    /// Set the dialog size ladder; defaults to [`DialogSize::Medium`].
    /// Its width also controls automatic navigation measurement for sheet presentation.
    pub fn size(mut self, size: DialogSize) -> Self {
        self.size = size;
        self
    }

    /// Set caller-owned drill state; defaults to false.
    /// Stacked navigation shows only the pane when true, even without a Back callback.
    /// Tabs and Sidebar keep navigation with the pane. A wired Back control appears
    /// in the header when true, unless [`Self::headerless`] suppresses that row.
    pub fn drilled_in(mut self, yes: bool) -> Self {
        self.drilled_in = yes;
        self
    }

    /// Set the Back message for a drilled-in header.
    /// With Stacked navigation and drill state false, wiring Back selects the list-only
    /// step. Without Back or drill state, Stacked shows the list above the pane.
    pub fn on_back(mut self, msg: Message) -> Self {
        self.on_back = Some(msg);
        self
    }

    /// Wire the drill-down a narrow window gets: when the page titles fit
    /// neither as tabs nor as a sidebar, the dialog shows the page list, a
    /// page opens over it with a Back button and the page's title, and Back
    /// returns to the list.
    ///
    /// `drilled_in` is the app's "a page is open" flag: set it when the
    /// `on_select` message arrives, clear it on `on_back` and whenever
    /// settings open. It only applies while navigation resolves to
    /// [`SettingsNav::Stacked`], so the app can keep it set across window
    /// sizes. Every multi-page settings dialog should wire this; without it a
    /// narrow window falls back to a scrolling tab strip.
    pub fn stacked_navigation(mut self, drilled_in: bool, on_back: Message) -> Self {
        self.stacked_drilled_in = drilled_in;
        self.stacked_back = Some(on_back);
        self
    }

    /// Override the backdrop scrim colour. Unset by default (from the theme).
    pub fn scrim(mut self, c: Color) -> Self {
        self.scrim = Some(c);
        self
    }

    /// Override the centred dialog fill; unset uses the theme.
    /// This option is not forwarded to sheet surfaces.
    pub fn surface_background(mut self, c: Color) -> Self {
        self.surface_background = Some(c);
        self
    }

    /// Override the centred dialog border ink; unset uses the theme.
    /// This option is not forwarded to sheet surfaces.
    pub fn surface_border(mut self, c: Color) -> Self {
        self.surface_border = Some(c);
        self
    }

    /// Override the header title ink. Unset by default (from the theme).
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }

    /// Namespace page, Back, close, footer and strip-scroll IDs.
    /// Defaults to `"enz-settings-tab"`; use distinct prefixes for coexisting dialogs.
    /// Page control IDs remain stable across navigation presentations.
    pub fn page_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.page_id_prefix = Some(prefix.into());
        self
    }

    /// Suppress the title and Back row when navigation resolves to [`SettingsNav::None`].
    /// This includes forced None with multiple pages. The modal host keeps its title
    /// as the dialog name. Dialog close controls remain overlaid; sheet close controls
    /// move above the pane. In-pane headings remain inside the scrolling body.
    pub fn headerless(mut self, yes: bool) -> Self {
        self.headerless = yes;
        self
    }

    /// Replace the dialog-wide commands pinned below the scrolling pane.
    /// The footer uses a wrapping button flow separated by a divider. Empty lists
    /// build no footer. Use pane controls for settings with values.
    pub fn actions(
        mut self,
        actions: impl IntoIterator<Item = SettingsAction<'a, Message>>,
    ) -> Self {
        self.actions = actions.into_iter().collect();
        self
    }

    /// Add one command to the pinned footer. See [`actions`](Self::actions).
    pub fn action(mut self, action: SettingsAction<'a, Message>) -> Self {
        self.actions.push(action);
        self
    }

    /// The number of footer commands registered.
    #[must_use]
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }

    /// Report whether any footer commands are registered, including while closed.
    #[must_use]
    pub fn has_footer(&self) -> bool {
        !self.actions.is_empty()
    }

    /// Size the dialog to its page, up to the geometry rung's height cap.
    /// Defaults to true: a short page no longer sits above a band of empty
    /// surface (as macOS settings windows size to the selected pane). Pass
    /// false to keep a fixed height across pages. Sheets keep their fixed
    /// resting height unless this is called with true and navigation
    /// resolves to None.
    pub fn fit_content(mut self, yes: bool) -> Self {
        self.fit_content = yes;
        self.fit_content_explicit = true;
        self
    }

    /// Report whether the dialog sizes to its page.
    #[must_use]
    pub fn is_fit_content(&self) -> bool {
        self.fit_content
    }

    /// Report whether header suppression is requested and navigation resolves to None.
    #[must_use]
    pub fn is_headerless(&self) -> bool {
        self.headerless && self.resolved_nav() == SettingsNav::None
    }

    fn id_prefix_in_force(&self) -> String {
        self.page_id_prefix
            .clone()
            .unwrap_or_else(|| DEFAULT_SETTINGS_PAGE_ID_PREFIX.to_owned())
    }

    /// Resolve navigation without constructing page bodies; never returns Automatic.
    /// Zero or one page returns None. Otherwise forced modes win, or labels are
    /// measured against the dialog width minus content padding via [`settings_nav_for`].
    /// If any page has an icon, measurement reserves icon space for every page.
    #[must_use]
    pub fn resolved_nav(&self) -> SettingsNav {
        if self.pages.len() <= 1 {
            return SettingsNav::None;
        }
        if self.nav != SettingsNav::Automatic {
            return self.nav;
        }
        // A sheet spans the window; measure against the surface actually drawn,
        // so tabs that fit a small window stay tabs (the page list is only for
        // tabs that would be cut off).
        let panel_w = if self.resolved_presentation() == EnzSettingsPresentation::Sheet {
            self.window.width
        } else {
            self.panel_geometry().1
        };
        let strip_w = panel_w - 2.0 * self.content_padding;
        let labels: Vec<&str> = self.pages.iter().map(|(p, _)| p.title.as_str()).collect();
        let icons = self.pages.iter().any(|(p, _)| !p.icon.is_empty());
        settings_nav_for(&labels, strip_w, icons)
    }

    /// The navigation actually drawn. A stacked list needs a way back from a
    /// page: without [`Self::stacked_navigation`] or [`Self::on_back`] it
    /// would put every page title above the open page, in the same scroll,
    /// where it reads as more settings, so a scrolling tab strip is drawn
    /// instead. A forced [`SettingsNav::Stacked`] is drawn as asked.
    fn drawn_nav(&self) -> SettingsNav {
        match self.resolved_nav() {
            SettingsNav::Stacked
                if self.nav == SettingsNav::Automatic
                    && self.on_back.is_none()
                    && self.stacked_back.is_none() =>
            {
                SettingsNav::Tabs
            }
            nav => nav,
        }
    }

    /// The tab strip's width before the sidebar allowance: does it fit the
    /// preset's own width?
    fn tabs_fit_preset(&self) -> bool {
        let (_, panel_w, _) = settings_dialog_geometry_sized(self.window, self.size);
        let labels: Vec<&str> = self.pages.iter().map(|(p, _)| p.title.as_str()).collect();
        let icons = self.pages.iter().any(|(p, _)| !p.icon.is_empty());
        SwitcherTokens::ROUNDED_STRIP.strip_width(&labels, icons)
            <= panel_w - 2.0 * self.content_padding
    }

    /// `(gutter, width, height)` of the surface: the size preset's geometry,
    /// widened by a sidebar rail when a multi-page dialog's tabs do not fit the
    /// preset width and the window has room. Without the allowance a Compact
    /// or Small dialog could never reach the sidebar threshold, so it fell back
    /// to a list of page titles stacked above the page.
    fn panel_geometry(&self) -> (f32, f32, f32) {
        let (gutter, w, h) = settings_dialog_geometry_sized(self.window, self.size);
        let wants_rail = self.pages.len() > 1
            && matches!(self.nav, SettingsNav::Automatic | SettingsNav::Sidebar)
            && self.window.width >= 600.0
            && (self.nav == SettingsNav::Sidebar || !self.tabs_fit_preset());
        if !wants_rail {
            return (gutter, w, h);
        }
        let avail = (self.window.width - 2.0 * gutter).max(240.0);
        (gutter, (w + SIDEBAR_RAIL_WIDTH).min(avail), h)
    }

    /// Store the opener ID for [`Self::restore_focus_operation_for`].
    /// Hosted constructors do not automatically restore focus to this ID.
    pub fn invoker(mut self, id: iced::advanced::widget::Id) -> Self {
        self.invoker = Some(id);
        self
    }

    /// Set the key forwarded as the modal host's presentation scope ID.
    /// Use the same key when deriving a scope for focus operations.
    pub fn scope_key(mut self, key: impl Into<String>) -> Self {
        self.scope_key = Some(key.into());
        self
    }

    /// Choose a centred dialog, bottom sheet or viewport-adaptive presentation.
    /// Defaults to [`EnzSettingsPresentation::Adaptive`]. Build with
    /// [`Self::into_presented_surface`] and host at [`Self::host_placement`] so
    /// the choice takes effect; [`Self::into_surface`] always builds a dialog.
    pub fn presentation(mut self, p: EnzSettingsPresentation) -> Self {
        self.presentation = p;
        self
    }

    /// The resolved presentation for this dialog's window — never
    /// [`EnzSettingsPresentation::Adaptive`].
    ///
    /// [`EnzSettingsPresentation::Adaptive`] is resolved to
    /// [`EnzSettingsPresentation::Sheet`] or
    /// [`EnzSettingsPresentation::Dialog`] by
    /// [`enz_settings_presentation_for`].
    #[must_use]
    pub fn resolved_presentation(&self) -> EnzSettingsPresentation {
        match self.presentation {
            EnzSettingsPresentation::Adaptive => enz_settings_presentation_for(self.window),
            other => other,
        }
    }

    /// Set whether a retained hosted surface is showing; unset follows `open`.
    /// Keep `open` true while exiting and pass false here so the host can animate
    /// the retained surface. Pair with [`Self::on_exit_settled`].
    pub const fn presentation_open(mut self, showing: bool) -> Self {
        self.presentation_open = Some(showing);
        self
    }

    /// Set the hosted surface's exit-completion callback.
    /// Capture the [`ModalPresentationKey`](crate::modal_host::ModalPresentationKey)
    /// and validate it with [`finish_exit`](crate::motion::RetainedPresentation::finish_exit)
    /// so a late message cannot discard a reopened presentation.
    pub fn on_exit_settled(mut self, callback: impl Fn() -> Message + 'a) -> Self {
        self.on_exit_settled = Some(Box::new(callback));
        self
    }

    /// The resolved "is showing" flag handed to the host: the explicit
    /// [`presentation_open`](Self::presentation_open) when set, else `open`.
    #[must_use]
    pub fn is_presentation_open(&self) -> bool {
        self.presentation_open.unwrap_or(self.open)
    }

    /// Whether the dialog is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The number of panes registered.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// The id set by [`invoker`](Self::invoker), if any.
    #[must_use]
    pub fn invoker_id(&self) -> Option<&iced::advanced::widget::Id> {
        self.invoker.as_ref()
    }

    /// The dismissal policy this dialog uses.
    #[must_use]
    pub const fn policy(&self) -> DismissPolicy {
        settings_dismiss_policy()
    }

    /// Resolve the selected ID to its first matching page index.
    /// Unset or unknown IDs return 0. An empty page list also returns 0, without a pane.
    #[must_use]
    pub fn selected_index(&self) -> usize {
        match &self.selected {
            Some(id) => self
                .pages
                .iter()
                .position(|(p, _)| p.id == *id)
                .unwrap_or(0),
            None => 0,
        }
    }

    /// The focus-restore operation `event` calls for, using the invoker stored
    /// on this builder.
    ///
    /// Returns `None` when the event is not a dismissal, or when no invoker was
    /// set. Hand the operation to `iced::advanced::widget::operate` from the app's
    /// `update`, so it runs over the view rebuilt *after* the dismissal.
    #[must_use]
    pub fn restore_focus_operation_for<T>(
        &self,
        event: &SettingsDialogEvent,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        event.restore_focus_operation(self.invoker.as_ref())
    }

    /// Build a centred dialog card without a modal host.
    /// Builds regardless of `open` and uses the supplied close factory instead of
    /// [`Self::on_done`]. `None` omits the close control. A caller-owned host must
    /// gate existence and wire Escape, scrim dismissal and focus navigation.
    /// For sheet presentation use [`Self::into_sheet_surface`] and bottom-edge placement.
    #[must_use]
    pub fn into_surface(
        mut self,
        on_done: Option<Box<dyn Fn() -> Message + 'a>>,
    ) -> Element<'a, Message> {
        let surface_bg_pin = self.surface_background;
        let surface_border_pin = self.surface_border;
        let content_padding = self.content_padding;
        let surface_radius = self.surface_radius;
        let surface_border_width = self.surface_border_width;

        let (_, panel_w, panel_h) = self.panel_geometry();
        let close_label =
            enzui_core::localized!("enzui-settings-dialog-close", "title" => self.title);
        let prefix = self.id_prefix_in_force();
        // Dialogs size to the page for every navigation mode (see `fit_content`).
        let fit_content = self.fit_content;
        let headerless = self.headerless && self.resolved_nav() == SettingsNav::None;

        let trailing: Option<Element<'a, Message>> = Some(
            Space::new()
                .width(Length::Fixed(
                    crate::modal_surface::CLOSE_CONTROL_SIZE
                        + crate::modal_surface::close_inset(surface_radius),
                ))
                .into(),
        );

        // A headerless dialog must drop the spacer rather than reserve a dead band.
        // The scroll viewport runs to the surface's right edge, so its
        // scrollbar sits at the frame; the content pads itself instead.
        let content =
            self.build_settings_content(trailing, false, content_padding, content_padding);

        let surface_height = if fit_content {
            Length::Shrink.max(panel_h)
        } else {
            Length::Fixed(panel_h)
        };

        let mut surface = crate::modal_surface::enz_modal_surface(content)
            .material_opt(self.material)
            .grain(self.grain)
            .width(Length::Fixed(panel_w))
            .height(surface_height)
            .padding(content_padding)
            .padding_top(if headerless {
                content_padding
            } else {
                crate::modal_surface::close_inset(surface_radius)
            })
            .padding_bottom(0.0)
            .padding_right(0.0)
            .radius(surface_radius)
            .border_width(surface_border_width)
            .close_button_id(close_button_id(&prefix));

        if let Some(f) = on_done {
            surface = surface.on_dismiss(f);
            surface = surface.close_label(close_label);
        }

        if let Some(bg) = surface_bg_pin {
            surface = surface.background(bg);
        }
        if let Some(bc) = surface_border_pin {
            surface = surface.border_color(bc);
        }

        surface.into_element()
    }

    /// Build the surface the resolved presentation calls for: the centred
    /// dialog ([`Self::into_surface`]) or, in a small window, the bottom sheet
    /// ([`Self::into_sheet_surface`]). Host it at [`Self::host_placement`].
    pub fn into_presented_surface(
        self,
        on_done: Option<Box<dyn Fn() -> Message + 'a>>,
    ) -> Element<'a, Message> {
        match self.resolved_presentation() {
            EnzSettingsPresentation::Sheet => self.into_sheet_surface(on_done),
            _ => self.into_surface(on_done),
        }
    }

    /// Where a modal host places [`Self::into_presented_surface`]: centred
    /// for a dialog, against the bottom edge for a sheet.
    #[must_use]
    pub fn host_placement(&self) -> crate::modal_host::SurfacePlacement {
        match self.resolved_presentation() {
            EnzSettingsPresentation::Sheet => crate::modal_host::SurfacePlacement::BottomEdge,
            _ => crate::modal_host::SurfacePlacement::Centred,
        }
    }

    /// Build settings content in a bottom-sheet surface without a modal host.
    /// Builds regardless of `open` and uses the supplied close factory instead of
    /// [`Self::on_done`]. The close button sits in the header, or above a headerless
    /// pane. No drag callback is wired, so no drag handle appears.
    /// Caller-owned hosts must gate existence and use bottom-edge placement.
    #[must_use]
    pub fn into_sheet_surface(
        mut self,
        on_done: Option<Box<dyn Fn() -> Message + 'a>>,
    ) -> Element<'a, Message> {
        let content_padding = self.content_padding;
        // Only an explicit request fits a sheet; the dialog default doesn't.
        let fit_content = self.fit_content_explicit
            && self.fit_content
            && self.resolved_nav() == SettingsNav::None;

        let sheet_height = enz_settings_sheet_height(self.window);

        let close_control: Option<Element<'a, Message>> = on_done.map(|f| {
            let label =
                enzui_core::localized!("enzui-settings-dialog-close", "title" => self.title);
            crate::modal_surface::build_close_button(
                f,
                Some(close_button_id(&self.id_prefix_in_force())),
                Some(label),
            )
        });

        // The close control sits in the header (or above a headerless pane)
        // at the inset concentric with the sheet's corner, and the scroll
        // viewport runs to the sheet's right edge, as on the dialog surface.
        let close_inset = crate::modal_surface::close_inset(crate::sheet::sheet_radius());
        let content =
            self.build_settings_content(close_control, true, content_padding, close_inset);

        // Declare fixed height only on the sheet; inner padding reduces its content area.
        let padded: Element<'a, Message> = container(content)
            .padding(iced::Padding {
                top: close_inset,
                right: 0.0,
                bottom: 0.0,
                left: content_padding,
            })
            .width(Length::Fill)
            .height(if fit_content {
                Length::Shrink
            } else {
                Length::Fill
            })
            .into();

        let mut sheet = crate::sheet::enz_sheet(padded).padding(0.0).bordered(true);
        if !fit_content {
            sheet = sheet.height(sheet_height);
        }
        sheet.into_element()
    }

    /// `edge_inset` is right padding the surface left out so the scroll
    /// viewport reaches its edge; everything but the scrolled content takes it
    /// here, and the scrolled content takes it as an inset. The header, and a
    /// headerless pane's promoted close control, take `header_inset` instead.
    fn build_settings_content(
        &mut self,
        header_trailing: Option<Element<'a, Message>>,
        promote_trailing: bool,
        edge_inset: f32,
        header_inset: f32,
    ) -> Element<'a, Message> {
        let inset = |right: f32| {
            move |element: Element<'a, Message>| -> Element<'a, Message> {
                if right > 0.0 {
                    container(element)
                        .width(Length::Fill)
                        .padding(iced::Padding::ZERO.right(right))
                        .into()
                } else {
                    element
                }
            }
        };
        let pad = inset(edge_inset);
        let pad_header = inset(header_inset);
        let text_pin = self.text_color;
        let content_padding = self.content_padding;
        let content_spacing = self.content_spacing;

        let selected_index = self.selected_index();
        let nav = self.drawn_nav();
        let headerless = self.headerless && nav == SettingsNav::None;
        // A content-fit page takes its natural height up to the surface's
        // cap, and the pinned footer keeps its room below it.
        // `promote_trailing` is the sheet: it fits only on an explicit request
        // and without navigation (see `into_sheet_surface`).
        let fit = if promote_trailing {
            self.fit_content_explicit && self.fit_content && nav == SettingsNav::None
        } else {
            self.fit_content
        };
        let page_height = if fit { Length::Shrink } else { Length::Fill };
        let prefix = self.id_prefix_in_force();
        // The app's own subpage drill (`drilled_in`/`on_back`) is the deeper
        // level and wins; the stacked drill applies only to a stacked list.
        let stacked = nav == SettingsNav::Stacked;
        let stacked_drill = stacked && self.stacked_drilled_in && self.stacked_back.is_some();
        let drilled_in = self.drilled_in || stacked_drill;
        let on_back = if self.drilled_in {
            self.on_back.clone()
        } else {
            self.on_back
                .clone()
                .or_else(|| self.stacked_back.clone().filter(|_| stacked))
        };
        // A page opened from the stacked list is titled by the page, as on a
        // phone: "< Back   Notifications".
        let header_title = if stacked && drilled_in {
            self.pages
                .get(selected_index)
                .map_or_else(|| self.title.clone(), |(p, _)| p.title.clone())
        } else {
            self.title.clone()
        };
        let on_select = self.on_select.clone();
        let actions = std::mem::take(&mut self.actions);
        let descriptors: Vec<SettingsPage> = self.pages.iter().map(|(p, _)| p.clone()).collect();

        let mut title_font =
            enzui_theme::text_defaults::font(enzui_theme::FontRole::Ui, self.title_weight);
        if let Some(typography) = self.title_typography {
            title_font.weight = typography.iced_weight(self.title_weight);
        }
        let title_el: Element<'a, Message> = text(header_title)
            .size(self.title_size.resolve_size(self.title_typography.as_ref()))
            .font(title_font)
            .style(move |t: &iced::Theme| iced::widget::text::Style {
                color: Some(text_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text)),
            })
            .into();

        let back_control: Option<Element<'a, Message>> = match (drilled_in, on_back.clone()) {
            (true, Some(msg)) => Some(
                crate::button::enz_button_with(
                    enzui_core::localization::text("enzui-settings-back"),
                    Box::new(move || msg.clone()),
                )
                .kind(ButtonKind::Quiet)
                .icon("navigation.back")
                .aria_label(enzui_core::localization::text("enzui-settings-back"))
                .id(back_button_id(&prefix))
                .into(),
            ),
            _ => None,
        };

        let (header, standalone_trailing): (Element<'a, Message>, Option<Element<'a, Message>>) =
            if headerless {
                let _ = (title_el, back_control);
                let promoted = if promote_trailing {
                    header_trailing
                } else {
                    drop(header_trailing);
                    None
                };
                (Space::new().into(), promoted)
            } else {
                let mut r = row![]
                    .align_y(iced::Alignment::Center)
                    .spacing(content_spacing);
                if let Some(back) = back_control {
                    r = r.push(container(back).align_y(iced::alignment::Vertical::Center));
                }
                r = r.push(
                    container(title_el)
                        .width(Length::Fill)
                        .align_y(iced::alignment::Vertical::Center),
                );
                if let Some(trailing) = header_trailing {
                    r = r.push(container(trailing).align_y(iced::alignment::Vertical::Center));
                }
                // As tall as the close button, so a header starting at the
                // close button's inset shares its centre line.
                let header = container(r)
                    .height(Length::Fixed(crate::modal_surface::CLOSE_CONTROL_SIZE))
                    .align_y(iced::alignment::Vertical::Center);
                (pad_header(header.into()), None)
            };

        let footer: Option<Element<'a, Message>> = if actions.is_empty() {
            None
        } else {
            let mut bar = crate::flow::enz_flow::<Message>()
                .gap(crate::surface_stack::SpaceToken::Sm)
                .align_items(iced::Alignment::Center);
            for a in actions {
                let id = footer_action_id(&prefix, &a.id);
                let live = a.enabled && a.on_activate.is_some();
                let mut b = match a.on_activate {
                    Some(f) => crate::button::enz_button_with(a.label.clone(), f),
                    None => crate::button::enz_inert_button(a.label.clone(), a.kind),
                }
                .kind(a.kind)
                .enabled(live)
                .aria_label(a.label)
                .id(id);
                if !a.icon.is_empty() {
                    b = b
                        .icon(a.icon)
                        .icon_position(crate::button::ButtonIconPosition::Leading);
                }
                bar = bar.push(b.into_element());
            }
            let footer: Element<'a, Message> = column![
                crate::divider::enz_divider().into_element(),
                container(bar.into_element())
                    .width(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Right)
                    .padding(iced::Padding {
                        top: 0.0,
                        right: 0.0,
                        bottom: content_padding,
                        left: 0.0,
                    })
            ]
            .spacing(content_spacing)
            .width(Length::Fill)
            .into();
            Some(footer)
        };

        // Realize bodies inside pane branches so the list-only step builds none.
        let mut pages = std::mem::take(&mut self.pages);
        let mut scroll_body = move || -> Element<'a, Message> {
            let (owns_scroll, body): (bool, Element<'a, Message>) = if selected_index < pages.len()
            {
                let (page, body) = pages.swap_remove(selected_index);
                (page.owns_scroll, body.realize())
            } else {
                (false, Space::new().into())
            };
            if owns_scroll {
                return container(body)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(
                        iced::Padding::ZERO
                            .bottom(content_padding)
                            .right(edge_inset),
                    )
                    .into();
            }
            enz_scroll_view(body)
                .width(Length::Fill)
                .height(page_height)
                .insets(0.0, edge_inset, 0.0, content_padding)
                .into()
        };

        let content: Element<'a, Message> = match nav {
            SettingsNav::Automatic => unreachable!(
                "resolved_nav never returns Automatic; it is the request, not the answer"
            ),
            SettingsNav::None => {
                let _ = (&on_select, &descriptors);
                if headerless {
                    let mut col = column![]
                        .spacing(content_spacing)
                        .width(Length::Fill)
                        .height(page_height);
                    if let Some(trailing) = standalone_trailing {
                        col = col.push(pad_header(
                            container(trailing)
                                .width(Length::Fill)
                                .align_x(iced::alignment::Horizontal::Right)
                                .into(),
                        ));
                    }
                    col = col.push(scroll_body());
                    col.into()
                } else {
                    column![header, scroll_body()]
                        .spacing(content_spacing)
                        .width(Length::Fill)
                        .height(page_height)
                        .into()
                }
            }
            SettingsNav::Tabs => {
                let strip = tab_strip(&descriptors, selected_index, &prefix, on_select);
                // Start-aligned, on the edge the header title and the page's
                // rows share (as in System Settings' and GNOME's view switchers
                // inside a pane).
                let start = if enzui_core::placement::resolve_direction(None)
                    == enzui_core::placement::Direction::RightToLeft
                {
                    iced::alignment::Horizontal::Right
                } else {
                    iced::alignment::Horizontal::Left
                };
                let strip: Element<'_, Message> =
                    pad(container(strip).width(Length::Fill).align_x(start).into());
                column![header, strip, scroll_body()]
                    .spacing(content_spacing)
                    .width(Length::Fill)
                    .height(page_height)
                    .into()
            }
            SettingsNav::Sidebar => {
                let rail = page_rail(
                    &descriptors,
                    Some(selected_index),
                    &prefix,
                    on_select,
                    Length::Fixed(SIDEBAR_RAIL_WIDTH),
                    page_height,
                    content_padding,
                );
                column![
                    header,
                    row![rail, scroll_body()]
                        .spacing(content_spacing)
                        .width(Length::Fill)
                        .height(page_height),
                ]
                .spacing(content_spacing)
                .width(Length::Fill)
                .height(page_height)
                .into()
            }
            SettingsNav::Stacked if drilled_in => column![header, scroll_body()]
                .spacing(content_spacing)
                .width(Length::Fill)
                .height(page_height)
                .into(),
            SettingsNav::Stacked if on_back.is_some() => {
                // The page list: rows that each open a page, no selection.
                let rail = page_rail(
                    &descriptors,
                    None,
                    &prefix,
                    on_select,
                    Length::Fill,
                    page_height,
                    content_padding,
                );
                column![header, pad(rail)]
                    .spacing(content_spacing)
                    .width(Length::Fill)
                    .height(page_height)
                    .into()
            }
            SettingsNav::Stacked => {
                let rail = page_rail(
                    &descriptors,
                    Some(selected_index),
                    &prefix,
                    on_select,
                    Length::Fill,
                    Length::Shrink,
                    content_padding,
                );
                column![header, pad(rail), scroll_body()]
                    .spacing(content_spacing)
                    .width(Length::Fill)
                    .height(page_height)
                    .into()
            }
        };

        match footer {
            None => content,
            // The page is capped by what the footer leaves of the surface,
            // so a long page scrolls instead of pushing the footer out.
            Some(footer) if fit => crate::capped_dialog_body::enz_capped_dialog_body(
                Vec::new(),
                content,
                vec![pad(footer)],
                content_spacing,
                f32::INFINITY,
            )
            .into_element(),
            Some(footer) => column![content, pad(footer)]
                .spacing(content_spacing)
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        }
    }

    /// Host settings over the background, routing dismissal and focus events to `on_event`.
    /// Closed builders supply no surface. The resolved presentation selects a centred
    /// dialog or bottom sheet. [`Self::on_done`] is unused by this constructor.
    #[must_use]
    pub fn into_host_with(
        self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(SettingsDialogEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        self.into_modal_host(background, on_event).into_element()
    }

    /// Build a configurable settings modal host before conversion to an element.
    /// Closed builders supply no surface. Sheets use
    /// [`BottomEdge`](crate::modal_host::SurfacePlacement::BottomEdge) placement.
    /// Dismissal and focus events use `on_event`, independently of [`Self::on_done`].
    #[must_use]
    pub fn into_modal_host(
        mut self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(SettingsDialogEvent) -> Message + 'a,
    ) -> crate::modal_host::EnzModalHost<'a, Message> {
        let policy = self.policy();
        let label = self.title.clone();
        let scrim = self.scrim;
        let open = self.open;
        let scope_key = self.scope_key.clone();
        let presentation_open = self.presentation_open;
        let on_exit_settled = self.on_exit_settled.take();
        let use_sheet = self.resolved_presentation() == EnzSettingsPresentation::Sheet;

        let on_event = std::rc::Rc::new(on_event);
        let done_msg: Box<dyn Fn() -> Message + 'a> = {
            let on_event = std::rc::Rc::clone(&on_event);
            Box::new(move || on_event(SettingsDialogEvent::Dismissed(DismissCause::CloseButton)))
        };
        let surface = if open {
            if use_sheet {
                Some(self.into_sheet_surface(Some(done_msg)))
            } else {
                Some(self.into_surface(Some(done_msg)))
            }
        } else {
            None
        };

        let mut host = crate::modal_host::enz_modal_host(background, surface, move |event| {
            on_event(SettingsDialogEvent::from_host(event))
        })
        .policy(policy)
        .label(label);
        if use_sheet {
            host = host.placement(crate::modal_host::SurfacePlacement::BottomEdge);
        }
        if let Some(showing) = presentation_open {
            host = host.presentation_open(showing);
        }
        if let Some(settled) = on_exit_settled {
            host = host.on_exit_settled_with(settled);
        }
        if let Some(scrim) = scrim {
            host = host.scrim(scrim);
        }
        if let Some(ref key) = scope_key {
            host = host.scope(crate::modal_host::presentation_scope_id(key));
        }
        host
    }

    /// Build settings over an empty background using [`Self::on_done`] for dismissal.
    /// Closed builders or builders without that message return an empty [`Space`].
    /// Focus-navigation events are disabled; use [`Self::into_host_with`] to host
    /// settings over application content and handle focus events.
    pub fn into_element(self) -> Element<'a, Message> {
        if !self.open {
            return Space::new().into();
        }
        match self.on_done.clone() {
            Some(on_done) => {
                let background: Element<'a, Message> =
                    Space::new().width(Length::Fill).height(Length::Fill).into();
                self.into_modal_host(background, move |_event| on_done.clone())
                    .no_focus_events()
                    .into_element()
            }
            None => Space::new().into(),
        }
    }
}

/// Derive a Back control ID from the dialog prefix.
#[must_use]
pub fn back_button_id(prefix: &str) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(prefix, "back")
}

/// Derive a close control ID from the dialog prefix.
#[must_use]
pub fn close_button_id(prefix: &str) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(prefix, "close")
}

/// Derive a footer control ID using the `action:` prefix and stable action ID.
#[must_use]
pub fn footer_action_id(prefix: &str, action_id: &str) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(prefix, &format!("action:{action_id}"))
}

/// The settings tab list's accessible name, independent of the dialog title.
pub const SETTINGS_TAB_LIST_LABEL: &str = "Settings pages";

fn tab_strip<'a, Message: Clone + 'a>(
    pages: &[SettingsPage],
    selected_index: usize,
    prefix: &str,
    on_select: Option<std::rc::Rc<dyn Fn(String) -> Message + 'a>>,
) -> Element<'a, Message> {
    if pages.is_empty() {
        return Space::new().into();
    }
    let switcher_pages: Vec<SwitcherPage> = pages
        .iter()
        .map(|p| {
            let mut sp = SwitcherPage::new(p.id.clone(), p.title.clone());
            if !p.icon.is_empty() {
                sp = sp.with_icon(p.icon.clone());
            }
            sp
        })
        .collect();

    // Reveal operations require a named scrollable with the dialog's unique prefix.
    let scroll_id = enzui_core::identity::derive_id(prefix, "strip-scroll");

    let sw = enz_view_switcher_unwired(switcher_pages, selected_index)
        .on_select_id_maybe(
            on_select
                .map(|f| Box::new(move |id: String| f(id)) as Box<dyn Fn(String) -> Message + 'a>),
        )
        .presentation(SwitcherPresentation::Automatic)
        .scrollable(true)
        .scroll_id(scroll_id)
        .tab_id_prefix(prefix.to_owned())
        // Name the page group separately from the dialog and its visible heading.
        .aria_label(enzui_core::localization::text("enzui-settings-pages"));

    let target_id: Option<iced::advanced::widget::Id> = pages
        .get(selected_index)
        .map(|p| enzui_core::identity::derive_id(prefix, &p.id));
    RevealSelectedTab::new(sw.into_element(), target_id).into()
}

fn page_rail<'a, Message: Clone + 'a>(
    pages: &[SettingsPage],
    selected_index: Option<usize>,
    prefix: &str,
    on_select: Option<std::rc::Rc<dyn Fn(String) -> Message + 'a>>,
    width: Length,
    height: Length,
    bottom_inset: f32,
) -> Element<'a, Message> {
    let mut col = crate::sidebar_content::enz_sidebar_content::<Message>()
        .width(width)
        .height(height);
    for (i, p) in pages.iter().enumerate() {
        let id = p.id.clone();
        let on_select = on_select.clone();
        let activate: Option<Box<dyn Fn() -> Message + 'a>> =
            on_select.map(|f| Box::new(move || f(id.clone())) as Box<dyn Fn() -> Message + 'a>);
        let mut r = crate::sidebar_row::enz_sidebar_row_lazy::<Message>(p.title.clone())
            .selected(selected_index == Some(i))
            .show_highlight(true)
            .on_activate_maybe_with(activate)
            .id(enzui_core::identity::derive_id(prefix, &p.id));
        // A drill-down list (no selection) marks each row as opening a page.
        if selected_index.is_none() {
            r = r.trailing(
                crate::icon::enz_icon_for_direction(
                    "navigation.forward",
                    enzui_core::placement::resolve_direction(None),
                )
                .size(14.0),
            );
        }
        if !p.icon.is_empty() {
            r = r.icon(p.icon.clone());
        }
        col = col.push(r.into_element());
    }
    // The inset belongs inside the scrollable so its final row can reach it.
    enz_scroll_view(col.into_element())
        .width(width)
        .height(height)
        .insets(0.0, 0.0, 0.0, bottom_inset)
        .into()
}

impl<'a, Message: Clone + 'a> From<EnzSettingsDialog<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzSettingsDialog<'a, Message>) -> Self {
        w.into_element()
    }
}
