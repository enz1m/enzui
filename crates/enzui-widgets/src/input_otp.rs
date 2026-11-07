//! Segmented one-time-code entry with one native editor and a configurable group seam.
//!
//! Defaults to six 40 px cells, a seam every three cells and unrestricted
//! grapheme input. Narrow hosts compress cell widths/gaps and cap character
//! size while retaining the normal field height and editor identity.
//!
//! Native edits are pattern-filtered and truncated before `on_input` runs.
//! Supplied values are displayed as given, limited to the visible cell count.
//! Routed accessibility SetValue commits reach the callback without that
//! filter; the host owns validation and storing the next value.
//!
//! Colours follow the runtime theme unless tokens or a palette are pinned.
//! Token overrides win over a palette; an explicit size tier overrides token
//! cell size. Geometry resolves when building, and colour styles resolve on draw.
//!
//! Focus chrome follows the editor's live state. `focused(true)` forces paint
//! while enabled; false leaves live focus authoritative. The highlighted cell
//! is the editor's own cursor: a caret in the next empty cell while typing, no
//! caret once every cell is filled (the last cell keeps the ring). Clicking a
//! filled cell, or arrowing onto one, selects its character, so typing
//! replaces it and moves on, as one-time-code fields do on Apple platforms
//! and on the web.
//!
//! Read-only retains the editor for focus/selection but suppresses edits and
//! submit. Disabled removes the editor and focus chrome. Disabled cell colours
//! replace their alpha with 0.55, including colours with a supplied alpha.
//! One TextInput node publishes the label, value and state; empty value is omitted.
//! A caller-supplied [`EnzInputOtp::id`] enables routed accessibility identity.
//! Without it, the private editor ID survives width-dependent rebuilds only.

use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::widget::Id;
use iced::widget::text_input::{Status, Style as InputStyle};
use iced::widget::{Space, container, row, stack, text, text_input};
use iced::{Alignment, Background, Border, Color, Element, Length};
use unicode_segmentation::UnicodeSegmentation;

/// Character filter for native edit callbacks; the host owns final validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OtpPattern {
    /// Keep whole extended grapheme clusters (the default).
    #[default]
    Any,
    /// Keep ASCII digit scalars only.
    Digits,
}

/// Filter to the selected character policy and truncate to `length` cells.
/// `Any` retains whole extended grapheme clusters. `Digits` keeps ASCII digit
/// scalars, so a keycap sequence such as `1️⃣` contributes the bare digit `1`.
pub fn filter_value(raw: &str, pattern: OtpPattern, length: usize) -> String {
    match pattern {
        OtpPattern::Any => raw.graphemes(true).take(length).collect(),
        OtpPattern::Digits => raw
            .chars()
            .filter(char::is_ascii_digit)
            .take(length)
            .collect(),
    }
}

/// Colour + geometry tokens for the OTP field.
#[derive(Debug, Clone, Copy)]
pub struct InputOtpTokens {
    /// Cell fill (default the filled-field body, `surface_alt`, like every
    /// other field).
    pub background: Color,
    /// Idle cell border colour (default `field_border`).
    pub border: Color,
    /// Cell border colour when invalid (default `destructive`).
    pub error_border: Color,
    /// Border colour of the active (focused) cell (default `focus_ring`).
    pub focus_border: Color,
    /// Character ink (default `text`).
    pub text: Color,
    /// Caret colour in the active cell (default `text`).
    pub caret: Color,

    /// Natural cell width and height in logical pixels (default 40); narrow hosts compress width.
    pub cell_size: f32,
    /// Cell corner radius in px (default 6).
    pub cell_radius: f32,
    /// Idle border width in px (default 1).
    pub border_width: f32,
    /// Active-cell border width in px (default 2).
    pub focus_border_width: f32,
    /// Gap between adjacent cells in px (default 8).
    pub cell_gap: f32,
    /// Gap between groups (the seam every `group_size` cells) in px (default 16).
    pub group_gap: f32,
    /// Character text size in px (default 17).
    pub char_size: f32,
}

