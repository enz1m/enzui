//! Internal keyed vertical composition for fields with optional labels and captions.
//!
//! Callers must supply unique, position-independent keys and keep the input's
//! key stable across rebuilds. State is reused only for compatible widget tags;
//! this helper does not enforce the number or roles of children.
//!
//! Absent slots are skipped. Width defaults to `Fit`; the keyed column's fit
//! height inherits child constraints during diff, including a fill-height editor.

use iced::{Element, Length};
pub(crate) type FieldKey = &'static str;
pub struct FieldColumn<'a, Message> {
    entries: Vec<(FieldKey, Element<'a, Message>)>,
    spacing: f32,
    width: Length,
}

impl<'a, Message: 'a> FieldColumn<'a, Message> {
    pub fn new(spacing: f32) -> Self {
        Self {
            entries: Vec::new(),
            spacing,
            width: Length::Fit,
        }
    }
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    pub fn slot(mut self, key: FieldKey, child: Option<Element<'a, Message>>) -> Self {
        if let Some(child) = child {
            self.entries.push((key, child));
        }
        self
    }
    pub fn field(self, key: FieldKey, child: Element<'a, Message>) -> Self {
        self.slot(key, Some(child))
    }
    pub fn into_element(self) -> Element<'a, Message> {
        let keyed: Vec<(String, Element<'a, Message>)> = self
            .entries
            .into_iter()
            .map(|(key, child)| ((*key).to_owned(), child))
            .collect();
        crate::keyed_column::keyed_column(keyed)
            .spacing(self.spacing)
            .width(self.width)
            .into()
    }
}

impl<'a, Message: 'a> From<FieldColumn<'a, Message>> for Element<'a, Message> {
    fn from(column: FieldColumn<'a, Message>) -> Self {
        column.into_element()
    }
}
