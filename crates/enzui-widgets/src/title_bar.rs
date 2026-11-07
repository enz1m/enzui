//! Compose client-side window chrome with ordered slots and routed window gestures.
//!
//! App icon, title, content and controls are optional. [`EnzTitleBar::center`]
//! centers a child against the whole band while reserving control space; ordinary
//! content occupies the remaining leading area. Pass overflow action descriptors
//! to [`EnzTitleBar::actions`] so folding accounts for the band’s control budget.
//! Standalone [`crate::titlebar_actions`] does not receive that budget.
//!
//! The band adopts its shell slot’s height and tone unless explicitly overridden.
//! Outside a shell it uses [`DEFAULT_TITLEBAR_HEIGHT`] and the standalone tone.
//! Window buttons are built when [`EnzTitleBar::on_gesture`] is wired. Route emitted
//! [`WindowCommand`] values to the originating window’s ID. Child controls receive
//! events before the drag surface, and window controls are non-draggable.
//!
//! The bar publishes a named [`Role::Group`](enzui_core::semantics::Role::Group); the title is a plain label unless
//! [`EnzTitleBar::title_heading`] assigns an outline level for pane chrome.
//! [`EnzTitleBar::id`] supplies stable per-window control identity. Without it the
//! control nodes are anonymous and cannot receive routed accessibility actions;
//! inside [`crate::app_shell`] the shell's own window controls are identified.
//! [`ActionItem::command`](crate::action_bar::ActionItem::command) actions are
//! identified by their descriptor id within [`EnzTitleBar::action_menu_id_prefix`],
//! which must be tree-unique when more than one bar with actions can coexist.
//!
//! [`crate::titlebar_icon_group`] supplies cluster chrome for toolbar icons.
//! [`crate::header_bar`] supplies in-content headers. A bare titlebar does not add
//! window resize edges; [`crate::app_shell`] supplies those. See [`CSD_LIMITATIONS`]
//! for compositor and desktop-policy restrictions.

use crate::surface::SurfaceTone;
use crate::title_bar_gesture::WindowCommand;
use crate::titlebar_drag_region::titlebar_drag_region;
use crate::window_controls::{WindowControlKind, enz_window_controls};
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Widget, overlay, renderer};
use iced::widget::{Space, container, row};
use iced::{Alignment, Color, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// The band's default background tone — `surface_alt`, the tone a
/// standalone bar renders. Changing this shifts every non-shell caller, so
/// the `default_tone_still_resolves_surface_alt` unit test pins it.
///
/// This is the *bottom* rung of the ladder in [`EnzTitleBar::resolved_tone`]:
/// an explicit [`EnzTitleBar::tone`] outranks it, and so does the ambient
/// shell-band tone published by [`with_shell_band_tone`].
pub const DEFAULT_TITLEBAR_TONE: SurfaceTone = SurfaceTone::SurfaceAlt;

include!("title_bar/shell_band.rs");
/// The titlebar height, in px: the shell's band and a bar built outside any
/// shell slot. A 28 px control centred in it sits 8 px from the top, the
/// same as the toggle's stand-off from the window edge, so the close
/// button's corner is square.
pub const DEFAULT_TITLEBAR_HEIGHT: f32 = 44.0;

/// Default horizontal padding inside the bar (`spacing-md`).
pub const DEFAULT_TITLEBAR_PADDING_X: f32 = 12.0;

/// Horizontal padding of a bar used as a pane header (the shell's
/// `sidebar_titlebar`): the sidebar's content rail, so the header title starts
/// on the same vertical line as the section captions and row icons below it
/// ([`crate::sidebar_content`]'s 8 px side padding plus
/// [`crate::sidebar_row::default_label_inset_x`]'s 12 px).
pub const PANE_HEADER_PADDING_X: f32 = 20.0;

/// The gap between neighbouring titlebar icons (28 px boxes on a 32 px
/// pitch). A sidebar toggle trailing a pane header keeps this gap from the
/// header's last icon, so it reads as one more of them.
pub const PANE_HEADER_ICON_GAP: f32 = 4.0;

/// A pane header's padding on the toggle's side. Trailing, the shell's
/// [`PANE_HEADER_ICON_GAP`] already separates the last icon from the toggle;
/// leading, this adds to the shell's pane gutter so the title starts 12 px
/// after the toggle, like a titlebar title after its leading icon.
pub const PANE_HEADER_TOGGLE_GAP: f32 = 0.0;
const PANE_HEADER_TITLE_AFTER_TOGGLE: f32 = 4.0;

/// Default gap between slots (`spacing-sm`).
pub const DEFAULT_TITLEBAR_SPACING: f32 = 8.0;

/// The width the band keeps for the [`actions`](EnzTitleBar::actions)
/// slot before the leading chrome is allowed to grow — one overflow
/// trigger's worth ([`crate::action_bar::DEFAULT_TRIGGER_WIDTH`]).
///
/// This is not a breakpoint and not a per-app rung: it is the floor that makes
/// the *measured* fold safe. Everything above it is decided by measurement; it
/// only guarantees that the `⋯` the fold produces has somewhere to land, so a
/// long title can shorten the visible action run but can never make the folded
/// commands unreachable.
pub const MIN_ACTIONS_RESERVE: f32 = crate::action_bar::DEFAULT_TRIGGER_WIDTH;

/// The share of the band's residual width the window title keeps
/// when an [`actions`](EnzTitleBar::actions) slot is present.
///
/// [`crate::window_title`] is a `Length::Fill` container (it centres itself
/// inside its slot) and reports 0 under the unbounded intrinsic probe, with
/// no channel for the width of the text it holds. A titled bar therefore needs
/// this fallback. Measurable caller chrome does not: applying the same floor to
/// an intrinsic icon group reserves hundreds of empty pixels in a wide band.
/// The internal titlebar layout receives whether a window title was actually
/// authored and applies this share only for that known zero-measuring child.
pub const LEADING_CHROME_SHARE: f32 = 0.4;

/// Diagnostic ID for content extending beyond its reserved titlebar slot.
///
/// Oversubscribed slots publish a layout-spill probe; fitting slots publish none.
pub const CONTENT_OVERSUBSCRIPTION_ID: &str = "enz-title-bar-content-oversubscribed";

/// The id [`CONTENT_OVERSUBSCRIPTION_ID`] findings carry.
#[must_use]
pub fn content_oversubscription_id() -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::new(CONTENT_OVERSUBSCRIPTION_ID)
}