impl InputOtpTokens {
    /// Derive from a palette. 40 px cells, `radius-md` = 6 px,
    /// `spacing-sm` = 8 px cell gap, `spacing-lg` = 16 px group seam gap,
    /// `title-size` = 17 px character, 1 px hairline / 2 px active border.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: crate::tokens::field_fill_for(p, crate::tokens::FieldVariant::Filled),
            border: p.field_border,
            error_border: p.destructive,
            focus_border: p.focus_ring,
            text: p.text,
            caret: p.text,
            cell_size: 40.0,
            cell_radius: 6.0,
            border_width: 1.0,
            focus_border_width: enzui_theme::text_defaults::focus_ring_width(),
            cell_gap: 8.0,
            group_gap: 16.0,
            char_size: 17.0,
        }
    }

    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Replace invalid geometry with the stock metric defaults; colours pass through.
    /// Non-finite or negative lengths fall back, while zero lengths are preserved.
    /// Character size must be finite and positive, otherwise it falls back to 17 px.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        Self {
            cell_size: FiniteNonNeg::new_or(self.cell_size, 40.0).get(),
            cell_radius: FiniteNonNeg::new_or(self.cell_radius, 6.0).get(),
            border_width: FiniteNonNeg::new_or(self.border_width, 1.0).get(),
            focus_border_width: FiniteNonNeg::new_or(self.focus_border_width, 2.0).get(),
            cell_gap: FiniteNonNeg::new_or(self.cell_gap, 8.0).get(),
            group_gap: FiniteNonNeg::new_or(self.group_gap, 16.0).get(),
            // The text shaper rejects zero line height.
            char_size: FinitePositive::new_or(self.char_size, 17.0).get(),
            ..*self
        }
    }
}

/// Return exactly `length` cells, each containing one extended grapheme cluster.
/// Missing cells are `None`; excess input clusters are omitted. No pattern filter
/// is applied, and zero length returns an empty vector.
pub fn otp_cells(value: &str, length: usize) -> Vec<Option<&str>> {
    let mut out = Vec::with_capacity(length);
    let mut graphemes = value.graphemes(true);
    for _ in 0..length {
        out.push(graphemes.next());
    }
    out
}

/// Count extended grapheme clusters, capped at `length`.
pub fn filled_count(value: &str, length: usize) -> usize {
    value.graphemes(true).count().min(length)
}

/// Index the first empty grapheme cell, capped at the final cell when full.
/// Returns zero for a zero-length control; this does not follow the editor cursor.
pub fn active_index(value: &str, length: usize) -> usize {
    if length == 0 {
        return 0;
    }
    filled_count(value, length).min(length - 1)
}

/// OTP cell size tier — the same shared tier as text fields, combo boxes and
/// spin boxes, so cells can join an inline control row without a private pixel
/// height.
pub type InputOtpSize = crate::text_field::TextFieldSize;

/// Builder for a one-time-code (OTP) input row. See [`enz_input_otp`].
#[derive(Clone)]
#[must_use]
pub struct EnzInputOtp<'a, Message> {
    value: &'a str,
    on_input: std::rc::Rc<dyn Fn(String) -> Message + 'a>,
    on_submit: Option<Message>,
    length: usize,
    group_size: usize,
    pattern: OtpPattern,
    invalid: bool,
    enabled: bool,
    read_only: bool,
    focused: bool,
    label: std::borrow::Cow<'a, str>,
    id: Option<Id>,
    size: Option<InputOtpSize>,
    tokens: Option<InputOtpTokens>,
    palette: Option<Palette>,
}

