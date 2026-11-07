//! Low-level sheet builder and gesture-event vocabulary.

use super::*;
// `iced::widget::column` is both a function and a macro, so the glob
// above and the prelude are two equally-valid candidates (E0659). An
// explicit import beats a glob and settles it.
use iced::widget::column;

/// One step of a real pointer or touch drag, in the vocabulary
/// [`SheetDrag`] already speaks.
///
/// This is the whole gesture *adapter*: the widget turns raw
/// [`Event::Mouse`]/[`Event::Touch`] into these four cases, and the host
/// applies them to the [`SheetDrag`] it already owns. The model is not moved
/// into the widget deliberately — the host needs the live offset to feed back
/// through [`EnzSheet::offset`], and a drag that lived only in widget state
/// could not be rendered until it had already ended.
///
/// `Moved` carries `(distance, dt)` rather than a position because that is
/// exactly [`SheetDrag::moved`]'s signature: the adapter measures, the model
/// decides. In particular a non-finite distance is forwarded unchanged —
/// see [`SheetGesture::Moved`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SheetGesture {
    /// A pointer or finger went down on the sheet: [`SheetDrag::press`].
    Pressed,
    /// The owning pointer or finger moved: [`SheetDrag::moved`].
    ///
    /// `distance` is signed and unsanitised: an upward pull is negative.
    /// The low-level offset path clamps it while [`SheetPresenter`] consumes
    /// it for expansion. A non-finite distance is passed straight through
    /// rather than being clamped to `0.0`. Clamping it here would
    /// snap a dragging sheet back to rest under the finger, which is exactly
    /// the contract [`SheetDrag::moved`] documents.
    Moved {
        /// Signed px below the press point.
        distance: f32,
        /// Seconds since the previous sample, from the frame clock.
        dt: f32,
    },
    /// The owning pointer or finger lifted: [`SheetDrag::release`].
    ///
    /// Carries the terminal sample `(distance, dt)` — the finger's final
    /// position below the press point and the frame-clock gap since the
    /// previous `Moved`. The host folds it in through
    /// [`SheetDrag::release`]'s `terminal` argument so a lift after a rest
    /// decays the stale flick velocity rather than committing on it.
    /// Same unsanitised contract as [`Moved`](SheetGesture::Moved): the
    /// distance is signed and the model owns the clamp.
    Released {
        /// Signed px below the press point at the lift.
        distance: f32,
        /// Seconds since the previous sample, from the frame clock.
        dt: f32,
    },
    /// The gesture was abandoned — the pointer left, the window lost focus,
    /// or the compositor took the touch: [`SheetDrag::cancel`].
    Cancelled,
}

/// A bottom sheet surface.
pub struct EnzSheet<'a, Message> {
    pub(crate) content: Element<'a, Message>,
    /// The host's gesture sink. `None` leaves the sheet inert to drags, the
    /// default.
    pub(crate) on_drag: Option<Box<dyn Fn(SheetGesture) -> Message + 'a>>,
    /// Caller-pinned tokens, or `None` to resolve from the runtime theme.
    ///
    /// `None` by default: snapshotting the palette at build time
    /// renders the sheet in whatever theme was live when the view function
    /// ran, which is the wrong theme the moment the system preference
    /// changes. Resolved inside the style closures instead.
    pub(crate) tokens: Option<SheetTokens>,
    pub(crate) shape: SheetShape,
    pub(crate) offset: f32,
    /// Handle visibility override. `None` (the default) means "follow
    /// `on_drag`": the handle is a drag affordance, so a sheet that cannot be
    /// dragged shows none, and one that can shows it. `Some(_)` is an
    /// explicit caller override for a decorative handle or a forced hide.
    pub(crate) show_handle: Option<bool>,
    pub(crate) height: Option<f32>,
    pub(crate) padding: f32,
    pub(crate) grabber_drag_band: Option<f32>,
    pub(crate) on_scroll_boundary: Option<Box<dyn Fn(SheetResizeDirection) -> Message + 'a>>,
    pub(crate) scroll_handoff: bool,
    /// Whether the one-pixel surface outline is visible. Geometry is retained
    /// while transparent so the default flip cannot move content.
    pub(crate) bordered: bool,
}

/// Construct a sheet around `content`.
///
/// Colours follow the runtime `iced::Theme`. Pin them with
/// [`EnzSheet::tokens`] only when the sheet must ignore the live theme.
pub fn enz_sheet<'a, Message: 'a>(
    content: impl Into<Element<'a, Message>>,
) -> EnzSheet<'a, Message> {
    EnzSheet {
        content: content.into(),
        on_drag: None,
        tokens: None,
        shape: SheetShape::default(),
        offset: 0.0,
        show_handle: None,
        height: None,
        padding: SHEET_PADDING,
        grabber_drag_band: None,
        on_scroll_boundary: None,
        scroll_handoff: false,
        bordered: false,
    }
}

