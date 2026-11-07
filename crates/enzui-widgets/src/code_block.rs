//! EnzCodeBlock: flat-by-default code-snippet display with an opt-in frame.
//!
//! A code / prose-code display block: monospace body inside a rounded
//! rectangle, optional header row with a language chip and a Copy button,
//! wrap or horizontal-scroll body mode. The default Copy icon writes the full
//! source to the runtime clipboard; `.show_copy(false)` hides it.
//!
//! The body is selectable text: a drag selects part of it and Ctrl+C copies
//! that part. Inside an [`crate::selection_scope`] (a selectable transcript,
//! say) the block leaves selection to that scope, so one drag runs from the
//! prose into the code.
//!
//! # Where this belongs
//!
//! Reach for [`crate::markdown`] for CommonMark source and want the
//! whole document (headings, paragraphs, fenced code) rendered as one; the
//! Markdown renderer delegates its fenced-code blocks to this widget. Use
//! `code_block` directly for one snippet in hand.
//!
//! # Syntax highlighting
//!
//! Source language is detected by default through [`crate::syntax::Language::detect`].
//! Set [`.language("rust")`](EnzCodeBlock::language) to force a supported tokenizer,
//! or `"text"` to keep plain monospace text. Unknown code falls back to plain text.
//! Syntax classes resolve against the live theme palette.
//!
//! # "Copied" pulse animation
//!
//! The copy button runs a brief background highlight (the "copied" pulse)
//! when `.copied(true)` is set, driven by `iced::animation::Animation<bool>`
//! over the theme's `MotionToken::Fast` duration (120 ms by default).
//! When reduced motion is active (theme-level or per-instance override), the
//! pulse snaps instantly and requests no intermediate frames. Once settled,
//! the widget requests no further redraws.

use crate::focus_button::{FocusStatus, focus_button_maybe_with};
use crate::scroll_view::{Axis, enz_scroll_view};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::Shell;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{overlay, renderer};
use iced::animation::Animation;
use iced::mouse;
use iced::time::Instant;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{Space, column, container, row, text};
use iced::window;
use iced::{Border, Color, Element, Event, Length, Rectangle, Size};
use std::cell::Cell;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// Instrumentation: tokenize counter
// ---------------------------------------------------------------------------

/// Instrumentation: how many times [`highlighted_body`] has run
/// `syntax::tokenize` and built a fresh `RichText` for a code body.
///
/// `highlighted_body` is the only production caller of `syntax::tokenize`
/// (`code_block.rs`), and it is the expensive half of this pipeline: it walks
/// the whole source, classifies every run, then builds a span per run. The
/// assertable invariant
/// is that this counter does not move when only the theme changes.
///
/// Counted rather than eliminated: a code block genuinely must tokenize once.
/// The invariant is about *repetition*, which only a counter can see — the
/// rendered pixels are identical whether tokenization ran once or once per
/// frame.
#[doc(hidden)]
pub static BODY_TOKENIZATIONS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Read [`BODY_TOKENIZATIONS`].
#[doc(hidden)]
#[must_use]
pub fn body_tokenizations() -> usize {
    BODY_TOKENIZATIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset [`BODY_TOKENIZATIONS`] to zero and return its previous value.
#[doc(hidden)]
pub fn reset_body_tokenizations() -> usize {
    BODY_TOKENIZATIONS.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// How the code body handles lines wider than its container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodeBlockScroll {
    /// Long lines wrap; body stays inside the parent width.
    #[default]
    Wrap,
    /// Long lines preserve; body scrolls horizontally.
    Scroll,
}

/// Builder for a syntax-neutral code block. See [`enz_code_block`].
#[must_use]
pub struct EnzCodeBlock<'a, Message: Clone> {
    code: String,
    language: String,
    scroll_mode: CodeBlockScroll,
    show_header: bool,
    show_copy: bool,
    code_padding_x: f32,
    code_padding_y: f32,
    code_max_height: Option<f32>,
    body_size: f32,
    copied: bool,
    copy_label: String,
    copied_label: String,
    base_bg: Option<Color>,
    header_bg: Option<Color>,
    text_color: Option<Color>,
    chip_bg: Option<Color>,
    chip_text: Option<Color>,
    /// Draw the outer border. Default `false`.
    framed: bool,
    on_copy_requested: Option<Box<dyn Fn(String) -> Message + 'a>>,
    /// Explicit reduced-motion override; `None` follows the theme's policy.
    reduced_motion: Option<bool>,
    /// Namespace for the copy button's derived id. Defaults to
    /// [`DEFAULT_ID_PREFIX`]; two code blocks on one screen give each its own
    /// prefix so their copy buttons do not collide on one id.
    id_prefix: String,
}

/// The namespace the copy button derives its id under when no
/// [`EnzCodeBlock::id_prefix`] is supplied. An anonymous copy button has
/// its `Click` stripped at release (`enzui_core::collect::SemanticsCollector::to_tree_update_with_ids`), so an AT user could see
/// the button and not press it; a fixed `"copy"` suffix under this namespace
/// makes it routable.
pub const DEFAULT_ID_PREFIX: &str = "enz-code-block";

/// Construct a code block displaying `code`. Defaults: detected language,
/// wrapping scroll mode, header and copy button shown.
pub fn enz_code_block<Message: Clone>(code: impl Into<String>) -> EnzCodeBlock<'static, Message> {
    EnzCodeBlock {
        code: code.into(),
        language: "auto".into(),
        scroll_mode: CodeBlockScroll::Wrap,
        show_header: true,
        show_copy: true,
        code_padding_x: 14.0,
        code_padding_y: 12.0,
        code_max_height: None,
        body_size: enzui_theme::text_scale::ui_px(12.0),
        copied: false,
        copy_label: enzui_core::localization::text("enzui-code-block-copy"),
        copied_label: enzui_core::localization::text("enzui-code-block-copied"),
        base_bg: None,
        header_bg: None,
        text_color: None,
        chip_bg: None,
        chip_text: None,
        framed: false,
        on_copy_requested: None,
        reduced_motion: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
    }
}

