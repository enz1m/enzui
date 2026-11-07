//! The public [`EnzExpander`] builder and its `into_element` composition.

use super::*;
use iced::widget::column;

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Expander builder.
#[must_use]
pub struct EnzExpander<'a, Message> {
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) icon: String,
    /// App-owned leading icon slot. When set, wins over `icon`;
    /// bundled names go through the same tinted path `icon` uses,
    /// app-owned `Svg` / `Image` handles render untinted so a vendor
    /// brand mark keeps its own colours.
    pub(crate) icon_source: Option<crate::icon::IconSource>,
    pub(crate) expanded: bool,
    pub(crate) variant: ExpanderVariant,
    pub(crate) content: Option<Element<'a, Message>>,
    on_toggle: Box<dyn Fn(bool) -> Message + 'a>,
    pub(crate) tokens: Option<ExpanderTokens>,
    /// Optional accessible-name override (default: the title).
    pub(crate) aria_label: Option<String>,
    /// Optional stable widget id for programmatic focus.
    id: Option<iced::advanced::widget::Id>,
    /// Namespace for the title-derived default id.
    id_prefix: String,
    /// AdwExpanderRow-style enable gate.
    pub(crate) show_enable_switch: bool,
    pub(crate) enable_value: bool,
    on_enable_toggled: Option<Box<dyn Fn(bool) -> Message + 'a>>,
    /// Caller-supplied gate switch a11y label override.
    /// `None` = use the default `"Enable {title}"` template.
    pub(crate) gate_label: Option<String>,
    /// Per-instance reduced-motion override (`None` = follow theme).
    reduced_motion: Option<bool>,
    /// An arbitrary trailing action `Element` placed before the
    /// chevron in the header row (a `+` new-thread button, a kebab menu,
    /// a small pill). Its space is always reserved when set — the widget
    /// does not reveal-on-hover, because that reflows the list underneath
    /// as the pointer approaches (14 rows can be shoved down 14 px). A caller
    /// that wants a hover-only reveal can wrap the action element itself; the header row's layout stays stable.
    /// Activating this action must not toggle the section — the outer
    /// header button honours `shell.is_event_captured()`, so an inner
    /// button that captures its own press wins the click.
    pub(crate) header_action: Option<Element<'a, Message>>,
    /// Which hover scope this expander publishes. The default
    /// [`HoverScope::Header`] limits hover to the header;
    /// [`HoverScope::Group`] wraps the whole expander in
    /// [`crate::hover_group::EnzHoverGroup`] so the header and its expanded
    /// body tint as one unit (the sidebar-section shape). See
    /// [`HoverScope`] for the pick.
    pub(crate) hover_scope: HoverScope,
}

/// Construct an expander.
pub fn enz_expander<'a, Message: Clone + 'a>(
    title: impl Into<String>,
    expanded: bool,
    on_toggle: impl Fn(bool) -> Message + 'a,
) -> EnzExpander<'a, Message> {
    EnzExpander {
        title: title.into(),
        subtitle: String::new(),
        icon: String::new(),
        icon_source: None,
        expanded,
        variant: ExpanderVariant::default(),
        content: None,
        on_toggle: Box::new(on_toggle),
        tokens: None,
        aria_label: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        show_enable_switch: false,
        enable_value: true,
        on_enable_toggled: None,
        gate_label: None,
        reduced_motion: None,
        header_action: None,
        hover_scope: HoverScope::Header,
    }
}

