//! Persistent activity status over [`ActivityCenter`], in two presentations.
//!
//! * **Compact** ([`ActivityPresentation::Compact`]): one sidebar-style row
//!   (an icon, `3 downloading`, the aggregate percentage) with a thin
//!   aggregate bar under its label. Activating it routes the
//!   [`ActivityOverflowTarget`] that reveals every row ([`status_target`]):
//!   the provider's own panel when one provider owns every row, the
//!   all-activity destination otherwise. Per-operation controls live there,
//!   not in the sidebar. This is the default at the bottom of the shell's
//!   sidebar.
//! * **Detailed** ([`ActivityPresentation::Detailed`]): a panel of rows in the
//!   center's revealed, attention-ordered view and row budget. Terminal rows
//!   show outcomes without progress bars; live fractional rows show
//!   percentages. Operation controls remain per row, while `OpenPanel` is
//!   coalesced per visible provider. Overflow routes to the hidden providers.
//!   This is the default where the shell floats activity over content.
//!
//! [`ActivityPresentation::Auto`] (the default) picks by placement: compact
//! when the surface is built for [`ActivityPlacement::SidebarBottom`] (the
//! shell's [`crate::app_shell::EnzAppShell::activity`] slot tells it, or
//! [`EnzActivitySurface::placement`] does), detailed otherwise.
//!
//! Hosts advance the center’s deadlines and return no activity element while
//! [`EnzActivitySurface::is_empty`] is true; converting an empty surface still
//! creates a panel. [`crate::app_shell`] provides sidebar or floating placement.
//!
//! Each presentation publishes one polite status region; the detailed panel's
//! rows are list items containing named controls. [`REGION_ID`] is shared, so
//! mount one surface per interface.
//!
//! # Example
//!
//! ```
//! use enzui_core::operation_activity::{
//!     Activity, ActivityCenter, ActivityControl, ActivityKey, ActivityKind, ProviderId,
//! };
//! use enzui_widgets::activity_surface::enz_activity_surface;
//!
//! #[derive(Debug, Clone)]
//! enum Msg {
//!     Control(ActivityKey, ActivityControl),
//! }
//!
//! let mut center = ActivityCenter::new();
//! center.register_provider(ProviderId("files"), true);
//! center.publish(
//!     Activity::new(ActivityKey::new("files", 1), ActivityKind::Copy, "Copying 3 items"),
//!     0,
//! );
//! // A running row shows once it has run for the center's reveal delay.
//! assert!(enz_activity_surface::<Msg>(&center).is_empty());
//! center.prune(800);
//! let surface = enz_activity_surface(&center).on_control(Msg::Control);
//! assert!(!surface.is_empty());
//! let element: iced::Element<'_, Msg> = surface.into();
//! ```

use crate::app_shell::ActivityPlacement;
use crate::button::{ButtonKind, ButtonSize, enz_button_with};
use crate::enz_text::{TextSize, TextTone, enz_text};
use crate::gauge::Sampled;
use crate::icon::{IconTone, enz_icon};
use crate::progress_bar::{ProgressTone, enz_progress_bar, enz_progress_bar_sampled};
use crate::semantics_container::SemanticsContainer;
use crate::sidebar_row::{SidebarRowTokens, enz_sidebar_row, enz_sidebar_row_lazy};
use crate::surface::enz_surface;
pub use enzui_core::operation_activity::ActivityOverflowTarget;
use enzui_core::operation_activity::{
    Activity, ActivityCenter, ActivityControl, ActivityKey, ActivityPhase, ActivityProgress,
    ActivitySummary,
};
use enzui_core::semantics::{Live, NumericValue, Role, Semantics};
use enzui_theme::{SizingTokens, SpacingScale};
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length, Padding};
use std::cell::Cell;

/// English fallback prefix of the activity region's accessible name.
pub const REGION_LABEL: &str = "Activity";

/// Stable widget identity of the activity live region.
pub const REGION_ID: &str = "enzui-activity-region";

/// Stable widget identity of the compact status row, unless
/// [`EnzActivitySurface::overflow_id`] pins another. A dialog opened from the
/// row can return focus here.
pub const STATUS_ROW_ID: &str = "enzui-activity-status";

/// Height of the compact presentation's aggregate bar: a hairline meter, not a
/// control.
const COMPACT_BAR_HEIGHT: f32 = 3.0;

