//! EnzFullScreenOverlay: dimming scrim + centred content + close button,
//! hosted over [`EnzModalHost`](crate::modal_host::EnzModalHost).
//!
//! Renders a full-window scrim
//! with the caller's content centred on both axes and a top-right
//! close button. A tap anywhere on the scrim fires `on_close`; the
//! close button fires the same message; Escape does too.
//!
//! # Why a host and not a bare `stack`
//!
//! A passive scrim on iced's `stack` only *looks* modal and owns no modality:
//! the vendored `stack`
//! traverses every layer for operations and events
//! (`iced/widget/src/stack.rs`), so Tab walked straight into
//! the shielded content behind the scrim and the AT tree announced it as
//! reachable — and a stack has no unified Escape dismissal. These are
//! not separate responsibilities; they belong to the modal layer, and
//! [`EnzModalHost`](crate::modal_host) is that layer. This module composes
//! it — the surface (the centred content) is built here, and every *exit*
//! (a Close press, Escape, a scrim press) and every focus move funnels
//! through one [`FullScreenOverlayEvent`].
//!
//! # Animation
//!
//! A fade-in requires a caller-owned `Subscription` timer to lerp the scrim
//! alpha because iced 0.15 has no built-in property animation. This widget
//! currently presents the final scrim alpha immediately.

use crate::button::{ButtonIconPosition, ButtonKind, enz_button_with};
use crate::modal_host::ModalHostEvent;
use crate::tokens::{ButtonSizeTokens, ButtonTokens};
use enzui_core::dismissal::DismissPolicy;
use enzui_core::modal::DismissCause;
use iced::widget::{container, stack, text};
use iced::{Color, Element, Length};

/// What a presented full-screen overlay reports to its host.
///
/// # One exit, not several
///
/// A full-screen overlay can end by its Close button, by Escape, or by a
/// press on the scrim. Only the first originates inside the overlay; the
/// other two are noticed by the [`modal host`](crate::modal_host) wrapped
/// *around* it. Routing them to separate messages gives the application
/// several places to close one overlay, and the one it forgets is the one
/// that leaks. So every ending is a [`Dismissed`](Self::Dismissed), whatever
/// noticed it, and [`from_host`](Self::from_host) is the funnel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FullScreenOverlayEvent {
    /// The overlay should close, for this reason.
    ///
    /// The reason is a [`DismissCause`] rather than a bare unit, so the
    /// application can tell a Close press from an Escape from a scrim press
    /// if it ever needs to — a plain overlay treats them alike.
    Dismissed(DismissCause),
    /// Tab inside the overlay: move focus onward, wrapping at the end.
    ///
    /// Carried through rather than swallowed, because focus containment is
    /// the host's job.
    FocusNext,
    /// Shift+Tab inside the overlay.
    FocusPrevious,
}