impl<'a, Message: Clone + 'a> EnzCodeBlock<'a, Message> {
    /// Set a language label to force its tokenizer; `"auto"` detects the source.
    /// `"text"` or an unsupported label uses plain monospace text.
    pub fn language(mut self, s: impl Into<String>) -> Self {
        self.language = s.into();
        self
    }
    /// Set the wrap-vs-horizontal-scroll behaviour. Defaults to
    /// [`CodeBlockScroll::Wrap`].
    pub fn scroll_mode(mut self, m: CodeBlockScroll) -> Self {
        self.scroll_mode = m;
        self
    }
    /// Show or hide the header row (language chip + copy button). Defaults to
    /// `true`.
    pub fn show_header(mut self, v: bool) -> Self {
        self.show_header = v;
        self
    }
    /// Show or hide the copy button within the header. Defaults to `true`.
    pub fn show_copy(mut self, v: bool) -> Self {
        self.show_copy = v;
        self
    }
    /// Horizontal padding around the code body, in px. Non-finite or negative
    /// input falls back to the default 14.
    pub fn code_padding_x(mut self, p: f32) -> Self {
        self.code_padding_x = enzui_core::metrics::FiniteNonNeg::new_or(p, 14.0).get();
        self
    }
    /// Vertical padding around the code body, in px. Non-finite or negative
    /// input falls back to the default 12.
    pub fn code_padding_y(mut self, p: f32) -> Self {
        self.code_padding_y = enzui_core::metrics::FiniteNonNeg::new_or(p, 12.0).get();
        self
    }
    /// Cap the body height, in px; taller code becomes vertically scrollable.
    /// Unset by default (grows to fit the content).
    pub fn code_max_height(mut self, h: f32) -> Self {
        self.code_max_height = Some(h);
        self
    }
    /// Monospace body text size, in px. Non-finite or non-positive input falls
    /// back to the default 12.
    pub fn body_size(mut self, s: f32) -> Self {
        self.body_size = enzui_core::metrics::FinitePositive::new_or(s, 12.0).get();
        self
    }
    /// Set the copy button's transient "copied" state. The caller owns this
    /// flag (there is no built-in timer); flip it on after a copy and back off
    /// later. Defaults to `false`.
    pub fn copied(mut self, v: bool) -> Self {
        self.copied = v;
        self
    }
    /// Accessible label on the copy icon in its idle state. Defaults to `"Copy"`.
    pub fn copy_label(mut self, s: impl Into<String>) -> Self {
        self.copy_label = s.into();
        self
    }
    /// Label on the copy button while [`Self::copied`] is `true`. Defaults to
    /// `"Copied!"`.
    pub fn copied_label(mut self, s: impl Into<String>) -> Self {
        self.copied_label = s.into();
        self
    }
    /// Override the card background. Unset by default (resolved from the theme).
    pub fn base_bg(mut self, c: Color) -> Self {
        self.base_bg = Some(c);
        self
    }
    /// Override the header background. Unset by default (resolved from the theme).
    pub fn header_bg(mut self, c: Color) -> Self {
        self.header_bg = Some(c);
        self
    }
    /// Override the body text (ink) colour. Unset by default (from the theme).
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }
    /// Override the language chip background. Unset by default (from the theme).
    pub fn chip_bg(mut self, c: Color) -> Self {
        self.chip_bg = Some(c);
        self
    }
    /// Override the language chip text colour. Unset by default (from the theme).
    pub fn chip_text(mut self, c: Color) -> Self {
        self.chip_text = Some(c);
        self
    }

    /// Paint the outer border and header/body separator. Default `false`.
    ///
    /// The one-pixel outer stroke and separator slots, the tonal fills and the
    /// radius remain in both modes; `false` makes only the line ink
    /// transparent. This keeps the flat default layout-identical to the
    /// explicit framed presentation.
    pub fn framed(mut self, framed: bool) -> Self {
        self.framed = framed;
        self
    }
    /// Override the built-in clipboard action with an application callback.
    /// It receives the full source once per activation. The app owns copying
    /// and any confirmation state when this callback is supplied.
    /// [`.show_copy(false)`](Self::show_copy) hides the action in either mode.
    pub fn on_copy_requested<F: Fn(String) -> Message + 'a>(mut self, f: F) -> Self {
        self.on_copy_requested = Some(Box::new(f));
        self
    }

    /// Skip the copied-pulse animation for users who prefer reduced motion
    /// (instant snap). This is an explicit per-instance override; when never
    /// called the widget follows the runtime theme's reduced-motion policy
    /// (see [`crate::theme_lookup::reduced_motion`]).
    pub fn reduced_motion(mut self, on: bool) -> Self {
        self.reduced_motion = Some(on);
        self
    }

    /// Namespace the copy button's derived id. Two code blocks on one
    /// screen would otherwise derive the same `{DEFAULT_ID_PREFIX}-copy` id and
    /// collide (release drops the second); a distinct prefix per block keeps
    /// each copy button routable.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Number of lines in the code, clamped to at least 1 (an empty string
    /// still counts as one line).
    pub fn line_count(&self) -> usize {
        self.code.lines().count().max(1)
    }

    /// Consume the builder and produce the widget tree.
    ///
    /// Prefer using `Into<Element>` (the `From` impl) which wraps the output
    /// in a [`CodeBlockPulse`] widget that drives the copied-pulse animation.
    /// This method is the inner builder that accepts a shared pulse-progress
    /// cell for the wrapper to update.
    fn build_inner(self, pulse_progress: Rc<Cell<f32>>) -> Element<'a, Message> {
        // card ground, header ground, seam/border, body ink and
        // the language chip all resolve against the live theme; caller-set
        // colours still win.
        let base_bg_pin = self.base_bg;
        let header_bg_pin = self.header_bg;
        let text_pin = self.text_color;
        let chip_bg_pin = self.chip_bg;
        let chip_text_pin = self.chip_text;
        let framed = self.framed;

        let automatic = self.language.trim().eq_ignore_ascii_case("auto");
        let language = if automatic {
            crate::syntax::Language::detect(&self.code)
        } else {
            crate::syntax::Language::from_label(&self.language)
        };
        let language_label = if automatic {
            use crate::syntax::Language;
            match language {
                Language::Rust => "rust",
                Language::C => "c",
                Language::Python => "python",
                Language::Json => "json",
                Language::Shell => "shell",
                Language::Markdown => "markdown",
                Language::Plain => "text",
            }
            .to_owned()
        } else {
            self.language.clone()
        };

        // Header row.
        let mut children_col = column![];
        if self.show_header {
            // Language label: monospace, caption2 (10 px), semibold, muted
            // tone, with no fill behind it. The `chip-bg` / `chip-text` pins
            // only style the copy button; this label carries neither.
            let chip = container(
                text(language_label)
                    .width(Length::Fill)
                    .wrapping(iced::advanced::text::Wrapping::None)
                    .ellipsis(iced::advanced::text::Ellipsis::End)
                    .size(crate::enz_text::TextSize::Caption2.px())
                    .font({
                        // ensure the upright-face fallback is installed at
                        // first use; keep the custom Semibold weight.
                        crate::monospace_fallback::ensure_monospace_fallback();
                        iced::Font {
                            family: iced::font::Family::Monospace,
                            weight: iced::font::Weight::Semibold,
                            ..iced::Font::DEFAULT
                        }
                    })
                    .style(move |t: &iced::Theme| iced::widget::text::Style {
                        // resolved against the header fill this block
                        // paints behind the chip, not the raw token.
                        color: Some(language_chip_ink(
                            &crate::theme_lookup::palette(t),
                            framed,
                            header_bg_pin,
                            chip_text_pin,
                        )),
                    }),
            )
            .width(Length::Fill);

            let mut header = row![chip]
                .align_y(iced::alignment::Vertical::Center)
                .spacing(8);

            if self.show_copy {
                let copy_label_text = if self.copied {
                    self.copied_label.clone()
                } else {
                    self.copy_label.clone()
                };
                // the copy factory runs at real activation time (and
                // re-reads `code` then), not at view-build time.
                let code = self.code.clone();
                let aria_label = copy_label_text;
                // give the copy button a stable derived id, or
                // release strips its Click and an AT user can see it but not
                // press it. Namespace + fixed "copy" suffix.
                let copy_id = enzui_core::identity::derive_id(&self.id_prefix, "copy");
                let icon = crate::icon::enz_icon(if self.copied { "check" } else { "action.copy" })
                    .size(enzui_theme::text_scale::ui_px(14.0))
                    .tint_resolved(move |t| {
                        copy_chip_ink(&crate::theme_lookup::palette(t), chip_bg_pin, chip_text_pin)
                    });
                let mut button = focus_button_maybe_with(
                    icon,
                    self.on_copy_requested.map(|f| {
                        Box::new(move || f(code.clone())) as Box<dyn Fn() -> Message + 'a>
                    }),
                );
                if button.on_press.is_none() {
                    button = button.copy_text(self.code.clone());
                }
                let btn_element: Element<'a, Message> = button
                    .id(copy_id)
                    .padding(5)
                    .semantics(
                        Semantics::new(Role::Button)
                            .label(aria_label)
                            .default_action(),
                    )
                    // Row motion animates this style itself.
                    .reduced_motion(true)
                    .style_focus({
                        let pulse = Rc::clone(&pulse_progress);
                        move |t: &iced::Theme, status: FocusStatus| {
                            let p = crate::theme_lookup::palette(t);
                            copy_button_style(
                                &p,
                                // Round like every icon button, by the
                                // theme's button shape.
                                crate::theme_lookup::shape_radius(t, |shape| shape.button),
                                chip_bg_pin,
                                chip_text_pin,
                                status,
                                pulse.get(),
                            )
                        }
                    })
                    .into();
                header = header.push(btn_element);
            }

            // Header treatment: 'top header section with the
            // programming language name is not rounded and no border like
            // at the bottom section'. Give the header its own
            // top-rounded radius matching the outer card so the top
            // corners follow the card curve, and paint a 1 px bottom
            // border (border colour) so the header separates cleanly
            // from the body — rounded top-half.
            let header_container = container(header)
                .width(Length::Fill)
                .padding(
                    iced::Padding::default()
                        .left(12.0)
                        .right(12.0)
                        .top(6.0)
                        .bottom(6.0),
                )
                .style(move |t: &iced::Theme| iced::widget::container::Style {
                    background: Some(
                        header_bg_pin
                            .unwrap_or_else(|| crate::theme_lookup::palette(t).surface_elevated)
                            .into(),
                    ),
                    border: Border {
                        color: Color::TRANSPARENT,
                        width: 0.0,
                        radius: {
                            // Concentric with a framed block's 1 px edge.
                            let r = (crate::theme_lookup::radius(t).lg
                                - if framed { 1.0 } else { 0.0 })
                            .max(0.0);
                            iced::border::radius(0).top_left(r).top_right(r)
                        },
                    },
                    ..Default::default()
                });
            children_col = children_col.push(header_container);
            // Always retain the one-pixel seam slot; the flat default only
            // removes its ink, so code and every following sibling stay put.
            let header_seam =
                container(Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                    .width(Length::Fill)
                    .height(Length::Fixed(1.0))
                    .style(move |t: &iced::Theme| iced::widget::container::Style {
                        background: Some(
                            if framed {
                                crate::theme_lookup::palette(t).border
                            } else {
                                Color::TRANSPARENT
                            }
                            .into(),
                        ),
                        ..Default::default()
                    });
            children_col = children_col.push(header_seam);
        }

        // Body: mono text, wrap or horizontal scroll.
        // route through `monospace_font()` so the upright-face fallback is
        // installed at first use, no app opt-in needed.
        let body_font = crate::monospace_fallback::monospace_font();

        // run the pure tokenizer over the code and route the classified
        // runs through the shared `rich_text` adapter. The tokenizer emits
        // semantic CLASSES (keyword/string/comment/…), never colours; the
        // class→tone map below is the theme-honouring layer, and each toned
        // span is re-resolved against the live theme at draw time by
        // `rich_text`'s `resolve_color` hook (see `rich_text.rs`). Unknown /
        // `"text"` languages produce a single unclassified run, so the body
        // stays flat mono — identical to the compatibility rendering.
        let body_text: Element<'a, Message> = highlighted_body(
            &self.code,
            language,
            self.body_size,
            body_font,
            text_pin,
            framed,
            base_bg_pin,
        );

        // Horizontal scrolling fits the source width; capped vertical scrolling
        // fills the body width so its scrollbar stays at the trailing edge.
        // Both regions hug short content vertically.
        //
        // Keyboard paging is handed to the *vertical* region whenever one
        // exists (`code_max_height` set). Both wrappers would otherwise see the
        // same PageDown and the inner one wins: `KeyboardScroll::update`
        // forwards the real event down first and returns early on
        // `shell.is_event_captured()` (`scroll_view.rs`), so the horizontal
        // strip would capture PageDown and slide the code sideways instead of
        // paging it down. Vertical paging is what a reader means by PageDown in
        // a capped code block, so the outer region gets the keys.
        // Forwarding invariant: deliberately NO `id`/`on_scroll`
        // forwarding on either scroll region below. Measured on the real
        // laid-out tree (Scroll mode + `code_max_height`, copy button shown):
        // the two scroll regions span y=[43.3,163.3] and y=[43.3,979.3], and
        // the block's only focusable is the copy button at y=[6.0,24.3] —
        // *above* both, because it lives in the header row, which is pushed to
        // the column before the body. The scrolled content is the tokenized
        // `rich_text` body, which is inert (no interactive spans). So
        // `reveal_focused` has nothing to
        // reveal, and an app cannot meaningfully own the offset of a
        // read-only code viewport.
        //
        // Revisit if the body ever gains focusable content (e.g. selectable
        // line anchors, or if the live highlighting path grows *interactive*
        // spans — the runs are toned but not yet clickable/selectable) — that
        // content would be *inside* the region and the forwarding would become
        // real.
        let has_vertical_cap = matches!(self.code_max_height, Some(h) if h.is_finite() && h > 0.0);
        let body_inner: Element<'a, Message> = match self.scroll_mode {
            // `body_text` is already an `Element` (the tokenized rich_text body).
            CodeBlockScroll::Wrap => body_text,
            CodeBlockScroll::Scroll => enz_scroll_view(body_text)
                .axis(Axis::Horizontal)
                .width(Length::Fit)
                .height(Length::Fit)
                .keyboard_scroll(!has_vertical_cap)
                .into(),
        };

        // Height defaults to content-fit. Only cap when the caller opts in
        // via `code_max_height(h)`. iced 0.15 Container/Scrollable have no
        // `max_height`, and `.height(Length::Fixed(h))` on the scrollable
        // *pins* the body to exactly `h` (a scrollable has no intrinsic
        // height, so `resolve_height` clamps the request to the parent with
        // no reference to content — an empty block still reserves `h`px). To
        // cap as a MAXIMUM — hug short content, scroll past tall — the scroll
        // view hugs its content (`Length::Shrink`) and `MaxHeight` imposes the
        // ceiling, inheriting the compression guard + debug assertion.
        let body_capped: Element<'a, Message> = match self.code_max_height {
            Some(h) if h.is_finite() && h > 0.0 => {
                // also composed through `EnzScrollView`. The primitive
                // takes `impl Into<Length>` for its height, so the deliberate
                // `Length::Shrink` above survives the migration verbatim --
                // this is a hug-then-cap region, not a pinned one.
                let sv = enz_scroll_view(body_inner)
                    .axis(Axis::Vertical)
                    .width(Length::Fill)
                    .height(Length::Shrink);
                crate::max_height::MaxHeight::new(sv, h).into()
            }
            _ => body_inner,
        };
        let body_container = container(body_capped).width(Length::Fill).padding(
            iced::Padding::default()
                .left(self.code_padding_x)
                .right(self.code_padding_x)
                .top(self.code_padding_y)
                .bottom(self.code_padding_y),
        );

        children_col = children_col.push(body_container);

        // A flat block has no edge, so its fill spans the header's full width.
        container(children_col)
            .width(Length::Fill)
            .padding(if framed { 1.0 } else { 0.0 })
            .style(move |t: &iced::Theme| {
                let p = crate::theme_lookup::palette(t);
                iced::widget::container::Style {
                    background: Some(base_bg_pin.unwrap_or(p.surface_alt).into()),
                    border: Border {
                        // A shade of the block's own fill.
                        color: if framed {
                            crate::surface_edge::edge_color(
                                base_bg_pin.unwrap_or(p.surface_alt),
                                crate::surface_edge::DEFAULT_EDGE_DEPTH,
                            )
                        } else {
                            Color::TRANSPARENT
                        },
                        width: if framed { 1.0 } else { 0.0 },
                        radius: crate::theme_lookup::radius(t).lg.into(),
                    },
                    ..Default::default()
                }
            })
            .into()
    }
}

