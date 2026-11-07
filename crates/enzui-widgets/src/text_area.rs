//! Multiline editing over caller-owned [`Content`], with field chrome and optional rows and slots.
//!
//! Apply emitted [`Action`]s to the buffer and rebuild the view. Read-only areas
//! retain selection and copy while refusing edits; disabled areas use static
//! text and release editor state. A routed SetValue replaces the buffer: through
//! [`EnzTextArea::on_set_value`] when set, else as edit actions.
//!
//! [`AreaHeight`] selects fixed, row-sized, filling or bounded growing height.
//! Optional label and footer rows retain the editor under a stable key. Use
//! [`crate::keyed_column`] when conditionally inserting sibling controls around it.
//!
//! Accessible naming and derived IDs use the first nonempty label, aria label
//! or placeholder. Duplicate names need explicit IDs or distinct prefixes.
//! Published values use the bounded prefix policy in [`semantics_value`].
//! [`EnzTextArea::highlight_with`] accepts an application parser and a live-theme
//! formatter without adding grammar bundles or changing the editor's font.
//! [`EnzTextArea::spell_check`] underlines misspelled words and puts their
//! suggestions in the edit menu.

use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{EnzTheme, FontFamily, FontRole, Palette};
use iced::advanced::widget::Id;
use iced::widget::text::Wrapping;
use iced::widget::text_editor::{
    Action, Binding, Content, KeyPress, Position as EditorPosition, Status as EditorStatus,
    Style as EditorStyle,
};
use iced::widget::{row, text};
use iced::{Background, Border, Color, Element, Length, Padding, Pixels};
use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

/// Shared text-field geometry and typography tiers.
pub use crate::text_field::TextFieldSize as FieldSize;
/// Field chrome treatments independent of the fill-role selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAreaVariant {
    /// Rounded field surface and interactive edge; the default.
    #[default]
    Field,
    /// Transparent square surface; validation errors may still paint an edge.
    Plain,
}
/// Native editor wrapping policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// Wrap at word boundaries; the default.
    #[default]
    Word,
    /// Keep logical lines unwrapped and allow horizontal scrolling.
    None,
}

impl WrapMode {
    fn to_iced(self) -> Wrapping {
        match self {
            WrapMode::Word => Wrapping::Word,
            WrapMode::None => Wrapping::None,
        }
    }
}
/// Vertical policy for the editor surface, excluding label and footer rows.
/// Parent limits can constrain the requested height. Fixed and row-based policies
/// scroll overflow; AutoGrow uses native wrapped-content layout before its cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AreaHeight {
    /// Fill the bounded parent height.
    Fill,
    /// Request logical surface height before row snapping; default is 96 px.
    Fixed(f32),
    /// Request a positive number of rows plus vertical padding.
    Rows(usize),
    /// Grow with rendered content between normalized row bounds.
    AutoGrow {
        /// Minimum rendered rows; normalized to at least one.
        min_rows: usize,
        /// Maximum rendered rows; normalized to at least the minimum.
        max_rows: usize,
    },
}

impl Default for AreaHeight {
    fn default() -> Self {
        AreaHeight::Fixed(96.0)
    }
}
/// Colour and geometry overrides for field chrome, editor text and captions.
#[derive(Debug, Clone, Copy)]
pub struct TextAreaTokens {
    /// Enabled field fill; defaults to the shared field-body role.
    pub background: Color,
    /// Idle edge ink; defaults to the palette field border.
    pub border: Color,
    /// Hovered edge ink; defaults to the palette strong field border.
    pub hover_border: Color,
    /// Validation edge, error helper and over-limit counter ink; defaults to destructive.
    pub error_border: Color,
    /// Focused edge ink; defaults to the palette focus ring.
    pub focus_border: Color,
    /// Disabled field fill; defaults to surface_alt.
    pub disabled_background: Color,
    /// Editor body ink; disabled static text applies theme opacity to it.
    pub text: Color,
    /// Enabled placeholder and caption ink; defaults to text_muted.
    pub muted: Color,
    /// Disabled placeholder and caption ink, used without an additional opacity multiplier.
    pub disabled_text: Color,
    /// Text-selection fill; defaults to accent at 30% alpha.
    pub selection: Color,
    /// Default fixed surface height in logical pixels: 96, before row snapping.
    pub area_height: f32,
    /// Chrome inset on each side in logical pixels; defaults to 10.
    pub padding: f32,
    /// Label, helper and counter size in logical pixels; defaults to scaled 12 px.
    pub label_size: f32,
}

impl TextAreaTokens {
    fn normalized(mut self) -> Self {
        use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
        self.area_height = FinitePositive::new_or(self.area_height, 96.0).get();
        self.padding = FiniteNonNeg::new_or(self.padding, 10.0).get();
        self.label_size =
            FinitePositive::new_or(self.label_size, enzui_theme::text_scale::ui_px(12.0)).get();
        self
    }

    /// Resolve palette roles with fixed height and padding and desktop-scaled caption size.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: crate::tokens::field_fill(p),
            border: p.field_border,
            hover_border: p.field_border_strong,
            error_border: p.destructive,
            focus_border: p.focus_ring,
            disabled_background: p.surface_alt,
            text: p.text,
            muted: p.text_muted,
            disabled_text: crate::color::disabled_text_ink(p, p.surface_alt),
            selection: crate::color::with_alpha(p.accent, 0.30),
            area_height: 96.0,
            padding: 10.0,
            label_size: enzui_theme::text_scale::ui_px(12.0),
        }
    }
    /// Resolve editor inks against an opaque paper colour.
    /// Paper luminance selects the stock palette; body and disabled inks use contrast
    /// resolution, while caption ink moves toward paper only when its contrast clears
    /// the non-text floor. Borders and selection retain that palette’s roles.
    pub fn page(paper: Color) -> Self {
        use enzui_theme::ContrastFloor;

        let palette = if crate::accessibility_metrics::luminance(paper) > 0.5 {
            Palette::light()
        } else {
            Palette::dark()
        };
        let mut tokens = Self::from_palette(&palette);
        let body = crate::tokens::ink_for(paper, &palette);
        tokens.background = paper;
        tokens.disabled_background = paper;
        tokens.text = body;
        tokens.muted = quieter_on(body, paper, ContrastFloor::NonText);
        tokens.disabled_text = crate::color::disabled_text_ink(&palette, paper);
        tokens
    }
    /// Use the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Use the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}
fn quieter_on(ink: Color, paper: Color, floor: enzui_theme::ContrastFloor) -> Color {
    let candidate = crate::color::blend(ink, paper);
    let ratio = crate::accessibility_metrics::contrast_ratio(candidate, paper);
    if ratio.is_finite() && ratio >= floor.minimum_ratio() {
        candidate
    } else {
        ink
    }
}
/// Format count / max when max is nonzero, otherwise the count alone.
pub fn counter_text(count: usize, max: usize) -> String {
    if max > 0 {
        format!("{count} / {max}")
    } else {
        count.to_string()
    }
}
/// Return whether count exceeds a configured nonzero maximum.
pub fn is_over_limit(count: usize, max: usize) -> bool {
    max > 0 && count > max
}
mod char_scan_counter {
    use std::cell::Cell;

