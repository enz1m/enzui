//! Padded sidebar column with separate row and section spacing.
//!
//! Defaults to Fill width and height, 8 px side/bottom padding, 2 px top
//! padding, 4 px row gaps and 20 px section gaps on the stock spacing scale.
//! Section captions use the shipped heading band and a 12 px horizontal inset.
//!
//! Token dimensions resolve against [`EnzSidebarContent::spacing_scale`] when
//! the element is built, falling back to the stock scale. Each dimension's
//! last token or `_px` setter wins; explicit pixels ignore the scale.
//! Negative or non-finite resolved dimensions become zero. The default
//! caption inset is explicit pixels and does not scale with the ladder.
//!
//! Later section captions have a gap of `max(row_gap, section_gap)` before
//! their band. A caption pushed first has no extra gap. Earlier void children
//! still count for this decision even though the column later omits them.
//! [`crate::sidebar_row`] owns its own painted gutter; column padding is
//! independent of that gutter. Custom row tokens may require a caption inset.

use enzui_theme::{SpaceToken, SpacingScale};
use iced::{Alignment, Element, Length, Padding};
#[derive(Debug, Clone, Copy, PartialEq)]
enum Dim {
    Token(SpaceToken),
    Px(f32),
}

impl Dim {
    fn resolve(self, scale: &SpacingScale) -> f32 {
        let v = match self {
            Dim::Token(t) => t.resolve(scale),
            Dim::Px(p) => p,
        };
        if v.is_finite() && v >= 0.0 { v } else { 0.0 }
    }
}

/// Stock section-gap token: [`SpaceToken::Space9`], 20 logical pixels on the default scale.
pub const DEFAULT_SECTION_GAP: SpaceToken = SpaceToken::Space9;
enum Child<'a, Message> {
    Item(Element<'a, Message>),
    Section(Element<'a, Message>),
}

/// Sidebar-content builder.
#[must_use]
pub struct EnzSidebarContent<'a, Message> {
    padding_left: Dim,
    padding_right: Dim,
    padding_top: Dim,
    padding_bottom: Dim,
    spacing: Dim,
    section_gap: Dim,
    section_inset_x: Dim,
    width: Length,
    height: Length,
    spacing_scale: Option<SpacingScale>,
    children: Vec<Child<'a, Message>>,
}