/// Construct an OTP row. `on_input` receives every edit from the transparent
/// capture overlay (typing, paste, delete).
pub fn enz_input_otp<'a, Message: Clone + 'a>(
    value: &'a str,
    on_input: impl Fn(String) -> Message + 'a,
) -> EnzInputOtp<'a, Message> {
    EnzInputOtp {
        value,
        on_input: std::rc::Rc::new(on_input),
        on_submit: None,
        length: 6,
        group_size: 3,
        pattern: OtpPattern::Any,
        invalid: false,
        enabled: true,
        read_only: false,
        focused: false,
        label: enzui_core::localization::text("enzui-input-otp-label").into(),
        id: None,
        size: None,
        tokens: None,
        palette: None,
    }
}

impl<'a, Message: Clone + 'a> EnzInputOtp<'a, Message> {
    /// Number of cells (code length). Defaults to 6.
    pub fn length(mut self, n: usize) -> Self {
        self.length = n;
        self
    }
    /// Set cells per visual group; defaults to three.
    /// Zero disables group seams. `group_gap` replaces the ordinary gap at each seam.
    pub fn group_size(mut self, n: usize) -> Self {
        self.group_size = n;
        self
    }
    /// Character policy. Defaults to [`OtpPattern::Any`].
    pub fn pattern(mut self, p: OtpPattern) -> Self {
        self.pattern = p;
        self
    }
    /// Mark the input invalid (draws the error border). Defaults to `false`.
    pub fn invalid(mut self, i: bool) -> Self {
        self.invalid = i;
        self
    }
    /// Enable or disable input. Defaults to `true`.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Make the field read-only (shows a value but rejects edits). Defaults to
    /// `false`.
    pub fn read_only(mut self, r: bool) -> Self {
        self.read_only = r;
        self
    }
    /// Force active-cell focus paint when true and enabled; defaults to false.
    /// False still allows the live editor focus state to paint chrome.
    pub fn focused(mut self, f: bool) -> Self {
        self.focused = f;
        self
    }
    /// Overrides the localized accessible name for the whole control.
    pub fn label(mut self, l: impl Into<std::borrow::Cow<'a, str>>) -> Self {
        self.label = l.into();
        self
    }
    /// Message emitted when the code is submitted (Enter). Unset by default.
    pub fn on_submit(mut self, msg: Message) -> Self {
        self.on_submit = Some(msg);
        self
    }
    /// Stable widget id for programmatic focus. Unset by default.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }
    /// Use the shared inline-control size tier for cell height and natural width.
    /// Without a tier, token/default cell size applies (40 px stock). An explicit tier
    /// wins over token cell size regardless of setter order; narrow hosts compress width.
    pub fn size(mut self, size: InputOtpSize) -> Self {
        self.size = Some(size);
        self
    }

    /// Pin sanitized colour/geometry tokens, overriding palette-derived values.
    /// The size tier still overrides cell size. See [`InputOtpTokens::sanitized`]
    /// for numeric fallbacks and zero handling.
    pub fn tokens(mut self, t: InputOtpTokens) -> Self {
        self.tokens = Some(t.sanitized());
        self
    }
    /// Pin a palette to derive tokens from, instead of the runtime theme.
    /// Unset by default.
    pub fn palette(mut self, p: Palette) -> Self {
        self.palette = Some(p);
        self
    }
    fn semantics(&self) -> Semantics {
        let mut s = Semantics::new(Role::TextInput)
            .label(self.label.to_string())
            .invalid(self.invalid)
            .disabled(!self.enabled)
            .read_only(self.read_only)
            .editable(true);
        if !self.value.is_empty() {
            s = s.value(self.value.to_owned());
        }
        s
    }

    fn geometry(&self) -> InputOtpTokens {
        let mut geo = self.tokens.unwrap_or_else(|| {
            InputOtpTokens::from_palette(&self.palette.clone().unwrap_or_else(Palette::dark))
        });
        if let Some(size) = self.size {
            geo.cell_size = size.tokens().height;
        }
        geo
    }

    /// Produce the field, compressing cells and gaps to keep the complete code
    /// inside narrow hosts. The normal cell height and editor identity persist.
    pub fn into_element(self) -> Element<'a, Message> {
        let geo = self.geometry();
        let width = row_width(self.length, self.group_size, &geo);
        let probe_id = self.id.clone().unwrap_or_else(Id::unique);
        crate::responsive::enz_responsive(move |allocation: iced::Size| {
            self.clone()
                .into_element_at(allocation.width, probe_id.clone())
        })
        .width(Length::Fixed(width))
        .height(Length::Fixed(geo.cell_size))
        .into()
    }

    fn into_element_at(self, available_width: f32, probe_id: Id) -> Element<'a, Message> {
        let semantics = self.semantics();
        let mut geo = self.geometry();
        let cell_height = geo.cell_size;
        let natural_width = row_width(self.length, self.group_size, &geo);
        if available_width.is_finite() && available_width < natural_width && natural_width > 0.0 {
            let ratio = available_width.max(0.0) / natural_width;
            geo.cell_size *= ratio;
            geo.cell_gap *= ratio;
            geo.group_gap *= ratio;
            geo.char_size = geo.char_size.min(geo.cell_size);
        }
        let pin = self.palette;
        let tok_override = self.tokens;
        let enabled = self.enabled;
        let invalid = self.invalid;
        let force_focused = self.focused && enabled;
        let length = self.length;
        let group_size = self.group_size;
        let value_owned = self.value.to_owned();
        let active = active_index(self.value, self.length);
        fn resolve(
            theme: &iced::Theme,
            tok_override: Option<InputOtpTokens>,
            pin: Option<Palette>,
        ) -> InputOtpTokens {
            if let Some(t) = tok_override {
                t
            } else {
                let p = pin.unwrap_or_else(|| crate::theme_lookup::palette(theme));
                InputOtpTokens::from_palette(&p)
            }
        }

        let cells = otp_cells(self.value, self.length);
        let mut row_children: Vec<Element<'a, Message>> = Vec::with_capacity(self.length * 2);
        for (idx, ch) in cells.into_iter().enumerate() {
            if idx > 0 {
                let gap = if group_size > 0 && idx % group_size == 0 {
                    geo.group_gap
                } else {
                    geo.cell_gap
                };
                row_children.push(Space::new().width(Length::Fixed(gap)).into());
            }
            let pin_c = pin.clone();
            let inner: Element<'a, Message> = match ch {
                Some(c) => {
                    let c = c.to_owned();
                    let ch_col = move |theme: &iced::Theme| text::Style {
                        color: Some(dim_if(
                            resolve(theme, tok_override, pin_c.clone()).text,
                            !enabled,
                        )),
                    };
                    text(c).size(geo.char_size).style(ch_col).into()
                }
                None => Space::new().into(),
            };
            let pin_cell = pin.clone();
            let cell = container(inner)
                .width(Length::Fixed(geo.cell_size))
                .height(Length::Fixed(cell_height))
                .center_x(Length::Fixed(geo.cell_size))
                .center_y(Length::Fixed(cell_height))
                .style(move |theme: &iced::Theme| {
                    let t = resolve(theme, tok_override, pin_cell.clone());
                    let (border_col, border_w) = if invalid {
                        (t.error_border, t.border_width)
                    } else {
                        (t.border, t.border_width)
                    };
                    container::Style {
                        background: Some(Background::Color(dim_if(t.background, !enabled))),
                        border: Border {
                            color: dim_if(border_col, !enabled),
                            width: border_w,
                            radius: t.cell_radius.into(),
                        },
                        ..container::Style::default()
                    }
                });
            row_children.push(cell.into());
        }

        let cell_row: Element<'a, Message> = row(row_children)
            .spacing(0)
            .align_y(Alignment::Center)
            .into();

        let total_w = row_width(length, group_size, &geo);

        let on_input = self.on_input;
        let pattern = self.pattern;
        let editable = enabled && !self.read_only;
        let renders_input = enabled;
        // The private editor ID survives responsive rebuilds; only the caller ID is published.
        let published_id = self.id.clone();
        let editor_pad_y = ((cell_height - 1.3 * geo.char_size) / 2.0).max(0.0);

        let content: Element<'a, Message> = if renders_input {
            let typed = std::rc::Rc::clone(&on_input);
            let mut input = text_input("", &value_owned)
                .size(geo.char_size)
                .width(Length::Fixed(total_w))
                .padding(iced::Padding::from([editor_pad_y, 0.0]))
                .id(probe_id.clone())
                .purpose(match pattern {
                    OtpPattern::Digits => iced::advanced::input_method::Purpose::Pin,
                    OtpPattern::Any => iced::advanced::input_method::Purpose::Normal,
                })
                .style(move |_theme: &iced::Theme, _status: Status| transparent_input());
            if editable {
                input = input.on_input(move |raw| typed(filter_value(&raw, pattern, length)));
                if let Some(msg) = self.on_submit {
                    input = input.on_submit(msg);
                }
            }
            let overlay: Element<'a, Message> = container(input)
                .width(Length::Fixed(total_w))
                .height(Length::Fixed(cell_height))
                .center_y(Length::Fixed(cell_height))
                .into();

            let stacked = stack![cell_row, overlay]
                .width(Length::Fixed(total_w))
                .height(Length::Fixed(cell_height));

            OtpChrome::new(
                stacked,
                geo,
                cell_height,
                group_size,
                active,
                invalid,
                tok_override,
                pin,
                force_focused,
            )
            .editing(&value_owned, length, editable)
            .into()
        } else {
            let inert = container(cell_row)
                .width(Length::Fixed(total_w))
                .height(Length::Fixed(cell_height))
                .center_y(Length::Fixed(cell_height));
            OtpChrome::new(
                inert,
                geo,
                cell_height,
                group_size,
                active,
                invalid,
                tok_override,
                pin,
                false,
            )
            .interactive(false)
            .into()
        };
        let routed = std::rc::Rc::clone(&on_input);
        crate::text_field_semantics::TextFieldSemantics::new(
            content,
            probe_id,
            published_id,
            semantics,
        )
        .on_input(move |s| routed(s))
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzInputOtp<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzInputOtp<'a, Message>) -> Self {
        w.into_element()
    }
}