    thread_local! {
        pub(super) static BUFFER_CHAR_SCANS: Cell<usize> = const { Cell::new(0) };
    }
}
/// Read this thread’s count of full-buffer character scans performed for counter rows.
#[doc(hidden)]
#[must_use]
pub fn buffer_char_scans() -> usize {
    char_scan_counter::BUFFER_CHAR_SCANS.with(std::cell::Cell::get)
}
/// Reset this thread’s full-buffer character-scan counter and return its previous value.
#[doc(hidden)]
pub fn reset_buffer_char_scans() -> usize {
    char_scan_counter::BUFFER_CHAR_SCANS.with(|c| c.replace(0))
}
fn scan_char_count(content: &Content) -> usize {
    char_scan_counter::BUFFER_CHAR_SCANS.with(|c| c.set(c.get().wrapping_add(1)));
    content.text().chars().count()
}
mod semantics_value_counter {
    use std::cell::Cell;

    thread_local! {
        pub(super) static SEMANTICS_VALUE_MATERIALIZATIONS: Cell<usize> =
            const { Cell::new(0) };
    }
}
/// Read this thread’s count of bounded semantics-value materializations.
#[doc(hidden)]
#[must_use]
pub fn semantics_value_materializations() -> usize {
    semantics_value_counter::SEMANTICS_VALUE_MATERIALIZATIONS.with(std::cell::Cell::get)
}
#[inline]
pub(crate) fn note_semantics_value_materialization() {
    semantics_value_counter::SEMANTICS_VALUE_MATERIALIZATIONS
        .with(|c| c.set(c.get().wrapping_add(1)));
}
/// Reset this thread’s semantics-value counter and return its previous value.
#[doc(hidden)]
pub fn reset_semantics_value_materializations() -> usize {
    semantics_value_counter::SEMANTICS_VALUE_MATERIALIZATIONS.with(|c| c.replace(0))
}
/// Default UTF-8 byte budget for the value prefix: 8 KiB, excluding the truncation marker.
pub const SEMANTICS_VALUE_BUDGET: usize = 8 * 1024;
/// Suffix appended when complete buffer lines exceed the value-prefix budget.
pub const SEMANTICS_VALUE_TRUNCATION_MARKER: &str = "… [truncated]";
/// Copy complete leading buffer lines within a UTF-8 byte budget.
/// A fitting document equals Content::text(). On overflow, trailing ASCII spaces
/// and newlines are removed before appending [`SEMANTICS_VALUE_TRUNCATION_MARKER`];
/// the marker may extend the result beyond budget. A first line larger than budget
/// yields the marker alone. Increments [`semantics_value_materializations`] once.
pub fn semantics_value(content: &Content, budget: usize) -> String {
    note_semantics_value_materialization();

    let line_count = content.line_count();
    let mut out = String::new();
    let mut truncated = false;

    for i in 0..line_count {
        let Some(line) = content.line(i) else { break };
        // Match Content::text(): every nonfinal logical line contributes one newline.
        let sep_len = if i + 1 < line_count { 1 } else { 0 };
        let addition = line.text.len() + sep_len;

        if out.len().saturating_add(addition) > budget {
            truncated = true;
            break;
        }
        out.push_str(&line.text);
        if sep_len == 1 {
            out.push('\n');
        }
    }

    if truncated {
        // Trim only ASCII spaces and newlines before the marker.
        while out.ends_with('\n') || out.ends_with(' ') {
            out.pop();
        }
        out.push_str(SEMANTICS_VALUE_TRUNCATION_MARKER);
    }

    out
}
/// Keep a finite positive height; otherwise return default without validating it.
pub fn sanitize_height(requested: f32, default: f32) -> f32 {
    if requested.is_finite() && requested > 0.0 {
        requested
    } else {
        default
    }
}
/// Snap height minus twice padding to the nearest positive whole row count.
/// An invalid or nonpositive row size returns height unchanged. A residual below
/// 1 px also preserves height; otherwise at least one row plus padding is returned.
/// Height and padding are not independently validated by this helper.
pub fn round_height_to_rows(height: f32, row_px: f32, padding: f32) -> f32 {
    let pad_v = padding * 2.0;
    if !(row_px.is_finite() && row_px > 0.0) {
        return height;
    }
    let content_h = (height - pad_v).max(0.0);
    let rows = (content_h / row_px).round().max(1.0);
    let residual = (content_h - rows * row_px).abs();
    if residual < 1.0 {
        return height;
    }
    rows * row_px + pad_v
}
/// Measure word-wrapped default Body text at the supplied width and text size.
/// Does not inherit an area’s font, wrapping or line height. Empty text and
/// nonpositive or nonfinite widths yield zero; use [`EnzTextArea::row_count`]
/// for a configured editor.
pub fn row_count_at(text: &str, width: f32, text_size_px: f32) -> usize {
    let typo = crate::text_measure::MeasureTypography::new(crate::enz_text::TextSize::Body)
        .pin_size_px(text_size_px);
    crate::text_measure::enz_text_measure_with(text, width, typo).line_count
}

struct RowCountCache {
    text: String,
    last_measurement: Option<(
        iced::advanced::graphics::text::Version,
        f32,
        crate::text_measure::MeasureTypography,
        usize,
    )>,
}
fn dim_if(c: Color, disabled: bool, theme: &iced::Theme) -> Color {
    if disabled {
        crate::color::with_opacity(c, crate::theme_lookup::opacity(theme).disabled())
    } else {
        c
    }
}
/// Default namespace for IDs derived from nonempty accessible-name keys.
pub const DEFAULT_ID_PREFIX: &str = "enz-text-area";
type NativeEditor<'a, Message> =
    iced::widget::TextEditor<'a, iced::advanced::text::parser::PlainText, Message>;
