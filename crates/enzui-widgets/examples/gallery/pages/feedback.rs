use crate::specimen::SIZE_NAMES;
use crate::*;
use enzui_widgets::{
    AvatarShape, BadgeSize, BadgeTone, GaugeStyle, GaugeTone, Presence, ProgressTone, SpinnerTone,
    StatusDotMotion, StatusPageTone, ToastTone,
};
use iced::widget::{column, row};

const TONE_NAMES: &[&str] = &["Accent", "Success", "Warning", "Danger"];
const BANNER_TONES: [BannerTone; 4] = [
    BannerTone::Info,
    BannerTone::Success,
    BannerTone::Warning,
    BannerTone::Danger,
];
const BANNER_TONE_NAMES: &[&str] = &["Info", "Success", "Warning", "Danger"];
const STATUSES: [StatusKind; 6] = [
    StatusKind::Success,
    StatusKind::Info,
    StatusKind::Warning,
    StatusKind::Error,
    StatusKind::Running,
    StatusKind::Offline,
];
const STATUS_NAMES: &[&str] = &["Success", "Info", "Warning", "Error", "Running", "Offline"];
const BADGE_TONES: [BadgeTone; 6] = [
    BadgeTone::Default,
    BadgeTone::Accent,
    BadgeTone::AccentStrong,
    BadgeTone::Success,
    BadgeTone::Warning,
    BadgeTone::Error,
];
const BADGE_TONE_NAMES: &[&str] = &[
    "Default",
    "Accent",
    "AccentStrong",
    "Success",
    "Warning",
    "Error",
];
const BADGE_SIZES: [BadgeSize; 3] = [BadgeSize::Sm, BadgeSize::Md, BadgeSize::Lg];
const SHAPES: [AvatarShape; 3] = [
    AvatarShape::Circle,
    AvatarShape::RoundedSquare,
    AvatarShape::Square,
];
const SHAPE_NAMES: &[&str] = &["Circle", "RoundedSquare", "Square"];
const PRESENCES: [Presence; 5] = [
    Presence::None,
    Presence::Online,
    Presence::Away,
    Presence::Busy,
    Presence::Offline,
];
const PRESENCE_NAMES: &[&str] = &["None", "Online", "Away", "Busy", "Offline"];
const TOAST_TONES: [ToastTone; 5] = [
    ToastTone::Success,
    ToastTone::Info,
    ToastTone::Warning,
    ToastTone::Error,
    ToastTone::Neutral,
];
const TOAST_TONE_NAMES: &[&str] = &["Success", "Info", "Warning", "Error", "Neutral"];

impl Gallery {
    pub(crate) fn feedback_page(&self) -> Element<'_, Message> {
        column![
            self.banner_specimen(),
            self.progress_bar_specimen(),
            self.spinner_specimen(),
            self.gauge_specimen(),
            self.status_dot_specimen(),
            self.badge_specimen(),
            self.avatar_specimen(),
            self.skeleton_specimen(),
            self.specimen(
                "enz_progress_text",
                "A short status line with a working indicator, for work without a known length.",
                Element::from(enzui_widgets::progress_text::enz_progress_text("Indexing files…")),
            )
            .code(
                &code("enzui_widgets::progress_text", "enz_progress_text")
                    .arg(quoted("Indexing files…"))
                    .end(None),
            )
            .into_element(),
            self.specimen(
                "enz_animated_image",
                "An animated GIF, played on its own frame delays. It holds still with reduced motion.",
                enzui_widgets::animated_image::enz_animated_image(&self.spinner)
                    .width(48)
                    .height(48)
                    .alt_text("Syncing"),
            )
            .code(
                &code("enzui_widgets::animated_image", "enz_animated_image")
                    .arg("&self.frames")
                    .set("width", "48")
                    .set("height", "48")
                    .set("alt_text", quoted("Syncing"))
                    .end(None),
            )
            .into_element(),
            self.toast_specimen(),
            self.rating_specimen(),
            self.status_page_specimen(),
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }

    fn banner_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let title = k.text("banner.title", "Sync failed");
        let tone = k.pick("banner.tone", 2);
        let body = k.flag("banner.body", true);
        let action = k.flag("banner.action", true);
        let dismissible = k.flag("banner.dismissible", true);
        let icon = k.flag("banner.icon", true);
        let mut banner = enz_banner(title)
            .tone(BANNER_TONES[tone])
            .show_icon(icon)
            .dismissible(dismissible)
            .on_dismiss(Message::Clicked("dismiss"));
        if body {
            banner = banner.body("The last three changes are still local.");
        }
        if action {
            banner = banner
                .action_text("Retry")
                .on_action(Message::Clicked("retry"));
        }
        self.specimen(
            "enz_banner",
            "A message across the top of a page or pane, with an optional action. The tone sets its colour and icon.",
            banner.into_element(),
        )
        .controls([
            k.text_control("banner.title", "Title", "Sync failed"),
            k.pick_control("banner.tone", "Tone", BANNER_TONE_NAMES, 2),
            k.flag_control("banner.body", "Body", true),
            k.flag_control("banner.action", "Action", true),
            k.flag_control("banner.dismissible", "Dismissible", true),
            k.flag_control("banner.icon", "Icon", true),
        ])
        .code(
            &code("enzui_widgets", "enz_banner")
                .arg(quoted(title))
                .import("enzui_widgets::BannerTone")
                .set("tone", format!("BannerTone::{}", BANNER_TONE_NAMES[tone]))
                .set_if(body, "body", quoted("The last three changes are still local."))
                .set_if(action, "action_text", quoted("Retry"))
                .set_if(action, "on_action", "Message::Retry")
                .set_if(dismissible, "on_dismiss", "Message::DismissBanner")
                .set_if(!dismissible, "dismissible", "false")
                .set_if(!icon, "show_icon", "false"),
        )
        .into_element()
    }

    fn progress_bar_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let tone = k.pick("progress.tone", 0);
        let labelled = k.flag("progress.label", false);
        let animated = k.flag("progress.animated", true);
        let tones = [
            ProgressTone::Accent,
            ProgressTone::Success,
            ProgressTone::Warning,
            ProgressTone::Danger,
        ];
        let mut bar = enz_progress_bar(self.progress)
            .tone(tones[tone])
            .animated(animated);
        if labelled {
            bar = bar.label("Uploading");
        }
        self.specimen(
            "enz_progress_bar",
            "How far a task has got, from 0.0 to 1.0. The fill glides to each new value.",
            column![
                bar.into_element(),
                row![
                    container(
                        enz_slider(0.0..=1.0, self.progress, Message::Progress)
                            .label("Value")
                            .into_element()
                    )
                    .width(240),
                    enz_button("Jump", Message::NextValue)
                        .kind(ButtonKind::Secondary)
                        .size(ButtonSize::Sm)
                        .into_element(),
                ]
                .spacing(16)
                .align_y(iced::Alignment::Center),
            ]
            .spacing(16),
        )
        .controls([
            k.pick_control("progress.tone", "Tone", TONE_NAMES, 0),
            k.flag_control("progress.label", "Label", false),
            k.flag_control("progress.animated", "Animated", true),
        ])
        .code(
            &code("enzui_widgets", "enz_progress_bar")
                .arg("self.progress")
                .variant(
                    tone != 0,
                    "tone",
                    "enzui_widgets",
                    "ProgressTone",
                    TONE_NAMES[tone],
                )
                .set_if(labelled, "label", quoted("Uploading"))
                .set_if(!animated, "animated", "false"),
        )
        .into_element()
    }

    fn spinner_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let size = k.number("spinner.size", 24.0);
        let tone = k.pick("spinner.tone", 0);
        let tones = [
            SpinnerTone::Accent,
            SpinnerTone::Success,
            SpinnerTone::Warning,
            SpinnerTone::Danger,
        ];
        self.specimen(
            "enz_spinner",
            "Work in progress with no known length. It turns on the compositor, so it costs the app no frames.",
            enz_spinner()
                .size(size)
                .tone(tones[tone])
                .label("Loading")
                .into_element(),
        )
        .controls([
            k.number_control("spinner.size", "Size", 16.0..=64.0, 4.0, 24.0),
            k.pick_control("spinner.tone", "Tone", TONE_NAMES, 0),
        ])
        .code(
            &code("enzui_widgets", "enz_spinner")
                .set_if(size != 24.0, "size", number(size))
                .variant(tone != 0, "tone", "enzui_widgets", "SpinnerTone", TONE_NAMES[tone])
                .set("label", quoted("Loading")),
        )
        .into_element()
    }

    fn gauge_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let diameter = k.number("gauge.diameter", 80.0);
        let tone = k.pick("gauge.tone", 0);
        let labelled = k.flag("gauge.label", true);
        let arc = k.pick("gauge.style", 0) == 1;
        let width = k.number("gauge.width", 0.0);
        let tones = [
            GaugeTone::Accent,
            GaugeTone::Success,
            GaugeTone::Warning,
            GaugeTone::Danger,
        ];
        let mut gauge = enz_gauge(self.progress)
            .diameter(diameter)
            .tone(tones[tone]);
        if arc {
            gauge = gauge.style(GaugeStyle::Arc);
        }
        if width > 0.0 {
            gauge = gauge.track_width(width);
        }
        if labelled {
            gauge = gauge.label("Disk");
        }
        self.specimen(
            "enz_gauge",
            "A value as a ring, for a level or a share. It sweeps to each new value; drag the progress slider above.",
            gauge.into_element(),
        )
        .controls([
            k.number_control("gauge.diameter", "Diameter", 48.0..=160.0, 8.0, 80.0),
            k.pick_control("gauge.style", "Style", &["Ring", "Arc"], 0),
            k.number_control("gauge.width", "Line width (0 = auto)", 0.0..=24.0, 2.0, 0.0),
            k.pick_control("gauge.tone", "Tone", TONE_NAMES, 0),
            k.flag_control("gauge.label", "Label", true),
        ])
        .code(
            &code("enzui_widgets", "enz_gauge")
                .arg("self.disk_used")
                .set_if(diameter != 80.0, "diameter", number(diameter))
                .variant(arc, "style", "enzui_widgets", "GaugeStyle", "Arc")
                .set_if(width > 0.0, "track_width", number(width))
                .variant(tone != 0, "tone", "enzui_widgets", "GaugeTone", TONE_NAMES[tone])
                .set_if(labelled, "label", quoted("Disk")),
        )
        .into_element()
    }

    fn status_dot_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let kind = k.pick("dot.kind", 0);
        let motion = k.pick("dot.motion", 0);
        let motions = [
            StatusDotMotion::Still,
            StatusDotMotion::Breathe,
            StatusDotMotion::Ripple,
        ];
        const MOTION_NAMES: &[&str] = &["Still", "Breathe", "Ripple"];
        self.specimen(
            "enz_status_dot",
            "A small coloured dot for a state: online, failing, running. It can breathe or ripple for a live state.",
            row![
                enz_status_dot(STATUSES[kind])
                    .motion(motions[motion])
                    .into_element(),
                enz_text(STATUS_NAMES[kind]).into_element(),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center),
        )
        .controls([
            k.pick_control("dot.kind", "Kind", STATUS_NAMES, 0),
            k.pick_control("dot.motion", "Motion", MOTION_NAMES, 0),
        ])
        .code(
            &code("enzui_widgets", "enz_status_dot")
                .import("enzui_widgets::status_dot::StatusKind")
                .arg(format!("StatusKind::{}", STATUS_NAMES[kind]))
                .variant(
                    motion != 0,
                    "motion",
                    "enzui_widgets",
                    "StatusDotMotion",
                    MOTION_NAMES[motion],
                ),
        )
        .into_element()
    }

    fn badge_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.text("badge.label", "3 new");
        let tone = k.pick("badge.tone", 0);
        let size = k.pick("badge.size", 1);
        let icon = k.flag("badge.icon", false);
        let mut badge = enz_badge(label)
            .tone(BADGE_TONES[tone])
            .size(BADGE_SIZES[size]);
        if icon {
            badge = badge.icon("bell");
        }
        self.specimen(
            "enz_badge",
            "A short label or count beside something else.",
            badge.into_element(),
        )
        .controls([
            k.text_control("badge.label", "Label", "3 new"),
            k.pick_control("badge.tone", "Tone", BADGE_TONE_NAMES, 0),
            k.pick_control("badge.size", "Size", SIZE_NAMES, 1),
            k.flag_control("badge.icon", "Icon", false),
        ])
        .code(
            &code("enzui_widgets", "enz_badge")
                .arg(quoted(label))
                .variant(
                    tone != 0,
                    "tone",
                    "enzui_widgets",
                    "BadgeTone",
                    BADGE_TONE_NAMES[tone],
                )
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "BadgeSize",
                    SIZE_NAMES[size],
                )
                .set_if(icon, "icon", quoted("bell")),
        )
        .into_element()
    }

    fn avatar_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let name = k.text("avatar.name", "Ada Lovelace");
        let size = k.number("avatar.size", 40.0);
        let shape = k.pick("avatar.shape", 0);
        let presence = k.pick("avatar.presence", 0);
        self.specimen(
            "enz_avatar",
            "A person's picture, or their initials when there is none.",
            enz_avatar(name)
                .avatar_size(size)
                .shape(SHAPES[shape])
                .presence(PRESENCES[presence])
                .into_element(),
        )
        .controls([
            k.text_control("avatar.name", "Name", "Ada Lovelace"),
            k.number_control("avatar.size", "Size", 24.0..=96.0, 4.0, 40.0),
            k.pick_control("avatar.shape", "Shape", SHAPE_NAMES, 0),
            k.pick_control("avatar.presence", "Presence", PRESENCE_NAMES, 0),
        ])
        .code(
            &code("enzui_widgets", "enz_avatar")
                .arg(quoted(name))
                .set_if(size != 40.0, "avatar_size", number(size))
                .variant(
                    shape != 0,
                    "shape",
                    "enzui_widgets",
                    "AvatarShape",
                    SHAPE_NAMES[shape],
                )
                .variant(
                    presence != 0,
                    "presence",
                    "enzui_widgets",
                    "Presence",
                    PRESENCE_NAMES[presence],
                ),
        )
        .into_element()
    }

    fn skeleton_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let kind = k.pick("skeleton.kind", 0);
        let lines = k.number("skeleton.lines", 3.0) as usize;
        let (shape, shape_code) = match kind {
            1 => (SkeletonKind::Circle, "SkeletonKind::Circle".to_owned()),
            2 => (SkeletonKind::Card, "SkeletonKind::Card".to_owned()),
            _ => (
                SkeletonKind::Text { lines },
                format!("SkeletonKind::Text {{ lines: {lines} }}"),
            ),
        };
        let mut controls =
            vec![k.pick_control("skeleton.kind", "Kind", &["Text", "Circle", "Card"], 0)];
        if kind == 0 {
            controls.push(k.number_control("skeleton.lines", "Lines", 1.0..=6.0, 1.0, 3.0));
        }
        self.specimen(
            "enz_skeleton",
            "A placeholder in the shape of content that is still loading. It pulses unless motion is reduced.",
            container(enz_skeleton(shape).into_element()).width(Length::Fill.max(420.0)),
        )
        .controls(controls)
        .code(
            &code("enzui_widgets", "enz_skeleton")
                .import("enzui_widgets::skeleton::SkeletonKind")
                .arg(shape_code),
        )
        .into_element()
    }

    fn toast_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let text = k.text("toast.text", "Message sent");
        let tone = k.pick("toast.tone", 0);
        let description = k.flag("toast.description", false);
        let action = k.flag("toast.action", true);
        let dismissible = k.flag("toast.dismissible", true);
        let mut toast = enzui_widgets::enz_toast(text, Message::Clicked("toast"))
            .tone(TOAST_TONES[tone])
            .dismissible(dismissible);
        if description {
            toast = toast.description("It arrives in a few seconds.");
        }
        if action {
            toast = toast.action("Undo", Message::Clicked("undo send"));
        }
        self.specimen(
            "enz_toast",
            "A brief message about something that just happened. Shown here inline; an app stacks them in a toast host over its window.",
            container(toast.into_element()).width(Length::Fill.max(420.0)),
        )
        .controls([
            k.text_control("toast.text", "Text", "Message sent"),
            k.pick_control("toast.tone", "Tone", TOAST_TONE_NAMES, 0),
            k.flag_control("toast.description", "Description", false),
            k.flag_control("toast.action", "Action", true),
            k.flag_control("toast.dismissible", "Dismissible", true),
        ])
        .code(
            &code("enzui_widgets", "enz_toast")
                .arg(quoted(text))
                .arg("Message::DismissToast")
                .import("enzui_widgets::ToastTone")
                .set("tone", format!("ToastTone::{}", TOAST_TONE_NAMES[tone]))
                .set_if(description, "description", quoted("It arrives in a few seconds."))
                .set_if(action, "action", "\"Undo\", Message::UndoSend")
                .set_if(!dismissible, "dismissible", "false"),
        )
        .into_element()
    }

    fn rating_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let value = k.number("rating.value", 3.5);
        let max = k.number("rating.max", 5.0) as u8;
        self.specimen(
            "enz_rating",
            "A read-only star rating; halves show as half stars.",
            Element::from(enzui_widgets::rating::enz_rating(value.min(f32::from(max))).max(max)),
        )
        .controls([
            k.number_control("rating.value", "Value", 0.0..=10.0, 0.5, 3.5),
            k.number_control("rating.max", "Stars", 3.0..=10.0, 1.0, 5.0),
        ])
        .code(
            &code("enzui_widgets::rating", "enz_rating")
                .arg(number(value.min(f32::from(max))))
                .set_if(max != 5, "max", max.to_string())
                .end(None),
        )
        .into_element()
    }

    fn status_page_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let title = k.text("status.title", "No messages");
        let error = k.pick("status.tone", 0) == 1;
        let compact = k.flag("status.compact", true);
        let action = k.flag("status.action", false);
        let mut page = enzui_widgets::enz_status_page(title)
            .description("New mail shows up here.")
            .icon(if error { "circle-alert" } else { "inbox" })
            .tone(if error {
                StatusPageTone::Error
            } else {
                StatusPageTone::Empty
            })
            .compact(compact);
        if action {
            page = page.primary_action(
                enzui_core::Action::new("refresh", Message::Clicked("refresh"))
                    .with_label("Refresh"),
            );
        }
        self.specimen(
            "enz_status_page",
            "Fills an empty or failed view with an icon, a title, a line of help and an optional action.",
            page.into_element(),
        )
        .controls([
            k.text_control("status.title", "Title", "No messages"),
            k.pick_control("status.tone", "Tone", &["Empty", "Error"], 0),
            k.flag_control("status.compact", "Compact", true),
            k.flag_control("status.action", "Action", false),
        ])
        .code(
            &code("enzui_widgets", "enz_status_page")
                .arg(quoted(title))
                .set("description", quoted("New mail shows up here."))
                .set("icon", quoted(if error { "circle-alert" } else { "inbox" }))
                .variant(error, "tone", "enzui_widgets", "StatusPageTone", "Error")
                .set_if(compact, "compact", "true")
                .import_if(action, "enzui_core::Action")
                .set_if(
                    action,
                    "primary_action",
                    "Action::new(\"refresh\", Message::Refresh).with_label(\"Refresh\")",
                ),
        )
        .into_element()
    }
}