/// Deprecated spelling of [`EnzInputOtp`]; use the replacement builder name.
#[deprecated(
    since = "0.2.0",
    note = "renamed for RFC 430 acronym casing — use `EnzInputOtp`; \
            the `InputOtpTokens` sibling already uses the new spelling, and the \
            two names match"
)]
pub type EnzInputOTP<'a, Message> = EnzInputOtp<'a, Message>;
/// The first text-input state in `tree`, depth first.
fn input_state<Renderer: iced::advanced::text::Renderer>(
    tree: &iced::advanced::widget::Tree,
) -> Option<&text_input::State<Renderer::Paragraph>> {
    if tree.tag == iced::advanced::widget::tree::Tag::of::<text_input::State<Renderer::Paragraph>>()
    {
        return Some(tree.state.downcast_ref());
    }
    tree.children.iter().find_map(input_state::<Renderer>)
}

/// [`input_state`], mutably.
fn input_state_mut<Renderer: iced::advanced::text::Renderer>(
    tree: &mut iced::advanced::widget::Tree,
) -> Option<&mut text_input::State<Renderer::Paragraph>> {
    if tree.tag == iced::advanced::widget::tree::Tag::of::<text_input::State<Renderer::Paragraph>>()
    {
        return Some(tree.state.downcast_mut());
    }
    tree.children
        .iter_mut()
        .find_map(|child| input_state_mut::<Renderer>(child))
}