/// The widget-kind tag for derived ids. Not an identity on its own — every
/// bar in the process shares it — it is only ever combined with the host's own
/// key by [`EnzTitleBar::owner`]. See [`crate::window_controls::DEFAULT_ID_PREFIX`]
/// for the full reasoning this mirrors.
pub const DEFAULT_ID_PREFIX: &str = "enz-title-bar";

/// Compositor and desktop-policy restrictions for client-side titlebars.
///
/// Native window movement belongs to the compositor. Hosts supply control order,
/// window-command routing and resize edges when this bar is used without a shell.
pub const CSD_LIMITATIONS: &[&str] = &[
    "window drag is compositor-owned once started; no cancel, constrain or completion callback",
    "Wayland exposes no global cursor position, so a manual window-move fallback is impossible",
    "show_system_menu is best-effort; compositors may ignore it with no capability query",
    "control order is a desktop setting (button-layout); this widget accepts an order, never reads one",
    "CSD windows must supply their own resize edges via window::drag_resize; this bar covers the band only",
    "double-click titlebar action is a desktop setting; toggle-maximize is the implemented default",
];

/// Builder for [`enz_title_bar`].
pub struct EnzTitleBar<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    /// Writing direction override; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    app_icon: Option<String>,
    title: Option<String>,
    subtitle: Option<String>,
    title_heading: Option<enzui_core::semantics::HeadingLevel>,
    leading: Vec<Element<'a, Message>>,
    content: Vec<Element<'a, Message>>,
    fluid_content_min_width: Option<f32>,
    center: Option<Element<'a, Message>>,
    actions: Vec<crate::action_bar::ActionItem<'a, Message>>,
    on_toggle_overflow: Option<Message>,
    // Explicit dismissal is separate from the trigger toggle because a row
    // command may already have closed the menu before dismissal is reduced.
    on_action_dismiss: Option<Message>,
    overflow_open: bool,
    tooltip_delay: Option<std::time::Duration>,
    action_fold_policy: crate::action_bar::FoldPolicy,
    action_collector: crate::action_bar::Collector,
    action_collector_visibility: crate::action_bar::CollectorVisibility,
    action_collector_placement: crate::action_bar::CollectorPlacement,
    action_max_inline: Option<usize>,
    action_menu_id_prefix: Option<String>,
    action_overflow_trigger_id: Option<iced::advanced::widget::Id>,
    tone: Option<SurfaceTone>,
    height: Option<f32>,
    padding_x: f32,
    padding_x_explicit: bool,
    spacing: f32,
    draggable: bool,
    modal_navigation: bool,
    show_minimize: bool,
    show_maximize: bool,
    show_close: bool,
    maximized: bool,
    control_order: Option<Vec<WindowControlKind>>,
    fullscreen: bool,
    on_gesture: Option<Box<dyn Fn(WindowCommand) -> Message + 'a>>,
    id_prefix: Option<String>,
    id_override: Option<String>,
}

/// Construct an empty titlebar. Every slot is opt-in.
pub fn enz_title_bar<'a, Message>() -> EnzTitleBar<'a, Message> {
    EnzTitleBar {
        material: None,
        direction: None,
        app_icon: None,
        title: None,
        subtitle: None,
        // Not a rung. A window title is chrome; only a bar the caller
        // uses as pane/section chrome names one.
        title_heading: None,
        leading: Vec::new(),
        content: Vec::new(),
        fluid_content_min_width: None,
        center: None,
        actions: Vec::new(),
        on_toggle_overflow: None,
        on_action_dismiss: None,
        overflow_open: false,
        tooltip_delay: None,
        action_fold_policy: crate::action_bar::FoldPolicy::Measured,
        action_collector: crate::action_bar::Collector::Kebab {
            icon: crate::menu_button::ICON_ONLY_TRIGGER_ICON.to_owned(),
            tokens: None,
        },
        action_collector_visibility: crate::action_bar::CollectorVisibility::Automatic,
        action_collector_placement: crate::action_bar::CollectorPlacement::TrailingCluster,
        action_max_inline: None,
        action_menu_id_prefix: None,
        action_overflow_trigger_id: None,
        // Not `Some(DEFAULT_TITLEBAR_TONE)`. "The caller never said"
        // and "the caller asked for surface_alt" must stay distinguishable,
        // or the shell-band rung below could never apply.
        tone: None,
        // Not `Some(DEFAULT_TITLEBAR_HEIGHT)`: "the caller never said" must
        // stay distinguishable from a pinned height, or a bar in a shell slot
        // could not take the band height the shell's tokens chose.
        height: None,
        padding_x: DEFAULT_TITLEBAR_PADDING_X,
        padding_x_explicit: false,
        spacing: DEFAULT_TITLEBAR_SPACING,
        draggable: true,
        modal_navigation: false,
        show_minimize: false,
        show_maximize: false,
        show_close: true,
        maximized: false,
        control_order: None,
        fullscreen: false,
        on_gesture: None,
        id_prefix: None,
        id_override: None,
    }
}