impl FullScreenOverlayEvent {
    /// Funnel a [`ModalHostEvent`] into
    /// the overlay's own vocabulary.
    ///
    /// This is what makes "the overlay ended" a single case for the
    /// application no matter who noticed it.
    #[must_use]
    pub const fn from_host(event: ModalHostEvent) -> Self {
        match event {
            ModalHostEvent::Dismissed(cause) => Self::Dismissed(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// The focus operation this event calls for, if it is a focus move.
    ///
    /// This operation has no modal-stack context and therefore always wraps
    /// focus. A root overlay can call it directly. A host that can cover the
    /// overlay through a [`ModalStack`](enzui_core::modal::ModalStack)
    /// must first check
    /// [`ModalStack::is_topmost`](enzui_core::modal::ModalStack::is_topmost)
    /// or use [`crate::modal_host::scoped_focus_operation`].
    #[must_use]
    pub fn focus_operation<T>(&self) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }
}

/// The dismissal policy a full-screen overlay uses.
///
/// A media overlay is not an alert — it is informational, so Escape and the
/// scrim both back out of it. A free function so the render path and a test
/// ask the same question.
#[must_use]
pub const fn full_screen_dismiss_policy() -> DismissPolicy {
    DismissPolicy::modal()
}

/// The namespace the overlay's close button derives its id under. A
/// full-screen overlay is modal (one at a time), so a fixed `"close"` suffix
/// under this prefix is a unique, stable id that keeps the close affordance
/// routable instead of stripped at release.
pub const DEFAULT_ID_PREFIX: &str = "enz-full-screen-overlay";

/// Builder for a full-screen scrim overlay with centred content and a close
/// button. See [`enz_full_screen_overlay`].
#[must_use]
pub struct EnzFullScreenOverlay<'a, Message: Clone> {
    content: Element<'a, Message>,
    on_close: Option<Message>,
    scrim_color: Color,
    close_color: Color,
    close_size: f32,
    close_icon_size: f32,
    close_inset: f32,
    close_tokens: Option<ButtonTokens>,
    show_close_button: bool,
    /// Accessible name the presented dialog publishes on the modal host.
    ///
    /// Threaded to [`crate::modal_host::EnzModalHost::label`] so the overlay
    /// appears in the AT tree as a named `Role::Dialog`, not an unnamed one.
    /// Defaults to `"Overlay"` when unset — the sibling card dialogs default
    /// to their app-name / "Keyboard shortcuts" — so the AT tree announces
    /// *something* rather than an anonymous dialog.
    label: Option<String>,
}

/// Convenience constructor: `enz_full_screen_overlay(content)`.
pub fn enz_full_screen_overlay<'a, Message: Clone + 'a>(
    content: Element<'a, Message>,
) -> EnzFullScreenOverlay<'a, Message> {
    EnzFullScreenOverlay {
        content,
        on_close: None,
        scrim_color: DEFAULT_SCRIM,
        // White is *measured*-correct here, not assumed. Unlike an
        // accent/danger fill, the close button sits on the scrim, and the
        // scrim is 55% black over arbitrary app content — so the worst case a
        // caller can produce is pure white behind it, which composites to
        // rgb(0.45) and still gives white ink 4.76:1 (AA for text, and the
        // glyph only owes WCAG 1.4.11's 3:1). Every darker backdrop is
        // strictly better: over the shipped dark background it is 19.44:1.
        // `default_scrim_close_ink_meets_aa_over_any_content` pins that, so
        // lightening DEFAULT_SCRIM fails a test instead of shipping.
        close_color: Color::WHITE,
        close_size: 36.0,
        close_icon_size: CLOSE_ICON_SIZE,
        close_inset: 16.0,
        close_tokens: None,
        show_close_button: true,
        label: None,
    }
}

/// Content-text ink for the no-`on_close` arm.  Like the close button, the
/// content sits on the dark scrim, so its text colour must be light
/// regardless of the app theme.  White is the same choice the close glyph
/// makes and gives the same audited contrast floor (4.76:1 worst case).
///
/// Without this, content text inherits the app theme's ink, which in light
/// theme is `(0x30, 0x2D, 0x32)` — a dark colour on a dark scrim at 1.85:1.
///
/// # It only reaches content that declined to choose
///
/// The overlay establishes this on a container, i.e. on iced's inherited
/// `renderer::Style::text_color` channel — which a leaf reads only when it
/// installed no colour of its own. A bare `iced::widget::text` reads it; an
/// [`enz_text`](mod@crate::enz_text) does not (it pins its own ink, which is
/// what `TextTone` and `subtle_on` are), and an [`icon`](mod@crate::icon) has no
/// text channel at all. So content built from this framework's own text
/// primitives paints the app-theme ink on a 55%-black scrim — 2.60:1 on the
/// light preset. Pin this constant on the content
/// (`enz_text(..).color(CONTENT_INK)`, `enz_icon(..).tint(CONTENT_INK)`), or
/// pass `enz_text(..).inherit_color(true)` to opt that run into the channel.
/// [`crate::inherited_ink`] states the rule and ships the probe that measures
/// it.
pub const CONTENT_INK: Color = Color::WHITE;

