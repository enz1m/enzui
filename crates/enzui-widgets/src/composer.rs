//! Compose a multiline edit buffer, optional status strip and action rail.
//!
//! The caller owns the editor Content and handles edit messages. The input
//! grows by logical line count, clamped to 1–6 rows by default; wrapped lines
//! do not increase that estimate. Supply an explicit row count for measured
//! wrapping. Height reserves each scaled text line plus vertical padding once.
//!
//! Idle Send requires a callback and non-whitespace buffer text. Busy Stop
//! requires a stop callback and can act on an empty buffer. If either callback
//! exists, an unwired current mode still paints a disabled control; with both
//! callbacks absent, the control is omitted. The send/stop id remains stable
//! across mode changes when the input has an explicit or name-derived id.
//!
//! An idle send callback also wires the submission chord (Enter by default),
//! including for empty buffers. Busy inputs have no submission callback.
//! [`EnzComposer::trailing_in_field`] overlays a 40-pixel cell at the field's
//! trailing edge; it does not reserve editor text width, so text can run under
//! it. Rail leading/trailing items and the status strip are caller-supplied.

use enzui_theme::Palette;
use iced::advanced::widget::Id;
use iced::widget::text_editor::Content;
use iced::widget::{column, container, row};
use iced::{Element, Length};
use std::borrow::Cow;

use crate::button::{ButtonIconPosition, ButtonKind, enz_button};
use crate::text_area::{FieldSize, TextAreaTokens, enz_text_area};
use crate::tokens::ButtonSizeTokens;

/// Default text-editor line-height ratio used by composer height calculations.
pub const LINE_HEIGHT: f32 = 1.3;

// Height and painted text must use the same ambient text scale.
fn field_text_size(size: FieldSize) -> f32 {
    crate::text_area::EnzTextArea::<'static, ()>::text_size_for(size)
}

/// Reserve one text row plus vertical padding, in logical pixels.
///
/// Uses `ceil(scaled_text_size * LINE_HEIGHT) + 2 * padding`.
/// Padding is used directly without validation.
#[must_use]
pub fn composer_row_height(size: FieldSize, padding: f32) -> f32 {
    (field_text_size(size) * LINE_HEIGHT).ceil() + 2.0 * padding
}

/// Reserve `rows` text line boxes plus vertical padding once, in logical pixels.
///
/// Zero rows become one. Uses the ambient text scale; padding is used directly
/// without validation.
#[must_use]
pub fn composer_height_for_rows(rows: usize, size: FieldSize, padding: f32) -> f32 {
    let rows = rows.max(1) as f32;
    rows * (field_text_size(size) * LINE_HEIGHT).ceil() + 2.0 * padding
}

/// The default id-derivation prefix for [`EnzComposer`]'s input.
///
/// The send/stop control receives a stable child identity derived from that
/// input identity, so both nodes remain routable through `NodeIdMap`.
pub const DEFAULT_ID_PREFIX: &str = "enz-composer";

fn action_id(input_id: &Id) -> Id {
    enzui_core::identity::derive_id(&format!("{input_id:?}"), "send-stop")
}

const SEND_BUTTON_DIAMETER: f32 = 34.0;

const SEND_BUTTON_ICON: f32 = 18.0;

/// Builder for a caller-owned chat or prompt edit buffer and action slots.
#[must_use]
pub struct EnzComposer<'a, Message> {
    content: &'a Content,
    on_action: Box<dyn Fn(iced::widget::text_editor::Action) -> Message + 'a>,
    on_send: Option<Message>,
    submit_key: crate::text_area::SubmitKey,
    on_stop: Option<Message>,
    placeholder: Cow<'a, str>,
    size: FieldSize,
    tokens: Option<TextAreaTokens>,
    palette: Option<Palette>,
    min_rows: usize,
    max_rows: usize,
    rows_override: Option<usize>,
    busy: bool,
    trailing_in_field: Option<Element<'a, Message>>,
    rail_leading: Vec<Element<'a, Message>>,
    rail_trailing: Vec<Element<'a, Message>>,
    status: Option<Element<'a, Message>>,
    id: Option<Id>,
    aria_label: Option<String>,
}

/// Build a composer over caller-owned Content, forwarding editor actions.
///
/// Defaults are Md field size, 1–6 logical rows and Enter submission when an
/// idle send callback is supplied. With both send and stop callbacks absent,
/// the rail contains no send/stop control.
pub fn enz_composer<'a, Message: Clone + 'a>(
    content: &'a Content,
    on_action: impl Fn(iced::widget::text_editor::Action) -> Message + 'a,
) -> EnzComposer<'a, Message> {
    EnzComposer {
        content,
        on_action: Box::new(on_action),
        on_send: None,
        submit_key: crate::text_area::SubmitKey::Enter,
        on_stop: None,
        placeholder: Cow::Borrowed(""),
        size: FieldSize::default(),
        tokens: None,
        palette: None,
        min_rows: 1,
        max_rows: 6,
        rows_override: None,
        busy: false,
        trailing_in_field: None,
        rail_leading: Vec::new(),
        rail_trailing: Vec::new(),
        status: None,
        id: None,
        aria_label: None,
    }
}