/// Tokenize `code` under `language` and build the classified [`RichText`]
/// document the body renders. Kept as a dedicated helper so a test can
/// inspect the produced spans directly and prove that a recognised language
/// highlights (distinct toned runs) while `Plain`/`"text"` stays a single flat
/// run. `highlighted_body` is the only production caller.
///
/// The tokenizer partitions the source exactly, so the produced spans reproduce
/// `code` byte-for-byte. `from_ranges` fills the gaps between styled ranges with
/// default (ambient) runs; the tokenizer already guarantees non-overlapping,
/// in-bounds, char-boundary ranges, so it cannot fail — but if a tokenizer bug
/// ever produced a bad range, fall back to one flat plain run rather than
/// dropping the body.
fn body_richtext(code: &str, language: crate::syntax::Language) -> crate::rich_text::RichText {
    use crate::rich_text::{RichText, SpanStyle, StyledRange};

    // the only production tokenize call site. Counted so a test can assert
    // a theme switch does not re-tokenize — see [`BODY_TOKENIZATIONS`].
    BODY_TOKENIZATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tokens = crate::syntax::tokenize(code, language);
    let ranges: Vec<StyledRange> = tokens
        .iter()
        .filter_map(|t| {
            tone_for_class(t.class).map(|tone| StyledRange {
                start: t.start,
                end: t.end,
                style: SpanStyle {
                    tone: Some(tone),
                    ..SpanStyle::default()
                },
            })
        })
        .collect();

    RichText::from_ranges(code, ranges)
        .unwrap_or_else(|_| RichText::new(vec![crate::rich_text::RichSpan::plain(code)]))
}