impl<'a, Message: 'a> EnzSheet<'a, Message> {
    /// Set the inner inset in logical pixels (default 24).
    /// Nonfinite or negative values use 24; zero removes the inset.
    #[must_use]
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = enzui_core::metrics::FiniteNonNeg::new_or(padding, SHEET_PADDING).get();
        self
    }

    /// Pin the colour tokens, overriding runtime theme resolution.
    #[must_use]
    pub const fn tokens(mut self, tokens: SheetTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Show the one-pixel sheet outline. Off by default.
    #[must_use]
    pub const fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    /// Edge-attached or floating.
    #[must_use]
    pub const fn shape(mut self, shape: SheetShape) -> Self {
        self.shape = shape;
        self
    }

    /// Translate the surface down by a live drag offset.
    ///
    /// Negative and non-finite offsets resolve to rest: the sheet never lifts
    /// off its edge, and an unmeasurable drag is not a position.
    #[must_use]
    pub fn offset(mut self, offset: f32) -> Self {
        self.offset = if offset.is_finite() {
            offset.max(0.0)
        } else {
            0.0
        };
        self
    }

    /// Route real pointer and touch drags to `on_drag`.
    ///
    /// Without this the sheet is inert to gestures: it renders whatever
    /// [`EnzSheet::offset`] it is given and nothing produces one. With it, a
    /// press/move/release over the surface becomes a [`SheetGesture`] stream
    /// the host applies to its [`SheetDrag`].
    ///
    /// Opt-in rather than always-on: a sheet whose host has nowhere to put
    /// the offset would follow the finger and then teleport back on the
    /// next frame.
    #[must_use]
    pub fn on_drag(mut self, on_drag: impl Fn(SheetGesture) -> Message + 'a) -> Self {
        self.on_drag = Some(Box::new(on_drag));
        self
    }

    /// Show or hide the drag handle, overriding the default.
    ///
    /// By default the handle *follows [`Self::on_drag`]*: it is a drag
    /// affordance, so a sheet with no gesture sink shows no grip, and one with
    /// a sink shows it. Call this only to force a decorative handle on a non-
    /// draggable sheet (`true`) or to hide the grip on a draggable one
    /// (`false`).
    #[must_use]
    pub const fn show_handle(mut self, show: bool) -> Self {
        self.show_handle = Some(show);
        self
    }

    pub(crate) fn grabber_drag_band(mut self, height: f32) -> Self {
        self.grabber_drag_band = Some(height);
        self
    }

    pub(crate) fn on_scroll_boundary(
        mut self,
        on_scroll_boundary: impl Fn(SheetResizeDirection) -> Message + 'a,
    ) -> Self {
        self.on_scroll_boundary = Some(Box::new(on_scroll_boundary));
        self
    }

    pub(crate) fn scroll_handoff(mut self, enabled: bool) -> Self {
        self.scroll_handoff = enabled;
        self
    }

    /// Fix the surface height to a resolved detent.
    #[must_use]
    pub fn height(mut self, height: f32) -> Self {
        self.height = if height.is_finite() && height > 0.0 {
            Some(height)
        } else {
            None
        };
        self
    }

    /// Whether the drag handle is actually rendered.
    ///
    /// The explicit [`Self::show_handle`] override wins; otherwise the handle
    /// follows [`Self::on_drag`] — present only when the sheet can be dragged.
    /// A default sheet (no `on_drag`, no override) therefore shows no grip, so
    /// there is never a drag affordance the sheet will not honour.
    /// Exposed so the rendered handle's presence is assertable without
    /// rasterising.
    #[must_use]
    pub fn effective_show_handle(&self) -> bool {
        self.show_handle.unwrap_or_else(|| self.on_drag.is_some())
    }

    /// The height the sheet's visible box takes while dragging, or `None` when
    /// the sheet is at rest or has no resolved detent to shorten.
    ///
    /// A bottom-anchored box whose visible height is `offset` shorter has its
    /// top edge exactly `offset` lower, which is what translates the sheet
    /// down; the remainder overhangs the viewport edge and is clipped. Exposed
    /// so the translation is assertable without rasterising — the render path
    /// is not reachable from a unit test.
    #[must_use]
    pub fn visible_height(&self) -> Option<f32> {
        let height = self.height?;
        if self.offset <= 0.0 {
            return None;
        }
        Some((height - self.offset).max(0.0))
    }

    /// The corner radii this sheet paints with: the theme's `dialog` radius,
    /// since a sheet is a dialog presented from an edge.
    ///
    /// Exposed so a test can assert the edge-attached/floating distinction
    /// without rasterising: the bottom corners of an edge-attached sheet are
    /// square because the surface continues past the viewport edge, and a
    /// rounded bottom would draw a gap that is not there.
    #[must_use]
    pub fn radii(&self) -> [f32; 4] {
        let radius = sheet_radius();
        match self.shape {
            // top-left, top-right, bottom-right, bottom-left
            SheetShape::EdgeAttached => [radius, radius, 0.0, 0.0],
            SheetShape::Floating => [radius; 4],
        }
    }

    /// Build the element.
    #[must_use]
    pub fn into_element(mut self) -> Element<'a, Message> {
        let radii = self.radii();
        let tokens = self.tokens;
        // Computed before `on_drag` is taken, since it depends on it, and by
        // the same call a test makes, so the render path and the assertion
        // cannot drift.
        let show_handle = self.effective_show_handle();
        let bordered = self.bordered;
        let on_drag = self.on_drag.take();
        let on_scroll_boundary = self.on_scroll_boundary.take();
        let grabber_drag_band = self.grabber_drag_band;
        let scroll_handoff = self.scroll_handoff;
        // Resolved before `self.content` is moved out, and by the same call a
        // test makes, so the render path and the assertion cannot drift.
        let visible_height = self.visible_height();

        let handle: Element<'a, Message> = if show_handle {
            container(
                container(space::Space::new())
                    .width(Length::Fixed(HANDLE_WIDTH))
                    .height(Length::Fixed(HANDLE_HEIGHT))
                    .style(move |theme: &iced::Theme| container::Style {
                        background: Some(Background::Color(resolve_tokens(tokens, theme).handle)),
                        border: Border {
                            radius: HANDLE_RADIUS.into(),
                            ..Border::default()
                        },
                        ..container::Style::default()
                    }),
            )
            .width(Length::Fill)
            .center_x(Length::Fill)
            .into()
        } else {
            space::Space::new().into()
        };

        let body: Element<'a, Message> = if show_handle {
            column![handle, self.content].spacing(8.0).into()
        } else {
            self.content
        };

        let mut surface = container(body)
            .padding(self.padding)
            .width(Length::Fill)
            .style(move |theme: &iced::Theme| {
                let tokens = resolve_tokens(tokens, theme);
                container::Style {
                    background: Some(Background::Color(tokens.surface)),
                    border: Border {
                        color: if bordered {
                            tokens.border
                        } else {
                            Color::TRANSPARENT
                        },
                        width: 1.0,
                        radius: iced::border::Radius::default()
                            .top_left(radii[0])
                            .top_right(radii[1])
                            .bottom_right(radii[2])
                            .bottom_left(radii[3]),
                    },
                    ..container::Style::default()
                }
            });

        // The surface always lays out at its full resting height, so a drag
        // never reflows the content inside it.
        if let Some(height) = self.height {
            surface = surface.height(Length::Fixed(height));
        }

        // A live drag translates the surface *down*, past the bottom edge.
        //
        // There is no transform to do that with, and the obvious trick -- a
        // spacer above the surface inside a column -- does nothing here: the
        // host bottom-aligns the sheet, so growing the column simply grows it
        // upwards and the surface stays welded to the edge.
        //
        // What actually translates a bottom-anchored box downwards is making
        // its *visible* box shorter while its content keeps the resting
        // height: bottom-aligned and `offset` shorter means the top edge sits
        // `offset` lower, and the overhanging remainder is clipped by the
        // viewport edge exactly as a real sheet's is.
        // One owner for the decision: the render path asks the same function
        // the tests do, so a change to the translation rule cannot silently
        // apply to only one of them. `None` means "at rest, or no detent to
        // shorten" -- both of which draw the surface unmodified.
        //
        // Clipped descendants must leave *interaction and
        // semantic* traversal, not just paint. Native `container(..).clip(true)`
        // narrows drawing only: iced's `Container::operate` descends into its
        // child unconditionally (`iced/widget/src/container.rs`), so
        // a control translated below the visible edge mid-drag stays Tab-
        // reachable and keeps publishing an AT node — a screen reader lands on,
        // and announces, a button that is sliding off-screen.
        //
        // `InertClip` is the interaction-boundary clip (paint + cursor + focus
        // + AT), the scroll-stack sibling: its `ClipOperation` drops
        // any node whose bounds do not intersect the visible rectangle.
        //
        // The rule here is deliberately the audit rule (drop out-of-view), not
        // the semantics rule (keep the whole tree). A scrollable's off-screen
        // content is *reachable by scrolling*, so AT keeps it; a sheet's
        // clipped remainder is being *actively translated off-screen by an
        // in-progress dismissal* and is not reachable — landing focus or an AT
        // cursor on it mid-swipe is what this clip prevents. At rest
        // (`visible_height() == None`) nothing is clipped and the whole surface
        // participates.
        let translated: Element<'a, Message> = match visible_height {
            Some(visible) => crate::inert_clip::InertClip::new(
                container(surface).height(Length::Fixed(visible)),
                true,
            )
            .into(),
            None => surface.into(),
        };

        // The gesture adapter wraps the *translated* surface, so its bounds
        // are the sheet as the user sees it: a drag that has already pushed
        // the sheet halfway off screen is grabbed where the sheet now is, not
        // where it started.
        match on_drag {
            Some(on_drag) => Element::new(SheetGestureArea {
                content: translated,
                on_drag,
                grabber_drag_band,
                on_scroll_boundary,
                scroll_handoff,
            }),
            None => translated,
        }
    }
}
