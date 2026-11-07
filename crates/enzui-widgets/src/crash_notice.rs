//! The notice at start-up that the app crashed last time.
//!
//! [`CrashReporter`](enzui_core::crash_report::CrashReporter) writes a report
//! when the app panics, but nobody reads a file they do not know about. At the
//! next start, show [`enz_crash_notice`] for the newest of
//! [`unseen_reports`](enzui_core::crash_report::unseen_reports): a warning
//! banner naming the app, with "Show Report" (open the report's
//! [`path`](enzui_core::crash_report::SavedReport::path), for example through
//! the OpenURI portal) and a dismiss button. Call
//! [`mark_seen`](enzui_core::crash_report::SavedReport::mark_seen) on either,
//! so the notice is shown once.

use iced::Element;

use crate::banner::{BannerTone, enz_banner};

/// A banner telling the user that `app_name` crashed last time and a report
/// was saved; `on_show` opens it, `on_dismiss` closes the notice.
pub fn enz_crash_notice<'a, Message: Clone + 'a>(
    app_name: &str,
    on_show: Message,
    on_dismiss: Message,
) -> Element<'a, Message> {
    enz_banner(enzui_core::localized!(
        "enzui-crash-notice-title",
        "name" => app_name.to_owned()
    ))
    .body(enzui_core::localized!("enzui-crash-notice-body"))
    .tone(BannerTone::Warning)
    .action_text(enzui_core::localized!("enzui-crash-notice-show"))
    .on_action(on_show)
    .on_dismiss(on_dismiss)
    .into()
}