impl<'a, Message: Clone + 'a> EnzComposer<'a, Message> {
    /// Placeholder shown while the buffer is empty.
    pub fn placeholder(mut self, p: impl Into<Cow<'a, str>>) -> Self {
        self.placeholder = p.into();
        self
    }
    /// Field size (drives text size and therefore row height). Default `Md`.
    pub fn size(mut self, s: FieldSize) -> Self {
        self.size = s;
        self
    }
    /// Pin text-area tokens, including colours and padding.
    ///
    /// Reserved height uses the supplied padding directly. Unpinned colours
    /// follow the live theme; default layout padding is derived at build time.
    pub fn tokens(mut self, t: TextAreaTokens) -> Self {
        self.tokens = Some(t);
        self
    }
    /// Pin a palette for text-area token derivation unless explicit tokens override it.
    pub fn palette(mut self, p: Palette) -> Self {
        self.palette = Some(p);
        self
    }
    /// The floor on visible rows the input never shrinks below. Default 1.
    /// Raising the floor also raises a lower configured cap.
    pub fn min_rows(mut self, n: usize) -> Self {
        self.min_rows = n;
        self.normalize_row_bounds();
        self
    }
    /// The cap on visible rows; past it the input scrolls instead of growing.
    /// Default 6. A cap below the configured floor resolves to that floor.
    pub fn max_rows(mut self, n: usize) -> Self {
        self.max_rows = n;
        self.normalize_row_bounds();
        self
    }

    fn normalize_row_bounds(&mut self) {
        self.min_rows = self.min_rows.max(1);
        self.max_rows = self.max_rows.max(self.min_rows);
    }
    /// Override logical-line growth with an explicit visible-row count.
    ///
    /// The count is still clamped to the configured row floor and cap.
    pub fn rows(mut self, n: usize) -> Self {
        self.rows_override = Some(n);
        self
    }
    /// Switch Send to Stop (default `false`).
    ///
    /// Busy inputs do not submit from the keyboard. Stop requires a callback but
    /// not buffer text; an unwired current mode is disabled if the other handler
    /// keeps the control present.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }
    /// Choose the submission chord used while idle with a send callback.
    ///
    /// Defaults to Enter, with Shift+Enter inserting a newline. CommandEnter
    /// leaves Enter for newlines and submits on Ctrl/Cmd+Enter.
    pub fn submit_key(mut self, key: crate::text_area::SubmitKey) -> Self {
        self.submit_key = key;
        self
    }
    /// Publish Send from the idle button or submission chord.
    ///
    /// The button requires non-whitespace text. Keyboard submission does not
    /// check buffer content. Neither sends while busy.
    pub fn on_send(mut self, msg: Message) -> Self {
        self.on_send = Some(msg);
        self
    }
    /// Message emitted when the stop button is pressed (only reachable while
    /// [`busy`](Self::busy)).
    pub fn on_stop(mut self, msg: Message) -> Self {
        self.on_stop = Some(msg);
        self
    }
    /// Overlay an affordance in a 40-pixel cell at the input's trailing edge.
    ///
    /// The cell is vertically centered but does not reserve editor text width;
    /// text may overlap the affordance.
    pub fn trailing_in_field(mut self, el: Element<'a, Message>) -> Self {
        self.trailing_in_field = Some(el);
        self
    }
    /// The leading action-rail items (attachment / image / link).
    pub fn rail_leading(mut self, items: Vec<Element<'a, Message>>) -> Self {
        self.rail_leading = items;
        self
    }
    /// The trailing action-rail items (model / effort / access — app
    /// vocabulary passed straight through).
    pub fn rail_trailing(mut self, items: Vec<Element<'a, Message>>) -> Self {
        self.rail_trailing = items;
        self
    }
    /// Add a status strip above the input, regardless of busy state.
    pub fn status(mut self, el: Element<'a, Message>) -> Self {
        self.status = Some(el);
        self
    }
    /// A stable id for the input (routing / focus) and the namespace from
    /// which the send/stop control's id is derived. When unset, the input id is
    /// derived from the accessible name, then the placeholder.
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }
    /// The accessible name of the input.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    fn effective_id(&self) -> Option<Id> {
        self.id.clone().or_else(|| {
            self.aria_label
                .as_deref()
                .filter(|name| !name.is_empty())
                .or_else(|| (!self.placeholder.is_empty()).then_some(self.placeholder.as_ref()))
                .map(|key| enzui_core::identity::derive_id(DEFAULT_ID_PREFIX, key))
        })
    }

    fn effective_padding(&self) -> f32 {
        if let Some(t) = self.tokens {
            t.padding
        } else {
            TextAreaTokens::from_palette(&self.palette.clone().unwrap_or_else(Palette::dark))
                .padding
        }
    }

    /// The number of visible rows to show, clamped to `[min_rows, max_rows]`.
    ///
    /// Source of the unclamped count: an explicit [`rows`](Self::rows)
    /// override if given, else `Content::line_count()` — the logical-line
    /// estimate. That estimate cannot account for wrapping: a wrapped single
    /// logical line reads as one row here. Supplying a measured wrapped-row
    /// count changes only this source.
    #[must_use]
    pub fn rows_for(&self) -> usize {
        let raw = self
            .rows_override
            .unwrap_or_else(|| self.content.line_count());
        raw.clamp(self.min_rows, self.max_rows)
    }

    /// Read the reserved input height in logical pixels.
    ///
    /// Uses the clamped row count, scaled field text size and configured padding.
    #[must_use]
    pub fn input_height(&self) -> f32 {
        composer_height_for_rows(self.rows_for(), self.size, self.effective_padding())
    }

    fn has_text(&self) -> bool {
        !self.content.text().trim().is_empty()
    }
}

