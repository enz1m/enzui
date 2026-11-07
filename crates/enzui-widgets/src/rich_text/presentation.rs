//! Theme-relative link ink and underline policy for immutable rich text.

use super::tone::{ThemedTokens, tone_key};
use crate::text::TextTone;
use iced::widget::text::Rich;
use std::cell::RefCell;
use std::rc::Rc;

/// Decoration applied to links independently of their semantic role and focus ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkUnderline {
    /// Keep links underlined at rest and during interaction.
    Always,
    /// Underline hovered or focused links.
    HoverFocus,
    /// Add no link underline; explicitly authored text underlines remain visible.
    Never,
}

/// Semantic foreground tones resolved from the active text-token source.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinkPresentationTokens {
    /// Foreground while the link has neither pointer hover nor focus.
    pub rest: TextTone,
    /// Foreground under the pointer when unfocused.
    pub hover: TextTone,
    /// Foreground while focused, taking precedence over hover.
    pub focus: TextTone,
    /// Link decoration policy; keyboard focus rings remain independent.
    pub underline: LinkUnderline,
}

impl LinkPresentationTokens {
    /// Body-coloured resting links with accent ink and underline during interaction.
    pub const QUIET: Self = Self {
        rest: TextTone::Default,
        hover: TextTone::Accent,
        focus: TextTone::Accent,
        underline: LinkUnderline::HoverFocus,
    };
}

impl Default for LinkPresentationTokens {
    fn default() -> Self {
        Self {
            rest: TextTone::Accent,
            hover: TextTone::Accent,
            focus: TextTone::Accent,
            underline: LinkUnderline::Always,
        }
    }
}

pub(super) type LinkFocus = Rc<RefCell<Vec<bool>>>;

pub(super) fn apply_presentation<'a, L: Clone + 'static, Message>(
    rich: Rich<'a, L, Message>,
    presentation: Option<LinkPresentationTokens>,
    authored_colors: Vec<bool>,
    themed: Rc<ThemedTokens<'a>>,
) -> (Rich<'a, L, Message>, Option<LinkFocus>) {
    let Some(tokens) = presentation else {
        return (rich, None);
    };
    let focus = Rc::new(RefCell::new(vec![false; authored_colors.len()]));
    let painted_focus = Rc::clone(&focus);
    let rich = rich.link_style(move |theme, index, hovered| {
        let focused = painted_focus.borrow().get(index).copied().unwrap_or(false);
        let tone = if focused {
            tokens.focus
        } else if hovered {
            tokens.hover
        } else {
            tokens.rest
        };
        iced::widget::text::LinkStyle {
            color: if authored_colors.get(index).copied().unwrap_or(false) {
                None
            } else {
                themed.resolve(theme, tone_key(tone))
            },
            underline: match tokens.underline {
                LinkUnderline::Always => true,
                LinkUnderline::HoverFocus => hovered || focused,
                LinkUnderline::Never => false,
            },
        }
    });
    (rich, Some(focus))
}