/// How an [`EnzActivitySurface`] presents the center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivityPresentation {
    /// Compact at [`ActivityPlacement::SidebarBottom`], detailed elsewhere.
    #[default]
    Auto,
    /// One sidebar-style status row with an aggregate bar; activating it
    /// routes to where the work is managed.
    Compact,
    /// The panel of per-operation rows with their controls.
    Detailed,
}

thread_local! {
    /// The site the shell is building its activity slot for, if any.
    static SHELL_PLACEMENT: Cell<Option<ActivityPlacement>> = const { Cell::new(None) };
}

struct PlacementGuard(Option<ActivityPlacement>);

impl Drop for PlacementGuard {
    fn drop(&mut self) {
        SHELL_PLACEMENT.with(|c| c.set(self.0));
    }
}

/// Build `f`'s value as the shell's activity slot at `site`, so an
/// [`ActivityPresentation::Auto`] surface built inside resolves its
/// presentation from where it will be shown.
pub(crate) fn with_shell_placement<R>(site: ActivityPlacement, f: impl FnOnce() -> R) -> R {
    let previous = SHELL_PLACEMENT.with(|c| c.replace(Some(site)));
    let _guard = PlacementGuard(previous);
    f()
}

/// English fallback prefix of an activity row's accessible name.
pub const ROW_LABEL_PREFIX: &str = "Activity: ";

/// Formats the count of hidden rows as a `+N more` label.
#[must_use]
pub fn overflow_label(count: usize) -> String {
    enzui_core::localized!("enzui-activity-surface-more", "count" => count)
}

fn normalized_fraction(fraction: f32) -> Option<f32> {
    fraction.is_finite().then(|| fraction.clamp(0.0, 1.0))
}

fn percentage_text(fraction: f32) -> String {
    format!("{}%", (fraction * 100.0).round() as u32)
}

/// Names nonempty activity buckets and includes a finite aggregate percentage.
///
/// Finite fractions clamp to 0–100%; absent or nonfinite fractions omit it.
/// An empty summary reads “No active operations”.
#[must_use]
pub fn summary_sentence(summary: &ActivitySummary) -> String {
    if summary.is_empty() {
        return enzui_core::localization::text("enzui-activity-surface-no-active-operations");
    }
    let mut parts = Vec::new();
    if summary.running > 0 {
        parts.push(
            enzui_core::localized!("enzui-activity-surface-running", "count" => summary.running),
        );
    }
    if summary.waiting > 0 {
        parts.push(
            enzui_core::localized!("enzui-activity-surface-waiting", "count" => summary.waiting),
        );
    }
    if summary.paused > 0 {
        parts.push(
            enzui_core::localized!("enzui-activity-surface-paused", "count" => summary.paused),
        );
    }
    if summary.failed > 0 {
        parts.push(
            enzui_core::localized!("enzui-activity-surface-failed", "count" => summary.failed),
        );
    }
    if summary.completed > 0 {
        parts.push(enzui_core::localized!("enzui-activity-surface-completed", "count" => summary.completed));
    }
    if summary.cancelled > 0 {
        parts.push(enzui_core::localized!("enzui-activity-surface-cancelled", "count" => summary.cancelled));
    }
    let head = parts.join(", ");
    match summary.fraction.and_then(normalized_fraction) {
        Some(fraction) => {
            enzui_core::localized!("enzui-activity-surface-summary-progress", "summary" => head, "percentage" => percentage_text(fraction))
        }
        None => head,
    }
}

/// Maps activity phase to progress-bar emphasis.
#[must_use]
pub fn phase_tone(phase: ActivityPhase) -> ProgressTone {
    match phase {
        ActivityPhase::Completed => ProgressTone::Success,
        ActivityPhase::Paused | ActivityPhase::Waiting => ProgressTone::Warning,
        ActivityPhase::Failed | ActivityPhase::Cancelled => ProgressTone::Danger,
        ActivityPhase::Queued | ActivityPhase::Running => ProgressTone::Accent,
    }
}

/// Returns an icon and tone for a terminal outcome, otherwise none.
#[must_use]
pub fn outcome_icon(phase: ActivityPhase) -> Option<(&'static str, IconTone)> {
    match phase {
        ActivityPhase::Completed => Some(("circle-check", IconTone::Success)),
        ActivityPhase::Failed => Some(("circle-alert", IconTone::Danger)),
        ActivityPhase::Cancelled => Some(("circle-slash", IconTone::Muted)),
        ActivityPhase::Queued
        | ActivityPhase::Running
        | ActivityPhase::Paused
        | ActivityPhase::Waiting => None,
    }
}