/// Map a semantic [`crate::syntax::TokenClass`] to a [`TextTone`], or `None`
/// for runs that should inherit the ambient body ink (whitespace, plain,
/// identifiers, punctuation).
///
/// Delegates to [`crate::syntax::tone_for_class`], the single shared class →
/// tone decision used by both the read-only code block and the editable
/// `EnzTextArea` highlighter, so identical source classifies identically in
/// both widgets.
fn tone_for_class(class: crate::syntax::TokenClass) -> Option<crate::enz_text::TextTone> {
    crate::syntax::tone_for_class(class)
}

/// Build the code-body element: tokenize `code` under `language`, convert the
/// classified runs into a [`crate::rich_text::RichText`], and render it through
/// the shared `rich_text` adapter in monospace at `size`.
///
/// The tokenizer partitions the source exactly, so the produced spans
/// reproduce `code` byte-for-byte and no character is dropped or duplicated.
/// `text_pin` (the caller's `text_color` override, if any) is installed as the
/// theme-reactive ambient ink, so untoned runs follow the live palette.
fn highlighted_body<'a, Message: Clone + 'a>(
    code: &str,
    language: crate::syntax::Language,
    size: f32,
    font: iced::Font,
    text_pin: Option<Color>,
    framed: bool,
    base_bg_pin: Option<Color>,
) -> Element<'a, Message> {
    let doc = body_richtext(code, language);

    let tokens = crate::enz_text::TextTokens::from_palette(&enzui_theme::Palette::dark());
    let rendered = doc.view_routed_themed(
        &tokens,
        size,
        Some(font),
        None,
        None::<fn(()) -> Message>,
        Some(move |t: &iced::Theme| {
            text_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text)
        }),
        // a `Comment` run is `TextTone::Muted` and lands on the ground
        // this block paints for itself, where the raw token measures 4.35:1 in
        // the light preset. `FromRuntimePalette` would hand back that raw
        // token; this source resolves the weak rungs against the actual ground
        // (and keeps them byte-identical wherever the token already clears
        // 4.5:1, including the whole dark preset). A frameless block paints no
        // ground, so it keeps the runtime tokens unresolved.
        crate::rich_text::ThemedTokens::Custom(Box::new(move |t: &iced::Theme| {
            body_tokens(&crate::theme_lookup::palette(t), framed, base_bg_pin)
        })),
    );

    // publish the code itself to assistive tech. Without this the whole
    // body is silent -- a screen-reader user is told there is a "Copy" button
    // and a language chip but cannot read a single character of the snippet.
    // See [`BodySemantics`] for why this is one node and not one per run.
    // The code reads as selectable text: a drag selects part of it and
    // Ctrl+C copies that part. Inside a selectable document the scope steps
    // aside, so the document's selection runs through the code.
    let selectable = crate::selection_scope::enz_selection_scope(rendered)
        .yield_to_enclosing(true)
        .into_element();
    BodySemantics::wrap(selectable, code)
}