fn dim_if(c: Color, disabled: bool) -> Color {
    if disabled {
        crate::color::with_alpha(c, 0.55)
    } else {
        c
    }
}
fn transparent_input() -> InputStyle {
    InputStyle {
        background: Background::Color(Color::TRANSPARENT),
        border: Border::default(),
        icon: Color::TRANSPARENT,
        placeholder: Color::TRANSPARENT,
        value: Color::TRANSPARENT,
        selection: Color::TRANSPARENT,
    }
}

fn row_width(length: usize, group_size: usize, geo: &InputOtpTokens) -> f32 {
    let inter = length.saturating_sub(1);
    let seams = inter.checked_div(group_size).unwrap_or(0);
    length as f32 * geo.cell_size
        + inter.saturating_sub(seams) as f32 * geo.cell_gap
        + seams as f32 * geo.group_gap
}

fn cell_left(idx: usize, geo: &InputOtpTokens, group_size: usize) -> f32 {
    let mut x = 0.0;
    for i in 0..idx {
        let gap = if group_size > 0 && (i + 1) % group_size == 0 {
            geo.group_gap
        } else {
            geo.cell_gap
        };
        x += geo.cell_size + gap;
    }
    x
}
struct OtpChrome<'a, Message, Renderer = iced::Renderer> {
    child: Element<'a, Message, iced::Theme, Renderer>,
    geo: InputOtpTokens,
    cell_height: f32,
    group_size: usize,
    /// The cell the chrome highlights when the editor's cursor is unknown
    /// (a forced focus paint): the first empty cell.
    active: usize,
    invalid: bool,
    tok_override: Option<InputOtpTokens>,
    pin: Option<Palette>,
    force_focused: bool,
    interactive: bool,
    /// The value the cells show, and how many cells there are.
    value: String,
    length: usize,
    /// Whether clicks and arrows may place a selection.
    editable: bool,
}

