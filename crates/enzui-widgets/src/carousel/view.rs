//! Composition: turning an [`EnzCarousel`](super::EnzCarousel) builder into a
//! rendered [`Element`] tree.
//!
//! This is the seam that assembles the page body, the indicator strip (either
//! the per-dot form or the compact form built by
//! [`build_compact_indicators`]), the optional prev/next chrome, the
//! [`CarouselInput`](super::widget::CarouselInput) event wrapper, and the
//! surrounding [`SemanticsContainer`]. `into_element` is where the single
//! paging path (`PageRequest` -> `apply`) is wired to every input method.

use super::tokens::{
    ARROW_GLYPH, BODY_PADDING, BODY_SPACING, CHROME_INSET, CHROME_SIZE, CarouselStageStyle,
    CarouselTokens, DOT_SIZE, INDICATOR_SPACING, INSIDE_ARROW_INSET, Indicator, MAX_VISIBLE_DOTS,
    chrome_clearance_for,
};
use super::widget::CarouselInput;
use super::{ArrowPlacement, EnzCarousel};
use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use crate::row_motion::{RowMotion, RowMotionProgress, lerp_color};
use crate::semantics_container::SemanticsContainer;
use crate::swipe::{SwipeAxis, SwipePolicy};
use enzui_core::identity::derive_id;
use enzui_core::paging::{
    PageCursor, PageId, PageRequest, apply, carousel_semantics, duplicate_ids, indicator_semantics,
};
use iced::{Element, Length};
use std::cell::Cell;
use std::rc::Rc;