impl<'a, Message: Clone + 'a> EnzExpander<'a, Message> {
    /// Optional muted subtitle line, rendered below the title.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = s.into();
        self
    }

    /// Optional leading icon (Lucide canonical or semantic alias
    /// resolved via `enzui-icons`). Empty omits the slot.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// Optional leading icon from an app-owned source — an SVG
    /// or raster handle, or a bundled Lucide name. Wins over
    /// [`Self::icon`] when set.
    ///
    /// A provider brand mark on a settings section, an account avatar
    /// on an accounts pane, a project favicon on a workspace group —
    /// all sit naturally in the expander's header icon slot but cannot
    /// ship in the Lucide bundle. Accepts `String`/`&str` (bundled
    /// name), `iced::widget::svg::Handle` (app-owned SVG) or
    /// `iced::widget::image::Handle` (app-owned raster: `.ico` / `.png`)
    /// through [`crate::icon::IconSource`]'s `From` impls.
    ///
    /// # Tinting
    ///
    /// A bundled name follows the expander's `icon_color` token so a
    /// Lucide fallback beside an app-owned favicon still tracks the
    /// theme. An app-supplied handle is rendered untinted — a brand
    /// mark recoloured to the header text tone loses recognisability.
    ///
    /// # Accessibility
    ///
    /// The expander's `title` is its accessible name (`.aria_label` when
    /// set overrides), so the header icon is decorative regardless of
    /// source. An icon-only header is not a supported shape here —
    /// supply a title.
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.icon_source = Some(source.into());
        self
    }

    /// Frame variant — layout-stable `Flat` (default), visible `Card`, or
    /// structure-removing `Plain`.
    pub fn variant(mut self, v: ExpanderVariant) -> Self {
        self.variant = v;
        self
    }

    /// Body content, shown while expanded.
    pub fn content(mut self, c: Element<'a, Message>) -> Self {
        self.content = Some(c);
        self
    }

    /// Override the tokens. Pinning tokens also pins the colours (they no
    /// longer follow the runtime theme).
    pub fn tokens(mut self, t: ExpanderTokens) -> Self {
        // Normalize at the public boundary so no non-finite geometry token can
        // reach renderer coordinates.
        self.tokens = Some(t.sanitized());
        self
    }

    /// Override the accessible name (defaults to the title).
    pub fn aria_label(mut self, s: impl Into<String>) -> Self {
        self.aria_label = Some(s.into());
        self
    }

    /// Set a stable widget id so a host can move keyboard focus onto the
    /// disclosure header (`focus-header()`) and an assistive technology can
    /// route Expand/Collapse to it.
    ///
    /// This overrides the title-derived default. Use an explicit id when the
    /// title is dynamic or duplicated on the same screen.
    pub fn id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Namespace for the title-derived default identity.
    ///
    /// Set a distinct prefix when two expanders with the same title share a
    /// screen. Prefer [`Self::id`] when the title itself can change.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Show the AdwExpanderRow-style enable switch in the header (before the
    /// chevron). While the gate is off the section can't expand, collapses,
    /// the title dims, and the chevron is hidden.
    pub fn show_enable_switch(mut self, on: bool) -> Self {
        self.show_enable_switch = on;
        self
    }

    /// Current enable-gate value (only meaningful with `show_enable_switch`).
    pub fn enable_value(mut self, on: bool) -> Self {
        self.enable_value = on;
        self
    }

    /// Callback fired when the header enable switch flips (receives the new
    /// value). Only wired when `show_enable_switch` is set.
    pub fn on_enable_toggled(mut self, cb: impl Fn(bool) -> Message + 'a) -> Self {
        self.on_enable_toggled = Some(Box::new(cb));
        self
    }

    /// Override the gate switch's accessible name.
    ///
    /// By default the gate announces `"Enable {title}"` (or
    /// `"Enable {aria_label}"` when set). This builder lets a caller
    /// supply a fully localised label — e.g. `"Activer les options avancées"`
    /// — without duplicating visible title text.
    ///
    /// Empty string is treated as "no override" and falls back to the
    /// default template, because an empty a11y name makes the switch
    /// invisible to screen readers. For a blank label, pass
    /// a single space.
    pub fn gate_label(mut self, label: impl Into<String>) -> Self {
        let s: String = label.into();
        self.gate_label = if s.is_empty() { None } else { Some(s) };
        self
    }

    /// Per-instance override for reduced-motion behaviour. `None` (the
    /// default) follows the theme's policy; `Some(true)` forces instant
    /// transitions; `Some(false)` forces animation even if the theme says
    /// reduced.
    pub fn reduced_motion(mut self, rm: Option<bool>) -> Self {
        self.reduced_motion = rm;
        self
    }

    /// Trailing action `Element` in the header row, placed before the
    /// chevron. The convention (VS Code Explorer sections, Slack
    /// channel groups, Finder tags) is a button beside the disclosure
    /// chevron: `+ new thread`, kebab overflow, a small pill.
    ///
    /// # Interaction — the click does not toggle the section
    ///
    /// The action is a distinct widget inside the header row. When the
    /// user clicks it, the inner control's `update` runs first and
    /// captures the press (`shell.capture_event()` — the shape every
    /// button in the framework uses). The outer expander header's
    /// `focus_button` update sees `shell.is_event_captured()` and bails
    /// out before firing `on_toggle`. Enter and Space on a keyboard-focused
    /// inner button are captured the same way. The gate here is that the
    /// caller's `Element` must capture its press — every framework
    /// button (`enz_button`, `focus_button`, `toolbar_icon`, `menu_button`)
    /// already does. A caller passing a bare `text("+")` with no capture
    /// would toggle the section on click; a text label is not an action.
    ///
    /// # Reserved space — no reflow on hover
    ///
    /// The slot is always occupied when set. A reveal-on-hover `+` button
    /// that appears only under the pointer shoves every row below it down
    /// by the button's height the instant the pointer arrives, so a list of
    /// 14 rows slides out from under the cursor as the pointer approaches it
    /// (measured as a 14-row shift). Callers that want a hover-only reveal
    /// wrap the action itself — the header row's LAYOUT stays stable.
    ///
    /// # Accessibility
    ///
    /// The action becomes a distinct a11y node with its own accessible
    /// name (the caller's `enz_button` label / `aria_label`), sibling to
    /// the header's [`Role::Disclosure`](enzui_core::semantics::Role::Disclosure).
    /// A screen-reader user Tabs onto the header, hears the section title
    /// and its expanded/collapsed state, then Tabs onto the action and
    /// hears its own name — the two are separately reachable.
    ///
    /// # Related
    ///
    /// This is the same `trailing(Element)` convention
    /// [`crate::pane_header`] uses for its "section header with an action"
    /// shape; the two together cover the header-with-trailing-action
    /// vocabulary from both the non-collapsible and collapsible sides.
    pub fn header_action(mut self, action: impl Into<Element<'a, Message>>) -> Self {
        self.header_action = Some(action.into());
        self
    }

    /// Select the hover scope this expander publishes. Default
    /// [`HoverScope::Header`] keeps hover
    /// tints only the header row; [`HoverScope::Group`] tints the header
    /// and its expanded body as one unit — the sidebar-section shape.
    ///
    /// The group tint is delivered by wrapping the whole expander in
    /// [`crate::hover_group::EnzHoverGroup`], so it is resolved at
    /// draw time from `cursor.is_over(bounds)` over the whole expander and
    /// carries no app state. The group quad paints BEHIND the content, so
    /// a nested [`crate::list_row_surface`] still paints its own stronger
    /// per-row hover on top — both scopes compose.
    ///
    /// The group fill is the palette's composited [`overlay_hover`](enzui_theme::Palette::overlay_hover)
    /// role — a translucent `text` wash over whatever surface the sidebar
    /// sits on. This is the same role the expander header itself reads
    /// and is visible on `surface_alt` (the sidebar body),
    /// unlike the opaque `surface_hover` token which is byte-identical to
    /// `surface_alt` and produces zero visual change on a sidebar.
    pub fn hover_scope(mut self, scope: HoverScope) -> Self {
        self.hover_scope = scope;
        self
    }

    /// Resolve the gate switch's a11y name.
    ///
    /// Returns the caller-supplied override if set, else the English default
    /// `"Enable {a11y_label}"`. Used by `into_element` and testable directly.
    pub(crate) fn resolved_gate_label(&self, a11y_label: &str) -> String {
        self.gate_label.clone().unwrap_or_else(
            || enzui_core::localized!("enzui-expander-enable", "label" => a11y_label),
        )
    }

    /// Convert into an iced [`Element`].
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: the gate name is computed whenever the \
                  switch shows"
    )]
    pub fn into_element(mut self) -> Element<'a, Message> {
        // Geometry comes from tokens (pinned or dark preset — the concrete
        // metrics are palette-independent); colours are resolved lazily from
        // the runtime theme when unpinned so light/custom/live themes are
        // correct.
        let geom = self.tokens.unwrap_or_else(ExpanderTokens::dark);
        let pinned = self.tokens;
        let framed = !matches!(self.variant, ExpanderVariant::Plain);
        // A framed header sits under the card's rounded clip; an unframed one
        // is a row of its own, so its hover and ring round like a list row's
        // instead of painting a square block.
        let header_radius = if framed {
            0.0
        } else {
            crate::list_row_surface::ListRowSurfaceTokens::dark().row_radius
        };
        let flat = matches!(self.variant, ExpanderVariant::Flat);

        // Gate semantics: the section is interactive only when
        // ungated or when the gate is on; a gated-off section is forced
        // collapsed regardless of `expanded`.
        let show_switch = self.show_enable_switch;
        let enable_value = self.enable_value;
        let section_active = !show_switch || enable_value;
        let effective_expanded = self.expanded && section_active;

        // Capture the accessible name before the title is moved into the view.
        let a11y_label = self
            .aria_label
            .clone()
            .unwrap_or_else(|| self.title.clone());
        // The accessible label above names the node to a user; this
        // widget Id is the separate routing identity NodeIdMap requires. Match
        // EnzDisclosure's established default: title-derived, with explicit id
        // precedence and a caller-overridable namespace for repeated titles.
        let header_id = self
            .id
            .clone()
            .unwrap_or_else(|| enzui_core::identity::derive_id(&self.id_prefix, &self.title));

        // A colour resolver: pinned tokens win, else resolve from the runtime
        // theme's palette.
        let resolve = move |theme: &iced::Theme| -> ExpanderTokens {
            pinned.unwrap_or_else(|| {
                crate::theme_lookup::resolve(theme, ExpanderTokens::from_palette)
            })
        };

        // ---- Header row ----
        let mut header_children: Vec<Element<'a, Message>> = Vec::new();

        // Resolve the gate label now, before `self` fields are
        // partially moved (the subtitle move happens shortly below).
        let gate_name = if show_switch {
            Some(self.resolved_gate_label(&a11y_label))
        } else {
            None
        };

        // `icon_source` wins over `icon`. The bundled-name path
        // keeps `.tint_resolved(icon_color)` so a Lucide fallback re-tints
        // on theme switch; app-owned SVG/raster handles render untinted
        // so a vendor brand mark keeps its own colours.
        let header_icon: Option<Element<'a, Message>> = match self.icon_source {
            Some(source) if !source.is_empty() => {
                Some(source.into_slot_element(geom.icon_size, |icon| {
                    icon.tint_resolved(move |theme| resolve(theme).icon_color)
                }))
            }
            Some(_) | None => {
                if !self.icon.is_empty() {
                    Some(
                        crate::icon::enz_icon(&self.icon)
                            .size(geom.icon_size)
                            .tint_resolved(move |theme| resolve(theme).icon_color)
                            .into(),
                    )
                } else {
                    None
                }
            }
        };
        if let Some(icon_el) = header_icon {
            header_children.push(icon_el);
        }

        // Title + subtitle stack (fills the remaining width). Title dims to
        // `text_muted` while the section is gated off.
        let subtitle_size = geom.subtitle_size;
        // The title paints `title_color`, not the shared `TextTone`.
        // Both resolve to `text` by default, so this is observable only when a
        // caller pins `.tokens(…)`. The title token must recolour the title,
        // independently of the focus ring. Resolved per-frame, so an unpinned expander still
        // re-tints on a theme switch. When the gate is off, the title dims
        // to `text_muted` (the palette's muted ink) regardless of the token.
        let title_widget = crate::enz_text::enz_text(self.title)
            .size(crate::enz_text::TextSize::Callout)
            .size_px(geom.title_size)
            .weight_rung(enzui_theme::WeightRung::Semibold)
            .color_resolved(move |theme| {
                if section_active {
                    resolve(theme).title_color
                } else {
                    crate::theme_lookup::resolve(theme, |p| p.text_muted)
                }
            });
        let stack: Element<'a, Message> = if self.subtitle.is_empty() {
            title_widget.into()
        } else {
            column![
                title_widget,
                // The subtitle paints `subtitle_color`, not the shared
                // `TextTone::Muted`. Both resolve to `text_muted` by default,
                // so this is observable only when a caller pins `.tokens(…)`;
                // the pinned subtitle token must control the subtitle ink. Resolved per-frame, so an unpinned expander
                // still re-tints on a light/dark/custom theme switch.
                crate::enz_text::enz_text(self.subtitle)
                    .size(crate::enz_text::TextSize::Footnote)
                    .size_px(subtitle_size)
                    .color_resolved(move |theme| resolve(theme).subtitle_color),
            ]
            .spacing(2)
            .into()
        };
        header_children.push(container(stack).width(Length::Fill).into());

        // Header enable switch (before the chevron), only when gated. Uses the
        // bare `EnzToggle` (no label — the title labels it) so it carries the
        // exact same theme-resolved accent track / thumb / focus-ring colours
        // as a standalone switch (the header is a bare `EnzToggle` too).
        // The low-level `switch` primitive's default palette is a flat grey, so
        // going through `EnzToggle` is what makes the on-state read accent.
        if show_switch {
            // The gate has no visible label (the header title labels it), but a
            // screen-reader user reaching the switch must still hear a name
            // Give it an accessibility-only name derived from the same
            // accessible label the header publishes, so it announces e.g.
            // "Enable <title>". `.aria_label` sets the switch's a11y name
            // without creating a visible text column.
            // The gate label is caller-localisable. The override
            // wins when non-empty; empty falls back to the English default.
            // Pre-computed above before `self` fields are partially moved.
            let gate_name = gate_name.expect("pre-computed when show_switch");
            if let Some(cb) = self.on_enable_toggled {
                // `Box<dyn Fn(bool) -> Message>` implements `Fn`, so it can be
                // handed straight to the toggle callback.
                header_children.push(
                    crate::toggle::enz_toggle(enable_value, "", cb)
                        .aria_label(gate_name)
                        .into(),
                );
            } else {
                // Read-only gate display when no callback is wired.
                //
                // CROSS-FILE INVARIANT: the `unreachable!` below is sound only
                // because `EnzToggle`'s `!enabled` arm (in the `toggle` module, the
                // `on_toggle_maybe(None)` line) drops the callback entirely when
                // `.enabled(false)` is set, so it can never be invoked. If that
                // drop ever goes away (e.g. to let a disabled toggle emit a
                // "why is this disabled?" hint), this line panics instead.
                header_children.push(
                    crate::toggle::enz_toggle(enable_value, "", |_| unreachable!())
                        .aria_label(gate_name)
                        .enabled(false)
                        .into(),
                );
            }
        }

        // Trailing header action — an arbitrary `Element` placed
        // before the chevron in the header row. Its space is always
        // reserved (see `.header_action` docs for why reveal-on-hover
        // reflows the list). Emitted regardless of gated state so the
        // header's layout is stable across gate flips too; the action is
        // the caller's widget, so a caller who wants to hide it while
        // gated off can pass a placeholder — or route the gate through
        // its own `on_press`.
        //
        // A click / Enter / Space on this action captures the event
        // inside the inner control's `update`, which the outer
        // `focus_button` respects (`is_event_captured`) — so the action
        // never toggles the section. This invariant is pinned by the
        // `header_action_click_does_not_toggle_the_section` gate.
        if let Some(action) = self.header_action.take() {
            header_children.push(action);
        }

        // Trailing chevron — Lucide SVG ( EnzIconSlot). Hidden
        // while the section is gated off.
        if section_active {
            // The collapsed glyph turns a quarter to point down as it opens.
            header_children.push(
                crate::icon::enz_icon("navigation.forward")
                    .size(geom.chevron_size)
                    .tint_resolved(move |theme| resolve(theme).icon_color)
                    .quarter_turn(effective_expanded),
            );
        }

        let header_row = row(header_children)
            .spacing(geom.content_gap)
            .align_y(Alignment::Center);

        let header_height = geom.header_height;

        // Accessibility: the header is a keyboard-focusable disclosure control.
        // `accessible-expandable` follows `section_active`; `expanded` follows
        // the effective (gated) state.
        let mut semantics = Semantics::new(Role::Disclosure)
            .label(a11y_label)
            .expanded(effective_expanded);
        if section_active {
            semantics = semantics.default_action();
        }

        // Lazy activation: build the toggle
        // message only on real activation, and only when the section is active
        // (a gated-off header ignores taps — the switch re-enables it).
        let on_toggle = self.on_toggle;
        let expanded_now = self.expanded;
        let mut header_button = crate::focus_button::focus_button_maybe(
            // Fixed-height header + top-aligned child = label & chevron
            // stuck against the top of a 44px box, so a hovered header reads as
            // a padded band at the top instead of one balanced disclosure row.
            // The inner `row` has `align_y(Center)` but that only centres
            // children within the row's own (shrink) height; the row itself
            // then sits at the container's default (Start) y. Ask the
            // container to centre its child in the fixed-height band so the
            // header row is optically centred within the same hover fill the
            // `focus_button` already paints across the whole row.
            container(header_row)
                .width(Length::Fill)
                .height(Length::Fixed(header_height))
                .align_y(Alignment::Center)
                .padding(Padding::from([0.0, geom.content_inset])),
            None,
        )
        .padding(Padding::ZERO)
        .width(Length::Fill)
        .semantics(semantics)
        .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
            let tok = resolve(theme);
            let radius: iced::border::Radius = header_radius.into();
            let bg = match status {
                FocusStatus::Hovered | FocusStatus::Pressed => tok.hover_background,
                _ => Color::TRANSPARENT,
            };
            // The focus ring uses the palette's semantic
            // `focus_ring` colour, not `title_color`. Using `title_color`
            // here would tie the focus ring to the same token the title
            // uses, so pinning `title_color` would recolour the focus
            // ring. The colour is resolved from the runtime theme (not
            // from the expander's own tokens) so it stays consistent with
            // every other widget's focus ring.
            let focus_color = crate::theme_lookup::resolve(theme, |p| p.focus_ring);
            let border = if matches!(status, FocusStatus::Focused) {
                Border {
                    color: focus_color,
                    width: 1.5,
                    radius,
                }
            } else {
                Border {
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius,
                }
            };
            ButtonStyle {
                background: Some(Background::Color(bg)),
                border,
                ..ButtonStyle::default()
            }
        });
        if section_active {
            header_button = header_button.on_press_with(move || (on_toggle)(!expanded_now));
        }
        header_button = header_button.id(header_id.clone());
        // The header's activation carrier *is* the toggle (it publishes
        // `(on_toggle)(!expanded)`), so it answers a routed Expand/Collapse
        // under the same id. Declared explicitly, because `activation::expand`
        // deliberately does not fall back to plain activation -- a fallback
        // would fire the wrong handler on a control that has no toggle (see
        // `activation::Expand`).
        //
        // Gated on `section_active` for the same reason the carrier is: an
        // inert header publishes no `on_press`, so `focus_button` offers no
        // carrier and nothing can be armed.
        if section_active {
            header_button = header_button.expand_alias(header_id);
        }

        // ---- Body ----
        // body inset: top = spacing-sm (8px), other sides = content-inset
        // (16px).
        //
        // Always build the body with content so the animation wrapper can
        // measure its preferred height and interpolate between 0 and that
        // value. The wrapper clips the overflow during the transition.
        let body_padding = Padding {
            top: geom.content_gap,
            right: geom.content_inset,
            bottom: geom.content_inset,
            left: geom.content_inset,
        };
        let body: Element<'a, Message> = match self.content {
            Some(c) => container(c)
                .padding(body_padding)
                .width(Length::Fill)
                .into(),
            None => Space::new()
                .width(Length::Fill)
                .height(Length::Fixed(0.0))
                .into(),
        };

        // ---- Frame ----
        // The divider is always built (when framed) so the wrapper can
        // include it in the animated body region.
        let raw_body_with_divider: Element<'a, Message> = if framed {
            column![
                // 1 px divider between header and body.
                container(Space::new().width(Length::Fill).height(Length::Fixed(1.0))).style(
                    move |theme: &iced::Theme| container::Style {
                        background: Some(
                            if flat {
                                Color::TRANSPARENT
                            } else {
                                resolve(theme).divider_color
                            }
                            .into(),
                        ),
                        ..Default::default()
                    }
                ),
                body,
            ]
            .into()
        } else {
            body
        };

        // The hidden body must stop doing work once the expander has
        // *settled* collapsed. `ExpanderAnim` owns the animation state, so it
        // decides frame-by-frame whether the body is hidden and writes that
        // verdict into this shared cell; `BodyGate` reads it and, when set,
        // suppresses the body's layout / update / draw / overlay entirely.
        // The cell is a shared handle: both widgets are built in this one
        // `into_element` call and `ExpanderAnim::layout/update/draw/operate`
        // runs top-down, so it writes the flag *before* recursion reaches
        // `BodyGate`. See `ExpanderAnim::settled_collapsed`.
        let body_gate = BodyGateShared::default();
        let body_with_divider: Element<'a, Message> =
            BodyGate::new(raw_body_with_divider, body_gate.clone()).into();

        let inner = column![header_button, body_with_divider];
        // The `Card` frame is not painted by a container
        // inside `ExpanderAnim`'s clip — that container is laid out at the full
        // expanded height, so a collapsed card's bottom border fell below the
        // animated clip and was scissored away (three-sided border). The
        // resolver below is handed to `ExpanderAnim`, which fills the quad at
        // its own animated bounds, outside the clip. Same tokens, same
        // per-frame theme resolution — only the painter moved.
        let frame: Option<FrameResolver> = if framed {
            Some(std::rc::Rc::new(move |theme: &iced::Theme| {
                let tok = resolve(theme);
                (
                    tok.background,
                    if flat { Color::TRANSPARENT } else { tok.border },
                )
            }))
        } else {
            None
        };
        let framed_inner: Element<'a, Message> = if framed {
            let _card_radius = geom.card_radius;
            // The clip must be rounded (see
            // the `.clip(true)` note below), but the mask cannot live at this
            // level — it is applied one layer out, by `ExpanderAnim::draw`.
            //
            // Why not here: `ExpanderAnim` wraps this container and already
            // opens a clip layer over the *animated* bounds, which during the
            // steady state are byte-identical to this container's. A second,
            // coincident layer is folded into the first by
            // `graphics::layer::Stack::merge`, whose mergeability test is
            // `candidate.bounds() != target.bounds()` and never consults
            // `clip_radius` (`iced/graphics/src/layer.rs`) — so on
            // wgpu the radius is silently discarded. Measured: a `RoundedClip`
            // wrapper placed here left 1601/1628 cutout px on wgpu while
            // tiny-skia (which does not call `merge`) read 1473/1628. Two
            // backends disagreeing is the tell.
            //
            // So `ExpanderAnim` rounds *its own* layer instead, which is both
            // correct during the open/close transition (the mask follows the
            // animated height) and free of a redundant layer.
            // This container carries no `.style(..)` — the
            // fill and border are drawn by `ExpanderAnim` at the animated
            // bounds (see the `frame` binding above). It is retained purely as
            // the `Length::Fill` width carrier so the composed layout is
            // byte-identical.
            container(inner)
                // Do not add `.clip(true)` here alongside the rounded mask:
                // `tiny_skia`'s clip stack replaces the mask per layer
                // rather than intersecting -- `adjust_clip_mask` opens with
                // `clip_mask.clear()` (`tiny_skia/src/engine.rs`) -- so an
                // inner *rectangular* layer wipes the outer rounded arc.
                // Measured with both in place: wgpu 0/1628 leaked but
                // tiny-skia 1473/1628, so one backend alone is not enough
                // evidence.
                //
                // Omitting it loses nothing. `ExpanderAnim`'s layer is the same
                // width and its height is `collapsed_h + body_h * progress`,
                // which is <= this container's full height for every value of
                // `progress` -- so the remaining clip is strictly tighter on
                // both axes. `collapsed_body_is_not_painted` pins that the
                // collapsed body still does not show.
                .width(Length::Fill)
                .into()
        } else {
            container(inner).width(Length::Fill).into()
        };

        // Wrap in the animation widget that interpolates the body height.
        // A `Plain` (unframed) expander paints no card, so its clip stays
        // square: masking to a radius nothing draws would carve the caller's
        // content for no reason.
        let mask_radius = if framed { geom.card_radius } else { 0.0 };
        let animated: Element<'a, Message> = ExpanderAnim::new(
            framed_inner,
            effective_expanded,
            self.reduced_motion,
            mask_radius,
            frame,
            body_gate,
        )
        .into();

        // `HoverScope::Group` wraps the whole animated expander in
        // an `EnzHoverGroup` so the header and its expanded body tint as
        // one unit while the pointer is anywhere inside. The tint is
        // resolved at draw time from `cursor.is_over(bounds)` over the
        // whole expander — no message, no `update`, no ordering guard,
        // and correct when scrolling moves content under a stationary
        // pointer.
        //
        // The style closure uses the palette's composited `overlay_hover`
        // (a translucent `text` wash), not the opaque `surface_hover`
        // token — the latter is byte-identical to `surface_alt` in both
        // shipped presets, and a sidebar's own background is `surface_alt`,
        // so a group-hover on a sidebar would paint zero visible change.
        // The overlay composites over whatever surface is behind it, so
        // the tint survives on `surface_alt`, `surface`, and
        // `surface_elevated` alike.
        //
        // The group quad paints BEHIND the animated content, so a nested
        // `list_row_surface` in the body still paints its own stronger
        // per-row hover on top — both scopes compose.
        match self.hover_scope {
            HoverScope::Header => animated,
            HoverScope::Group => crate::hover_group::enz_hover_group(animated)
                .fill_width()
                .style(|theme: &iced::Theme, hovered: bool| {
                    if !hovered {
                        return crate::hover_group::HoverGroupStyle::NONE;
                    }
                    let bg =
                        crate::theme_lookup::resolve(theme, enzui_theme::Palette::overlay_hover);
                    crate::hover_group::HoverGroupStyle {
                        background: bg,
                        border_color: Color::TRANSPARENT,
                        border_width: 0.0,
                        radius: 6.0,
                    }
                })
                .into(),
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzExpander<'a, Message>> for Element<'a, Message> {
    fn from(e: EnzExpander<'a, Message>) -> Self {
        e.into_element()
    }
}