impl<'a, Message, Renderer> OtpChrome<'a, Message, Renderer>
where
    Renderer: iced::advanced::text::Renderer,
{
    #[allow(
        clippy::too_many_arguments,
        reason = "the wrapper's slots, tokens and callbacks"
    )]
    fn new(
        child: impl Into<Element<'a, Message, iced::Theme, Renderer>>,
        geo: InputOtpTokens,
        cell_height: f32,
        group_size: usize,
        active: usize,
        invalid: bool,
        tok_override: Option<InputOtpTokens>,
        pin: Option<Palette>,
        force_focused: bool,
    ) -> Self {
        Self {
            child: child.into(),
            geo,
            cell_height,
            group_size,
            active,
            invalid,
            tok_override,
            pin,
            force_focused,
            interactive: true,
            value: String::new(),
            length: 0,
            editable: false,
        }
    }

    fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// The value shown, the cell count, and whether it takes edits.
    fn editing(mut self, value: &str, length: usize, editable: bool) -> Self {
        value.clone_into(&mut self.value);
        self.length = length;
        self.editable = editable;
        self
    }

    /// The cell under `x`, relative to the row's left edge.
    fn cell_at(&self, x: f32) -> Option<usize> {
        (0..self.length).find(|&idx| {
            let left = cell_left(idx, &self.geo, self.group_size);
            x >= left && x < left + self.geo.cell_size
        })
    }

    /// The cells the editor's cursor covers, and whether it is a bare caret
    /// (no selection). `None` while the editor is not focused.
    fn cursor_cells(&self, tree: &iced::advanced::widget::Tree) -> Option<(usize, usize, bool)> {
        let state = input_state::<Renderer>(tree)?;
        if !state.is_focused() || self.length == 0 {
            return None;
        }
        let value = text_input::Value::new(&self.value);
        Some(match state.cursor().state(&value) {
            text_input::cursor::State::Index(index) => {
                let cell = index.min(self.length - 1);
                (cell, cell + 1, true)
            }
            text_input::cursor::State::Selection { start, end } => {
                let (start, end) = (start.min(end), start.max(end));
                (
                    start.min(self.length - 1),
                    end.min(self.length).max(start + 1),
                    false,
                )
            }
        })
    }
    fn colors(&self, theme: &iced::Theme) -> (Color, Color) {
        let t = if let Some(t) = self.tok_override {
            t
        } else {
            let p = self
                .pin
                .clone()
                .unwrap_or_else(|| crate::theme_lookup::palette(theme));
            InputOtpTokens::from_palette(&p)
        };
        let ring = if self.invalid {
            t.error_border
        } else {
            t.focus_border
        };
        (ring, t.caret)
    }
}