type SyntaxInstaller<'a, Message> =
    Box<dyn FnOnce(NativeEditor<'a, Message>) -> Element<'a, Message> + 'a>;
/// Builder for a multiline field using a borrowed, host-managed editor buffer.
#[must_use]
pub struct EnzTextArea<'a, Message> {
    content: &'a Content,
    on_action: Rc<dyn Fn(Action) -> Message + 'a>,
    on_submit: Option<Message>,
    submit_key: SubmitKey,
    on_paste: Option<Message>,
    on_set_value: Option<Box<dyn Fn(String) -> Message + 'a>>,
    label: Cow<'a, str>,
    placeholder: Cow<'a, str>,
    aria_label: Option<String>,
    size: FieldSize,
    radius: Option<f32>,
    variant: TextAreaVariant,
    fill_variant: crate::tokens::FieldVariant,
    wrap: WrapMode,
    enabled: bool,
    read_only: bool,
    edit_menu: bool,
    error_text: Option<Cow<'a, str>>,
    description: Option<Cow<'a, str>>,
    show_count: bool,
    max_length: usize,
    area_height: Option<AreaHeight>,
    min_width: Option<f32>,
    id: Option<Id>,
    id_prefix: String,
    tokens: Option<TextAreaTokens>,
    palette: Option<Palette>,
    syntax: Option<crate::syntax::Language>,
    syntax_installer: Option<SyntaxInstaller<'a, Message>>,
    rich_formatting: Option<crate::rich_edit::RichEditPaintSettings>,
    background_highlights: Vec<(std::ops::Range<EditorPosition>, Color)>,
    retain_selection: bool,
    spell_check: Option<enzui_core::spelling::SpellCheck>,
    leading: Option<Element<'a, Message>>,
    trailing: Option<Element<'a, Message>>,
    font_family: Option<FontFamily>,
    font_role: FontRole,
    text_size_override: Option<f32>,
    line_height_override: Option<iced::advanced::text::LineHeight>,
    row_count_cache: RefCell<Option<RowCountCache>>,
    key_binding: Option<HostKeyBinding<'a, Message>>,
    on_drag_update: Option<Box<dyn Fn(iced::widget::text_editor::Cursor) -> Message + 'a>>,
    gutter: Option<iced::advanced::text::editor::gutter::Gutter>,
    floats: Vec<crate::text_float::TextFloat<'a, Message>>,
}

/// A host's key bindings, consulted before the field's own.
type HostKeyBinding<'a, Message> = Rc<dyn Fn(KeyPress) -> Option<Binding<Message>> + 'a>;
/// Create an enabled, editable field with word wrapping, UI font and fixed default height.
/// The callback emits actions for the host to apply. Defaults to Md sizing,
/// filled chrome, no labels or slots, and no counter or submit callback.
pub fn enz_text_area<'a, Message: Clone + 'a>(
    content: &'a Content,
    on_action: impl Fn(Action) -> Message + 'a,
) -> EnzTextArea<'a, Message> {
    EnzTextArea {
        content,
        on_action: Rc::new(on_action),
        on_submit: None,
        submit_key: SubmitKey::Enter,
        on_paste: None,
        on_set_value: None,
        label: Cow::Borrowed(""),
        placeholder: Cow::Borrowed(""),
        aria_label: None,
        size: FieldSize::default(),
        radius: None,
        variant: TextAreaVariant::default(),
        fill_variant: crate::tokens::FieldVariant::default(),
        wrap: WrapMode::default(),
        enabled: true,
        read_only: false,
        edit_menu: true,
        error_text: None,
        description: None,
        show_count: false,
        max_length: 0,
        area_height: None,
        min_width: None,
        id: None,
        id_prefix: DEFAULT_ID_PREFIX.to_owned(),
        tokens: None,
        palette: None,
        syntax: None,
        syntax_installer: None,
        rich_formatting: None,
        background_highlights: Vec::new(),
        retain_selection: false,
        spell_check: None,
        leading: None,
        trailing: None,
        font_family: None,
        font_role: FontRole::Ui,
        text_size_override: None,
        line_height_override: None,
        row_count_cache: RefCell::new(None),
        key_binding: None,
        on_drag_update: None,
        gutter: None,
        floats: Vec::new(),
    }
}