impl<'a, Message: Clone + 'a> EnzTitleBar<'a, Message> {
    /// Override hover delay for owned window controls and the action overflow trigger.
    /// Zero starts revealing immediately; supplied slot content keeps its own policy.
    #[must_use]
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Override the writing direction. By default the bar follows the
    /// locale; right-to-left puts the app icon and title on the right and
    /// the window controls on the left, close outermost.
    #[must_use]
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Set the leading app icon (an icon name). Absent by default.
    #[must_use]
    pub fn app_icon(mut self, name: impl Into<String>) -> Self {
        self.app_icon = Some(name.into());
        self
    }

    /// Set the title, shown at the band's leading edge after any
    /// [`leading`](Self::leading) controls. Absent by default; also becomes
    /// the semantic owner's label ("`{title}` title bar").
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the subtitle. Rendered only when a [`title`](Self::title) is set,
    /// because the composed [`crate::window_title`] stacks it under one.
    #[must_use]
    pub fn subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Publish this bar's [`title`](Self::title) as a heading at the given
    /// outline rung.
    ///
    /// # When a titlebar title is a heading
    ///
    /// This band is used for two different things, and they are not
    /// distinguishable from inside the widget:
    ///
    /// * the window's CSD band (the default) — the title is the app's
    ///   name, chrome rather than document structure, already narrated by this
    ///   bar's `"{title} title bar"` group owner. It stays a plain label, and
    ///   nothing here changes for those callers;
    /// * a pane's own header band — the shell's `sidebar_titlebar` slot,
    ///   an inspector's or a drawer's header. That caption introduces
    ///   everything under it, so it is the rung the pane's
    ///   [`crate::section_header`] bands hang from. Without this, migrating
    ///   such a header onto the shared bar deletes a heading and the
    ///   pane's outline starts below its own title (the defect).
    ///
    /// Forwarded verbatim to [`crate::window_title::EnzWindowTitle::heading`],
    /// which promotes the title run's existing label node rather than wrapping
    /// the composition in a second `Role::Heading`: one node, one
    /// reading-order location, no extra focus stop, identical pixels. Only the
    /// title is promoted — a [`subtitle`](Self::subtitle) is a caption of the
    /// heading, not a rung of its own.
    ///
    /// The rung is named, not numbered ([`HeadingLevel::Page`] for a pane's
    /// own title above its [`HeadingLevel::Section`] bands), and the frame is
    /// still audited by
    /// [`enzui_core::heading_outline::HeadingOutline`], which fails a
    /// second `Page` rung, a level skip and over-claiming.
    ///
    /// [`HeadingLevel::Page`]: enzui_core::semantics::HeadingLevel::Page
    /// [`HeadingLevel::Section`]: enzui_core::semantics::HeadingLevel::Section
    #[must_use]
    pub fn title_heading(mut self, level: enzui_core::semantics::HeadingLevel) -> Self {
        self.title_heading = Some(level);
        self
    }

    /// The outline rung this bar's title claims, or `None` (the default) for
    /// an ordinary window title.
    #[must_use]
    pub fn title_heading_level(&self) -> Option<enzui_core::semantics::HeadingLevel> {
        self.title_heading
    }

    /// Put a control before the title: a back button, or a control that
    /// belongs at the band's leading edge. It never folds or shrinks (a
    /// [`center`](Self::center) narrows before it does) and sits at the
    /// leading end in either writing direction.
    #[must_use]
    pub fn leading(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.leading.push(child.into());
        self
    }

    /// Push one custom content element into the leading content slot.
    ///
    /// # This slot is not the band, and not for centring
    ///
    /// Content pushed here lays out in the slot **between the title and the
    /// window controls**, and that slot deliberately *excludes* the controls'
    /// reserved width: `TitlebarLayout` measures the controls first
    /// and hands this slot `total_w − controls_w − gap`. So a child centred
    /// *inside* a `push`ed element — with `Space::Fill` on either side, say —
    /// is centred on that residual, which sits `(control_size + spacing) / 2`
    /// (≈18 px at the defaults) to the left of the band's true centre.
    /// This is invisible by eye and has shipped 18 px off.
    ///
    /// For a child centred on the whole band — the macOS/GNOME
    /// view-switcher-in-the-titlebar shape — reach for
    /// [`center`](Self::center) instead, which centres against the band and
    /// accounts for the controls it already measures.
    #[must_use]
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.content.push(child.into());
        self
    }

    /// Replace the whole leading content slot. See [`push`](Self::push) for
    /// why this slot is not the band, and [`center`](Self::center) for
    /// centring on the band.
    #[must_use]
    pub fn content(mut self, children: Vec<Element<'a, Message>>) -> Self {
        self.content = children;
        self
    }

    /// Let leading content consume the residual left after the complete action
    /// inventory, retaining `minimum` while actions progressively fold.
    ///
    /// Zero is a valid floor and still opts into residual-width allocation.
    /// Negative and non-finite values disable the policy, matching the default;
    /// a finite minimum larger than the residual is safely capped to the space
    /// actually available.
    #[must_use]
    pub fn fluid_content(mut self, minimum: f32) -> Self {
        self.fluid_content_min_width = (minimum.is_finite() && minimum >= 0.0).then_some(minimum);
        self
    }

    /// Centre a child on the whole band — the macOS/GNOME shape of a
    /// centred view switcher / segmented control with a title on the left and
    /// actions on the right.
    ///
    /// # Why this is not [`push`](Self::push)
    ///
    /// `push`/[`content`](Self::content) land in a slot that excludes the
    /// trailing window controls; centring inside that slot is off by
    /// `(control_size + spacing) / 2` (≈18 px at the defaults) because the
    /// slot's centre is left of the band's. `center` composes
    /// [`crate::center_clamp_row`] against the band's full width and
    /// clamps the centred child against the *measured* widths of the leading
    /// content and the trailing controls, so it never overlaps either and
    /// stays at the band's true centre regardless of which side is busier —
    /// exactly the guarantee `center_clamp_row` gives `header_bar`.
    ///
    /// The bar is the only party that can see all three numbers (leading
    /// content, the controls it builds, and the band width), which is why the
    /// correction lives here and not in every caller: an app correcting from
    /// outside re-derives private layout facts from public constants and
    /// breaks the day the control size or the gap changes.
    ///
    /// # The shell toggle is handled
    ///
    /// When [`crate::app_shell::EnzAppShell`] moves its reveal toggle into
    /// this band (once the sidebar stops being inline), the toggle is a
    /// sibling *outside* this bar, so the bar receives a band already narrowed
    /// by the toggle and centres on that narrowed band — which is the band the
    /// eye reads. No caller-side correction is needed or correct; adding one
    /// double-counts.
    #[must_use]
    pub fn center(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.center = Some(child.into());
        self
    }

    /// The trailing action slot, with a measured overflow model.
    ///
    /// This is the answer to "my titlebar controls wrap to a second row on a
    /// narrow window" and to "resizing silently hides my titlebar icons".
    /// Actions handed here are laid out against the width the band actually
    /// has left — after the window controls are reserved and after the
    /// leading chrome is measured — and every action that does not fit folds
    /// whole into a single trailing `⋯` overflow menu, in the fold priority
    /// its [`Descriptor`](enzui_core::action_overflow::Descriptor)
    /// declares. Nothing is clipped, nothing is dropped, and the band never
    /// becomes two rows.
    ///
    /// # What the band guarantees on top of [`crate::action_bar`]
    ///
    /// The fold itself is [`crate::action_bar::EnzActionBar`] — searched for
    /// ("toolbar overflow menu") rather than rebuilt, so the measurement, the
    /// conditional trigger reservation, the atomic whole-command fold, the
    /// menu overlay and its dismissal lifecycle are the ones already under
    /// test there. What this slot adds is the *band* discipline:
    ///
    /// * the window controls are measured first and never fold, so the
    ///   close button cannot be pushed out by a busy action cluster;
    /// * the leading chrome (app icon, title, [`push`](Self::push) content) is
    ///   pinned and measured second, but it is capped so that the `⋯`
    ///   trigger always keeps a slot — a long title can shorten the action
    ///   run, it cannot make the overflow itself unreachable;
    /// * the actions get the residual, on the same row, always.
    ///
    /// # The caller owns the menu's open state
    ///
    /// The trigger publishes `on_toggle_overflow`; feed the resulting state
    /// back through [`overflow_open`](Self::overflow_open). For row activation,
    /// Escape, and outside press, use [`action_dismiss`](Self::action_dismiss)
    /// with a message that sets the state to `false`. Without an explicit
    /// dismiss message, row activation publishes only its command while Escape
    /// and outside press use the toggle message.
    #[must_use]
    pub fn actions(
        mut self,
        actions: Vec<crate::action_bar::ActionItem<'a, Message>>,
        on_toggle_overflow: Message,
    ) -> Self {
        self.actions = actions;
        self.on_toggle_overflow = Some(on_toggle_overflow);
        self
    }

    /// Whether the `⋯` overflow menu of the [`actions`](Self::actions) slot is
    /// currently presented. Caller-owned; defaults to closed.
    #[must_use]
    pub fn overflow_open(mut self, open: bool) -> Self {
        self.overflow_open = open;
        self
    }

    /// Sets the idempotent message used to close the action overflow menu.
    ///
    /// Outside presses, Escape, and row activation publish this message.
    /// Activation publishes its command first, so the message must set the
    /// open state to `false` rather than toggle it. When unset, row activation
    /// publishes only its command and other dismissal routes use the toggle
    /// supplied to [`actions`](Self::actions).
    #[must_use]
    pub fn action_dismiss(mut self, message: Message) -> Self {
        self.on_action_dismiss = Some(message);
        self
    }

    /// Select measured or uniform-slot folding for the action slot.
    #[must_use]
    pub fn action_fold_policy(mut self, policy: crate::action_bar::FoldPolicy) -> Self {
        self.action_fold_policy = policy;
        self
    }

    /// Select the framework-owned collector presentation. The default is the
    /// titlebar vertical kebab on [`TitlebarAffordanceTokens`](crate::toolbar_icon::TitlebarAffordanceTokens)
    /// geometry.
    #[must_use]
    pub fn action_collector(mut self, collector: crate::action_bar::Collector) -> Self {
        self.action_collector = collector;
        self
    }

    /// Control whether the collector follows resolved overflow automatically
    /// or remains visible even with an empty current overflow set.
    #[must_use]
    pub fn action_collector_visibility(
        mut self,
        visibility: crate::action_bar::CollectorVisibility,
    ) -> Self {
        self.action_collector_visibility = visibility;
        self
    }

    /// Choose whether the collector alone or the whole surviving action
    /// cluster is trailing-aligned. The titlebar default is
    /// [`TrailingCluster`](crate::action_bar::CollectorPlacement::TrailingCluster).
    #[must_use]
    pub fn action_collector_placement(
        mut self,
        placement: crate::action_bar::CollectorPlacement,
    ) -> Self {
        self.action_collector_placement = placement;
        self
    }

    /// Cap foldable inline commands while preserving pinned and overflow-only
    /// policy. Priority decides which commands survive under the cap.
    #[must_use]
    pub fn action_inline_limit(mut self, limit: usize) -> Self {
        self.action_max_inline = Some(limit);
        self
    }

    /// Set the stable caller-owned id namespace for keyed rows in this bar's
    /// action overflow menu.
    ///
    /// The prefix is combined with each action's stable
    /// [`Descriptor::id`](enzui_core::action_overflow::Descriptor::id),
    /// so two titlebars may expose the same command id without aliasing one
    /// another. Choose a prefix that is unique among menus mounted in the same
    /// accessibility tree and stable across view rebuilds; for example,
    /// `"editor-titlebar-overflow"`.
    ///
    /// This names menu rows and inline command actions, not the `⋯` trigger. Name that separately with
    /// [`action_overflow_trigger_id`](Self::action_overflow_trigger_id). If
    /// unset, the composed [`crate::menu::EnzMenu`] keeps its existing default
    /// prefix, preserving source compatibility for callers that do not route
    /// rows by id.
    #[must_use]
    pub fn action_menu_id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.action_menu_id_prefix = Some(prefix.into());
        self
    }

    /// Give the action-slot overflow trigger (the compact `⋯` kebab) a stable
    /// widget id.
    ///
    /// The trigger is the only durable focusable that survives when a Find
    /// invoker folds into the overflow menu; naming it lets a dismissal path
    /// (`focus_restore_operation`) actually land keyboard focus on the
    /// surviving control instead of leaving the tree unfocused. Forwarded
    /// verbatim to [`crate::action_bar::EnzActionBar::overflow_trigger_id`].
    #[must_use]
    pub fn action_overflow_trigger_id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.action_overflow_trigger_id = Some(id.into());
        self
    }

    /// Override the band's background tone (default
    /// [`DEFAULT_TITLEBAR_TONE`] = `surface_alt`).
    ///
    /// Reuses the crate-wide [`SurfaceTone`] — the same enum `enz_surface`,
    /// `enz_status_page` and the other chrome widgets take — so the band
    /// resolves through the same token path as every other surface. A raw
    /// `Color` is deliberately not accepted: it would let a caller bypass
    /// the theme and pin an ink that fails contrast on a switched palette.
    ///
    /// The tone still resolves through [`crate::theme_lookup`] on every render,
    /// so the band stays theme-reactive and never snapshots a colour at build
    /// time. Use `.tone(SurfaceTone::Surface)` to merge with surface-toned content.
    ///
    ///
    /// # This is the top rung
    ///
    /// Calling this pins the band and overrides the ambient shell-band tone
    /// ([`with_shell_band_tone`]). A bar inside an
    /// [`crate::app_shell::EnzAppShell`] slot that never calls this adopts the
    /// tone the shell already painted; one that does calls it wins, because a
    /// caller who named a tone said something the framework must not silently
    /// discard. See
    /// [`Self::resolved_tone`] for the full ladder.
    #[must_use]
    pub fn tone(mut self, tone: SurfaceTone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// Put the bar on liquid glass of `style` (see [`crate::glass`]) in place
    /// of its tone. Its controls default to glass.
    #[must_use]
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// The tone this bar's band will actually paint, at the moment it is asked
    ///
    ///
    /// Highest rung first:
    ///
    /// | rung | source | wins because |
    /// |---|---|---|
    /// | 1. explicit | [`Self::tone`] | the caller named one exact tone |
    /// | 2. ambient | [`with_shell_band_tone`] | the shell owns the chrome it already painted |
    /// | 3. fallback | [`DEFAULT_TITLEBAR_TONE`] | a standalone bar is a `surface_alt` band |
    ///
    /// Rung 2 is the whole of: without it, a bar placed in a shell slot
    /// overpaints the shell's own (by default `Transparent`) band with an
    /// opaque `surface_alt` slab, and the shell's `PaneStyle` becomes
    /// unobservable.
    ///
    /// This reads the ambient scope, so it answers for where it is called.
    /// [`Self::into_element`] calls it exactly once, at build time, which is
    /// inside the shell's slot closure.
    #[must_use]
    pub fn resolved_tone(&self) -> SurfaceTone {
        self.tone
            .or_else(shell_band_tone)
            .unwrap_or(DEFAULT_TITLEBAR_TONE)
    }

    /// Pin the band height in px, overriding the ambient shell band.
    /// Non-finite, zero and negative are not expressible intent for a height
    /// and fall back to [`DEFAULT_TITLEBAR_HEIGHT`].
    ///
    /// # This is the TOP rung, for the genuine exception
    ///
    /// A bar composed into [`crate::app_shell::EnzAppShell`]'s titlebar slot
    /// that never calls this takes the shell's own resolved band height
    /// ([`DEFAULT_TITLEBAR_HEIGHT`] unless the shell's tokens say otherwise),
    /// so it fits the slot the shell reserved for it without every caller
    /// re-stating the number. Change the height on the shell
    /// ([`crate::app_shell::ShellTokens::titlebar_height`]), not here; call
    /// this only for a bar outside any shell that genuinely differs, and say
    /// why at the call site. See [`Self::resolved_height`] for the full ladder.
    #[must_use]
    pub fn height(mut self, h: f32) -> Self {
        self.height =
            Some(enzui_core::metrics::FinitePositive::new_or(h, DEFAULT_TITLEBAR_HEIGHT).get());
        self
    }

    /// The band height this bar will actually paint, at the moment it is asked
    ///
    ///
    /// Highest rung first:
    ///
    /// | rung | source | wins because |
    /// |---|---|---|
    /// | 1. explicit | [`Self::height`] | the caller pinned one exact height |
    /// | 2. ambient | [`with_shell_band`] | the shell owns the band it reserved |
    /// | 3. theme | `sizing.titlebar_height` | a standalone bar follows the theme band |
    ///
    /// Rung 2 makes a bar in a shell slot paint exactly the band the shell
    /// reserved, whatever height the shell's tokens give it. Rung 3 is the
    /// same theme token as the shell's default, so a bar outside any shell looks
    /// like one inside it.
    ///
    /// This reads the ambient scope, so it answers for where it is called;
    /// [`Self::into_element`] calls it exactly once, at build time, which is
    /// inside the shell's slot closure.
    #[must_use]
    pub fn resolved_height(&self) -> f32 {
        self.height.or_else(shell_band_height).unwrap_or_else(|| {
            enzui_core::metrics::FinitePositive::new_or(
                enzui_theme::text_defaults::sizing().titlebar_height,
                DEFAULT_TITLEBAR_HEIGHT,
            )
            .get()
        })
    }

    /// Horizontal padding in px. Zero is expressible intent (a flush bar)
    /// and is honoured literally; non-finite/negative fall back to the default.
    #[must_use]
    pub fn padding_x(mut self, p: f32) -> Self {
        self.padding_x =
            enzui_core::metrics::FiniteNonNeg::new_or(p, DEFAULT_TITLEBAR_PADDING_X).get();
        self.padding_x_explicit = true;
        self
    }

    /// Inter-slot spacing in px. Zero is honoured literally.
    #[must_use]
    pub fn spacing(mut self, s: f32) -> Self {
        self.spacing = enzui_core::metrics::FiniteNonNeg::new_or(s, DEFAULT_TITLEBAR_SPACING).get();
        self
    }

    /// Whether the bar's empty space drags the window (default `true`).
    ///
    /// `false` disables band drag and the window system menu. Window controls
    /// are non-draggable regardless of this flag.
    #[must_use]
    pub fn draggable(mut self, draggable: bool) -> Self {
        self.draggable = draggable;
        self
    }

    /// Yield ordinary slots and band paint to a modal navigation surface.
    ///
    /// Content, center and action slots keep their allocation and state, but stop
    /// painting, receiving input and publishing focus or accessibility nodes.
    /// Window buttons remain available; window drag and system menus stop.
    /// Defaults to false.
    #[must_use]
    pub fn modal_navigation(mut self, modal: bool) -> Self {
        self.modal_navigation = modal;
        self
    }

    /// Show the minimize control (hidden by default).
    #[must_use]
    pub fn show_minimize(mut self, show: bool) -> Self {
        self.show_minimize = show;
        self
    }

    /// Show the maximize control and declare the window's current maximized
    /// state, which selects the maximize vs restore glyph. Hidden by default.
    #[must_use]
    pub fn show_maximize(mut self, show: bool, maximized: bool) -> Self {
        self.show_maximize = show;
        self.maximized = maximized;
        self
    }

    /// Show or hide the close control (shown by default).
    #[must_use]
    pub fn show_close(mut self, show: bool) -> Self {
        self.show_close = show;
        self
    }

    /// Set the left-to-right control order. Forwarded verbatim to
    /// [`crate::window_controls::EnzWindowControls::order`], which owns the
    /// dedupe-and-filter policy. Defaults to that widget's `DEFAULT_ORDER`.
    ///
    /// This widget reads no platform API to decide the order — see
    /// [`CSD_LIMITATIONS`] item 4.
    #[must_use]
    pub fn control_order(mut self, order: impl IntoIterator<Item = WindowControlKind>) -> Self {
        self.control_order = Some(order.into_iter().collect());
        self
    }

    /// Suppress the window controls entirely (a fullscreen window has no
    /// chrome to put them in). Forwarded to
    /// [`crate::window_controls::EnzWindowControls::fullscreen`].
    #[must_use]
    pub fn fullscreen(mut self, fullscreen: bool) -> Self {
        self.fullscreen = fullscreen;
        self
    }

    /// Map every window gesture and control activation to a message.
    ///
    /// Until this is set the bar publishes nothing: the controls render but
    /// are inert, and the drag region is inert. That is what makes a
    /// decoration-only bar (a screenshot fixture, a disabled-chrome state)
    /// expressible without wiring a window id.
    #[must_use]
    pub fn on_gesture(mut self, f: impl Fn(WindowCommand) -> Message + 'a) -> Self {
        self.on_gesture = Some(Box::new(f));
        self
    }

    /// Set this bar's host-owned stable per-window key.
    ///
    /// Pass the host's own window identity — an `iced::window::Id` rendered to
    /// a string, a route name. It must be unique among the bars mounted in one
    /// AT tree and stable across view rebuilds. Without it the bar's controls
    /// are anonymous (see the module docs).
    #[must_use]
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id_override = Some(id.into());
        self
    }

    /// Set the namespace prefix. Prefer [`id`](Self::id) when the host has a
    /// real window key.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// This bar's owner namespace, or `None` when the caller supplied neither
    /// [`id`](Self::id) nor [`id_prefix`](Self::id_prefix).
    #[must_use]
    pub fn owner(&self) -> Option<enzui_core::identity::OwnerId> {
        enzui_core::identity::OwnerId::resolve(
            DEFAULT_ID_PREFIX,
            self.id_prefix.as_deref(),
            self.id_override.as_deref(),
        )
    }

    /// The accessible name of the bar's semantic owner: "`{title}` title bar"
    /// when a title is set, else "Title bar".
    #[must_use]
    pub fn accessible_label(&self) -> String {
        match self.title.as_deref().filter(|t| !t.is_empty()) {
            Some(title) => {
                enzui_core::localized!("enzui-title-bar-title-bar-detail", "title" => title)
            }
            None => enzui_core::localization::text("enzui-title-bar-title-bar"),
        }
    }

    /// The semantics this bar publishes: a labelled group owner. See the
    /// module docs for why this is `Role::Group` and not a new `TitleBar`
    /// variant.
    pub fn semantics(&self) -> Semantics {
        Semantics::new(Role::Group).label(self.accessible_label())
    }

    /// Whether the window-controls slot renders at all. Inside a shell that
    /// draws the window controls itself, the bar draws none.
    #[must_use]
    pub fn renders_controls(&self) -> bool {
        !self.fullscreen
            && !iced::window::decorations().fullscreen
            && !shell_owns_window_controls()
            && (self.show_close || self.show_minimize || self.show_maximize)
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // Resolved here, inside the shell's slot closure, so a bar with
        // no explicit height takes the shell band the closure published.
        // Standalone (no shell scope) this is `DEFAULT_TITLEBAR_HEIGHT`.
        let height = self.resolved_height();
        let semantics = self.semantics();
        let owner = self.owner();
        let modal_navigation = self.modal_navigation;
        let draggable = self.draggable && !modal_navigation;
        let renders_controls = self.renders_controls();
        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;

        let tone = if modal_navigation {
            SurfaceTone::Transparent
        } else {
            self.resolved_tone()
        };
        let material = self.material;

        let EnzTitleBar {
            app_icon,
            title,
            subtitle,
            title_heading,
            leading,
            content,
            fluid_content_min_width,
            center,
            actions,
            on_toggle_overflow,
            on_action_dismiss,
            overflow_open,
            tooltip_delay,
            action_fold_policy,
            action_collector,
            action_collector_visibility,
            action_collector_placement,
            action_max_inline,
            action_menu_id_prefix,
            action_overflow_trigger_id,
            padding_x,
            padding_x_explicit,
            spacing,
            show_minimize,
            show_maximize,
            show_close,
            maximized,
            control_order,
            fullscreen,
            on_gesture,
            ..
        } = self;

        // A pane header starts on the sidebar's content rail unless the caller
        // pinned a padding.
        let pane_header = in_pane_header();
        // `(leading, trailing)` padding. A pane header starts on the sidebar's
        // content rail, and its toggle side sits one icon gap from the toggle.
        let pane_padding =
            (pane_header && !padding_x_explicit).then(|| match pane_header_toggle() {
                PaneHeaderToggle::Leading => {
                    (PANE_HEADER_TITLE_AFTER_TOGGLE, PANE_HEADER_PADDING_X)
                }
                PaneHeaderToggle::Trailing => (PANE_HEADER_PADDING_X, PANE_HEADER_TOGGLE_GAP),
                PaneHeaderToggle::None => (PANE_HEADER_PADDING_X, PANE_HEADER_PADDING_X),
            });
        let mut slots: Vec<Element<'a, Message>> = Vec::new();
        // `enz_window_title` is the one framework child known to report zero
        // intrinsic width. Preserve that fact before moving `title` below so
        // `TitlebarLayout` can distinguish it from measurable caller chrome.
        let has_window_title = title.as_ref().is_some_and(|title| !title.is_empty());

        // ---- slot 0: leading controls (back, sidebar) -----------------
        slots.extend(leading);

        // ---- slot 1: app icon ------------------------------------------
        // Fixed, icon-sized: the row's `Alignment::Center` centres it on the
        // midline. Deriving its height from the band would be the layout
        // binding loop.
        if let Some(name) = app_icon {
            slots.push(crate::icon_slot::enz_icon_slot(name).into_element());
        }

        // ---- slot 2: title / subtitle ----------------------------------
        if let Some(title) = title.filter(|t| !t.is_empty()) {
            // The title starts at the band's leading edge, after any leading
            // controls, in every bar; `.center(..)` is for content that
            // belongs in the middle of the band.
            let mut wt = crate::window_title::enz_window_title(title)
                .centered(false)
                .pane_header(pane_header);
            if let Some(sub) = subtitle.filter(|s| !s.is_empty()) {
                wt = wt.subtitle(sub);
            }
            // A bar used as pane chrome keeps its heading rung. Absent
            // by default, so a window title publishes exactly what it did.
            if let Some(level) = title_heading {
                wt = wt.heading(level);
            }
            slots.push(wt.into_element());
        }

        // ---- slot 3: caller content ------------------------------------
        slots.extend(content);

        // ---- slot 3b: the trailing action slot -----------------
        //
        // Built before the content row, because whether it exists decides how
        // the content row is sized: with actions the leading chrome must
        // measure at its intrinsic width (`Shrink`) so the actions can be
        // handed the residual, and the actions' own `Fill` covers the gap the
        // drag region needs. With no actions nothing changes at all.
        let action_can_render_without_intrinsic_width = actions.iter().any(|item| {
            let descriptor = item.descriptor();
            descriptor.visible
                && (descriptor.policy != enzui_core::action_overflow::OverflowPolicy::Pinned
                    || item.declares_layout_extent())
        });
        let (action_min_width, action_reserve_width) = if actions.is_empty() {
            (0.0, 0.0)
        } else {
            let pinned = actions
                .iter()
                .filter(|item| {
                    let descriptor = item.descriptor();
                    descriptor.visible
                        && descriptor.policy == enzui_core::action_overflow::OverflowPolicy::Pinned
                })
                .count();
            let foldable = actions
                .iter()
                .filter(|item| {
                    let descriptor = item.descriptor();
                    descriptor.visible
                        && descriptor.policy
                            == enzui_core::action_overflow::OverflowPolicy::FoldIfNeeded
                })
                .count();
            let collector = usize::from(actions.iter().any(|item| {
                let descriptor = item.descriptor();
                descriptor.visible
                    && descriptor.policy != enzui_core::action_overflow::OverflowPolicy::Pinned
            }));
            let kept = action_max_inline.map_or(foldable, |limit| foldable.min(limit));
            let control =
                crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.effective_control_size();
            let width_for = |slots: usize| {
                slots as f32 * control
                    + slots.saturating_sub(1) as f32 * enzui_theme::SpaceToken::Xs.px()
            };
            (
                width_for(pinned + collector),
                width_for(pinned + kept + collector),
            )
        };
        let actions_element: Option<Element<'a, Message>> = match on_toggle_overflow {
            Some(toggle) if !actions.is_empty() => {
                let mut bar = crate::action_bar::enz_action_bar(actions, toggle)
                    .overflow_open(overflow_open)
                    .fold_policy(action_fold_policy)
                    .collector(action_collector)
                    .collector_visibility(action_collector_visibility)
                    .collector_placement(action_collector_placement)
                    .spacing(enzui_theme::SpaceToken::Xs)
                    // Actions are trailing chrome. Any residual between them
                    // and measured leading content remains draggable band.
                    .align_x(iced::alignment::Horizontal::Right);
                if let Some(limit) = action_max_inline {
                    bar = bar.max_inline_actions(limit);
                }
                if let Some(delay) = tooltip_delay {
                    bar = bar.tooltip_delay(delay);
                }
                if let Some(prefix) = action_menu_id_prefix {
                    bar = bar.menu_id_prefix(prefix);
                }
                if let Some(id) = action_overflow_trigger_id {
                    bar = bar.overflow_trigger_id(id);
                }
                if let Some(dismiss) = on_action_dismiss {
                    bar = bar.on_dismiss(dismiss);
                }
                Some(bar.into())
            }
            _ => None,
        };
        // The content row fills the HORIZONTAL axis so the controls pin to the
        // trailing edge and the drag surface covers the gap. The vertical axis
        // stays Fixed on the band below — see the module docs; do not swap
        // these.
        //
        // With an actions slot present the roles swap — the ACTIONS
        // element is the `Fill` one, so this row must shrink to its intrinsic
        // width or it would eat the whole residual and fold every action.
        //
        // It stays `Fill` even with an actions slot. `Shrink` looks
        // like the obvious way to let the actions have the residual, and it
        // silently DELETES the title: `enz_window_title` is a `Fill`-width
        // child, and a `Fill` child under `Shrink` compression resolves to its
        // minimum — zero. Captured at 375 px and 1400 px with the title simply
        // gone. `TitlebarLayout` instead measures this row's intrinsic width
        // under an unbounded probe and then assigns it exactly that width, so
        // a `Fill` child inside it still has a definite slot to fill.
        let content_row: Element<'a, Message> = row(slots)
            .mirrored(rtl)
            .spacing(spacing)
            .align_y(Alignment::Center)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();

        // ---- slot 4: window controls -----------------------------------
        //
        // The controls' width is RESERVED before content is measured,
        // so no caller content — a `Length::Fixed(2000.0)` child, an
        // unbounded label, a wide cluster — can push the trailing cluster
        // out of the band. Before this the row was a plain
        // `row(slots).push(controls)` and the band's `clip(true)` silently
        // truncated exactly the region the window controls occupied,
        // yielding a Close button that laid out at 0x28 and could not be
        // hit (and per-app an app's workaround). Presence tests
        // still passed because a clipped node is still published to
        // AccessKit — hence the gates in `tests` below assert **rendered
        // width**, not presence.
        //
        // Wrapped in a NON-draggable region: a press on close must never be
        // able to promote into a window move. The region still delegates to
        // the buttons first, so their clicks are unaffected.
        let controls_element: Option<Element<'a, Message>> =
            if renders_controls && let Some(on_gesture) = &on_gesture {
                let mut controls = enz_window_controls(on_gesture(WindowCommand::Close))
                    .mirrored(rtl)
                    .show_close(show_close)
                    .show_minimize(show_minimize, on_gesture(WindowCommand::Minimize))
                    .show_maximize(
                        show_maximize,
                        on_gesture(WindowCommand::ToggleMaximize),
                        maximized,
                    )
                    .fullscreen(fullscreen);
                if let Some(delay) = tooltip_delay {
                    controls = controls.tooltip_delay(delay);
                }
                if let Some(order) = control_order {
                    controls = controls.order(order);
                }
                // Identity comes from the caller's key plus each control's
                // FIXED role suffix, never from its slot position, so reordering
                // the cluster cannot repoint a routed AT request.
                if let Some(owner) = &owner {
                    controls = controls.id_prefix(owner.display());
                }
                Some(
                    titlebar_drag_region(controls.into_element())
                        .draggable(false)
                        .system_menu(false)
                        .into(),
                )
            } else {
                None
            };

        // The bar body: content on the leading side, controls pinned to the
        // trailing edge with their width RESERVED. See [`TitlebarLayout`].
        //
        // `padding_x` Is handed to the LAYOUT rather than applied as
        // band padding, because the two horizontal edges no longer take the
        // same inset. The leading edge (and the trailing edge of a bar with no
        // controls) is content, and takes `padding_x`; the controls edge takes
        // an inset the layout DERIVES from the band height it is actually
        // given. Applying `padding_x` uniformly on the band -- which is what
        // this did before -- is exactly what made the horizontal inset a
        // constant while the vertical one was derived.
        // The height the controls are actually centred in, measured
        // from the window. Inside a shell this is always the band the shell
        // reserved. Since a bar with no explicit `.height(..)` also
        // PAINTS its band at that same height (`resolved_height` above took the
        // shell scope), so band and centring height coincide by construction:
        //
        // * default (no `.height`) -- both `height` and this are the shell's
        //   band, so the bar lays out exactly in the slot with no clamp. (This
        //   is the stock case: shell band 40, bar band 40.)
        // * explicit `.height` SHORTER than the shell band -- the bar keeps its
        //   own height and the shell's `centred_in_band` adds
        //   `(shell_h - bar_h)/2` above it. The controls' distance from the
        //   band's top is then that slack plus `(bar_h - control)/2`, which
        //   telescopes to `(shell_h - control)/2`.
        //
        // Both cases come to "centred in the shell's band", so the shell's
        // height is the single right input and no `min`/`max` against the
        // bar's own height belongs here. Standalone there is no shell, and the
        // bar's own height is the answer.
        let centring_height = shell_band_height().unwrap_or(height);

        // A desktop that puts window buttons first (`close,minimize:`) gets
        // them at the start of the content, before the icon and title.
        let (content_row, controls_element) = match controls_element {
            Some(controls) if crate::window_controls::controls_lead() => (
                row![controls, content_row]
                    .spacing(spacing)
                    .align_y(Alignment::Center)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                None,
            ),
            controls => (content_row, controls),
        };

        let bar: Element<'a, Message> = TitlebarLayout::new(
            content_row,
            center,
            actions_element,
            controls_element,
            spacing,
            has_window_title,
            action_min_width,
            action_reserve_width,
            action_can_render_without_intrinsic_width,
            fluid_content_min_width,
            modal_navigation,
        )
        .padding_x(if pane_padding.is_some() {
            0.0
        } else {
            padding_x
        })
        .mirrored(rtl)
        .centring_height(centring_height)
        .height(Length::Fill)
        .into();
        let bar: Element<'a, Message> = match pane_padding {
            Some((leading, trailing)) => {
                let (left, right) = if rtl {
                    (trailing, leading)
                } else {
                    (leading, trailing)
                };
                container(bar)
                    .padding(Padding::ZERO.left(left).right(right))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
            None => bar,
        };

        // ---- the band --------------------------------------------------
        //
        // Length::Fixed, not Fill: a titlebar is a fixed band. `clip(true)`
        // makes an oversized slot child elide inside the band instead of
        // escaping it. The clip still bites on the content slot (an
        // oversized *title* still elides inside the band),
        // but it can no longer clip the controls — because they never overrun
        // in the first place.
        // NO horizontal padding here -- `TitlebarLayout` now insets each
        // edge itself, because the content edge and the controls edge take
        // different insets. `clip(true)` is unaffected: iced clips a container
        // to its OUTER bounds, which padding never entered into.
        let band = container(bar)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .padding(Padding::ZERO)
            .clip(true)
            // One style boundary, resolving one colour from the live theme per
            // render. See [`band_style`].
            .style({
                let paint = band_style(tone);
                // The glass paints the band.
                move |theme: &iced::Theme| match material {
                    Some(_) => container::Style::default(),
                    None => paint(theme),
                }
            });

        // The drag surface wraps the whole band so empty space moves the
        // window. It delegates to children first, so the controls and any
        // content buttons keep their clicks.
        let dragged: Element<'a, Message> = match on_gesture {
            Some(f) => titlebar_drag_region(band)
                .draggable(draggable)
                .system_menu(draggable)
                .on_gesture(f)
                .into(),
            // Unwired: no drag region at all, so an inert bar's geometry and
            // event routing are exactly a plain container's.
            None => band.into(),
        };
        let dragged = match material {
            Some(style) => {
                crate::material_boundary::with_chrome_visibility(!modal_navigation, || {
                    crate::glass::enz_glass(dragged)
                        .style(style)
                        .radius(0.0)
                        .elevation(0)
                        .into()
                })
            }
            None => dragged,
        };

        // The labelled semantic owner. `new` (group), not `leaf`: the controls
        // and content publish their own nodes and must become this group's AT
        // children.
        let mut wrapper = crate::semantics_container::SemanticsContainer::new(dragged, semantics);
        if let Some(owner) = &owner {
            wrapper = wrapper.id(owner.id("bar"));
        }
        wrapper.into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzTitleBar<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTitleBar<'a, Message>) -> Self {
        w.into_element()
    }
}

include!("title_bar/layout.rs");