impl<Message, Renderer> iced::advanced::Widget<Message, iced::Theme, Renderer>
    for OtpChrome<'_, Message, Renderer>
where
    Renderer: iced::advanced::text::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if !(self.interactive && self.editable) {
            return;
        }
        // A cursor resting before a filled character becomes a selection of it, so
        // the next key replaces it.
        let pressed_cell = match event {
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
                match cursor.position_in(layout.bounds()) {
                    Some(point) => self.cell_at(point.x),
                    None => return,
                }
            }
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { .. }) => None,
            _ => return,
        };
        let filled = filled_count(&self.value, self.length);
        let Some(state) = input_state_mut::<Renderer>(&mut tree.children[0]) else {
            return;
        };
        if !state.is_focused() {
            return;
        }
        let value = text_input::Value::new(&self.value);
        let target = match pressed_cell {
            Some(cell) => Some(cell),
            None => match state.cursor().state(&value) {
                text_input::cursor::State::Index(index) => Some(index),
                text_input::cursor::State::Selection { .. } => None,
            },
        };
        match target {
            Some(cell) if cell < filled => state.select_range(cell, cell + 1),
            Some(_) if pressed_cell.is_some() => state.move_cursor_to(filled),
            _ => return,
        }
        shell.request_redraw();
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        if !self.interactive {
            return;
        }
        let filled = filled_count(&self.value, self.length);
        let (first, end, bare) = match self.cursor_cells(&tree.children[0]) {
            Some(cells) => cells,
            None if self.force_focused => (self.active, self.active + 1, true),
            None => return,
        };
        let bounds = layout.bounds();
        let (ring, caret) = self.colors(theme);
        let cell_rect = |idx: usize| iced::Rectangle {
            x: bounds.x + cell_left(idx, &self.geo, self.group_size),
            y: bounds.y,
            width: self.geo.cell_size,
            height: self.cell_height,
        };
        for idx in first..end {
            crate::focus_button::paint_focus_ring(
                renderer,
                cell_rect(idx),
                crate::focus_button::FocusRingQuad {
                    ink: ring,
                    width: self.geo.focus_border_width,
                    radius: self.geo.cell_radius.into(),
                    outset: 0.0,
                    snap: iced::advanced::renderer::Quad::default().snap,
                },
            );
        }
        // The caret only ever waits in an empty cell: on a filled one the
        // character is selected instead, and a full code shows none.
        if !(bare && first >= filled && first < self.length) {
            return;
        }
        let cell = cell_rect(first);
        let cw = 2.0_f32;
        let ch = (self.cell_height * 0.5).max(0.0);
        let caret_rect = iced::Rectangle {
            x: cell.x + (cell.width - cw) * 0.5,
            y: cell.y + (cell.height - ch) * 0.5,
            width: cw,
            height: ch,
        };
        renderer.fill_quad(
            iced::advanced::renderer::Quad {
                bounds: caret_rect,
                ..Default::default()
            },
            Background::Color(caret),
        );
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Renderer> From<OtpChrome<'a, Message, Renderer>>
    for Element<'a, Message, iced::Theme, Renderer>
where
    Message: 'a,
    Renderer: 'a + iced::advanced::text::Renderer,
{
    fn from(w: OtpChrome<'a, Message, Renderer>) -> Self {
        Element::new(w)
    }
}