impl<'a, Message: Clone + 'a> EnzTextArea<'a, Message> {
    /// Bind keys before the field does, for a host that edits a document
    /// larger than the editor shows: route Copy, Cut or Select All to the
    /// whole selection it keeps, say. `Some(binding)` wins; `None` leaves the
    /// key to the field's own bindings (submit, the paste hook, read-only
    /// filtering); `Some(Binding::Sequence(vec![]))` swallows it. The edit
    /// menu carries out its entries as their shortcuts, so they reach this
    /// too.
    pub fn key_binding(
        mut self,
        key_binding: impl Fn(KeyPress) -> Option<Binding<Message>> + 'a,
    ) -> Self {
        self.key_binding = Some(Rc::new(key_binding));
        self
    }

    /// Notify the host of each drag-selection step instead of publishing it
    /// as an `Action::Drag`; the editor performs the drag itself. See
    /// `iced::widget::TextEditor::on_drag_update`.
    pub fn on_drag_update(
        mut self,
        on_drag_update: impl Fn(iced::widget::text_editor::Cursor) -> Message + 'a,
    ) -> Self {
        self.on_drag_update = Some(Box::new(on_drag_update));
        self
    }

    /// Float `float` in the text: the text flows around it and it scrolls
    /// with the text (see [`crate::text_float`]). Not combined with a
    /// [`Self::gutter`], which moves the text's origin.
    pub fn float(mut self, float: crate::text_float::TextFloat<'a, Message>) -> Self {
        self.floats.push(float);
        self
    }

    /// Show a line-number gutter before the text, numbered by the host (for
    /// a view into a larger document, its global line numbers).
    pub fn gutter(mut self, gutter: iced::advanced::text::editor::gutter::Gutter) -> Self {
        self.gutter = Some(gutter);
        self
    }

    /// Set the visible label and first accessible-name/derived-ID fallback; empty omits its row.
    pub fn label(mut self, l: impl Into<Cow<'a, str>>) -> Self {
        self.label = l.into();
        self
    }
    /// Set empty-editor placeholder text and the final accessible-name fallback.
    pub fn placeholder(mut self, p: impl Into<Cow<'a, str>>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// Select text-size and live corner-radius tiers; defaults to Md.
    /// Explicit text size and radius pins take precedence independently.
    pub fn size(mut self, s: FieldSize) -> Self {
        self.size = s;
        self
    }
    /// Pin chrome corners in logical pixels; invalid or negative values pin square corners.
    /// Plain surfaces remain square regardless of this pin.
    pub fn radius(mut self, radius: f32) -> Self {
        self.radius = Some(enzui_core::metrics::FiniteNonNeg::new_or(radius, 0.0).get());
        self
    }
    /// Select Filled field-body or Outlined page-surface fill when tokens are unpinned.
    /// Explicit token overrides win; [`Self::variant`] independently controls chrome.
    pub fn fill_variant(mut self, v: crate::tokens::FieldVariant) -> Self {
        self.fill_variant = v;
        self
    }
    /// Select Field or transparent square Plain chrome; defaults to Field.
    /// Validation errors can still paint an edge on Plain.
    pub fn variant(mut self, v: TextAreaVariant) -> Self {
        self.variant = v;
        self
    }
    /// Choose word wrapping or unwrapped horizontal scrolling; defaults to Word.
    pub fn wrap(mut self, w: WrapMode) -> Self {
        self.wrap = w;
        self
    }
    /// Enable editing and focus; defaults to true.
    /// False replaces the native editor with clipped static text, releasing its state
    /// and suppressing submit, paste and routed replacement callbacks.
    pub fn enabled(mut self, e: bool) -> Self {
        self.enabled = e;
        self
    }
    /// Keep selection, navigation and copy while refusing mutations; defaults to false.
    /// IME and clipboard edit actions reach the host callback as zero-line scroll
    /// no-ops. Submit, paste notifications and routed replacements are suppressed.
    pub fn read_only(mut self, r: bool) -> Self {
        self.read_only = r;
        self
    }
    /// Whether a right click (or the Menu key) opens the built-in edit menu.
    /// On by default; [`into_context_menu_host`](Self::into_context_menu_host)
    /// turns it off in favour of the application's menu.
    pub fn edit_menu(mut self, enabled: bool) -> Self {
        self.edit_menu = enabled;
        self
    }
    /// Set validation text and invalid state; an empty string removes both.
    /// Whitespace-only text remains an error. Error text takes precedence over description.
    pub fn error(mut self, t: impl Into<Cow<'a, str>>) -> Self {
        let t = t.into();
        self.error_text = if t.is_empty() { None } else { Some(t) };
        self
    }
    /// Set a helper row and accessible description when there is no error.
    /// Empty removes the helper; whitespace-only text is retained.
    pub fn description(mut self, t: impl Into<Cow<'a, str>>) -> Self {
        let t = t.into();
        self.description = if t.is_empty() { None } else { Some(t) };
        self
    }
    /// Show a Unicode-scalar count below the field; defaults to false.
    /// Only shown counter rows perform a full-buffer copy and count.
    pub fn show_count(mut self, s: bool) -> Self {
        self.show_count = s;
        self
    }
    /// Set a display-only character maximum for counter text and over-limit ink.
    /// Zero means no maximum. This does not reject or truncate edits.
    pub fn max_length(mut self, n: usize) -> Self {
        self.max_length = n;
        self
    }
    /// Request fixed surface height in logical pixels, with overflow scrolling.
    /// Invalid or nonpositive requests fall back to the token height before row
    /// snapping. Shares policy state with rows, fill_height and auto_grow; last wins.
    pub fn area_height(mut self, h: f32) -> Self {
        self.area_height = Some(AreaHeight::Fixed(h));
        self
    }
    /// Request at least one row of surface height plus vertical padding.
    /// Uses the configured text size and line height. Shares vertical policy state
    /// with area_height, fill_height and auto_grow; last wins.
    pub fn rows(mut self, n: usize) -> Self {
        self.area_height = Some(AreaHeight::Rows(n.max(1)));
        self
    }
    /// Fill the bounded parent’s available height, with overflow scrolling.
    /// Independent of width; replaces the other vertical policies.
    pub fn fill_height(mut self) -> Self {
        self.area_height = Some(AreaHeight::Fill);
        self
    }
    /// Grow from native wrapped-content layout between a row floor and cap, then scroll.
    /// The minimum is at least one row and the maximum at least the minimum. Edge
    /// slots can contribute to intrinsic height. Replaces the other vertical policies.
    pub fn auto_grow(mut self, min_rows: usize, max_rows: usize) -> Self {
        self.area_height = Some(AreaHeight::AutoGrow { min_rows, max_rows });
        self
    }
    /// Set a width floor while filling the parent, clamped to its maximum.
    /// Invalid or negative inputs retain the previous floor; zero and [`Self::fill`]
    /// clear it. Width and height policies are independent.
    pub fn min_width(mut self, w: f32) -> Self {
        if w.is_finite() && w >= 0.0 {
            self.min_width = if w == 0.0 { None } else { Some(w) };
        }
        self
    }
    /// Restore filling width with no minimum floor; height policy is unchanged.
    pub fn fill(mut self) -> Self {
        self.min_width = None;
        self
    }
    /// Notify before a focused editable editor requests native text paste.
    /// The ordinary paste binding still follows. Clipboard replies and IME commits
    /// do not notify; disabled and read-only fields emit nothing.
    pub fn on_paste(mut self, message: Message) -> Self {
        self.on_paste = Some(message);
        self
    }
    /// Emit this message for the selected submit chord in an editable, focused editor.
    /// The chord consumes its newline; no callback is configured by default.
    pub fn on_submit(mut self, msg: Message) -> Self {
        self.on_submit = Some(msg);
        self
    }
    /// Choose the submit chord; defaults to Enter without Shift.
    /// CommandEnter uses the platform command modifier and leaves bare Enter to
    /// insert a newline. Without a submit callback, command-Enter is left to the host.
    pub fn submit_key(mut self, key: SubmitKey) -> Self {
        self.submit_key = key;
        self
    }
    /// Service routed SetValue with a whole-buffer replacement string; the host
    /// replaces its Content. Requires a routable ID and is suppressed while
    /// disabled or read-only. Without this callback, a routed SetValue arrives
    /// through the action callback as `SelectAll` then a `Paste` of the new
    /// text, which replaces the buffer as one ordinary edit.
    pub fn on_set_value(mut self, on_set_value: impl Fn(String) -> Message + 'a) -> Self {
        self.on_set_value = Some(Box::new(on_set_value));
        self
    }
    /// Set an accessibility-only name fallback without adding a visible label row.
    /// Nonempty visible label wins; then nonempty aria label, then placeholder.
    /// The same key supplies a derived ID when no explicit ID is set.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }
    /// Namespace the name-derived ID to distinguish duplicate names.
    /// Ignored when an explicit [`Self::id`] is supplied.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }
    /// Set the stable ID used for focus and routed value changes.
    /// Without it, a nonempty accessible-name key derives an ID; unnamed fields
    /// remain anonymous and cannot receive routed replacement.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }
    /// Pin colours and geometry instead of deriving tokens from palette or runtime theme.
    /// Does not pin corners; use [`Self::radius`] for that. Invalid dimensions
    /// use defaults; height and caption size must be positive. Zero padding is retained.
    pub fn tokens(mut self, t: TextAreaTokens) -> Self {
        self.tokens = Some(t);
        self
    }
    /// Pin the palette used when no explicit tokens are supplied.
    /// Corners continue to follow the runtime theme unless explicitly pinned.
    pub fn palette(mut self, p: Palette) -> Self {
        self.palette = Some(p);
        self
    }
    /// Highlight with the shared syntax tokenizer; unset means unhighlighted text.
    /// Replaces a caller parser without changing the font role.
    /// Plain language supplies flat highlighting.
    pub fn syntax(mut self, language: crate::syntax::Language) -> Self {
        self.syntax = Some(language);
        self.syntax_installer = None;
        self
    }
    /// Choose syntax via the shared case-insensitive language-label resolver.
    /// Unknown labels resolve to Plain; font selection remains independent.
    pub fn syntax_label(self, label: &str) -> Self {
        self.syntax(crate::syntax::Language::from_label(label))
    }

    /// Highlight with a caller's line parser and live-theme token formatter.
    /// Replaces any syntax selection; a later `syntax` or `syntax_label` restores
    /// the shared lexer. Rich-edit formatting takes precedence. Disabled areas
    /// retain their static-text fallback and do not construct a parser.
    pub fn highlight_with<P: iced::advanced::text::Parser>(
        mut self,
        settings: P::Settings,
        to_format: fn(&P::Output, &iced::Theme) -> iced::advanced::text::highlighter::Style,
    ) -> Self
    where
        P::Settings: 'a,
    {
        self.syntax = None;
        self.syntax_installer = Some(Box::new(move |editor| {
            editor.highlight_with::<P>(settings, to_format).into()
        }));
        self
    }

    pub(crate) fn rich_formatting(
        mut self,
        settings: crate::rich_edit::RichEditPaintSettings,
    ) -> Self {
        self.rich_formatting = Some(settings);
        self
    }
    /// Set half-open editor-position ranges painted behind text, in supplied order.
    /// Later overlapping runs can cover earlier ones. The host owns range validity
    /// and must update positions when edits change the document.
    pub fn background_highlights(
        mut self,
        runs: Vec<(std::ops::Range<EditorPosition>, Color)>,
    ) -> Self {
        self.background_highlights = runs;
        self
    }
    /// Check spelling with `check`: misspelled words get a wavy underline in
    /// the theme's error colour, and a right click on one offers its
    /// suggestions (choosing one replaces the word through the ordinary edit
    /// callback), "Add to Dictionary" and "Ignore" above the edit commands.
    /// Share one [`SpellCheck`](enzui_core::spelling::SpellCheck) between
    /// fields so an ignored word is ignored in all of them. Read-only areas
    /// are not checked.
    pub fn spell_check(mut self, check: &enzui_core::spelling::SpellCheck) -> Self {
        self.spell_check = Some(check.clone());
        self
    }
    /// Keep the selection painted, in [`TextAreaTokens::selection`], while
    /// the editor is unfocused: for a formatting menu, link dialog or other
    /// popup that takes focus over the same selection. The caret still hides
    /// and keyboard focus does not move. Off by default.
    pub fn retain_selection(mut self, retain: bool) -> Self {
        self.retain_selection = retain;
        self
    }
    /// Place an element before the editor, reducing available text width.
    /// Fixed and row-sized surfaces top-align the slot; growing surfaces align the
    /// slot row at the bottom. Slot content supplies its own accessibility semantics.
    pub fn leading(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.leading = Some(element.into());
        self
    }
    /// Place an element after the editor, anchored at the surface bottom.
    /// Reduces text width and can affect AutoGrow intrinsic height. Slot content
    /// supplies its own accessibility semantics.
    pub fn trailing(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(element.into());
        self
    }
    pub(crate) fn with_font_family(mut self, family: FontFamily) -> Self {
        self.font_family = Some(family);
        self
    }

    /// Pin the current font role’s family from the supplied theme.
    /// Select the role first; regular weight remains application-wide.
    pub fn font_from_theme(mut self, theme: &EnzTheme) -> Self {
        self.font_family = Some(theme.font_catalog.family(self.font_role).clone());
        self
    }
    /// Choose the unpinned catalog font role; defaults to Ui.
    /// An explicit family pin retains precedence.
    pub fn font_role(mut self, role: FontRole) -> Self {
        self.font_role = role;
        self
    }
    /// Pin absolute logical text size independently of desktop text scale and size tier.
    /// The size tier still selects corners; this setter does not validate the value.
    pub fn text_size_px(mut self, px: f32) -> Self {
        self.text_size_override = Some(px);
        self
    }
    /// Override the editor’s default relative 1.3 line height.
    /// The same value drives row-sized and growing height calculations. Companion
    /// gutters must use matching line height and padding.
    pub fn line_height(mut self, lh: impl Into<iced::advanced::text::LineHeight>) -> Self {
        self.line_height_override = Some(lh.into());
        self
    }
    /// Resolve the pinned or application font family with regular weight.
    pub fn committed_font(&self) -> iced::Font {
        let mut font =
            enzui_theme::text_defaults::font(self.font_role, enzui_theme::WeightRung::Regular);
        if let Some(family) = &self.font_family {
            font.family = family.to_iced();
        }
        font
    }

    fn text_size(&self) -> f32 {
        let fallback = Self::text_size_for(self.size);
        enzui_core::metrics::FinitePositive::new_or(
            self.text_size_override.unwrap_or(fallback),
            fallback,
        )
        .get()
    }
    /// Resolve Sm/Md/Lg text sizes of 12/13/15 px at the effective desktop text scale.
    pub fn text_size_for(size: FieldSize) -> f32 {
        let ramp = match size {
            FieldSize::Sm => 12.0,
            FieldSize::Md => 13.0,
            FieldSize::Lg => 15.0,
        };
        enzui_theme::text_scale::effective().apply_px(ramp)
    }
    /// Return a positive finite override or the scaled tier size.
    pub fn resolved_text_size(&self) -> f32 {
        self.text_size()
    }
    /// Return per-side chrome padding from explicit tokens, otherwise 10 px.
    pub fn resolved_padding(&self) -> f32 {
        self.tokens.map_or(10.0, |t| t.normalized().padding)
    }
    /// Subtract both horizontal chrome insets from an outer width, flooring at zero.
    /// Does not subtract configured edge-slot widths or their spacing; callers must
    /// account for those before using this result as a text-measurement width.
    pub fn text_content_width(&self, area_width: f32) -> f32 {
        (area_width - 2.0 * self.resolved_padding()).max(0.0)
    }
    /// Measure visual rows with this area’s font, size, line height, shaping and wrapping.
    /// Width must already exclude chrome and slots. Empty content and invalid or
    /// nonpositive width yield zero. Repeated calls cache the immutable borrowed
    /// buffer snapshot and shaped result; font generation changes invalidate shaping.
    /// AutoGrow uses native editor layout instead of this helper.
    pub fn row_count(&self, width: f32) -> usize {
        if !width.is_finite() || width <= 0.0 {
            return 0;
        }

        let text_size = self.text_size();
        let committed = self.committed_font();
        let line_height = self
            .line_height_override
            .unwrap_or_default()
            .to_absolute(Pixels(text_size))
            .0;
        let typography =
            crate::text_measure::MeasureTypography::new(crate::enz_text::TextSize::Body)
                .pin_size_px(text_size)
                .pin_line_height_px(line_height)
                .font_family(committed.family)
                .weight(committed.weight)
                .italic(matches!(committed.style, iced::font::Style::Italic))
                .shaping(iced::advanced::text::Shaping::Advanced)
                .wrapping(self.wrap.to_iced());

        let generation = crate::revision_text::font_generation();
        let mut cache = self.row_count_cache.borrow_mut();
        let cache = cache.get_or_insert_with(|| RowCountCache {
            text: self.content.text(),
            last_measurement: None,
        });

        if let Some((cached_generation, cached_width, cached_typography, rows)) =
            cache.last_measurement
            && cached_generation == generation
            && cached_width == width
            && cached_typography == typography
        {
            return rows;
        }

        let rows =
            crate::text_measure::enz_text_measure_with(&cache.text, width, typography).line_count;
        // Lazy font loading can change generation during shaping; cache only a stable generation.
        let generation_after_shape = crate::revision_text::font_generation();
        cache.last_measurement =
            (generation == generation_after_shape).then_some((generation, width, typography, rows));
        rows
    }
    fn accessible_name_key(&self) -> Option<String> {
        if !self.label.is_empty() {
            Some(self.label.to_string())
        } else if let Some(aria) = self.aria_label.as_deref()
            && !aria.is_empty()
        {
            Some(aria.to_owned())
        } else if !self.placeholder.is_empty() {
            Some(self.placeholder.to_string())
        } else {
            None
        }
    }
    fn effective_id(&self) -> Option<Id> {
        self.id.clone().or_else(|| {
            self.accessible_name_key()
                .map(|key| enzui_core::identity::derive_id(&self.id_prefix, &key))
        })
    }
    fn semantics(&self) -> Semantics {
        let mut s = Semantics::new(Role::TextInput)
            .invalid(self.error_text.is_some())
            .disabled(!self.enabled)
            .read_only(self.read_only)
            .editable(true);
        if let Some(name) = self.accessible_name_key() {
            s = s.label(name);
        }
        let value = semantics_value(self.content, SEMANTICS_VALUE_BUDGET);
        if !value.trim().is_empty() {
            s = s.value(value);
        }
        if let Some(err) = self.error_text.as_deref() {
            s = s.description(err.to_owned());
        } else if let Some(desc) = self.description.as_deref() {
            s = s.description(desc.to_owned());
        }
        s
    }
    /// Build chrome, editor or disabled display, bounded semantics, slots and keyed optional rows.
    pub fn into_element(mut self) -> Element<'a, Message> {
        let semantics = self.semantics();
        let on_set_value = self.on_set_value.take();
        let replace_actions = Rc::clone(&self.on_action);
        let leading = self.leading.take();
        let trailing = self.trailing.take();
        let user_id = self.effective_id();
        // Anonymous editors still need an internal focus ID, which is not published for routing.
        let probe_id = user_id
            .clone()
            .unwrap_or_else(iced::advanced::widget::Id::unique);
        let pin = self.palette.clone();
        let tok_override = self.tokens;
        let plain = matches!(self.variant, TextAreaVariant::Plain);
        let fill_variant = self.fill_variant;
        let enabled = self.enabled;
        let read_only = self.read_only;
        let has_error = self.error_text.is_some();
        let size = self.size;
        let radius_pin = self.radius;
        let text_size = self.text_size();
        fn resolve(
            theme: &iced::Theme,
            tok_override: Option<TextAreaTokens>,
            pin: Option<Palette>,
            fill_variant: crate::tokens::FieldVariant,
        ) -> TextAreaTokens {
            if let Some(t) = tok_override {
                t
            } else {
                let p = pin.unwrap_or_else(|| crate::theme_lookup::palette(theme));
                let mut t = TextAreaTokens::from_palette(&p);
                t.background = crate::tokens::field_fill_for(&p, fill_variant);
                t
            }
        }

        let geo = tok_override
            .unwrap_or_else(|| {
                TextAreaTokens::from_palette(&pin.clone().unwrap_or_else(Palette::dark))
            })
            .normalized();
        let policy = self
            .area_height
            .unwrap_or(AreaHeight::Fixed(geo.area_height));
        let line_height_override = self.line_height_override;
        let rich_formatting = self.rich_formatting.take();
        let line_height_factor = match line_height_override {
            Some(iced::advanced::text::LineHeight::Relative(f)) => f,
            Some(iced::advanced::text::LineHeight::Absolute(px)) => px.0 / text_size,
            None => 1.3,
        };
        let row_px = text_size * line_height_factor;
        let (chrome_height, editor_height): (Length, Length) = match policy {
            AreaHeight::Fill => (Length::Fill, Length::Fill),
            AreaHeight::Fixed(h) => (
                Length::Fixed(round_height_to_rows(
                    sanitize_height(h, geo.area_height),
                    row_px,
                    geo.padding,
                )),
                Length::Fill,
            ),
            AreaHeight::Rows(n) => (
                Length::Fixed((n.max(1) as f32) * row_px + geo.padding * 2.0),
                Length::Fill,
            ),
            AreaHeight::AutoGrow { min_rows, max_rows } => {
                let min_rows = min_rows.max(1);
                let max_rows = max_rows.max(min_rows);
                let pad_v = geo.padding * 2.0;
                let min_px = (min_rows as f32) * row_px + pad_v;
                let max_px = (max_rows as f32) * row_px + pad_v;
                (Length::Shrink.min(min_px).max(max_px), Length::Shrink)
            }
        };
        let field_child: Element<'a, Message> = if enabled {
            let committed = self.committed_font();

            let mut editor = iced::widget::text_editor(self.content)
                .placeholder(self.placeholder.clone())
                .size(text_size)
                .font(committed)
                .padding(Padding::ZERO)
                .height(editor_height)
                .wrapping(self.wrap.to_iced());
            if let Some(lh) = line_height_override {
                editor = editor.line_height(lh);
            }
            editor = editor.id(probe_id.clone());
            {
                let pin_e = pin.clone();
                editor = editor.style(move |theme: &iced::Theme, status: EditorStatus| {
                    let t = resolve(theme, tok_override, pin_e.clone(), fill_variant);
                    let disabled = matches!(status, EditorStatus::Disabled);
                    EditorStyle {
                        background: Background::Color(Color::TRANSPARENT),
                        border: Border {
                            color: Color::TRANSPARENT,
                            width: 0.0,
                            radius: 0.0.into(),
                        },
                        placeholder: if disabled { t.disabled_text } else { t.muted },
                        value: dim_if(t.text, disabled, theme),
                        selection: t.selection,
                    }
                });
            }

            let on_action = self.on_action;
            if read_only {
                let editor_on_action = Rc::clone(&on_action);
                // IME and clipboard edits bypass key bindings, so refuse mutations at the callback boundary.
                editor = editor.on_action(move |action| {
                    if action.is_edit() {
                        // Preserve buffer, cursor and selection while returning the required host message.
                        editor_on_action(Action::Scroll { lines: 0 })
                    } else {
                        editor_on_action(action)
                    }
                });
                let host = self.key_binding.clone();
                editor = editor.key_binding(move |kp: KeyPress| {
                    host.as_ref()
                        .and_then(|host| host(kp.clone()))
                        .or_else(|| read_only_key_binding(kp))
                });
            } else {
                let editor_on_action = Rc::clone(&on_action);
                editor = editor.on_action(move |action| editor_on_action(action));
                let submit = self.on_submit.map(|message| (message, self.submit_key));
                let paste = self.on_paste;
                let host = self.key_binding.clone();
                editor = editor.key_binding(move |kp: KeyPress| {
                    if let Some(binding) = host.as_ref().and_then(|host| host(kp.clone())) {
                        return Some(binding);
                    }
                    editable_key_binding(
                        kp,
                        submit.as_ref().map(|(message, key)| (message, *key)),
                        paste.as_ref(),
                    )
                });
            }
            if let Some(on_drag_update) = self.on_drag_update.take() {
                editor = editor.on_drag_update(on_drag_update);
            }
            if let Some(gutter) = self.gutter.take() {
                editor = editor.gutter(gutter);
            }
            if !self.background_highlights.is_empty() {
                editor = editor.background_highlights(self.background_highlights);
            }
            if self.retain_selection {
                editor = editor.retain_selection(true);
            }
            let floats = std::mem::take(&mut self.floats);
            if !floats.is_empty() {
                editor =
                    editor.exclusions(floats.iter().map(crate::text_float::TextFloat::exclusion));
            }
            let editor: Element<'a, Message> = match (rich_formatting, self.syntax_installer.take())
            {
                (Some(settings), _) => {
                    let alignment = settings.clone();
                    editor
                        .line_alignment(move |index| alignment.line_alignment(index))
                        .highlight_with::<crate::rich_edit::RichEditHighlighter>(
                            settings.with_base_font(committed),
                            crate::rich_edit::format_for_rich_edit,
                        )
                        .into()
                }
                (None, Some(install)) => install(editor),
                (None, None) => match self.syntax {
                    Some(language) => editor
                        .highlight_with::<crate::syntax::SyntaxHighlighter>(
                            language,
                            crate::syntax::format_for_class,
                        )
                        .into(),
                    None => editor.into(),
                },
            };
            let editor = crate::text_float::text_floats(editor, self.content, floats);
            let spell_check = self.spell_check.take().filter(|_| !read_only);
            let editor = match &spell_check {
                Some(check) => {
                    crate::spell_underline::spell_underlines(editor, self.content, check.clone())
                }
                None => editor,
            };
            let editor = if self.edit_menu {
                crate::edit_menu::edit_menu_with_spelling(
                    editor,
                    crate::edit_menu::EditMenuOptions {
                        editable: !read_only,
                        secure: false,
                    },
                    spell_check.map(|check| crate::edit_menu::SpellMenu {
                        content: self.content,
                        check,
                    }),
                )
            } else {
                editor
            };

            crate::editor_scrollbar::EnzEditorScrollbar::with_shared_action(
                editor,
                self.content,
                on_action,
                matches!(self.wrap, WrapMode::None),
            )
            .into()
        } else {
            let shown = {
                let text_val = self.content.text();
                if text_val.trim().is_empty() {
                    self.placeholder.to_string()
                } else {
                    text_val
                }
            };
            let is_placeholder = self.content.text().trim().is_empty();
            let pin_d = pin.clone();
            iced::widget::container(
                text(shown)
                    .size(text_size)
                    .font(self.committed_font())
                    .width(Length::Fill)
                    .style(move |theme: &iced::Theme| {
                        let t = resolve(theme, tok_override, pin_d.clone(), fill_variant);
                        iced::widget::text::Style {
                            color: Some(if is_placeholder {
                                t.disabled_text
                            } else {
                                dim_if(t.text, true, theme)
                            }),
                        }
                    }),
            )
            .width(Length::Fill)
            .height(editor_height)
            .clip(true)
            .into()
        };
        let field_child: Element<'a, Message> = if leading.is_some() || trailing.is_some() {
            use iced::alignment::Vertical;
            let slot_height = editor_height;
            let mut slot_row = iced::widget::Row::new()
                .spacing(crate::text_field::SLOT_SPACING)
                .width(Length::Fill)
                .height(slot_height);
            // Shrink slots must not request Fill and force every growing field to its row cap.
            let is_auto_grow = !matches!(slot_height, Length::Fill);
            if is_auto_grow {
                slot_row = slot_row.align_y(Vertical::Bottom);
            }
            if let Some(leading) = leading {
                slot_row = slot_row.push(
                    iced::widget::container(leading)
                        .height(slot_height)
                        .align_y(Vertical::Top),
                );
            }
            slot_row = slot_row.push(field_child);
            if let Some(trailing) = trailing {
                slot_row = slot_row.push(
                    iced::widget::container(trailing)
                        .height(slot_height)
                        .align_y(Vertical::Bottom),
                );
            }
            slot_row.into()
        } else {
            field_child
        };
        let pin_f = pin.clone();
        let fallback_fixed = match chrome_height {
            Length::Fixed(px) => px,
            _ => geo.area_height,
        };
        let editor_wrap: Element<'a, Message> = crate::field_chrome::FieldChrome::new(
            field_child,
            fallback_fixed,
            Padding::from(geo.padding),
            move |theme: &iced::Theme, status: crate::field_chrome::ChromeStatus| {
                use crate::field_chrome::{ChromePaint, ChromeStatus};
                let t = resolve(theme, tok_override, pin_f.clone(), fill_variant);
                // Validation outranks Plain and every interaction state.
                let (border_col, border_width) = if has_error {
                    (
                        t.error_border,
                        if matches!(status, ChromeStatus::Focused) {
                            2.0
                        } else {
                            1.0
                        },
                    )
                } else if plain {
                    (Color::TRANSPARENT, 0.0)
                } else {
                    match status {
                        ChromeStatus::Focused => (t.focus_border, 2.0),
                        ChromeStatus::Hovered => (t.hover_border, 1.0),
                        ChromeStatus::Active => (t.border, 1.0),
                        ChromeStatus::Inert => (dim_if(t.border, true, theme), 1.0),
                    }
                };
                let bg = if plain {
                    Color::TRANSPARENT
                } else if matches!(status, ChromeStatus::Inert) {
                    t.disabled_background
                } else {
                    t.background
                };
                ChromePaint {
                    background: bg,
                    border_color: border_col,
                    border_width,
                    radius: if plain {
                        0.0
                    } else {
                        size.resolve_radius(theme, radius_pin)
                    },
                }
            },
        )
        .height_length(chrome_height)
        .interactive(enabled)
        .into();
        let pin_l = pin.clone();
        let label_widget: Option<Element<'a, Message>> = if self.label.is_empty() {
            None
        } else {
            let label_owned = self.label.to_string();
            let label_size = geo.label_size;
            Some(
                text(label_owned)
                    .size(label_size)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some({
                            let t = resolve(theme, tok_override, pin_l.clone(), fill_variant);
                            if enabled { t.muted } else { t.disabled_text }
                        }),
                    })
                    .into(),
            )
        };
        let label_size = geo.label_size;
        let mut foot_rows: Vec<(&'static str, Element<'a, Message>)> = Vec::new();

        if let Some(e) = self.error_text {
            let pin_h = pin.clone();
            let e_owned = e.into_owned();
            foot_rows.push((
                "helper",
                text(e_owned)
                    .size(label_size)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some(
                            resolve(theme, tok_override, pin_h.clone(), fill_variant).error_border,
                        ),
                    })
                    .into(),
            ));
        } else if let Some(description) = self.description {
            let pin_h = pin.clone();
            let d_owned = description.into_owned();
            foot_rows.push((
                "helper",
                text(d_owned)
                    .size(label_size)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some({
                            let t = resolve(theme, tok_override, pin_h.clone(), fill_variant);
                            if enabled { t.muted } else { t.disabled_text }
                        }),
                    })
                    .into(),
            ));
        }
        // Keep full-buffer materialization inside the opt-in counter path.
        if self.show_count {
            let count = scan_char_count(self.content);
            let counter = counter_text(count, self.max_length);
            let over_limit = is_over_limit(count, self.max_length);
            let pin_c = pin;
            let counter_row = row![
                iced::widget::Space::new().width(Length::Fill),
                text(counter)
                    .size(label_size)
                    .style(move |theme: &iced::Theme| iced::widget::text::Style {
                        color: Some({
                            let t = resolve(theme, tok_override, pin_c.clone(), fill_variant);
                            if over_limit {
                                t.error_border
                            } else if enabled {
                                t.muted
                            } else {
                                t.disabled_text
                            }
                        }),
                    }),
            ];
            foot_rows.push(("counter", counter_row.into()));
        }
        let mut wrapper = crate::text_field_semantics::TextFieldSemantics::new(
            editor_wrap,
            probe_id,
            user_id,
            semantics,
        );
        wrapper = match on_set_value {
            Some(on_set_value) => wrapper.on_input(on_set_value),
            None => wrapper.on_input_messages(move |text| {
                vec![
                    replace_actions(Action::SelectAll),
                    replace_actions(Action::Edit(iced::widget::text_editor::Edit::Paste(
                        std::sync::Arc::new(text),
                    ))),
                ]
            }),
        };
        let editor_node: Element<'a, Message> = wrapper.into();
        let mut col = crate::field_column::FieldColumn::new(4.0)
            .slot("label", label_widget)
            .field("field", editor_node);
        for (key, r) in foot_rows {
            col = col.slot(key, Some(r));
        }
        let col = if let Some(w) = self.min_width {
            col.width(Length::Fill.min(w))
        } else {
            col.width(Length::Fill)
        };
        col.into_element()
    }
}