/// Uses nonempty provider detail for terminal rows, otherwise the activity label.
#[must_use]
pub fn row_title(activity: &Activity) -> String {
    match &activity.detail {
        Some(detail) if activity.phase.is_terminal() && !detail.is_empty() => detail.clone(),
        _ => activity.label.clone(),
    }
}

/// The compact row's label, in priority order: live work first
/// (`3 downloading` when every row is a download, `3 running` otherwise), then
/// what needs attention, then outcomes still lingering.
#[must_use]
pub fn compact_headline(summary: &ActivitySummary, downloads_only: bool) -> String {
    let count = |key: &str, count: usize| enzui_core::localized!(key, "count" => count);
    if summary.running > 0 {
        let key = if downloads_only {
            "enzui-activity-surface-downloading"
        } else {
            "enzui-activity-surface-running"
        };
        count(key, summary.running)
    } else if summary.waiting > 0 {
        count("enzui-activity-surface-waiting", summary.waiting)
    } else if summary.paused > 0 {
        count("enzui-activity-surface-paused", summary.paused)
    } else if summary.failed > 0 {
        count("enzui-activity-surface-failed", summary.failed)
    } else if summary.completed > 0 {
        count("enzui-activity-surface-completed", summary.completed)
    } else if summary.cancelled > 0 {
        count("enzui-activity-surface-cancelled", summary.cancelled)
    } else {
        enzui_core::localization::text("enzui-activity-surface-no-active-operations")
    }
}

/// Where activating the compact status goes: the provider's own panel when
/// one provider owns every row, the all-activity destination otherwise.
#[must_use]
pub fn status_target(center: &ActivityCenter) -> ActivityOverflowTarget {
    let mut providers = Vec::new();
    for row in center.iter() {
        if !providers.contains(&row.key.provider) {
            providers.push(row.key.provider);
        }
    }
    match providers.as_slice() {
        [provider] => ActivityOverflowTarget::Provider(*provider),
        _ => ActivityOverflowTarget::AllActivity { providers },
    }
}

/// The compact presentation's aggregate: the center's own, or, while it
/// withholds one because some live row has no known size yet (a queued
/// download before its first response), the share of the sized byte work.
fn compact_fraction(center: &ActivityCenter, summary: &ActivitySummary) -> Option<f32> {
    if let Some(fraction) = summary.fraction.and_then(normalized_fraction) {
        return Some(fraction);
    }
    let (mut done, mut total) = (0u64, 0u64);
    for row in center.iter().filter(|row| !row.phase.is_terminal()) {
        if let ActivityProgress::Bytes {
            done: d,
            total: Some(t),
        } = row.progress
        {
            done = done.saturating_add(d.min(t));
            total = total.saturating_add(t);
        }
    }
    (total > 0).then(|| (done as f64 / total as f64) as f32)
}

fn control_kind(control: ActivityControl) -> ButtonKind {
    match control {
        ActivityControl::Cancel => ButtonKind::Destructive,
        ActivityControl::Retry | ActivityControl::Resolve => ButtonKind::Accent,
        ActivityControl::Pause | ActivityControl::Resume | ActivityControl::OpenPanel => {
            ButtonKind::Secondary
        }
    }
}

/// Activity panel borrowing a host-owned center and owning its presentation callbacks.
pub struct EnzActivitySurface<'a, Message> {
    center: &'a ActivityCenter,
    on_control: Option<Box<dyn Fn(ActivityKey, ActivityControl) -> Message + 'a>>,
    on_overflow: Option<Box<dyn Fn(ActivityOverflowTarget) -> Message + 'a>>,
    overflow_id: Option<iced::advanced::widget::Id>,
    width: Length,
    presentation: ActivityPresentation,
    placement: Option<ActivityPlacement>,
}

/// Builds an activity surface borrowing `center`, with fill width, no control
/// routes and [`ActivityPresentation::Auto`].
///
/// The detailed panel's overflow button gets a new identity per construction;
/// the compact row's is [`STATUS_ROW_ID`]. Conversion copies displayed text.
#[must_use]
pub fn enz_activity_surface<Message>(center: &ActivityCenter) -> EnzActivitySurface<'_, Message> {
    EnzActivitySurface {
        center,
        on_control: None,
        on_overflow: None,
        overflow_id: None,
        width: Length::Fill,
        presentation: ActivityPresentation::Auto,
        placement: None,
    }
}