/// Default scrim: semi-transparent dark. This is a deliberate divergence
/// from the `media-overlay-scrim` (`#000000cc`, alpha 0.80): ours is alpha
/// 0.55, the lightest scrim that still keeps the white close glyph legible over
/// *any* app content. The alpha is pinned to a measured contrast floor — a
/// worst-case white backdrop composites to rgb(0.45), giving white ink 4.76:1
/// (see [`enz_full_screen_overlay`]) — so it is chosen for that floor, not to
/// match the token's weight.
pub const DEFAULT_SCRIM: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.55);

/// Default close-glyph size in logical pixels, independent of the hit target.
pub const CLOSE_ICON_SIZE: f32 = 16.0;

// The close control floats on a dark scrim in both themes.
const CLOSE_HOVER_WASH: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.15);
const CLOSE_PRESSED_WASH: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.30);

fn scrim_close_button_tokens(close_color: Color) -> ButtonTokens {
    ButtonTokens {
        ghost_bg: Color::TRANSPARENT,
        ghost_hover_bg: CLOSE_HOVER_WASH,
        ghost_pressed_bg: CLOSE_PRESSED_WASH,
        ghost_fg: close_color,
        ghost_border_width: 0.0,
        focus_ring: Color::from_rgba(1.0, 1.0, 1.0, 0.9),
        ..ButtonTokens::dark()
    }
}