impl<'a, Message: Clone + 'a> EnzComposer<'a, Message> {
    const IN_FIELD_SLOT_WIDTH: f32 = 40.0;

    fn input_element(&mut self) -> Element<'a, Message> {
        let height = self.input_height();
        let content = self.content;
        let on_action = std::mem::replace(&mut self.on_action, Box::new(|_| unreachable!()));
        let mut area = enz_text_area(content, on_action)
            .placeholder(self.placeholder.clone())
            .size(self.size)
            .area_height(height);
        if let Some(t) = self.tokens {
            area = area.tokens(t);
        }
        if let Some(p) = &self.palette {
            area = area.palette(p.clone());
        }

        if !self.busy
            && let Some(send) = self.on_send.clone()
        {
            area = area.on_submit(send).submit_key(self.submit_key);
        }
        if let Some(id) = self.effective_id() {
            area = area.id(id);
        }
        if let Some(name) = &self.aria_label {
            area = area.aria_label(name.clone());
        }
        let area_el: Element<'a, Message> = area.into();

        match self.trailing_in_field.take() {
            None => area_el,
            Some(slot) => {
                let slot_cell = container(slot)
                    .width(Length::Fixed(Self::IN_FIELD_SLOT_WIDTH))
                    .height(Length::Fixed(height))
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Center);
                let trailing_row = row![iced::widget::Space::new().width(Length::Fill), slot_cell,]
                    .width(Length::Fill);
                iced::widget::stack![area_el, trailing_row].into()
            }
        }
    }

    fn send_control(&self) -> Element<'a, Message> {
        let busy = self.busy;

        let mode_msg = if busy {
            self.on_stop.clone()
        } else {
            self.on_send.clone()
        };
        let (glyph, aria) = if busy {
            (
                "square",
                enzui_core::localization::text("enzui-composer-stop"),
            )
        } else {
            (
                "send",
                enzui_core::localization::text("enzui-composer-send"),
            )
        };

        let enabled = mode_msg.is_some() && (busy || self.has_text());

        let Some(press) = mode_msg
            .or_else(|| self.on_send.clone())
            .or_else(|| self.on_stop.clone())
        else {
            return iced::widget::Space::new().width(Length::Fixed(0.0)).into();
        };
        let d = SEND_BUTTON_DIAMETER;
        let icon = SEND_BUTTON_ICON;
        let square = ButtonSizeTokens {
            height: d,
            padding_x: (d - icon) / 2.0,
            radius: d / 2.0,
            icon_size: icon,
            ..ButtonSizeTokens::MD
        };
        let mut button = enz_button("", press)
            .kind(ButtonKind::Primary)
            .icon(glyph)
            .icon_position(ButtonIconPosition::Only)
            .size_tokens(square)
            .aria_label(aria)
            .enabled(enabled);

        // Send and Stop are states of one control, so retain one child identity.
        if let Some(input_id) = self.effective_id() {
            button = button.id(action_id(&input_id));
        }
        button.into()
    }

    /// Assemble the whole composer: [`status`](Self::status) / input / rail.
    pub fn into_element(mut self) -> Element<'a, Message> {
        let gap = 8.0;
        let input = self.input_element();
        let send = self.send_control();

        let mut rail = row![].spacing(gap).align_y(iced::Alignment::Center);
        for item in std::mem::take(&mut self.rail_leading) {
            rail = rail.push(item);
        }
        rail = rail.push(iced::widget::Space::new().width(Length::Fill));
        for item in std::mem::take(&mut self.rail_trailing) {
            rail = rail.push(item);
        }
        rail = rail.push(send);

        let mut col = column![].spacing(gap);
        if let Some(status) = self.status.take() {
            col = col.push(status);
        }
        col = col.push(input);
        col = col.push(rail);
        col.width(Length::Fill).into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzComposer<'a, Message>> for Element<'a, Message> {
    fn from(c: EnzComposer<'a, Message>) -> Self {
        c.into_element()
    }
}