// ---------------------------------------------------------------------------
// BodySemantics -- the code body's accessibility node
// ---------------------------------------------------------------------------
//
// The body is built by `body_richtext` and rendered through the shared
// `rich_text` adapter. That adapter publishes AT nodes only for *link* spans
// (`rich_text.rs`'s `LinkSemantics`), and `body_richtext` produces no link
// spans -- every run is a syntax-classified tone. So the body published
// nothing: `code_block.rs` carried exactly one `Semantics` node and it was
// on the Copy button. An AT client saw the chrome and none of the content.
//
// This is the same shape `LinkSemantics` solves for inline links, and it is
// built the same way: a transparent single-child wrapper that delegates
// every `Widget` method to the child and publishes only in `operate()`. It
// adds no layout, no geometry and no state of its own, so it cannot move a
// pixel (asserted by `body_semantics_wrapper_is_geometry_neutral`).
//
// # One node, not one per syntax run
//
// `LinkSemantics` publishes one node per span because each link is
// separately actionable -- enumerating them is the entire point. Syntax runs
// are not actionable and not separately meaningful: a reader wants the
// snippet, and splitting `fn main() {` into `"fn"` / `" "` / `"main"` / `"("`
// would make an AT client announce four objects for four tokens. So the whole
// body is one `Role::Label` carrying the code verbatim as its accessible name.
//
// Per-*line* nodes were considered and rejected: a screen reader's own
// line-navigation operates on the announced string, so embedded newlines in a
// single label already give line-by-line inspection, without inventing N nodes
// whose bounds we would have to synthesise from `span_bounds` (which is keyed
// to spans, not lines -- a run can straddle a newline).
//
// # Not focusable, no default action
//
// The body is not interactive. It takes no Tab stop and advertises no action,
// so it cannot appear to an AT user as something to invoke. `Role::Label` is
// exactly "a non-interactive text label" (`enzui-core/src/semantics.rs`).

