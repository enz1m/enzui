//! Wrapping streamed text with a bidi-aware logical-end caret.
//!
//! The caller owns content and blink ticks. Toggle caret_visible every 300ms
//! while streaming; done removes the caret and its width reservation. Hidden
//! blink phases keep that reservation to prevent reflow. Effective reduced
//! motion pins the caret visible; an explicit override wins over the live theme.
//! Gate the caller subscription with blink_suppressed to avoid redundant ticks.
//!
//! Text and line height use the shared Body resolver. Narrow font setters win
//! over the token base in either order; a complete token font overrides family
//! and semantic weight. Body ink resolves pin, palette resolver, tone, then
//! live text; caret ink resolves pin, resolver, then the resolved body ink.
//!
//! Text shapes at unbounded height and paints within widget bounds and viewport.
//! Streaming trailing newlines reserve an empty caret line. Body text announces
//! by default; announce(false) suppresses its semantics while preserving selection.

use std::time::Duration;

use enzui_theme::{EnzTheme, FontFamily, FontRole, Palette};
use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
use iced::advanced::text::{self, Paragraph as _, Span};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Widget, layout, mouse, renderer};
use iced::{Color, Element, Length, Point, Rectangle, Size};
mod caret {
    use iced::advanced::text::{self, CursorPosition, Paragraph};
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Affinity {
        Before,
        After,
    }

    impl From<Affinity> for text::Affinity {
        fn from(a: Affinity) -> Self {
            match a {
                Affinity::Before => text::Affinity::Before,
                Affinity::After => text::Affinity::After,
            }
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub(super) struct CaretPoint {
        pub x: f32,
        pub line_top: f32,
        pub line_height: f32,
        pub is_rtl: bool,
    }

    impl CaretPoint {
        fn from_cursor(c: CursorPosition, line_height: f32) -> Self {
            CaretPoint {
                x: c.point.x,
                line_top: c.point.y,
                line_height,
                is_rtl: c.is_rtl(),
            }
        }
    }
    pub(super) fn resolve<P: Paragraph>(
        paragraph: &P,
        byte_offset: usize,
        affinity: Affinity,
        line_height: f32,
    ) -> Option<CaretPoint> {
        paragraph
            .logical_cursor(byte_offset, affinity.into())
            .map(|c| CaretPoint::from_cursor(c, line_height))
    }
}
/// Full caller-driven blink period: 600ms.
pub const CARET_BLINK: Duration = Duration::from_millis(600);
/// Caller tick interval for toggling visibility: 300ms.
pub const CARET_HALF_CYCLE: Duration = Duration::from_millis(300);
/// Streaming geometry and font base; colours resolve separately from the live palette.
#[derive(Debug, Clone)]
#[must_use]
pub struct StreamingTextTokens {
    /// Nominal stock body size, 13px; changing this field alone does not create a pin.
    /// Use EnzStreamingText::font_size to override the Body resolver.
    pub font_size: f32,
    font_size_explicit: bool,
    typography: Option<enzui_theme::TypographyScale>,
    /// Logical caret width, default 2; zero hides it and invalid values use 2.
    pub caret_width: f32,
    /// Logical caret height, default 16; nonpositive or non-finite values use 16.
    pub caret_height: f32,
    /// Logical end gap, default 2; zero is allowed and invalid values use 2.
    pub caret_gap: f32,
    /// Complete font override; bypasses family selection and semantic weight.
    pub font: Option<iced::Font>,
    font_family: Option<FontFamily>,
}

impl Default for StreamingTextTokens {
    fn default() -> Self {
        Self {
            font_size: 13.0,
            font_size_explicit: false,
            typography: None,
            caret_width: 2.0,
            caret_height: 16.0,
            caret_gap: 2.0,
            font: None,
            font_family: None,
        }
    }
}

impl StreamingTextTokens {
    /// Return default geometry; colours remain live and identical to light defaults.
    pub fn dark() -> Self {
        Self::default()
    }
    /// Return default geometry without pinning colours.
    pub fn light() -> Self {
        Self::default()
    }
    /// Pin a complete font, overriding family, weight, style and stretch.
    pub fn font(mut self, font: iced::Font) -> Self {
        self.font = Some(font);
        self
    }
    /// Return the complete font override, else selected UI family and Regular weight.
    /// Forwarded typography supplies weight before the application default; style and stretch are Normal.
    pub fn resolved_font(&self) -> iced::Font {
        match self.font {
            Some(explicit) => explicit,
            None => iced::Font {
                family: self
                    .font_family
                    .clone()
                    .unwrap_or_else(|| enzui_theme::text_defaults::family(FontRole::Ui))
                    .to_iced(),
                weight: self.typography.as_ref().map_or_else(
                    || enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Regular),
                    |scale| scale.iced_weight(enzui_theme::WeightRung::Regular),
                ),
                style: iced::font::Style::Normal,
                stretch: iced::font::Stretch::Normal,
                numeric: iced::font::Numeric::Proportional,
            },
        }
    }

