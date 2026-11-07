//! A star rating the user sets: click (or press) the n-th star to give n
//! stars, the current one again to clear it. For showing an aggregate score,
//! use [`rating`](crate::rating).

use iced::widget::{button, row};
use iced::{Element, Length};

use crate::decorative::decorative;
use crate::rating::{DEFAULT_GAP_PX, DEFAULT_STAR_PX, enz_rating};
use crate::semantics_container::SemanticsContainer;
use enzui_core::semantics::{Role, Semantics};

/// What pressing star `star` (1-based) sets when `value` is shown: that many
/// stars, or none when it is the current value.
#[must_use]
pub fn pressed(value: u8, star: u8) -> u8 {
    if star == value { 0 } else { star }
}

/// `max` stars with `value` of them filled; `on_change` receives the new
/// value. Each star is a focusable button named "n of max stars".
pub fn enz_rating_input<'a, Message: Clone + 'a>(
    value: u8,
    max: u8,
    on_change: impl Fn(u8) -> Message + 'a,
) -> Element<'a, Message> {
    let value = value.min(max);
    row((1..=max).map(|star| {
        let glyph = enz_rating(if star <= value { 1.0 } else { 0.0 })
            .max(1)
            .star(DEFAULT_STAR_PX * 1.25)
            .into_element();
        let name = enzui_core::localized!(
            "enzui-rating-input-star",
            "value" => star.to_string(),
            "max" => max.to_string()
        );
        let star_button = button(Element::new(decorative(glyph)))
            .padding(2)
            .style(button::text)
            .on_press(on_change(pressed(value, star)));
        SemanticsContainer::leaf(
            star_button,
            Semantics::new(Role::Button)
                .label_opt(Some(name))
                .selected(star <= value),
        )
        .into()
    }))
    .spacing(DEFAULT_GAP_PX)
    .width(Length::Shrink)
    .into()
}