/// Transparent wrapper publishing the code body as one non-interactive
/// [`Role::Label`] node carrying the code verbatim. See the module note above.
struct BodySemantics<'a, Message> {
    child: Element<'a, Message>,
    /// The code text, verbatim -- the node's accessible name.
    code: String,
}

impl<'a, Message: 'a> BodySemantics<'a, Message> {
    /// Wrap `child` so it publishes `code` as a `Role::Label`.
    ///
    /// An all-whitespace (or empty) body is left unwrapped: `collect.rs` drops
    /// whitespace-only text as carrying nothing announceable, so publishing it
    /// would add a node an AT client cannot read anyway.
    fn wrap(child: Element<'a, Message>, code: &str) -> Element<'a, Message> {
        if code.trim().is_empty() {
            return child;
        }
        Element::new(BodySemantics {
            child,
            code: code.to_owned(),
        })
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for BodySemantics<'_, Message> {
    fn tag(&self) -> widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        // The body publishes one non-interactive label: no focus registration
        // or default action, because it is content rather than a control.
        let mut node = enzui_core::collect::SemanticsNode::new(
            Semantics::new(Role::Label).label(self.code.clone()),
        );
        operation.custom(None, layout.bounds(), &mut node);

        // The label already carries the code, so the child's own text is
        // hidden from semantics collection (as a leaf `SemanticsContainer`
        // does); every other operation still reaches the child.
        operation.traverse(&mut |operation| {
            use enzui_core::collect::SemanticsCollectionBoundary;
            let mut hide = SemanticsCollectionBoundary::Hide;
            operation.custom(None, layout.bounds(), &mut hide);
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
            let mut restore = SemanticsCollectionBoundary::Restore;
            operation.custom(None, layout.bounds(), &mut restore);
        });
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &widget::Tree,
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
        tree: &widget::Tree,
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
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

/// Composite `wash` (a `text`-derived overlay with alpha) over the opaque
/// `base`, mirroring the alpha-over the renderer performs when a translucent
/// fill sits on a solid one. Used to derive the copy chip's hover/pressed
/// fills from its resting fill, exactly as ghost buttons composite
/// [`Palette::overlay_hover`]/[`overlay_pressed`](Palette::overlay_pressed)
/// over their background. Returns an opaque colour so the
/// three interaction states read as distinct solid fills.
fn over(wash: Color, base: Color) -> Color {
    let a = wash.a;
    Color::from_rgb(
        wash.r * a + base.r * (1.0 - a),
        wash.g * a + base.g * (1.0 - a),
        wash.b * a + base.b * (1.0 - a),
    )
}

/// Style the copy button chip, distinguishing every interaction state.
///
/// # Interaction feedback
///
/// The resting fill carries the standard compositing overlays on top of it:
/// [`Palette::overlay_hover`] (`text`-at 0.07) for `Hovered` and
/// [`Palette::overlay_pressed`] (`text`-at 0.12, one step heavier) for
/// `Pressed`, the same washes ghost buttons, SidebarRow and TabPill read.
/// `Active` keeps the resting fill unchanged. Because the overlay is
/// `text`-derived it is theme-reactive: light themes darken, dark themes
/// lighten, and a custom theme's own overlay definitions drive it.
///
/// `Focused` reuses the resting fill and overlays the tokenized focus ring, so
/// hover-while-focused shows the hover fill and the ring.
///
/// # Copied pulse
///
/// `pulse_progress` in `0.0..=1.0` blends the *resting* background toward the
/// theme's accent colour for the "copied" pulse (`0.0` = idle, `1.0` = fully
/// highlighted). The pulse is applied to the resting fill first; the
/// hover/pressed overlay then composites on top, so a copy pulse and a hover
/// remain independently visible.
fn copy_button_style(
    palette: &Palette,
    radius: f32,
    chip_bg_pin: Option<Color>,
    chip_text_pin: Option<Color>,
    status: FocusStatus,
    pulse_progress: f32,
) -> ButtonStyle {
    let base_bg = chip_bg_pin.unwrap_or(palette.surface_hover);
    // Resting fill, blended toward accent by the copied-pulse progress.
    let resting = lerp_color(base_bg, palette.accent, pulse_progress);
    // Per-state fill: overlay the standard hover/pressed wash over the resting
    // fill so the three states read distinctly. `Active`/`Focused` keep the
    // resting fill; `Disabled` (unreachable while the chip always has a handler,
    // but kept total) also rests.
    let chip_bg = match status {
        FocusStatus::Hovered => over(palette.overlay_hover(), resting),
        FocusStatus::Pressed => over(palette.overlay_pressed(), resting),
        FocusStatus::Active | FocusStatus::Focused | FocusStatus::Disabled => resting,
    };
    let focus_w = crate::tokens::ButtonTokens::from_palette(palette).focus_ring_width;
    let (border_c, border_w) = if matches!(status, FocusStatus::Focused) {
        (palette.focus_ring, focus_w)
    } else {
        (chip_bg, 0.0)
    };
    ButtonStyle {
        background: Some(chip_bg.into()),
        border: Border {
            color: border_c,
            width: border_w,
            radius: radius.into(),
        },
        text_color: copy_chip_ink(palette, chip_bg_pin, chip_text_pin),
        shadow: iced::Shadow::default(),
        snap: false,
    }
}

/// The opaque ground this block paints under its own body, or `None` when
/// it paints none.
///
/// A framed block fills its card with `base_bg` (pin, else `surface_alt`); a
/// `framed(false)` block deliberately paints a transparent ground, so whatever
/// is behind it belongs to the composing caller and this widget cannot know
/// it. The distinction matters because the syntax body paints weak ink: a
/// `Comment` run is [`TextTone::Muted`], which measures 4.35:1 on the
/// light preset's `surface_alt`. Where the block owns the fill it
/// resolves the weak rungs against it; where it does not, the raw tokens stay
/// byte-for-byte and the ambient fill remains the caller's problem.
fn body_ground(framed: bool, base_bg_pin: Option<Color>, p: &Palette) -> Option<Color> {
    framed.then(|| base_bg_pin.unwrap_or(p.surface_alt))
}

/// The [`crate::enz_text::TextTokens`] the syntax body's toned runs resolve
/// through on every draw.
///
/// Named (rather than inlined into the token-source closure) so a test can
/// observe the exact rule the body paints with. Where the block owns its
/// ground it resolves the weak rungs against it ([`body_ground`]); where it
/// paints none the runtime tokens are handed back unresolved, which is
/// byte-for-byte the compatibility rendering.
fn body_tokens(
    p: &Palette,
    framed: bool,
    base_bg_pin: Option<Color>,
) -> crate::enz_text::TextTokens {
    match body_ground(framed, base_bg_pin, p) {
        Some(ground) => crate::enz_text::TextTokens::from_palette_on(p, ground),
        None => crate::enz_text::TextTokens::from_palette(p),
    }
}

/// The opaque ground the header row paints under the language chip, or
/// `None` for a frameless block, which paints no header fill at all.
fn header_ground(framed: bool, header_bg_pin: Option<Color>, p: &Palette) -> Option<Color> {
    framed.then(|| header_bg_pin.unwrap_or(p.surface_elevated))
}

/// The language-chip ink: the `chip_text` pin, else the muted rung
/// resolved against the header ground this block paints ([`header_ground`]).
///
/// The chip label is a normal-size text run, so the applicable clause is
/// SC 1.4.3 at 4.5:1 ([`Palette::text_muted_on`]), not the 3:1 non-text floor.
/// On the shipped presets the header ground is `surface_elevated`, where raw
/// `text_muted` already measures 5.02:1 (light) / 5.58:1 (dark), so this is
/// pixel-identical there and moves only a themed or pinned header.
fn language_chip_ink(
    p: &Palette,
    framed: bool,
    header_bg_pin: Option<Color>,
    chip_text_pin: Option<Color>,
) -> Color {
    chip_text_pin.unwrap_or_else(|| match header_ground(framed, header_bg_pin, p) {
        Some(ground) => p.text_muted_on(ground),
        None => p.text_muted,
    })
}

/// The resting fill of the copy button: the `chip_bg` pin, else
/// `surface_hover`. The copied-pulse blend toward `accent` and the
/// hover/pressed overlays composite *on top* of this, so it is the backdrop a
/// resting glyph is read against and the one [`copy_chip_ink`] resolves on.
fn copy_chip_ground(p: &Palette, chip_bg_pin: Option<Color>) -> Color {
    chip_bg_pin.unwrap_or(p.surface_hover)
}

/// The copy-button glyph ink: the `chip_text` pin, else the muted rung
/// resolved against the button's own resting fill ([`copy_chip_ground`]).
///
/// The button chooses that fill itself, and on the light preset's
/// `surface_hover` (`#E8E8ED`) raw `text_muted` measures 4.40:1, below the
/// 4.5:1 floor for the button's normal-size label. The dark preset measures
/// 6.29:1 and does not move.
fn copy_chip_ink(p: &Palette, chip_bg_pin: Option<Color>, chip_text_pin: Option<Color>) -> Color {
    chip_text_pin.unwrap_or_else(|| p.text_muted_on(copy_chip_ground(p, chip_bg_pin)))
}

/// Linear interpolation between two colours, channel by channel.
fn lerp_color(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

impl<'a, Message: Clone + 'a> EnzCodeBlock<'a, Message> {
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

impl<'a, Message: Clone + 'a> From<EnzCodeBlock<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzCodeBlock<'a, Message>) -> Self {
        CodeBlockPulse::new(w).into()
    }
}

// ---------------------------------------------------------------------------
// CodeBlockPulse — custom widget wrapper driving the "copied" pulse animation
// ---------------------------------------------------------------------------

/// `MotionScale::default().fast_ms` — the raw duration used before any
/// theme has been observed (the state is constructed before the first `draw`).
const DEFAULT_FAST_MS: f32 = 120.0;

/// Widget state for the copied-pulse animation.
///
/// # Raw transport
///
/// The theme's raw `motion-fast` and its reduced-motion flag are latched
/// *separately* in `draw`, exactly as `switch.rs::WidgetState` and
/// `row_motion.rs::RowMotionState` do, and combined at the point of use by
/// [`crate::motion::MotionLatch::effective_ms`]. Latching
/// `MotionScale::ms(Fast)` instead would store an already-reduced `0.0`, and
/// a per-instance `.reduced_motion(false)` on top of a reduced-motion theme
/// could then never recover the real duration: the reduction is lossy and
/// nothing can invert it.
///
/// The duration actually baked into `anim` is tracked in the latch's plain
/// applied-duration field, not re-read out of the latch cell. Reading the
/// cell back would compare it against the value `draw` had just written
/// there, so the comparison could never differ and the rebuild would never
/// fire: a runtime theme-duration change would then have no visible effect.
#[derive(Debug, Clone)]
struct PulseState {
    /// The `Animation<bool>` driving the pulse. `true` = copied / highlighted.
    anim: Animation<bool>,
    /// The `copied` flag observed at the previous diff, to detect edges.
    prev_copied: bool,
    /// Shared raw-duration, unknown-policy, easing, and applied-duration latch.
    motion: crate::motion::MotionLatch,
    // Timestamp from the latest `RedrawRequested`; all lifecycle phase reads
    // within one frame resolve through this single sample.
    frame_at: Cell<Option<Instant>>,
    // Visual endpoint used until the first compositor timestamp arrives.
    pre_frame_endpoint: bool,
}

impl PulseState {
    fn new(copied: bool, reduced_motion: bool) -> Self {
        let applied_ms = if reduced_motion { 0.0 } else { DEFAULT_FAST_MS };
        let easing = enzui_theme::MotionEasing::default();
        let anim = Self::make_anim(copied, applied_ms, easing);
        Self {
            anim,
            prev_copied: copied,
            motion: crate::motion::MotionLatch::new(DEFAULT_FAST_MS, applied_ms, easing),
            frame_at: Cell::new(None),
            pre_frame_endpoint: copied,
        }
    }

    /// Build an `Animation<bool>` with the given effective duration and the
    /// shared easing. `0` yields [`Duration::ZERO`], i.e. the pulse lands on its
    /// target on the very frame the flag changes — correct for a transition,
    /// and the literal reading of a `0` the caller/theme actually asked for.
    fn make_anim(
        initial: bool,
        effective_ms: f32,
        easing: enzui_theme::MotionEasing,
    ) -> Animation<bool> {
        crate::motion::build_anim(initial, effective_ms, easing)
    }

    /// Rebuild the animation when the effective duration changes — including
    /// collapsing to 0 under reduced motion and expanding back when the policy
    /// (or the theme's raw duration) changes. Preserves the current target.
    fn sync_duration(&mut self, effective_ms: f32) {
        self.motion.sync_animation(&mut self.anim, effective_ms);
    }

    fn note_frame(&self, now: Instant) {
        self.frame_at.set(Some(now));
    }

    // Eased phase at the latched frame timestamp. Before the first frame the
    // defined phase is the transition start, never wall time.
    fn sampled_progress(&self) -> f32 {
        self.frame_at.get().map_or_else(
            || if self.pre_frame_endpoint { 1.0 } else { 0.0 },
            |now| self.anim.interpolate(0.0, 1.0, now).clamp(0.0, 1.0),
        )
    }

    fn sampled_animating(&self) -> bool {
        self.frame_at.get().map_or_else(
            || self.anim.value() != self.pre_frame_endpoint,
            |now| self.anim.is_animating(now),
        )
    }
}

/// A thin wrapper widget that owns the composed code-block tree and drives the
/// copy button's "copied" pulse via `Animation<bool>`. The pulse progress is
/// shared with the copy button's style closure through an `Rc<Cell<f32>>`.
pub struct CodeBlockPulse<'a, Message: Clone> {
    inner: Element<'a, Message>,
    /// The `copied` flag from the builder — used to detect state edges.
    copied: bool,
    /// Per-instance reduced-motion override (`None` = follow theme).
    reduced_motion_override: Option<bool>,
    /// Shared cell: the wrapper writes the current pulse progress here;
    /// the copy button style closure reads it at draw time.
    pulse_progress: Rc<Cell<f32>>,
}