impl<'a, Message: 'a> EnzSidebarContent<'a, Message> {
    /// Override left padding with a semantic [`SpaceToken`] (default `Sm` = 8).
    pub fn padding_left(mut self, t: SpaceToken) -> Self {
        self.padding_left = Dim::Token(t);
        self
    }
    /// Override right padding with a semantic [`SpaceToken`] (default `Sm` = 8).
    pub fn padding_right(mut self, t: SpaceToken) -> Self {
        self.padding_right = Dim::Token(t);
        self
    }
    /// Override top padding with a semantic [`SpaceToken`] (default `Xxs` = 2).
    pub fn padding_top(mut self, t: SpaceToken) -> Self {
        self.padding_top = Dim::Token(t);
        self
    }
    /// Override bottom padding with a semantic [`SpaceToken`] (default `Sm` = 8).
    pub fn padding_bottom(mut self, t: SpaceToken) -> Self {
        self.padding_bottom = Dim::Token(t);
        self
    }
    /// Set the row-gap token; defaults to `Xs` (4 px on the stock scale).
    /// Later section gaps cannot be smaller than the resolved row gap.
    pub fn spacing(mut self, t: SpaceToken) -> Self {
        self.spacing = Dim::Token(t);
        self
    }

    /// Set the section-gap token; defaults to [`DEFAULT_SECTION_GAP`] (20 px stock).
    /// The gap before a later caption is the maximum of this value and the row gap.
    pub fn section_gap(mut self, t: SpaceToken) -> Self {
        self.section_gap = Dim::Token(t);
        self
    }

    /// Set a token for the caption's horizontal inset.
    /// The default is an explicit 12 px row-label inset and does not scale.
    pub fn section_inset_x(mut self, t: SpaceToken) -> Self {
        self.section_inset_x = Dim::Token(t);
        self
    }

    /// Override left padding with an explicit pixel value. Non-finite/negative
    /// input clamps to `0`.
    pub fn padding_left_px(mut self, v: f32) -> Self {
        self.padding_left = Dim::Px(v);
        self
    }
    /// Override right padding with an explicit pixel value.
    pub fn padding_right_px(mut self, v: f32) -> Self {
        self.padding_right = Dim::Px(v);
        self
    }
    /// Override top padding with an explicit pixel value.
    pub fn padding_top_px(mut self, v: f32) -> Self {
        self.padding_top = Dim::Px(v);
        self
    }
    /// Override bottom padding with an explicit pixel value.
    pub fn padding_bottom_px(mut self, v: f32) -> Self {
        self.padding_bottom = Dim::Px(v);
        self
    }
    /// Override inter-item (row) spacing with an explicit pixel value.
    pub fn spacing_px(mut self, v: f32) -> Self {
        self.spacing = Dim::Px(v);
        self
    }
    /// Override the section gap with an explicit pixel value.
    pub fn section_gap_px(mut self, v: f32) -> Self {
        self.section_gap = Dim::Px(v);
        self
    }
    /// Override the section caption's horizontal inset with an explicit pixel
    /// value.
    pub fn section_inset_x_px(mut self, v: f32) -> Self {
        self.section_inset_x = Dim::Px(v);
        self
    }

    /// Resolve token dimensions against this scale when building the element.
    /// Without a forwarded scale, the stock ladder applies. Explicit pixel dimensions
    /// ignore the scale; a later token setter replaces an earlier pixel setter.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }

    /// Override the outer width allocation. The default is [`Length::Fill`]
    /// to match a sidebar pane that owns its own width. Independent of any
    /// Fill child.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }
    /// Override the outer height allocation. The default is [`Length::Fill`].
    /// Independent of any Fill child.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Append a child row.
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(Child::Item(child.into()));
        self
    }

    /// Append a muted, all-caps heading using [`crate::section_header`].
    /// The caption receives the configured horizontal inset. A caption pushed first
    /// has no extra top gap; later captions use the maximum of row and section gaps.
    /// Void children pushed earlier still count when deciding whether it is first.
    pub fn push_section(self, title: impl Into<String>) -> Self
    where
        Message: 'a,
    {
        let header = crate::section_header::enz_section_header::<Message>(title).into_element();
        self.push_section_element(header)
    }

    /// Append a caller-owned caption with the same inset and gap rules as [`Self::push_section`].
    /// The supplied element owns its heading semantics and style.
    pub fn push_section_element(mut self, header: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(Child::Section(header.into()));
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let default = SpacingScale::default();
        let scale = self.spacing_scale.as_ref().unwrap_or(&default);
        let row_gap = self.spacing.resolve(scale);
        let section_gap = self.section_gap.resolve(scale);
        let section_extra = (section_gap - row_gap).max(0.0);
        let section_inset_x = self.section_inset_x.resolve(scale);
        let children: Vec<Element<'a, Message>> = self
            .children
            .into_iter()
            .enumerate()
            .map(|(i, child)| match child {
                Child::Item(el) => el,
                Child::Section(el) => iced::widget::container(el)
                    .width(Length::Fill)
                    .padding(Padding {
                        top: if i == 0 { 0.0 } else { section_extra },
                        right: section_inset_x,
                        bottom: 0.0,
                        left: section_inset_x,
                    })
                    .into(),
            })
            .collect();
        let col = crate::stack::collapsing_column(
            children,
            row_gap,
            Padding::ZERO,
            self.width,
            self.height,
            Alignment::Start,
        );
        iced::widget::container(col)
            .width(self.width)
            .height(self.height)
            .padding(Padding {
                top: self.padding_top.resolve(scale),
                right: self.padding_right.resolve(scale),
                bottom: self.padding_bottom.resolve(scale),
                left: self.padding_left.resolve(scale),
            })
            .into()
    }
}

impl<'a, Message: 'a> From<EnzSidebarContent<'a, Message>> for Element<'a, Message> {
    fn from(s: EnzSidebarContent<'a, Message>) -> Self {
        s.into_element()
    }
}

/// Construct a sidebar-content builder with the semantic defaults
/// (`Sm`/`Sm`/`Xxs`/`Sm` padding, `Xs` spacing; Fill × Fill allocation).
pub fn enz_sidebar_content<'a, Message: 'a>() -> EnzSidebarContent<'a, Message> {
    EnzSidebarContent {
        padding_left: Dim::Token(SpaceToken::Sm),
        padding_right: Dim::Token(SpaceToken::Sm),
        padding_top: Dim::Token(SpaceToken::Xxs),
        padding_bottom: Dim::Token(SpaceToken::Sm),
        spacing: Dim::Token(SpaceToken::Xs),
        section_gap: Dim::Token(DEFAULT_SECTION_GAP),
        section_inset_x: Dim::Px(crate::sidebar_row::default_label_inset_x()),
        width: Length::Fill,
        height: Length::Fill,
        spacing_scale: None,
        children: Vec::new(),
    }
}