    fn sane(mut self) -> Self {
        // Bad pins release explicit precedence so forwarded typography or the Body rung can apply.
        if !(self.font_size.is_finite() && self.font_size > 0.0) {
            self.font_size = 13.0;
            self.font_size_explicit = false;
        }
        if !(self.caret_width.is_finite() && self.caret_width >= 0.0) {
            self.caret_width = 2.0;
        }
        if !(self.caret_height.is_finite() && self.caret_height > 0.0) {
            self.caret_height = 16.0;
        }
        if !(self.caret_gap.is_finite() && self.caret_gap >= 0.0) {
            self.caret_gap = 2.0;
        }
        self
    }
    fn resolve_text_metrics(&self) -> (f32, f32) {
        // Shared size and line-height resolution keeps streamed and static text metrics aligned.
        let mut probe = crate::enz_text::enz_text("").size(crate::enz_text::TextSize::Body);
        if self.font_size_explicit {
            probe = probe.size_px(self.font_size);
        }
        if let Some(scale) = self.typography {
            probe = probe.typography(scale);
        }
        (probe.effective_size_px(), probe.effective_line_height_px())
    }
}
/// Palette and reduced-motion lookup for the streaming leaf; implemented by iced::Theme.
pub trait StreamingTextTheme {
    /// Read the live palette.
    fn enz_palette(&self) -> Palette;
    /// Read raw theme motion policy before the instance override.
    fn enz_reduced_motion(&self) -> bool;
}

impl StreamingTextTheme for iced::Theme {
    fn enz_palette(&self) -> Palette {
        crate::theme_lookup::palette(self)
    }

    fn enz_reduced_motion(&self) -> bool {
        crate::theme_lookup::reduced_motion(self)
    }
}
type PaletteInkResolver = std::rc::Rc<dyn Fn(&Palette) -> Color>;

#[derive(Clone, Default)]
struct StreamingInk {
    text: crate::ambient_ink::AmbientInk,
    caret_color: Option<Color>,
    caret_resolver: Option<PaletteInkResolver>,
}

impl std::fmt::Debug for StreamingInk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamingInk")
            .field("text", &self.text)
            .field("caret_color", &self.caret_color)
            .field("caret_resolver", &self.caret_resolver.is_some())
            .finish()
    }
}

impl StreamingInk {
    fn text(&self, palette: &Palette) -> Color {
        self.text
            .resolve(palette, crate::ambient_ink::Fallback::Text)
    }
    fn caret(&self, palette: &Palette) -> Color {
        match (self.caret_color, self.caret_resolver.as_ref()) {
            (Some(c), _) => c,
            (None, Some(f)) => f(palette),
            (None, None) => self.text(palette),
        }
    }
}
#[derive(Default)]
struct State {
    paragraph: GraphicsParagraph,
    caret: Option<caret::CaretPoint>,
}
struct StreamingText {
    text: String,
    tokens: StreamingTextTokens,
    caret_visible: bool,
    done: bool,
    reduced_motion: Option<bool>,
    ink: StreamingInk,
}

impl StreamingText {
    fn caret_reserve(&self) -> f32 {
        // Hidden blink phases retain space; completion releases it.
        if self.done {
            0.0
        } else {
            self.tokens.caret_gap + self.tokens.caret_width
        }
    }
    fn motion_reduced(&self, reduced_theme: bool) -> bool {
        self.reduced_motion.unwrap_or(reduced_theme)
    }
    fn show_caret(&self, reduced_motion_effective: bool) -> bool {
        if self.done {
            return false;
        }
        if reduced_motion_effective {
            return true;
        }
        self.caret_visible
    }