impl<'a, Message: Clone + 'a> EnzCarousel<'a, Message> {
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows
    /// (`docs/widgets.md`).
    ///
    /// `From<Self> for Element` (below) forwards here, so the two spellings
    /// are one implementation. Prefer this one inside `row![]` / `column![]`,
    /// where `.into()` has no target type to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        use iced::widget::{Row, Space, column, container, row, stack, text};

        // --- Runtime deduplication ---
        // Duplicate PageIds are data, not a programming error — they can arrive
        // from application models, files, or server responses. In release the
        // debug_assert! compiles to nothing, leaving ambiguous selection and
        // mis-routed AccessKit activation. Fix: deduplicate pages by id,
        // keeping the first occurrence and dropping later duplicates.
        let mut pages = self.pages;
        {
            let ids_before: Vec<PageId> = pages.iter().map(|p| p.id.clone()).collect();
            debug_assert!(
                duplicate_ids(&ids_before).is_empty(),
                "EnzCarousel: duplicate page ids make selection ambiguous: {:?}",
                duplicate_ids(&ids_before)
            );
        }
        let dropped = Self::dedup_pages(&mut pages);
        let _ = dropped; // Used by tests; optimizer removes in production.

        let ids: Vec<PageId> = pages.iter().map(|p| p.id.clone()).collect();

        let cursor = PageCursor::resolve(&ids, self.selected.as_ref());
        let wrap = self.wrap;
        let enabled = self.enabled;
        let pinned = self.tokens;
        let bordered = self.bordered;
        let stage_style = self.stage_style;
        let indicator_style = self.indicator;
        let id_prefix = self.id_prefix;
        let selected = self.selected.clone();
        let on_select: std::rc::Rc<dyn Fn(PageId) -> Message + 'a> =
            std::rc::Rc::from(self.on_select);

        // Resolve, then build only the active page. Inactive builders are
        // dropped unused, so no inactive page ever becomes a widget tree.
        //
        // `active_page` is the *resolved* id -- the page actually on screen after
        // fallback, not the caller's requested id, which may be stale. It keys
        // the retained `Tree` in `CarouselInput::diff`, and keying on
        // the requested id would fail to invalidate when two different stale ids
        // both fall back to the same page.
        let mut active_page: Option<PageId> = None;
        let active_content: Element<'a, Message> = match cursor.active() {
            Some(index) => {
                let page = pages.swap_remove(index);
                active_page = Some(page.id.clone());
                (page.build)()
            }
            None => text("").into(),
        };
        drop(pages);

        // The single paging path. Every input below builds a PageRequest and
        // hands it here; `apply` owns clamping, wrapping, and the "no message
        // for a no-op" rule, so no two inputs can disagree.
        //
        // `resolve` is pure: it answers "which page would this request select?"
        // from `apply` alone, without touching `on_select`. This is the
        // *availability* question, and event routing (keyboard/swipe) uses it
        // directly.
        let resolve_request = {
            let ids = ids.clone();
            move |r: &PageRequest| -> Option<PageId> {
                if !enabled {
                    return None;
                }
                apply(&ids, selected.as_ref(), r, wrap)
            }
        };
        let request = {
            let resolve_request = resolve_request.clone();
            let on_select = Rc::clone(&on_select);
            move |r: PageRequest| -> Option<Message> { resolve_request(&r).map(|id| on_select(id)) }
        };
        // Lazy factories: built per activation, never at view time.
        //
        // The view-time probe calls only the pure `resolve_request`; it never
        // runs the user-supplied `on_select`. `on_select` is invoked exactly
        // once, inside the returned closure, at real activation. Because the
        // probe already resolved the target id, the activation closure can
        // construct the message from *that* id directly -- there is no second
        // `apply` call and thus no "available at view time" hope to `expect`.
        let factory = {
            let resolve_request = resolve_request.clone();
            let on_select = Rc::clone(&on_select);
            move |r: PageRequest| -> Option<Box<dyn Fn() -> Message + 'a>> {
                // Probe for *enablement* only, without side effects.
                let target = resolve_request(&r)?;
                let on_select = Rc::clone(&on_select);
                Some(Box::new(move || on_select(target.clone())))
            }
        };

        // Is prev/next chrome actually painted? Both the inset below and the
        // `stack!` push further down read this one predicate, so the body can
        // never inset itself clear of chrome that was never built (a
        // single-page carousel, or `show_navigation(false)`).
        let chrome_shown = self.show_navigation && cursor.len() > 1;
        // The opt-in body inset. The chrome is stacked OVER the body, so
        // by default a full-bleed page paints under the arrows -- right for a
        // gallery, wrong for a page of pressable cards. `chrome_clearance()` is
        // the widget's own arithmetic over its own chrome constants, called
        // here at the paint site, so a caller never rediscovers it and it
        // cannot drift from the chrome it describes (an app that hand-computes
        // it is the drift this prevents).
        let body_inset = if self.chrome_inset && chrome_shown {
            chrome_clearance_for(self.arrow_placement)
        } else {
            0.0
        };
        let active_content = crate::rounded_clip::RoundedClip::new(active_content, 0.0)
            .radius_resolved(move |theme: &iced::Theme| {
                if stage_style == CarouselStageStyle::Transparent {
                    return iced::border::Radius::new(0.0);
                }
                let tokens = pinned.unwrap_or_else(|| CarouselTokens::from_theme(theme));
                iced::border::Radius::new(tokens.radius)
            });
        // A newly shown page fades in; the alpha is the input wrapper's.
        let page_alpha = std::rc::Rc::new(std::cell::Cell::new(1.0));
        let active_content = super::widget::PageFade {
            child: active_content.into(),
            alpha: std::rc::Rc::clone(&page_alpha),
        };
        let placement = self.arrow_placement;
        // Outside arrows sit in a gutter the page narrows to leave them.
        let gutter = if chrome_shown && placement == ArrowPlacement::Outside {
            (CHROME_SIZE + 2.0 * CHROME_INSET - BODY_PADDING).max(0.0)
        } else {
            0.0
        };
        let side = BODY_PADDING + body_inset + gutter;
        let body = container(active_content)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .padding(iced::Padding {
                left: side,
                right: side,
                ..iced::Padding::ZERO
            });

        // ---- indicators ----------------------------------------------------
        //
        // Cap the number of *individually realised* dots at
        // [`MAX_VISIBLE_DOTS`]. Above that the strip is a single
        // roving-tab-stop `focus_button` whose content is a numeric
        // "Page X of N" readout — the count and every unrealised page stay
        // describable through the parent [`carousel_semantics`] container
        // node (announced as "Page X of N", built from `cursor` unchanged),
        // so the perf bound is not an a11y regression.
        //
        // The cap is *structural*, not a taste threshold: a strip of 20 dots
        // cannot be counted at a glance, so visually the compact form is
        // what a sighted user would want anyway. See [`MAX_VISIBLE_DOTS`]
        // for the derivation.
        let indicators: Element<'a, Message> =
            if indicator_style == Indicator::None || cursor.is_empty() {
                Space::new().into()
            } else if cursor.len() > MAX_VISIBLE_DOTS {
                build_compact_indicators::<Message>(
                    &ids,
                    cursor,
                    &id_prefix,
                    self.label.as_deref(),
                    pinned,
                    &factory,
                )
            } else {
                let count = cursor.len();
                let active = cursor.active();
                // One roving Tab stop for the strip: the selected dot. The others stay
                // addressable by id for routed activation, and clickable.
                let mut strip: Row<'a, Message> = row![].spacing(INDICATOR_SPACING);
                for (index, id) in ids.iter().enumerate() {
                    let is_selected = active == Some(index);
                    // Keyed by PageId, so the id follows its page across reorders.
                    let dot_id = derive_id(&id_prefix, id.as_str());
                    let mut dot = focus_button_maybe_with(
                        Space::new()
                            .width(Length::Fixed(indicator_style.width(is_selected)))
                            .height(Length::Fixed(indicator_style.height())),
                        factory(PageRequest::Id(id.clone())),
                    )
                    .id(dot_id)
                    .semantics(indicator_semantics(index, count, is_selected));
                    if is_selected {
                        // The selected dot has nothing to send but is the active item, not
                        // disabled chrome; it still advertises no Click.
                        dot = dot.disabled_semantics(false);
                        // The strip's one Tab stop, though it has nothing to press.
                        dot = dot.focusable_without_activation(true);
                    } else {
                        // Not a Tab stop, but still clickable and reachable by a routed Click.
                        dot = dot.embedded(true);
                    }
                    // `indicator_fade_ms` drives the cross-fade; the `RowMotion` wrapper below
                    // writes the progress each frame.
                    let dot_progress: RowMotionProgress = Rc::new(Cell::new(0.0));
                    // Row motion animates this style itself.
                    let dot = dot.reduced_motion(true).style_focus({
                        let dot_progress = Rc::clone(&dot_progress);
                        move |theme: &iced::Theme, status| {
                            let t = pinned.unwrap_or_else(|| CarouselTokens::from_theme(theme));
                            // Selection and hover ease; a press stays instant, as in `list_row_surface`.
                            let hovered = matches!(status, FocusStatus::Hovered);
                            let fill = if matches!(status, FocusStatus::Pressed) {
                                t.indicator_hover
                            } else if is_selected {
                                lerp_color(t.indicator, t.indicator_selected, dot_progress.get())
                            } else if hovered {
                                lerp_color(t.indicator, t.indicator_hover, dot_progress.get())
                            } else {
                                t.indicator
                            };
                            iced::widget::button::Style {
                                background: Some(iced::Background::Color(fill)),
                                border: iced::Border {
                                    radius: (indicator_style.height() / 2.0).into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        }
                    });
                    // Unpinned, the fade follows the theme's `motion-fast`.
                    let motion = RowMotion::new(dot.into(), is_selected, None, dot_progress)
                        .track_hover(true);
                    let dot: Element<'a, Message> = match pinned {
                        Some(t) => motion.duration_ms(f32::from(t.indicator_fade_ms)).into(),
                        None => motion.into(),
                    };
                    strip = strip.push(dot);
                }
                container(strip).center_x(Length::Fill).into()
            };

        // ---- prev/next chrome ----------------------------------------------
        //
        // The arrows are laid over the page's band only, so they centre on
        // the page rather than on the page and the indicator strip together.
        let mut page_band = stack![body];
        if chrome_shown {
            let inside = placement == ArrowPlacement::Inside;
            // From the stage's side to the arrow's outer edge.
            let reach = match placement {
                ArrowPlacement::Inside => BODY_PADDING + INSIDE_ARROW_INSET,
                ArrowPlacement::Edge => (BODY_PADDING - CHROME_SIZE / 2.0).max(0.0),
                ArrowPlacement::Outside => CHROME_INSET,
            };
            let chrome = |icon: &'static str,
                          request: PageRequest,
                          label: String,
                          align: iced::alignment::Horizontal| {
                let action = factory(request);
                let live = action.is_some();
                let glyph = crate::icon::enz_icon(icon).size(ARROW_GLYPH).tint_resolved(
                    move |theme: &iced::Theme| {
                        let t = pinned.unwrap_or_else(|| CarouselTokens::from_theme(theme));
                        let ink = if inside {
                            iced::Color::WHITE
                        } else {
                            t.chrome_foreground
                        };
                        match (live, inside) {
                            (true, _) => ink,
                            // A clamped end hides an arrow over the page and
                            // dims one beside it.
                            (false, true) => iced::Color::TRANSPARENT,
                            (false, false) => crate::color::with_opacity(
                                ink,
                                crate::theme_lookup::opacity(theme).disabled_surface(),
                            ),
                        }
                    },
                );
                let button = focus_button_maybe_with(
                    container(glyph)
                        .center_x(Length::Fill)
                        .center_y(Length::Fill),
                    action,
                )
                .width(Length::Fixed(CHROME_SIZE))
                .height(Length::Fixed(CHROME_SIZE))
                .semantics(
                    enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Button)
                        .label(label),
                )
                .style_focus(move |theme: &iced::Theme, status| {
                    let t = pinned.unwrap_or_else(|| CarouselTokens::from_theme(theme));
                    let disabled = matches!(status, FocusStatus::Disabled);
                    let (background, border) = if inside {
                        // A scrim that reads over light and dark pages alike.
                        let a = match status {
                            _ if disabled => 0.0,
                            FocusStatus::Pressed => 0.62,
                            FocusStatus::Hovered => 0.52,
                            _ => 0.4,
                        };
                        (
                            iced::Color {
                                a,
                                ..iced::Color::BLACK
                            },
                            iced::Color::TRANSPARENT,
                        )
                    } else {
                        let dim = |c: iced::Color| {
                            if disabled {
                                crate::color::with_opacity(
                                    c,
                                    crate::theme_lookup::opacity(theme).disabled_surface(),
                                )
                            } else {
                                c
                            }
                        };
                        (dim(t.chrome_background), dim(t.border))
                    };
                    iced::widget::button::Style {
                        background: Some(iced::Background::Color(background)),
                        border: iced::Border {
                            width: if inside { 0.0 } else { 1.0 },
                            color: border,
                            radius: (CHROME_SIZE / 2.0).into(),
                        },
                        ..Default::default()
                    }
                });
                container(button)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(align)
                    .align_y(iced::alignment::Vertical::Center)
                    .padding(iced::Padding {
                        left: reach,
                        right: reach,
                        ..iced::Padding::ZERO
                    })
            };
            page_band = page_band.push(chrome(
                "chevron-left",
                PageRequest::Previous,
                enzui_core::localization::text("enzui-carousel-previous-slide"),
                iced::alignment::Horizontal::Left,
            ));
            page_band = page_band.push(chrome(
                "chevron-right",
                PageRequest::Next,
                enzui_core::localization::text("enzui-carousel-next-slide"),
                iced::alignment::Horizontal::Right,
            ));
        }
        let layers = column![
            page_band,
            container(indicators).padding(iced::Padding {
                left: BODY_PADDING,
                right: BODY_PADDING,
                ..iced::Padding::ZERO
            })
        ]
        .spacing(BODY_SPACING)
        .padding(iced::Padding {
            top: BODY_PADDING,
            bottom: BODY_PADDING,
            ..iced::Padding::ZERO
        });

        let framed = container(layers)
            .width(self.width)
            .height(self.height)
            .style(move |theme: &iced::Theme| {
                // The transparent stage paints neither fill nor
                // border, so the surface *behind* the carousel reaches every
                // pixel of the stage. `radius` and `bordered` are documented
                // no-ops on this style — there is no band to round or stroke.
                // Resolved here inside the container style closure (not from a
                // pre-baked token), so a theme switch that moves the host
                // pixel behind the carousel moves that pixel straight through
                // to the AT observer without a rebuild, which a pre-baked
                // `.tokens(CarouselTokens)` knob cannot express.
                if matches!(stage_style, CarouselStageStyle::Transparent) {
                    return container::Style {
                        background: None,
                        border: iced::Border {
                            width: 0.0,
                            color: iced::Color::TRANSPARENT,
                            radius: 0.0.into(),
                        },
                        ..Default::default()
                    };
                }
                let t = pinned.unwrap_or_else(|| CarouselTokens::from_theme(theme));
                container::Style {
                    background: Some(iced::Background::Color(t.background)),
                    border: iced::Border {
                        width: 1.0,
                        color: if bordered {
                            t.border
                        } else {
                            iced::Color::TRANSPARENT
                        },
                        radius: t.radius.into(),
                    },
                    ..Default::default()
                }
            });

        // Keyboard paging and swipe wrap the composed tree, so the chrome and
        // indicators above keep their own pointer handling.
        //
        // `resolve_request` is the pure availability probe: on an arrow / Home
        // / End (`update`), we need to know which dot to focus *before*
        // the app has processed the paging message. Sharing this closure with
        // the pointer-side `factory` above means the two paths cannot disagree
        // about which page a request would select.
        let input = CarouselInput {
            content: framed.into(),
            request: Box::new(request),
            resolve_request: Box::new(resolve_request),
            id_prefix,
            swipe_enabled: self.swipe_enabled && cursor.len() > 1 && enabled,
            enabled,
            // Horizontal, both directions (leftward pages forward, rightward
            // back), no edge strip — the whole page body is grabbable — and
            // distance-only. The threshold stays the
            // carousel's own token, kept with its caller.
            policy: SwipePolicy::new(
                SwipeAxis::Horizontal,
                pinned.unwrap_or_else(CarouselTokens::dark).threshold(),
            ),
            width: self.width,
            height: self.height,
            active_page,
            page_alpha,
        };

        // Published as a GROUP, so the indicators stay addressable as its AT
        // children rather than being flattened away behind a leaf node.
        SemanticsContainer::new(
            Element::from(input),
            carousel_semantics(self.label.as_deref(), cursor),
        )
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzCarousel<'a, Message>> for Element<'a, Message> {
    fn from(c: EnzCarousel<'a, Message>) -> Self {
        c.into_element()
    }
}

