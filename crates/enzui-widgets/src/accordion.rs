//! A stack of sections of which at most one is open (an exclusive
//! accordion, Kirigami's and Bootstrap's accordion): opening one closes the
//! others. Each section is an [`expander`](crate::expander).

use std::rc::Rc;

use iced::Element;
use iced::widget::column;

use crate::expander::enz_expander;

/// The sections, with `open` the one shown; `on_open` receives the section
/// to show next, or `None` when the open one is closed.
pub fn enz_accordion<'a, Message: Clone + 'a>(
    sections: impl IntoIterator<Item = (String, Element<'a, Message>)>,
    open: Option<usize>,
    on_open: impl Fn(Option<usize>) -> Message + 'a,
) -> Element<'a, Message> {
    let on_open = Rc::new(on_open);
    column(
        sections
            .into_iter()
            .enumerate()
            .map(|(index, (title, body))| {
                let on_open = Rc::clone(&on_open);
                enz_expander(title, open == Some(index), move |expanded| {
                    on_open(expanded.then_some(index))
                })
                .content(body)
                .into()
            }),
    )
    .spacing(4)
    .into()
}