    fn text_content(&self) -> text::Text<&str> {
        let (size_px, line_height_px) = self.tokens.resolve_text_metrics();
        text::Text {
            content: self.text.as_str(),
            bounds: Size::new(f32::INFINITY, f32::INFINITY),
            size: size_px.into(),
            line_height: text::LineHeight::Absolute(line_height_px.into()),
            font: self.tokens.resolved_font(),
            align_x: text::Alignment::Default,
            align_y: iced::alignment::Vertical::Top,
            shaping: text::Shaping::Advanced,
            wrapping: text::Wrapping::Word,
            ellipsis: text::Ellipsis::None,
            hint_factor: None,
        }
    }
    fn resolve_caret(&self, paragraph: &GraphicsParagraph) -> Option<caret::CaretPoint> {
        if self.text.is_empty() {
            return None;
        }
        let (_, line_height) = self.tokens.resolve_text_metrics();
        caret::resolve(
            paragraph,
            self.text.len(),
            caret::Affinity::Before,
            line_height,
        )
    }
    fn caret_geometry(
        &self,
        resolved: Option<caret::CaretPoint>,
        caret_h: f32,
        paragraph_height: f32,
    ) -> (f32, f32) {
        // The shaper omits a trailing empty run; its logical end needs a new caret line.
        if self.text.ends_with('\n') {
            let (_, resolved_line_height) = self.tokens.resolve_text_metrics();
            let (last_top, step) = match resolved {
                Some(c) if c.line_height.is_finite() && c.line_height > 0.0 => {
                    (c.line_top, c.line_height)
                }
                _ => (0.0, resolved_line_height),
            };
            let new_y = last_top + step;
            return (0.0, new_y + (step - caret_h) / 2.0);
        }

        let Some(c) = resolved else {
            return (0.0, (paragraph_height - caret_h) / 2.0);
        };
        let gap = self.tokens.caret_gap;
        // Apply the end gap on the associated run side; RTL anchors cannot extend below zero.
        let cx = if c.is_rtl {
            (c.x - gap).max(0.0)
        } else {
            c.x + gap
        };

        let line_h = if c.line_height.is_finite() && c.line_height > 0.0 {
            c.line_height
        } else {
            caret_h
        };
        let cy = c.line_top + (line_h - caret_h) / 2.0;
        (cx, cy)
    }
}
impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for StreamingText
where
    Theme: StreamingTextTheme,
    Renderer: text::Renderer<Paragraph = GraphicsParagraph>,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Shrink, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) {
        let state = tree.state.downcast_mut::<State>();
        let caret_reserve = self.caret_reserve();
        let max = limits.max();
        let text_bounds = Size::new((max.width - caret_reserve).max(0.0), f32::INFINITY);
        let spans: [Span<'_, ()>; 1] = [Span::new(self.text.as_str())];
        let mut text = self.text_content();
        text.bounds = text_bounds;
        let paragraph_text = text::Text {
            content: spans.as_slice(),
            bounds: text.bounds,
            size: text.size,
            line_height: text.line_height,
            font: text.font,
            align_x: text.align_x,
            align_y: text.align_y,
            shaping: text.shaping,
            wrapping: text.wrapping,
            ellipsis: text.ellipsis,
            hint_factor: text.hint_factor,
        };
        state.paragraph = GraphicsParagraph::with_spans(paragraph_text);
        // Resolve logical-end affinity once per layout so paint reuses the shaped caret.
        state.caret = self.resolve_caret(&state.paragraph);

        let min = state.paragraph.min_bounds();
        // Streaming trailing newlines need the same resolved line height as the caret line.
        let trailing_newline_bump = if !self.done && self.text.ends_with('\n') {
            let (_, resolved_line_height) = self.tokens.resolve_text_metrics();
            resolved_line_height
        } else {
            0.0
        };
        let content_height = if self.done {
            min.height
        } else {
            (min.height + trailing_newline_bump).max(self.tokens.caret_height)
        };
        let intrinsic = Size::new(min.width + caret_reserve, content_height);
        let node_size = limits.resolve(Length::Shrink, Length::Shrink, intrinsic);
        layout::Node::new(node_size).apply(tree);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        // Unbounded paragraph shaping must still paint inside allocated bounds and viewport.
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        let palette = theme.enz_palette();
        let state = tree.state.downcast_ref::<State>();
        renderer.fill_paragraph(
            &state.paragraph,
            bounds.position(),
            self.ink.text(&palette),
            clip,
        );
        let reduced_effective = self.motion_reduced(theme.enz_reduced_motion());
        if !self.show_caret(reduced_effective) {
            return;
        }
        let caret_h = self.tokens.caret_height;
        let (cx, cy) = self.caret_geometry(state.caret, caret_h, bounds.height);
        let translation = bounds.position() - Point::ORIGIN;
        let caret = Rectangle {
            x: cx + translation.x,
            y: cy + translation.y,
            width: self.tokens.caret_width,
            height: caret_h,
        };
        if !(caret.x.is_finite() && caret.y.is_finite()) {
            return;
        }
        let Some(caret) = caret.intersection(&clip) else {
            return;
        };
        if caret.width <= 0.0 || caret.height <= 0.0 {
            return;
        }
        renderer.fill_quad(
            renderer::Quad {
                bounds: caret,
                border: iced::Border {
                    radius: iced::border::Radius::from(1.0),
                    ..Default::default()
                },
                ..Default::default()
            },
            self.ink.caret(&palette),
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_ref::<State>();
        operation.selectable_text(None, layout.bounds(), &self.text, &state.paragraph);
    }
}
/// Builder for streamed text, logical-end caret and optional accessible label.
#[must_use]
pub struct EnzStreamingText {
    text: String,
    done: bool,
    caret_visible: bool,
    reduced_motion: Option<bool>,
    tokens: StreamingTextTokens,
    font_size_override: Option<f32>,
    typography_override: Option<enzui_theme::TypographyScale>,
    font_family_override: Option<FontFamily>,
    ink: StreamingInk,
    aria_text: Option<String>,
    announce: bool,
}

impl EnzStreamingText {
    /// Set completion; true removes the caret and its reserved space.
    pub fn done(mut self, done: bool) -> Self {
        self.done = done;
        self
    }
    /// Set the current blink phase, default true; done hides it and reduced motion ignores phase.
    pub fn caret_visible(mut self, visible: bool) -> Self {
        self.caret_visible = visible;
        self
    }
    /// Override live theme motion policy in either direction; true pins the indicator visible.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }
    /// Replace the font and geometry base; narrow font setters retain precedence in either order.
    /// Invalid caret geometry normalizes at composition.
    pub fn tokens(mut self, tokens: StreamingTextTokens) -> Self {
        self.tokens = tokens;
        self
    }
    fn effective_tokens(&self) -> StreamingTextTokens {
        // Narrow font setters survive token-base replacement in either builder order.
        let mut tokens = self.tokens.clone();
        if let Some(px) = self.font_size_override {
            tokens.font_size = px;
            tokens.font_size_explicit = true;
        }
        if let Some(scale) = self.typography_override {
            tokens.typography = Some(scale);
        }
        if let Some(family) = &self.font_family_override {
            tokens.font_family = Some(family.clone());
        }
        tokens
    }
    /// Resolve the effective font before element composition, including narrow overrides.
    pub fn committed_font(&self) -> iced::Font {
        self.effective_tokens().resolved_font()
    }
    /// Pin logical body size, overriding forwarded typography and Body defaults in either order.
    /// Nonpositive or non-finite pins are dropped at composition, restoring forwarded or Body sizing.
    /// Line height uses the shared text resolver.
    pub fn font_size(mut self, px: f32) -> Self {
        self.font_size_override = Some(px);
        self
    }
    /// Forward Body typography and Regular weight; narrow size pins and complete fonts win.
    /// This value survives replacing the token base in either builder order.
    pub fn typography(mut self, scale: enzui_theme::TypographyScale) -> Self {
        self.typography_override = Some(scale);
        self
    }
    /// Capture the theme UI family without borrowing it; a complete token font still wins.
    /// Only family changes; this override survives replacing the token base.
    pub fn font_from_theme(mut self, theme: &EnzTheme) -> Self {
        self.font_family_override = Some(theme.font_catalog.family(FontRole::Ui).clone());
        self
    }
    /// Pin body ink, overriding resolver, tone and live palette text.
    pub fn text_color(mut self, color: Color) -> Self {
        self.ink.text.pin = Some(color);
        self
    }
    /// Resolve body ink from the live palette during painting, below pins and above tone.
    pub fn text_color_resolved(mut self, resolver: impl Fn(&Palette) -> Color + 'static) -> Self {
        self.ink.text.resolver = Some(std::rc::Rc::new(resolver));
        self
    }
    /// Resolve semantic body ink during painting, below pins and resolvers and above live text.
    pub fn text_tone(mut self, tone: crate::enz_text::TextTone) -> Self {
        self.ink.text.tone = Some(tone);
        self
    }
    /// Pin caret ink, overriding its resolver and resolved body-ink fallback.
    pub fn caret_color(mut self, color: Color) -> Self {
        self.ink.caret_color = Some(color);
        self
    }
    /// Resolve caret ink during painting, below its pin and above resolved body ink.
    pub fn caret_color_resolved(mut self, resolver: impl Fn(&Palette) -> Color + 'static) -> Self {
        self.ink.caret_resolver = Some(std::rc::Rc::new(resolver));
        self
    }
    /// Resolve body ink: pin, resolver, tone, then palette text.
    pub fn resolved_text_color(&self, palette: &Palette) -> Color {
        self.ink.text(palette)
    }
    /// Resolve caret ink: pin, resolver, then resolved body ink.
    pub fn resolved_caret_color(&self, palette: &Palette) -> Color {
        self.ink.caret(palette)
    }
    /// Override the accessible label; empty input restores the body-text default.
    pub fn aria_text(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        self.aria_text = if s.is_empty() { None } else { Some(s) };
        self
    }
    /// Publish a body label by default; false suppresses semantics while preserving text selection.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }
    /// Skip blink ticks when done or when the supplied effective motion policy pins the bar.
    /// Resolve any instance override before supplying the policy.
    pub fn blink_suppressed(&self, reduced_motion_effective: bool) -> bool {
        blink_suppressed(self.done, reduced_motion_effective)
    }
    /// Resolve the instance override before the supplied raw theme policy.
    pub fn effective_reduced_motion(&self, reduced_theme: bool) -> bool {
        self.reduced_motion.unwrap_or(reduced_theme)
    }
    /// Compose sanitized geometry and effective fonts, publishing a label unless suppressed or empty.
    pub fn into_element<'a, Message>(self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        let announce = self.announce;
        let resolved_tokens = self.effective_tokens().sane();

        let label: Option<String> = if announce {
            let l = self.aria_text.unwrap_or_else(|| self.text.clone());
            if l.is_empty() { None } else { Some(l) }
        } else {
            None
        };

        let widget = StreamingText {
            text: self.text,
            tokens: resolved_tokens,
            caret_visible: self.caret_visible,
            done: self.done,
            reduced_motion: self.reduced_motion,
            ink: self.ink,
        };
        let el: Element<'a, Message> = Element::new(widget);

        match label {
            Some(l) => crate::semantics_container::SemanticsContainer::leaf(
                el,
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Label).label(l),
            )
            .into(),
            None => crate::decorative::decorative(el).into(),
        }
    }
}

impl<'a, Message: 'a> From<EnzStreamingText> for Element<'a, Message> {
    fn from(t: EnzStreamingText) -> Self {
        t.into_element()
    }
}
/// Skip blink ticks when done or reduced; callers must supply the effective instance policy.
pub fn blink_suppressed(done: bool, reduced_motion_effective: bool) -> bool {
    done || reduced_motion_effective
}
/// Create an unfinished stream with a visible caret and live ink.
/// The caller owns blink ticks and completion; this widget installs no subscription.
pub fn enz_streaming_text(body: impl Into<String>) -> EnzStreamingText {
    EnzStreamingText {
        text: body.into(),
        done: false,
        caret_visible: true,
        reduced_motion: None,
        tokens: StreamingTextTokens::default(),
        font_size_override: None,
        typography_override: None,
        font_family_override: None,
        ink: StreamingInk::default(),
        aria_text: None,
        announce: true,
    }
}