impl<'a, Message: Clone + 'a> EnzTextArea<'a, Message> {
    /// Build this area inside a host-managed context menu.
    /// Disabled areas disable the host. A derived or explicit effective ID supplies
    /// focus return; an anonymous field supplies none. Closed menus leave editor keys alone.
    #[must_use]
    pub fn into_context_host_with(
        self,
        model: &'a enzui_core::menu_model::MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        self.into_context_menu_host(model, state, on_event)
            .into_element()
    }
    /// Create the context-menu builder for further host customization.
    /// Uses the field’s effective ID for focus return and visible label for naming;
    /// mirrors the field’s enabled state. Menu model and open state remain host-owned.
    pub fn into_context_menu_host(
        self,
        model: &'a enzui_core::menu_model::MenuModel,
        state: &'a crate::context_menu_host::ContextMenuState,
        on_event: impl Fn(crate::context_menu_host::ContextMenuEvent) -> Message + 'a,
    ) -> crate::context_menu_host::EnzContextMenuHost<'a, Message> {
        let invoker = self.effective_id();
        let enabled = self.enabled;
        let label = (!self.label.is_empty()).then(|| self.label.to_string());

        let mut host = crate::context_menu_host::enz_context_menu_host(
            self.edit_menu(false).into_element(),
            model,
            state,
            on_event,
        )
        .enabled(enabled);
        if let Some(invoker) = invoker {
            host = host.invoker(invoker);
        }
        if let Some(label) = label {
            host = host.label(label);
        }
        host
    }
}