impl<'a, Message: Clone + 'a> EnzActivitySurface<'a, Message> {
    /// Adds buttons for declared controls and routes them through this callback.
    ///
    /// The callback runs during element construction; activation publishes the
    /// resulting message. `OpenPanel` appears once per visible provider.
    #[must_use]
    pub fn on_control(
        mut self,
        on_control: impl Fn(ActivityKey, ActivityControl) -> Message + 'a,
    ) -> Self {
        self.on_control = Some(Box::new(on_control));
        self
    }

    /// Routes the overflow button to the destination derived from hidden rows.
    ///
    /// One hidden provider yields [`ActivityOverflowTarget::Provider`]; multiple
    /// providers yield [`ActivityOverflowTarget::AllActivity`]. Without a callback,
    /// overflow is plain text. The callback runs during element construction.
    ///
    /// The compact row activates through the same route, with
    /// [`status_target`] (every row, not only hidden ones); without a callback
    /// the row is an inert status.
    #[must_use]
    pub fn on_overflow(
        mut self,
        on_overflow: impl Fn(ActivityOverflowTarget) -> Message + 'a,
    ) -> Self {
        self.on_overflow = Some(Box::new(on_overflow));
        self
    }

    /// Routes every overflow destination to the same cloned message.
    ///
    /// Use [`Self::on_overflow`] when the destination depends on hidden providers.
    #[must_use]
    pub fn on_show_all(mut self, message: Message) -> Self {
        self.on_overflow = Some(Box::new(move |_| message.clone()));
        self
    }

    /// Pins the identity of the overflow button (detailed) or the status row
    /// (compact) across view rebuilds.
    ///
    /// The detailed default is a new unique id for each surface construction;
    /// the compact default is [`STATUS_ROW_ID`].
    #[must_use]
    pub fn overflow_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.overflow_id = Some(id);
        self
    }

    /// Chooses the presentation; defaults to [`ActivityPresentation::Auto`].
    #[must_use]
    pub fn presentation(mut self, presentation: ActivityPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    /// States where the surface is shown, for a surface mounted outside the
    /// shell's activity slot (inside it, the shell states it). Resolves
    /// [`ActivityPresentation::Auto`] and the compact row's chrome: flat on the
    /// sidebar's rail at [`ActivityPlacement::SidebarBottom`], on an elevated
    /// surface elsewhere.
    #[must_use]
    pub fn placement(mut self, placement: ActivityPlacement) -> Self {
        self.placement = Some(placement);
        self
    }

    fn resolved_placement(&self) -> Option<ActivityPlacement> {
        self.placement.or_else(|| SHELL_PLACEMENT.with(Cell::get))
    }

    /// The presentation [`Self::into_element`] builds, after resolving
    /// [`ActivityPresentation::Auto`] against the placement.
    #[must_use]
    pub fn resolved_presentation(&self) -> ActivityPresentation {
        match self.presentation {
            ActivityPresentation::Auto => {
                if self.resolved_placement() == Some(ActivityPlacement::SidebarBottom) {
                    ActivityPresentation::Compact
                } else {
                    ActivityPresentation::Detailed
                }
            }
            chosen => chosen,
        }
    }

    /// Sets panel width; defaults to [`Length::Fill`].
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Reports that the center has no revealed rows, including while work awaits its reveal delay.
    ///
    /// Hosts can use this to omit the panel; conversion does not check it.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.center.is_empty()
    }

    /// Resolves status-region semantics without constructing or laying out the panel.
    ///
    /// The label includes the summary; numeric progress exists only for a finite
    /// aggregate, clamped to 0–1.
    pub fn semantics_for_test(&self) -> Semantics {
        region_semantics(&self.center.view().summary)
    }

    fn control_button(
        &self,
        key: ActivityKey,
        control: ActivityControl,
    ) -> Option<Element<'a, Message>> {
        let route = self.on_control.as_ref()?;
        let message = route(key, control);
        Some(
            enz_button_with(control.label(), move || message.clone())
                .kind(control_kind(control))
                .size(ButtonSize::Xs)
                .into(),
        )
    }

    fn row(&self, activity: &Activity) -> Element<'a, Message> {
        let spacing = SpacingScale::default();
        let title = row_title(activity);

        let lines = if let Some((icon, tone)) = outcome_icon(activity.phase) {
            let mark = enz_icon(icon)
                .size(SizingTokens::default().icon_sm)
                .tint_theme_tone(tone);
            column![
                row![mark, enz_text(title).size(TextSize::Caption)]
                    .spacing(spacing.space2)
                    .align_y(Alignment::Center)
            ]
            .width(Length::Fill)
        } else {
            let mut lines = column![enz_text(title).size(TextSize::Caption)]
                .spacing(spacing.space2)
                .width(Length::Fill);
            if let Some(detail) = &activity.detail {
                lines = lines.push(
                    enz_text(detail.clone())
                        .size(TextSize::Caption2)
                        .tone(TextTone::Muted),
                );
            }
            let fraction = activity.progress.fraction();
            let sample: Sampled<f32> = fraction.into();
            let mut progress = enz_progress_bar_sampled(sample)
                .tone(phase_tone(activity.phase))
                .busy(activity.phase == ActivityPhase::Running)
                .bar_height(spacing.space2);
            if matches!(activity.progress, ActivityProgress::Fraction(_))
                && let Some(fraction) = fraction
            {
                // Caption and numeric semantics use the same sanitized fraction.
                progress = progress.value_text(&percentage_text(fraction));
            }
            lines.push(progress)
        };

        let mut controls = row![].spacing(spacing.space2);
        let mut has_row_controls = false;
        for control in activity
            .controls
            .iter()
            .filter(|control| **control != ActivityControl::OpenPanel)
        {
            if let Some(button) = self.control_button(activity.key, *control) {
                controls = controls.push(button);
                has_row_controls = true;
            }
        }

        // Separate vertical tracks keep long labels from overlapping row controls.
        let mut body = column![lines].spacing(spacing.space2).width(Length::Fill);
        if has_row_controls {
            body = body.push(controls);
        }

        SemanticsContainer::new(
            body,
            Semantics::new(Role::ListItem)
                .label(enzui_core::localized!("enzui-activity-row", "title" => row_title(activity)))
                .description(activity.announcement())
                .live(Some(Live::Off)),
        )
        .into()
    }

    /// Builds the panel even when the center has no revealed rows.
    ///
    /// Callbacks construct messages now; button activation publishes clones. The
    /// summary heading appears for multiple visible rows or any overflow.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        if self.resolved_presentation() == ActivityPresentation::Compact {
            return self.compact();
        }
        let spacing = SpacingScale::default();
        let view = self.center.view();

        let mut body = column![].spacing(spacing.space4).width(Length::Fill);
        if view.rows.len() > 1 || view.overflow > 0 {
            body = body.push(
                enz_text(summary_sentence(&view.summary))
                    .size(TextSize::Caption2)
                    .tone(TextTone::Muted),
            );
        }

        let mut open_panel_keys = Vec::new();
        for activity in &view.rows {
            body = body.push(self.row(activity));
            if activity.controls.contains(&ActivityControl::OpenPanel)
                && !open_panel_keys
                    .iter()
                    .any(|key: &ActivityKey| key.provider == activity.key.provider)
            {
                open_panel_keys.push(activity.key);
            }
        }

        for key in open_panel_keys {
            if let Some(button) = self.control_button(key, ActivityControl::OpenPanel) {
                body = body.push(button);
            }
        }

        if view.overflow > 0 {
            let label = overflow_label(view.overflow);
            match (self.on_overflow.as_ref(), view.overflow_target.clone()) {
                (Some(route), Some(target)) => {
                    let message = route(target);
                    body = body.push(
                        enz_button_with(label, move || message.clone())
                            .id(self
                                .overflow_id
                                .clone()
                                .unwrap_or_else(iced::advanced::widget::Id::unique))
                            .kind(ButtonKind::Ghost)
                            .size(ButtonSize::Xs),
                    );
                }
                _ => {
                    body = body.push(
                        enz_text(label)
                            .size(TextSize::Caption2)
                            .tone(TextTone::Muted),
                    );
                }
            }
        }

        let panel = enz_surface(body)
            .padding(spacing.space4)
            .elevation(1)
            .width(self.width);

        SemanticsContainer::new(panel, region_semantics(&view.summary))
            .id(iced::advanced::widget::Id::new(REGION_ID))
            .into()
    }
}

