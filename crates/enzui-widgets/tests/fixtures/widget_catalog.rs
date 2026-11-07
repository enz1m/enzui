#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "documentation fixtures compile as test support and are not public API"
)]

// The catalog generator splices the marked regions verbatim into docs/widgets.md.
// The contracts target compiles these documentation fixtures.

// ---------------------------------------------------------------------------
// Example `shape` - the shape every widget in the catalog shares.
// ---------------------------------------------------------------------------
#[allow(dead_code, reason = "documentation code blocks, compiled but not run")]
mod shape {
    // catalog-example:shape:start
    use enzui_widgets::{enz_badge, enz_button, enz_text};
    use iced::Element;

    #[derive(Debug, Clone)]
    enum Message {
        Save,
    }

    fn view<'a>() -> Element<'a, Message> {
        let row = iced::widget::row![
            enz_text("Unsaved changes").into_element(),
            enz_badge("draft").into_element(),
            enz_button("Save", Message::Save).into_element(),
        ]
        .spacing(8);

        row.into()
    }
    // catalog-example:shape:end
}

// ---------------------------------------------------------------------------
// Example `compose` - reaching for a group's container instead of hand-rolling.
// ---------------------------------------------------------------------------
#[allow(dead_code, reason = "documentation code blocks, compiled but not run")]
mod compose {
    // catalog-example:compose:start
    use enzui_theme::SpaceToken;
    use enzui_widgets::banner::BannerTone;
    use enzui_widgets::status_dot::StatusKind;
    use enzui_widgets::{enz_banner, enz_card, enz_status_dot, enz_text, enz_vstack};
    use iced::Element;

    #[derive(Debug, Clone)]
    enum Message {
        Retry,
    }

    fn view<'a>() -> Element<'a, Message> {
        let banner = enz_banner("Sync failed")
            .body("The last three changes are still local.")
            .tone(BannerTone::Warning)
            .action_text("Retry")
            .on_action(Message::Retry)
            .into_element();

        let status = enz_status_dot(StatusKind::Warning).into_element();

        enz_card(
            enz_vstack(vec![
                banner,
                iced::widget::row![status, enz_text("3 changes pending").into_element()]
                    .spacing(6)
                    .into(),
            ])
            .gap(SpaceToken::Sm)
            .into_element(),
        )
        .title("Status")
        .into_element()
    }
    // catalog-example:compose:end
}