impl<'a, Message: Clone + 'a> CodeBlockPulse<'a, Message> {
    fn new(builder: EnzCodeBlock<'a, Message>) -> Self {
        let pulse_progress = Rc::new(Cell::new(0.0));
        let copied = builder.copied;
        let reduced_motion_override = builder.reduced_motion;
        let inner = builder.build_inner(Rc::clone(&pulse_progress));
        Self {
            inner,
            copied,
            reduced_motion_override,
            pulse_progress,
        }
    }
}

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for CodeBlockPulse<'a, Message>
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PulseState>()
    }

    fn state(&self) -> widget::tree::State {
        // Policy unknown here (no theme): seed from the explicit override
        // if present, else suppress motion until the first `draw` latches the
        // theme.
        let initial_rm = self.reduced_motion_override.unwrap_or(true);
        widget::tree::State::new(PulseState::new(self.copied, initial_rm))
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        let state = tree.state.downcast_mut::<PulseState>();

        // Resync first from what `draw` latched, so a reduced-motion policy (or
        // a changed theme duration) collapses/rebuilds *this* transition rather
        // than the one after it — the ordering `switch.rs::diff` and
        // `row_motion.rs::diff` both document. The policy is resolved through
        // the shared latch, so a still-unknown (`None`) theme means reduced
        // motion; the raw duration is only un-reduced once a
        // real theme has been observed.
        let effective_ms = state.motion.effective_ms(self.reduced_motion_override);
        state.sync_duration(effective_ms);

        // Detect edge on the `copied` flag.
        if self.copied != state.prev_copied {
            if state.frame_at.get().is_none() {
                state.pre_frame_endpoint = state.prev_copied;
            }
            state.prev_copied = self.copied;
            // A copied-state edge needs one anchor because `diff` has no frame
            // event. This is not a phase sample; lifecycle reads use `frame_at`.
            state.anim.go_mut(self.copied, Instant::now());
        }

        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Update reduced-motion from the latched theme policy.
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<PulseState>();
            state.note_frame(*now);
            let effective_ms = state.motion.effective_ms(self.reduced_motion_override);
            state.sync_duration(effective_ms);

            self.pulse_progress.set(state.sampled_progress());

            if state.sampled_animating() {
                shell.request_redraw();
            }
        }

        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Latch the theme's reduced-motion and motion-fast duration so `update`
        // (which has no `&Theme`) can honour them.
        let state = tree.state.downcast_ref::<PulseState>();
        let motion = crate::theme_lookup::motion(theme);
        // `Some(..)` so `update` can tell "policy known" from the compatibility draw
        // `None`.
        state.motion.observe(
            motion.fast_ms,
            motion.reduced_motion,
            motion.easing_or(None),
        );

        // Update pulse progress from the same timestamp update latched for
        // this frame; work between update and draw cannot move it.
        self.pulse_progress.set(state.sampled_progress());

        self.inner.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.inner.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: Clone + 'a> From<CodeBlockPulse<'a, Message>> for Element<'a, Message> {
    fn from(w: CodeBlockPulse<'a, Message>) -> Self {
        Element::new(w)
    }
}