impl<'a, Message: Clone + 'a> EnzFullScreenOverlay<'a, Message> {
    /// Message emitted when the overlay is dismissed (close button, scrim
    /// tap, or Escape). Unset by default; without it the overlay cannot be
    /// dismissed.
    pub fn on_close(mut self, msg: Message) -> Self {
        self.on_close = Some(msg);
        self
    }
    /// Override the backdrop scrim colour. Defaults to [`DEFAULT_SCRIM`] (55%
    /// black), whose alpha is what keeps the close glyph legible over any
    /// content.
    pub fn scrim_color(mut self, c: Color) -> Self {
        self.scrim_color = c;
        self
    }
    /// Set close-glyph ink; defaults to white on the dark scrim in both themes.
    /// A pinned [`Self::close_tokens`] snapshot takes precedence regardless of
    /// builder call order. Pair custom ink with a suitable [`Self::scrim_color`].
    pub fn close_color(mut self, c: Color) -> Self {
        self.close_color = c;
        self
    }
    /// Close-button size in px. Non-finite or non-positive input falls back to 36.
    pub fn close_size(mut self, s: f32) -> Self {
        self.close_size = enzui_core::metrics::FinitePositive::new_or(s, 36.0).get();
        self
    }
    /// Set close-glyph size in logical pixels; defaults to [`CLOSE_ICON_SIZE`].
    /// Invalid or non-positive values use the default. This does not resize the
    /// hit target; configure that with [`Self::close_size`].
    pub fn close_icon_size(mut self, size: f32) -> Self {
        self.close_icon_size =
            enzui_core::metrics::FinitePositive::new_or(size, CLOSE_ICON_SIZE).get();
        self
    }

    /// Set the close control's inset from the top and right edges in logical pixels.
    /// Defaults to `16.0`; invalid values use the default, and zero reaches the edges.
    pub fn close_inset(mut self, inset: f32) -> Self {
        self.close_inset = enzui_core::metrics::FiniteNonNeg::new_or(inset, 16.0).get();
        self
    }

    /// Pin the close control's paint to a [`ButtonTokens`] snapshot.
    /// The Ghost fill, hover/pressed fills, ink, border and focus fields apply;
    /// geometry comes from [`Self::close_size`] and [`Self::close_icon_size`].
    /// This takes precedence over [`Self::close_color`] regardless of call order.
    /// Unset by default, so the close control uses white ink and washes on the scrim.
    pub fn close_tokens(mut self, tokens: ButtonTokens) -> Self {
        self.close_tokens = Some(tokens);
        self
    }

    /// Show the close button. Defaults to `true`.
    pub fn show_close_button(mut self, v: bool) -> Self {
        self.show_close_button = v;
        self
    }
    /// Set the accessible name the presented dialog publishes on its
    /// [`crate::modal_host::EnzModalHost`]. Threaded to
    /// [`crate::modal_host::EnzModalHost::label`] in [`Self::into_modal_host`]
    /// so the overlay is a named `Role::Dialog` in the AT tree, not an
    /// anonymous one.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The accessible name, defaulting to `"Overlay"` when unset. Threaded
    /// through [`Self::into_modal_host`] so the presented dialog is named
    ///
    /// Returned as a `&str` (rather than as the raw `Option`) because a
    /// caller reading the label wants the *effective* name — the same string
    /// the host ultimately publishes to the AT tree — not to re-derive the
    /// default at every call site.
    #[must_use]
    pub fn resolved_label(&self) -> String {
        self.label.as_deref().map_or_else(
            || enzui_core::localization::text("enzui-full-screen-overlay-overlay"),
            str::to_owned,
        )
    }

    /// The dismissal policy this overlay uses.
    #[must_use]
    pub const fn policy(&self) -> DismissPolicy {
        full_screen_dismiss_policy()
    }

    /// Whether the overlay draws a close button when hosted.
    ///
    /// On the host path the dismiss message comes from the host's `on_event`
    /// (always available), so this depends only on [`show_close_button`]. The
    /// legacy [`into_element`](Self::into_element) path additionally needs an
    /// [`on_close`](Self::on_close) to have a message to emit; that extra
    /// condition lives at that call site, not here.
    ///
    /// [`show_close_button`]: Self::show_close_button
    #[must_use]
    pub const fn draws_close_button(&self) -> bool {
        self.show_close_button
    }

    fn close_layer(
        &self,
        close_msg: Option<Box<dyn Fn() -> Message + 'a>>,
    ) -> Option<Element<'a, Message>> {
        let msg = close_msg?;
        let close_size = self.close_size;
        let close_color = self.close_color;

        let geometry = ButtonSizeTokens {
            height: close_size,
            padding_x: 0.0,
            text_size: self.close_icon_size,
            icon_size: self.close_icon_size,
            radius: close_size / 2.0,
            gap: 0.0,
        };

        // Hand the *closure* to the button so the Close message is produced on
        // activation, never during host construction.
        let btn = enz_button_with("", msg)
            .kind(ButtonKind::Quiet)
            .icon("action.close")
            .icon_position(ButtonIconPosition::Only)
            .aria_label(enzui_core::localization::text(
                "enzui-full-screen-overlay-close-overlay",
            ))
            // Without an id the close button is anonymous and release strips its
            // Click; an AT user could see "Close overlay" and not press it. A
            // full-screen overlay is modal — at most one is up at a
            // time — so a fixed derived id is unique enough (the "close" suffix
            // mirrors the banner pattern).
            .id(enzui_core::identity::derive_id(DEFAULT_ID_PREFIX, "close"))
            .tokens(
                self.close_tokens
                    .clone()
                    .unwrap_or_else(|| scrim_close_button_tokens(close_color)),
            )
            .size_tokens(geometry)
            .width(Length::Fixed(close_size));

        Some(
            container(btn)
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Top)
                .padding(self.close_inset)
                .into(),
        )
    }

    /// Host this overlay over `background`, funnelling every exit and focus
    /// move through `on_event`.
    ///
    /// This is the full-fidelity entry point: the caller receives a
    /// [`FullScreenOverlayEvent`] that reports *why* a dismissal happened and
    /// carries focus moves for containment.
    ///
    /// The corner close button is stacked *above* the finished host, not
    /// inside the host's surface. That is the overlay's one structural
    /// difference from the card dialogs: its close button floats on the
    /// *scrim* at the window's top-right corner rather than beside the
    /// content. Folding it into the surface would make the host's
    /// `SwallowPress` cover the whole window and eat every scrim press, so it
    /// sits above the host, where it dismisses exactly like the scrim.
    #[must_use]
    pub fn into_host_with(
        self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(FullScreenOverlayEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        let on_event = std::rc::Rc::new(on_event);

        // The close button (`x`) reports a close-affordance dismissal, not an
        // affirmative action, funnelled into the same event the host produces
        // for Escape and the scrim.
        let close_msg: Option<Box<dyn Fn() -> Message + 'a>> =
            self.draws_close_button().then(|| {
                let on_event = std::rc::Rc::clone(&on_event);
                Box::new(move || {
                    on_event(FullScreenOverlayEvent::Dismissed(DismissCause::CloseButton))
                }) as Box<dyn Fn() -> Message + 'a>
            });
        let close_layer = self.close_layer(close_msg);

        let host = {
            let on_event = std::rc::Rc::clone(&on_event);
            self.into_modal_host(background, move |event| on_event(event))
        };
        let host_element = host.into_element();

        match close_layer {
            // Wrapped in `ExclusiveFocus`: The corner close button is a
            // focusable upper `stack` layer, and `stack::update` returns at
            // the first capture — so a control focused inside the dialog below
            // would keep its focus while the close button took focus too, two
            // owners at once. The wrapper re-runs the focus decision over the
            // layers dispatch skipped. Only the stacked branch needs it; the
            // `None` arm has a single layer and nothing to strand.
            Some(close) => crate::exclusive_focus::exclusive_focus(
                stack![host_element, close]
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .into(),
            None => host_element,
        }
    }

    /// The configured [`EnzModalHost`](crate::modal_host::EnzModalHost) that
    /// centres and shields the overlay's content, before it is built into an
    /// element.
    ///
    /// Split out for the same reason as
    /// [`EnzShortcutsDialog::into_modal_host`](crate::shortcuts_dialog::EnzShortcutsDialog::into_modal_host):
    /// returning the *element* leaves nothing to assert against, so a test
    /// could only re-ask the overlay what policy it wanted, never what the
    /// host was actually handed.
    ///
    /// The surface handed to the host is the bare centred content — no
    /// card chrome (that is the overlay's identity) and no close button. The
    /// host shrink-wraps and centres it, so its `SwallowPress` covers only the
    /// content and a press anywhere else on the window reaches the scrim and
    /// dismisses. The corner close button is added by
    /// [`into_host_with`](Self::into_host_with) *above* this host; see the
    /// note there.
    #[must_use]
    pub fn into_modal_host(
        self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(FullScreenOverlayEvent) -> Message + 'a,
    ) -> crate::modal_host::EnzModalHost<'a, Message> {
        let policy = self.policy();
        let scrim = self.scrim_color;
        // The presented dialog owes a name to the AT tree. The label is
        // threaded here
        // rather than resolved inside `into_modal_host`'s call site so the
        // effective name is the same one `resolved_label` reports.
        let label = self.resolved_label();
        let content = self.content;

        crate::modal_host::enz_modal_host(background, Some(content), move |event| {
            on_event(FullScreenOverlayEvent::from_host(event))
        })
        .policy(policy)
        .label(label)
        .scrim(scrim)
    }

    /// Consume the builder and produce the widget tree.
    ///
    /// This is the convenience seam exercised by
    /// [`From<Self> for Element`] and by anything that ends `.into()`. It
    /// hosts the overlay over an *empty* `iced::widget::Space` rather than
    /// the caller's real application background, so:
    ///
    /// - Nothing is behind the scrim. A press outside the surface
    ///   dismisses without revealing anything, because there is nothing to
    ///   reveal. The modal does not sit *over* the app; it *replaces* it in
    ///   its slot. Real applications almost always want the opposite — the
    ///   app rendered behind the scrim and made inert.
    /// - Tab publishes nothing. The caller supplied only a single
    ///   `on_close` message and has no way to run a focus
    ///   `Operation` against a runtime
    ///   task, so the host is configured with
    ///   [`crate::modal_host::EnzModalHost::no_focus_events`]: Tab is still
    ///   captured (it cannot walk into anything) but no message is emitted.
    ///   Escape, the close button, and a scrim press all still route to
    ///   `on_close`; only focus events are suppressed.
    ///
    /// **For anything user-facing use [`into_host_with`](Self::into_host_with)
    /// instead**, handing it the app's element as the background and a
    /// callback that discriminates [`FullScreenOverlayEvent`] variants. That
    /// path renders the app behind an [`crate::inert::Inert`] wrapper so it
    /// stays visible-but-frozen (scroll offsets, text cursors, gestures
    /// preserved), wires focus properly, and tells Escape from a
    /// scrim press from the Close button.
    ///
    /// `.into()` is retained for probes and inert previews where an empty
    /// background is exactly what is wanted.
    pub fn into_element(self) -> Element<'a, Message> {
        let background: Element<'a, Message> = iced::widget::Space::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        match self.on_close.clone() {
            // Every dismissal collapses onto the single `on_close` message
            // the builder promises; Escape and the scrim reach it too.
            //
            // Tab must not collapse onto `on_close`.
            // The rich `into_host_with` path publishes `FocusNext`/
            // `FocusPrevious` for the application to run against
            // [`scoped_focus_operation`](crate::modal_host::scoped_focus_operation);
            // here the caller has supplied only a single message and cannot
            // run an operation, so the honest wiring is to *suppress* the
            // focus events (still capturing Tab so it cannot walk into the
            // background). Achieved by building the host directly with
            // [`EnzModalHost::no_focus_events`] rather than going through
            // `into_host_with`, which unconditionally wires focus.
            Some(on_close) => {
                let on_event = std::rc::Rc::new(move |_event| on_close.clone());
                let close_msg: Option<Box<dyn Fn() -> Message + 'a>> =
                    self.draws_close_button().then(|| {
                        let on_event = std::rc::Rc::clone(&on_event);
                        Box::new(move || {
                            on_event(FullScreenOverlayEvent::Dismissed(DismissCause::CloseButton))
                        }) as Box<dyn Fn() -> Message + 'a>
                    });
                let close_layer = self.close_layer(close_msg);
                let host = {
                    let on_event = std::rc::Rc::clone(&on_event);
                    self.into_modal_host(background, move |event| on_event(event))
                        .no_focus_events()
                };
                let host_element = host.into_element();
                match close_layer {
                    // Same composition as `into_host_with` above.
                    Some(close) => crate::exclusive_focus::exclusive_focus(
                        stack![host_element, close]
                            .width(Length::Fill)
                            .height(Length::Fill),
                    )
                    .into(),
                    None => host_element,
                }
            }
            // No `on_close`: the overlay cannot be dismissed by the user (the
            // builder doc says so). The modal host needs a `Message` for every
            // exit it wires, and there is none, so present a themed scrim over
            // the centred content directly rather than hosting. This is an
            // inert preview (no scrim dismiss, no Escape) — the only path that
            // reaches it is a caller who deliberately built an overlay with no
            // way out, e.g. a static A/B specimen.
            None => {
                let _ = background;
                let scrim_color = self.scrim_color;
                let content = self.content;
                let scrim_layer = container(text(""))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(move |_t: &iced::Theme| iced::widget::container::Style {
                        background: Some(scrim_color.into()),
                        ..Default::default()
                    });
                // The scrim is always dark (55% black), so content
                // text must be light or it drowns.  Establishing white here
                // mirrors the close-button treatment; a caller who set an
                // explicit colour on their own text widget still wins because
                // iced resolves the widget's own `color` first and falls
                // back to the container's `text_color` only when none is set.
                //
                // Read that last clause as the limit it is. This ink
                // reaches a **bare `iced::widget::text`** and nothing else.
                // Every enzUI text primitive installs a colour of its own, so
                // an `enz_text` (or an icon, which has no text channel at
                // all) content element paints `palette.text` here and the
                // white below is dead — measured at 2.60:1 on the light
                // preset's scrim by
                // `full_screen_overlay_content_ink_reaches_a_text_and_not_an_enz_text`
                // (`tests/foundations/inherited_ink.rs`). Callers who pass an
                // ink-owning element must pin [`CONTENT_INK`] on it. See
                // [`crate::inherited_ink`] for the whole rule.
                let content_layer = container(content)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill)
                    .style(|_t: &iced::Theme| iced::widget::container::Style {
                        text_color: Some(CONTENT_INK),
                        ..Default::default()
                    });
                stack![scrim_layer, content_layer]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzFullScreenOverlay<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzFullScreenOverlay<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