impl<'a, Message: Clone + 'a> From<EnzTextArea<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzTextArea<'a, Message>) -> Self {
        w.into_element()
    }
}

fn editable_key_binding<Message: Clone>(
    kp: KeyPress,
    submit: Option<(&Message, SubmitKey)>,
    paste: Option<&Message>,
) -> Option<Binding<Message>> {
    // Native Enter bindings otherwise insert a newline for the host’s command-Enter chord.
    if submit.is_none()
        && kp.modifiers.command()
        && matches!(
            kp.key.as_ref(),
            iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
        )
    {
        return None;
    }
    let binding = match submit {
        Some((submit, key)) => submit_key_binding(kp, submit, key),
        None => Binding::from_key_press(kp),
    }?;
    Some(match (binding, paste) {
        (Binding::Paste, Some(message)) => {
            Binding::Sequence(vec![Binding::Custom(message.clone()), Binding::Paste])
        }
        (binding, _) => binding,
    })
}
fn submit_key_binding<Message: Clone>(
    kp: KeyPress,
    submit: &Message,
    key: SubmitKey,
) -> Option<Binding<Message>> {
    use iced::keyboard::{Key, key::Named};
    let enter = matches!(kp.key.as_ref(), Key::Named(Named::Enter));
    let submits = match key {
        SubmitKey::Enter => enter && !kp.modifiers.shift(),
        SubmitKey::CommandEnter => enter && kp.modifiers.command(),
    };
    if submits {
        Some(Binding::Custom(submit.clone()))
    } else {
        Binding::from_key_press(kp)
    }
}
/// Submit-key policies for an editable text area with a callback.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubmitKey {
    /// Submit on Enter without Shift, including other modifiers; Shift+Enter inserts a newline.
    #[default]
    Enter,
    /// Submit on command-Enter, including Shift; bare Enter inserts a newline.
    CommandEnter,
}
fn read_only_key_binding<Message>(kp: KeyPress) -> Option<Binding<Message>> {
    match Binding::from_key_press(kp)? {
        b @ (Binding::Move(_)
        | Binding::Select(_)
        | Binding::SelectWord
        | Binding::SelectLine
        | Binding::SelectAll
        | Binding::Copy
        | Binding::Unfocus) => Some(b),
        // An empty sequence captures mutating keys without issuing an edit.
        _ => Some(Binding::Sequence(Vec::new())),
    }
}