// ---------------------------------------------------------------------------
// Compact indicator for large page counts
// ---------------------------------------------------------------------------

/// Build the *compact* indicator surface used when a carousel has more pages
/// than [`MAX_VISIBLE_DOTS`].
///
/// The realisation bound is a single [`focus_button`] carrying the numeric
/// "Page X of N" readout, so the strip's cost is `O(1)` in the page count
/// rather than `O(N)`. Regardless of how many pages the carousel holds, the
/// compact strip realises exactly one dot's worth of focusable state,
/// [`RowMotion`] wrappers and paint (compared with `N` for the ordinary path).
///
/// The semantics bound is [`carousel_semantics`] on the parent container,
/// which announces "Page X of N" from the live [`PageCursor`]. Unrealised
/// pages are therefore still *describable*: an AT user reading the carousel
/// group hears the current position and the total, which is exactly the
/// information the per-dot strip provides for smaller page counts.
///
/// The one focusable this returns is the roving Tab stop —
/// [`focusable_without_activation(true)`](crate::focus_button::FocusButton::focusable_without_activation),
/// stable [`Id`] derived from the selected page's [`PageId`] via the same
/// `derive_id(id_prefix, page_id)` rule as the ordinary strip — so the
/// focus-transfer path (`CarouselInput::diff` writes
/// `state.pending_focus_target = derive_id(prefix, active_page)`) reaches the
/// same id here as in the all-dots path. No branching in the transfer code.
fn build_compact_indicators<'a, Message>(
    ids: &[PageId],
    cursor: PageCursor,
    id_prefix: &str,
    label: Option<&str>,
    pinned: Option<CarouselTokens>,
    factory: &dyn Fn(PageRequest) -> Option<Box<dyn Fn() -> Message + 'a>>,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    use iced::widget::{container, text};
    let count = cursor.len();
    let active_index = cursor.active();
    let active_id = active_index.and_then(|i| ids.get(i)).cloned();

    let selected_pos = active_index.map_or(0, |i| i + 1);
    let readout =
        enzui_core::localized!("enzui-carousel-page-of", "page" => selected_pos, "count" => count);

    // The one interactive stop. Uses the same [`focus_button_maybe_with`] +
    // [`focusable_without_activation(true)`] shape the ordinary strip's
    // selected dot uses (built above in `carousel::view`), so a keyboard user reaches
    // this control by exactly the same `focus(id)` any AT tool would issue
    // — no separate mechanism to keep in sync.
    let inner_text = text(readout).size(enzui_theme::text_scale::ui_px(12.0));
    let mut button = focus_button_maybe_with(
        container(inner_text)
            .center_x(Length::Shrink)
            .center_y(Length::Shrink),
        active_id
            .as_ref()
            .and_then(|id| factory(PageRequest::Id(id.clone()))),
    );
    if let Some(active) = active_id.as_ref() {
        button = button.id(derive_id(id_prefix, active.as_str()));
    }
    // Selected “dot” has nothing to press (its factory yields `None` for its
    // own page); force enabled-and-selected + join the Tab ring (same shape
    // as the ordinary selected dot).
    button = button
        .disabled_semantics(false)
        .focusable_without_activation(true);
    // Semantics: mirror the ordinary strip's per-dot Tab, but with a
    // description that *names the page count*, so an AT user reading the
    // stop learns "Page 3 of 42" even though only one dot is realised.
    let index = active_index.unwrap_or(0);
    let announcement = match label {
        Some(label) => {
            enzui_core::localized!("enzui-carousel-page-named", "label" => label, "page" => index + 1, "count" => count.max(1))
        }
        None => {
            enzui_core::localized!("enzui-carousel-page-of", "page" => index + 1, "count" => count.max(1))
        }
    };
    button = button.semantics(indicator_semantics(index, count, true).label(announcement));
    let button = button.style_focus(move |theme: &iced::Theme, _status| {
        let t = pinned.unwrap_or_else(|| CarouselTokens::from_theme(theme));
        iced::widget::button::Style {
            background: Some(iced::Background::Color(t.chrome_background)),
            text_color: t.chrome_foreground,
            border: iced::Border {
                width: 1.0,
                color: t.border,
                radius: (DOT_SIZE * 2.0).into(),
            },
            ..Default::default()
        }
    });
    container(button).center_x(Length::Fill).into()
}