impl<'a, Message: Clone + 'a> EnzActivitySurface<'a, Message> {
    /// The compact presentation: one sidebar-style row and a thin aggregate bar.
    fn compact(self) -> Element<'a, Message> {
        let spacing = SpacingScale::default();
        let summary = self.center.view().summary;
        let target = status_target(self.center);
        let downloads_only = matches!(
            target,
            ActivityOverflowTarget::Provider(provider)
                if provider == enzui_core::operation_activity::downloads::DOWNLOADS
        );
        let failed_only = summary.active() == 0 && summary.failed > 0;
        let icon = if failed_only {
            "circle-alert"
        } else if downloads_only {
            "download"
        } else {
            "activity"
        };
        let fraction = compact_fraction(self.center, &summary);
        // Live work only: a lingering outcome has no progress to show.
        let live_fraction = fraction.filter(|_| summary.active() > 0);
        let percent = live_fraction.map(percentage_text).unwrap_or_default();

        let headline = compact_headline(&summary, downloads_only);
        let row = match self.on_overflow.as_ref() {
            Some(route) => enz_sidebar_row(headline, route(target)),
            None => enz_sidebar_row_lazy(headline),
        }
        .id(self
            .overflow_id
            .clone()
            .unwrap_or_else(|| iced::advanced::widget::Id::new(STATUS_ROW_ID)))
        .icon(icon)
        .count(percent)
        .into_element();

        let mut body = column![row].width(Length::Fill);
        if let Some(fraction) = live_fraction {
            // Under the label, from the label's start to the count's end.
            // Row geometry is palette-independent.
            let tokens = SidebarRowTokens::light();
            let lead = tokens.label_inset_x() + tokens.icon_size + tokens.icon_gap;
            let trail = tokens.label_inset_x();
            let rtl = enzui_core::placement::resolve_direction(None)
                == enzui_core::placement::Direction::RightToLeft;
            let (left, right) = if rtl { (trail, lead) } else { (lead, trail) };
            let mut bar = enz_progress_bar(fraction)
                .bar_height(COMPACT_BAR_HEIGHT)
                .busy(summary.running > 0);
            if summary.running == 0 {
                // Paused or waiting work is not moving: a quiet fill.
                bar = bar.fill_role(|palette| palette.text_subtle);
            }
            body = body.push(
                container(bar.into_element())
                    .padding(Padding {
                        top: 0.0,
                        right,
                        bottom: spacing.space2,
                        left,
                    })
                    .width(Length::Fill),
            );
        }

        let placed: Element<'a, Message> =
            if self.resolved_placement() == Some(ActivityPlacement::SidebarBottom) {
                // On the sidebar's own rail, like every other sidebar row.
                container(body)
                    .padding(Padding {
                        top: 0.0,
                        right: spacing.space4,
                        bottom: spacing.space4,
                        left: spacing.space4,
                    })
                    .width(self.width)
                    .into()
            } else {
                enz_surface(body)
                    .padding(spacing.space2)
                    .elevation(1)
                    .width(self.width)
                    .into()
            };

        let mut semantics = region_semantics(&summary);
        if semantics.numeric.is_none()
            && let Some(fraction) = live_fraction
        {
            // The bar's share of the sized work, published as the region's value.
            semantics = semantics.numeric(NumericValue {
                value: f64::from(fraction.clamp(0.0, 1.0)),
                min: 0.0,
                max: 1.0,
                step: None,
            });
        }
        SemanticsContainer::new(placed, semantics)
            .id(iced::advanced::widget::Id::new(REGION_ID))
            .into()
    }
}

fn region_semantics(summary: &ActivitySummary) -> Semantics {
    let sentence = summary_sentence(summary);
    let mut semantics = Semantics::new(Role::Status)
        .label(enzui_core::localized!("enzui-activity-region", "summary" => sentence))
        .live(Some(Live::Polite));
    if let Some(fraction) = summary.fraction.and_then(normalized_fraction) {
        semantics = semantics.numeric(NumericValue {
            value: f64::from(fraction),
            min: 0.0,
            max: 1.0,
            step: None,
        });
    }
    semantics
}

impl<'a, Message: Clone + 'a> From<EnzActivitySurface<'a, Message>> for Element<'a, Message> {
    fn from(surface: EnzActivitySurface<'a, Message>) -> Self {
        surface.into_element()
    }
}
