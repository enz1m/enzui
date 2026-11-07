//! `EnzDownloadList`: the pure ICED view over [`enzui_core::download`]
//!
//!
//! # Why this file exists
//!
//! The download reducer provides stable ids, attempt generations,
//! acknowledged states, bounded fair concurrency and coalesced progress. This
//! widget is the path from a runtime entry point to that reducer.
//!
//! # Row layout
//!
//! Every row shares one column grid, so the progress bars and the action
//! buttons line up whatever the name, status or phase:
//!
//! ```text
//! [file icon] [name / one muted status line] [progress, fixed] [icons, fixed]
//! ```
//!
//! * The status line is one muted run: the byte state and the phase that
//!   matters (`730 MB of 2.00 GB · Paused`, `1.40 GB of 4.00 GB · 35%`,
//!   `750 MB · Complete`). A failure reads `Failed · <cause>` and is the only
//!   coloured text in the row (danger ink); see [`DownloadRow::status_text`].
//! * The progress column has the same width and x in every row
//!   ([`progress_column_width`]). Live work fills it with the accent and the
//!   busy sweep, paused and queued work with a quiet fill; complete, failed and
//!   cancelled rows draw no bar and their text runs on into that space.
//! * Actions are the framework's quiet toolbar icons
//!   ([`crate::toolbar_icon`]) with a tooltip and an accessible name each, in a
//!   trailing column of fixed width ([`actions_column_width`]): pause, resume,
//!   retry and cancel from [`Download::actions`], plus an optional
//!   *Show in Folder* on complete rows when the host routes one
//!   ([`EnzDownloadList::on_show_in_folder`]). There are no phase chips and no
//!   text buttons; colour carries meaning only.
//!
//! # No second state machine
//!
//! Every decision on screen is read from the model:
//!
//! * the phase word is [`Phase::word`], not a `match` here;
//! * the progress fraction is [`Download::fraction`], including the
//!   *indeterminate* case ([`None`], for a transfer with no announced length);
//! * which buttons a row draws is [`Download::actions`], not a local rule.
//!
//! So a phase added to the model appears on screen without a widget edit, and
//! the widget cannot disagree with the reducer about what is possible. The one
//! thing the widget owns is the name of a download, because a display name
//! is presentation and the reducer deliberately holds only the identity
//! ([`DownloadId`]) — see [`DownloadRow`].
//!
//! # No I/O
//!
//! Nothing here reads a socket, a file or a clock, and nothing spawns. The
//! widget renders whatever [`Download`] values it is handed and emits an
//! action; the host applies it to the model. The transport and persistence
//! halves are separate concerns and are not modelled here. The file-type
//! icon is the desktop icon theme's artwork for the name's type
//! ([`crate::file_icon`], cached per process), or the bundled glyph when the
//! theme has none; [`EnzDownloadList::file_icons`] turns the icon off.
//!
//! # Accessibility
//!
//! Each row publishes one [`Role::ListItem`] node whose label is the download
//! name and whose description carries the phase word, the percentage (or
//! `indeterminate`), the explicitly labelled byte state, and the available
//! actions by name — so an AT user learns the same facts a sighted user reads,
//! from the same source. The percentage is additionally published as a
//! [`NumericValue`] on the row node when it is determinate. Each icon action
//! publishes its own button node named by the action's verb.
//!
//! A row may also carry one bounded line of caller-owned
//! [`secondary_detail`](DownloadRow::secondary_detail): transport telemetry
//! (peer count, transfer rate) the pure reducer does not model, or the cause
//! of a failure. It rides on the row's status line after a `·` (a failed row
//! reads `Failed · <detail>`) and is appended to the same row's accessible
//! description, so it keeps the download's identity and never grows the row.

use crate::badge::BadgeTone;
use crate::enz_text::{TextTone, enz_text};
use crate::icon_slot::enz_icon_slot;
use crate::keyed_column::keyed_column;
use crate::progress_bar::enz_progress_bar;
use crate::semantics_container::SemanticsContainer;
use crate::spinner::enz_spinner;
use crate::toolbar_icon::{
    TitlebarAffordanceTokens, enz_toolbar_icon, enz_toolbar_icon_maybe_with,
};
use enzui_core::download::{Download, DownloadAction, DownloadId, Phase};
use enzui_core::placement::Direction;
use enzui_core::semantics::{NumericValue, Role, Semantics};
use enzui_core::virtual_sequence::{Anchor, HeightModel, RealizationWindow, SequenceGeometry};
use enzui_theme::{SizingTokens, SpacingScale, TypographyScale};
use iced::widget::{column, container};
use iced::{Alignment, Element, Length};
use std::cell::Cell;
use std::rc::Rc;

/// English fallback wording for a transfer without an announced length.
pub const INDETERMINATE_WORD: &str = "indeterminate";

/// English fallback prefix of a download row's accessible name.
pub const ROW_LABEL_PREFIX: &str = "Download: ";

/// One download as the list should present it: the model's state plus the
/// display name, which is the only fact the reducer does not carry.
///
/// Borrowed rather than cloned so a host with a thousand-row history pays no
/// per-frame copy of its download table.
#[derive(Debug, Clone, Copy)]
pub struct DownloadRow<'a> {
    /// The human-readable name shown in the row and used as its accessible
    /// name.
    pub name: &'a str,
    /// The authoritative model state for this download.
    pub download: &'a Download,
    /// One bounded fragment of caller-owned secondary detail, or [`None`].
    ///
    /// This slot carries what a backend knows that the pure [`Download`]
    /// reducer deliberately does not model: a torrent's peer count and
    /// transfer rate while it runs, or the cause of a failure. It belongs to
    /// *this* row (so it keeps the download's identity and can be cleared the
    /// instant the transfer goes terminal), joins the row's status line after a
    /// `·` (a failed row reads `Failed · <detail>`, in danger ink), and is
    /// published on the row's own accessible node — never as an unnamed
    /// paragraph after the list.
    ///
    /// The status line is a single elided run
    /// ([`crate::text::EnzText::single_line_elided`]), so however long the
    /// string is, the row keeps its height. [`None`] omits the fragment and the
    /// AT fragment entirely.
    pub secondary_detail: Option<&'a str>,
}

impl<'a> DownloadRow<'a> {
    /// Pair a display name with a download's model state. No secondary detail;
    /// use [`DownloadRow::secondary`] to attach one.
    #[must_use]
    pub fn new(name: &'a str, download: &'a Download) -> Self {
        Self {
            name,
            download,
            secondary_detail: None,
        }
    }

    /// Attach one bounded line of [`secondary_detail`](Self::secondary_detail)
    /// to this row.
    ///
    /// The string is caller-owned and borrowed, so a host with live telemetry
    /// pays no per-frame copy. It joins the row's single elided status line and
    /// is appended to this row's accessible description, so it can never grow
    /// the row nor detach from the download it describes.
    #[must_use]
    pub fn secondary(mut self, detail: &'a str) -> Self {
        self.secondary_detail = Some(detail);
        self
    }

    /// The percentage text this row shows: `"42%"`, or
    /// [`INDETERMINATE_WORD`] when the length is unknown.
    ///
    /// Reads [`Download::percent`] — a `0%` here is a genuine zero and is
    /// rendered literally, never collapsed into the indeterminate word.
    #[must_use]
    pub fn percent_text(&self) -> String {
        match self.download.percent() {
            Some(p) => {
                enzui_core::localized!("enzui-download-percent", "percentage" => p)
            }
            None => enzui_core::localization::text("enzui-download-indeterminate"),
        }
    }

    /// The accessible description: phase, percentage, byte state, and the
    /// available actions by name.
    ///
    /// The action list comes from [`Download::actions`], so what an AT user is
    /// told is available is exactly what is drawn — a row cannot advertise a
    /// button it does not have.
    #[must_use]
    pub fn description(&self) -> String {
        self.description_with(None)
    }

    /// [`Self::description`] with one host action (Show in Folder) appended
    /// to the model's actions, so the row names every button it draws.
    fn description_with(&self, extra_action: Option<&str>) -> String {
        let mut labels: Vec<String> = self.download.actions().iter().map(|a| a.label()).collect();
        if let Some(extra) = extra_action {
            labels.push(extra.to_owned());
        }
        let names = if labels.is_empty() {
            enzui_core::localization::text("enzui-download-no-actions")
        } else {
            labels.join(", ")
        };
        let bytes = byte_state_text(self.download);
        // The secondary detail rides on this row's description between the byte
        // count and the action list, so an AT user hears it attributed to the
        // named download rather than as a loose trailing paragraph. Absent, it
        // contributes nothing.
        let secondary = match self.secondary_detail {
            Some(s) => {
                enzui_core::localized!("enzui-download-detail-separator", "detail" => s)
            }
            None => String::new(),
        };
        enzui_core::localized!("enzui-download-list-actions", "phase" => self.download.phase.word(), "percentage" => self.percent_text(), "bytes" => bytes, "secondary" => secondary, "actions" => names)
    }

    /// The one muted line under the row's name: the byte state, then the
    /// state that matters.
    ///
    /// | phase | line |
    /// |---|---|
    /// | active, known size | `1.40 GB of 4.00 GB · 35%` |
    /// | active, unknown size | `1.40 GB · Downloading` |
    /// | queued, nothing received | `Queued` |
    /// | queued or paused | `730 MB of 2.00 GB · Paused` |
    /// | failed | `Failed · <secondary detail>` (the cause), or `Failed` |
    /// | complete | `750 MB · Complete` |
    /// | cancelled | `Cancelled` |
    ///
    /// Outside a failure, a [`secondary_detail`](Self::secondary_detail) is
    /// appended after another `·`. Every word comes from the model
    /// ([`Phase::word`], [`Download::percent`]); sizes use
    /// [`crate::file_browser::format_bytes`].
    #[must_use]
    pub fn status_text(&self) -> String {
        let d = self.download;
        let pair = |first: String, second: &str| {
            let tail =
                enzui_core::localized!("enzui-download-detail-separator", "detail" => second);
            format!("{first}{tail}")
        };
        let bytes = match d.total() {
            Some(total) => {
                enzui_core::localized!("enzui-download-list-of", "received" => crate::file_browser::format_bytes(d.received), "total" => crate::file_browser::format_bytes(total))
            }
            None => crate::file_browser::format_bytes(d.received),
        };
        let word = d.phase.word();
        let line = match d.phase {
            Phase::Active => match d.percent() {
                Some(_) => pair(bytes, &self.percent_text()),
                None => pair(bytes, &word),
            },
            Phase::Queued if d.received == 0 => word,
            Phase::Queued | Phase::Paused => pair(bytes, &word),
            Phase::Failed => {
                return match self.secondary_detail {
                    Some(cause) if !cause.is_empty() => pair(word, cause),
                    _ => word,
                };
            }
            Phase::Complete => pair(
                crate::file_browser::format_bytes(d.total().unwrap_or(d.received)),
                &word,
            ),
            Phase::Cancelled => word,
        };
        match self.secondary_detail {
            Some(detail) if !detail.is_empty() => pair(line, detail),
            _ => line,
        }
    }

    /// Whether the status line takes the danger ink: only a failed download's.
    #[must_use]
    pub fn is_failure(&self) -> bool {
        self.download.phase == Phase::Failed
    }

    /// The row's accessible node. `Role::ListItem`, labelled by the name,
    /// described by [`DownloadRow::description`], and carrying the fraction as
    /// a [`NumericValue`] when it is determinate.
    pub fn semantics(&self) -> Semantics {
        self.semantics_with(None)
    }

    fn semantics_with(&self, extra_action: Option<&str>) -> Semantics {
        let mut s = Semantics::new(Role::ListItem)
            .label(enzui_core::localized!("enzui-download-row", "name" => self.name))
            .description(self.description_with(extra_action));
        if let Some(f) = self.download.fraction() {
            s = s.numeric(NumericValue {
                value: f64::from(f),
                min: 0.0,
                max: 1.0,
                step: None,
            });
        }
        s
    }
}

/// The tone a phase's status badge took when rows carried phase chips.
///
/// Rows no longer draw a chip: the phase is a word on the muted status line,
/// and only a failure is coloured ([`DownloadRow::is_failure`]).
#[deprecated(
    since = "0.1.0",
    note = "download rows no longer draw phase chips; the phase is part of `DownloadRow::status_text`"
)]
#[must_use]
pub fn phase_tone(phase: Phase) -> BadgeTone {
    match phase {
        Phase::Queued => BadgeTone::Default,
        Phase::Active => BadgeTone::Accent,
        Phase::Paused => BadgeTone::Warning,
        Phase::Complete => BadgeTone::Success,
        Phase::Failed | Phase::Cancelled => BadgeTone::Error,
    }
}

/// The toolbar icon an action is drawn with.
#[must_use]
pub fn action_icon(action: DownloadAction) -> &'static str {
    match action {
        DownloadAction::Pause => "pause",
        DownloadAction::Resume => "play",
        DownloadAction::Retry => "rotate-cw",
        DownloadAction::Cancel => "x",
    }
}

/// The toolbar icon of the host's optional Show in Folder action.
pub const SHOW_IN_FOLDER_ICON: &str = "folder-open";

fn action_key(action: DownloadAction) -> &'static str {
    match action {
        DownloadAction::Pause => "pause",
        DownloadAction::Resume => "resume",
        DownloadAction::Retry => "retry",
        DownloadAction::Cancel => "cancel",
    }
}

/// Stable widget id of one row's action button: `enzui-download-<id>-<verb>`.
///
/// Stable across rebuilds so keyboard focus stays on a button while its row's
/// progress ticks.
#[must_use]
pub fn action_button_id(id: DownloadId, action: DownloadAction) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("enzui-download-{}-{}", id.0, action_key(action)))
}

/// Stable widget id of one row's Show in Folder button.
#[must_use]
pub fn show_in_folder_button_id(id: DownloadId) -> iced::advanced::widget::Id {
    iced::advanced::widget::Id::from(format!("enzui-download-{}-reveal", id.0))
}

/// Gap between a row's columns — [`SpacingScale::space6`] (12 px), read from
/// the token scale rather than written as a literal.
fn column_gap() -> f32 {
    SpacingScale::default().space6
}

/// Gap between the stacked runs inside a row — [`SpacingScale::space2`] (4 px).
fn stack_gap() -> f32 {
    SpacingScale::default().space2
}

/// Gap between action icons — [`SpacingScale::space2`].
fn action_gap() -> f32 {
    SpacingScale::default().space2
}

/// Gap between rows — [`SpacingScale::space2`].
fn row_gap() -> f32 {
    SpacingScale::default().space2
}

/// A row's inner padding — [`SpacingScale::space4`] (8 px).
fn row_padding() -> f32 {
    SpacingScale::default().space4
}

/// The name run's size — [`TypographyScale::body_size`].
fn name_size() -> f32 {
    TypographyScale::ambient().body_size
}

/// The detail run's size — [`TypographyScale::label_size`].
fn detail_size() -> f32 {
    TypographyScale::ambient().label_size
}

/// The file-type glyph at the start of a row — [`SizingTokens::icon_lg`].
fn file_icon_size() -> f32 {
    SizingTokens::default().icon_lg
}

/// Icon-theme artwork at the start of a row — [`SizingTokens::icon_xl`]:
/// theme art is drawn on a padded canvas, so it needs the larger box to read
/// as large as the glyph. Still within the two-line text column's height.
fn themed_file_icon_size() -> f32 {
    SizingTokens::default().icon_xl
}

/// The progress column's share of a row's width, bounded below and above so a
/// narrow window keeps a readable name and a wide one no runway bar.
const PROGRESS_SHARE: f32 = 0.22;

/// Width of the progress column for a row `row_width` px wide.
///
/// Every row of one list has the same width, so every bar starts at the same
/// x and has the same length. 22% of the row, clamped to 96–200 px
/// (`space15` × 1.5 and × 3.125).
#[must_use]
pub fn progress_column_width(row_width: f32) -> f32 {
    let unit = SpacingScale::default().space15;
    let width = if row_width.is_finite() {
        row_width
    } else {
        0.0
    };
    (width * PROGRESS_SHARE).clamp(unit * 1.5, unit * 3.125)
}

/// Most icon actions a row shows at once: pause or resume, then cancel; or
/// retry; or Show in Folder.
const ACTION_SLOTS: f32 = 2.0;

/// Width of the trailing action column: two toolbar-icon controls and the gap
/// between them. Fixed, so the last control of every row shares one edge and
/// the progress column before it shares one x.
#[must_use]
pub fn actions_column_width() -> f32 {
    let control = TitlebarAffordanceTokens::DEFAULT.effective_control_size();
    control * ACTION_SLOTS + action_gap() * (ACTION_SLOTS - 1.0)
}

/// Row width below which the file icon is dropped so the name keeps room.
fn file_icon_breakpoint() -> f32 {
    SpacingScale::default().space15 * 7.5
}

/// Extra rows realized on each side of the visible range when
/// [`EnzDownloadList::viewport_height`] is set.
///
/// Matches `tree_view.rs`'s default: enough that a fast flick does not expose
/// unrealized space before the next frame, few enough that the realized count
/// stays a small constant.
pub const DEFAULT_OVERSCAN: usize = 4;

/// The laid-out height of one row, used as the virtualization height model.
///
/// A row's content is the taller of its text stack (name over the status line)
/// and its controls (a toolbar-icon hit target, the file icon), inside
/// `row_padding` on both sides, plus the inter-row gap the enclosing column
/// applies. The body is laid out at exactly that content height, so the pitch
/// is exact by construction. Derived from the same token reads the row itself
/// uses rather than written as a literal, so a theme that rescales spacing or
/// typography rescales the scroll geometry with it.
///
/// # A font size is not a line height
///
/// Text occupies a *line box*, not its point size:
/// [`line_box_px`](crate::enz_text::line_box_px) is the framework's single
/// definition of that box (`round(size * LINE_BOX_FACTOR)`, factor 1.35).
/// Summing raw point sizes would under-measure the row and over-realize the
/// virtualized window.
#[must_use]
pub fn row_height() -> f32 {
    row_content_height() + row_padding() * 2.0 + row_gap()
}

fn row_content_height() -> f32 {
    let text = crate::enz_text::line_box_px(name_size())
        + stack_gap()
        + crate::enz_text::line_box_px(detail_size());
    text.max(TitlebarAffordanceTokens::DEFAULT.effective_control_size())
        .max(themed_file_icon_size())
}

/// The row pitch when [`DownloadRow::secondary_detail`] is present.
///
/// The detail rides on the row's single status line, so this is
/// [`row_height`]: every row has one pitch and a uniform [`HeightModel`]
/// describes any list.
#[deprecated(
    since = "0.1.0",
    note = "secondary detail joins the status line; every row is `row_height()` tall"
)]
#[must_use]
pub fn row_height_with_secondary_detail() -> f32 {
    row_height()
}

/// An index-addressable source that projects [`DownloadRow`] descriptors only
/// when the download list asks for a realized row.
///
/// This is the high-cardinality path for retained download history. Unlike an
/// eager `Vec<DownloadRow>`, a provider does not construct one 40-byte borrowed
/// descriptor per tracked download on every frame: [`EnzDownloadList`] first
/// computes the visible window from [`Self::geometry`], then calls [`Self::row`]
/// only for that window. The returned row may borrow from `self`; the widget copies
/// the visible row's paint and accessibility data before it returns its
/// [`Element`].
///
/// `row(index)` must return the row at the same admission-order index for the
/// duration of one build. [`Download::id`] is used as the reconciliation key,
/// so insertions or removals above the window keep retained widget state with
/// the same download rather than with the same slot.
pub trait DownloadRowProvider {
    /// Sequence geometry for the full logical list.
    ///
    /// The contract is exactness, not persistence: for the duration of one
    /// build, this geometry's `len`, offsets, and total height must describe
    /// exactly the rows [`Self::row`] returns. Every row is [`row_height`]
    /// tall, secondary detail or not, so one [`HeightModel`] is exact; a
    /// [`enzui_core::virtual_sequence::VariableHeightModel`] whose entries are
    /// all [`row_height`] is equally valid.
    ///
    /// Retaining or reconciling that model in application state avoids an
    /// O(history) prefix rebuild each frame and is the preferred shape when
    /// that rebuild is material. A transient, *light* geometry rebuilt per
    /// build is allowed only while it stays within the immutable per-row budget
    /// `runtime_perf::download_queue_growth` gates: one `f64` prefix edge per
    /// row (8 B/row) is such a case, whereas rebuilding per-row descriptors or
    /// row content is not. The shipping Files adopter takes the transient
    /// route — it rebuilds exactly this 8-B/row prefix scan every frame and
    /// measures within Gate 2, while its descriptor and `Element` construction
    /// stay O(visible). Its per-frame geometry scan is still O(history), so the
    /// shipping view as a whole is not O(visible); only descriptor/element
    /// construction is.
    fn geometry(&self) -> &dyn SequenceGeometry;

    /// Number of tracked downloads, including offscreen retained history.
    fn len(&self) -> usize {
        self.geometry().len()
    }

    /// Project the row at `index`, or [`None`] when the index is out of range.
    /// This method is called only for the realized viewport plus overscan.
    fn row(&self, index: usize) -> Option<DownloadRow<'_>>;

    /// Whether no downloads are tracked.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

enum DownloadRows<'r> {
    Eager(Vec<DownloadRow<'r>>),
    Provider(&'r dyn DownloadRowProvider),
}

impl DownloadRows<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Eager(rows) => rows.len(),
            Self::Provider(provider) => provider.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn row(&self, index: usize) -> Option<DownloadRow<'_>> {
        match self {
            Self::Eager(rows) => rows.get(index).copied(),
            Self::Provider(provider) => provider.row(index),
        }
    }

    fn offset_of(&self, index: usize) -> f32 {
        match self {
            Self::Eager(rows) => HeightModel::new(rows.len(), row_height()).offset_of(index),
            Self::Provider(provider) => provider.geometry().offset_of(index),
        }
    }

    fn total_height(&self) -> f32 {
        match self {
            Self::Eager(rows) => HeightModel::new(rows.len(), row_height()).total_height(),
            Self::Provider(provider) => provider.geometry().total_height(),
        }
    }
}

/// Default viewport used by [`enz_download_list_provider`] until the caller
/// supplies the measured host height.
///
/// This matches [`crate::virtual_list`]'s default. The provider constructor is
/// windowed by default so forgetting a setter cannot silently restore
/// O(history) descriptor construction.
pub const DEFAULT_VIEWPORT_HEIGHT: f32 = 600.0;

/// A list of downloads with per-row progress, phase, and canonical actions.
///
/// Built via [`enz_download_list`] for a small eager list or
/// [`enz_download_list_provider`] for retained/high-cardinality history.
/// The two lifetimes are deliberately distinct. `'r` is how long the borrowed
/// [`Download`] values or provider live — only until
/// [`EnzDownloadList::into_element`] has read them — while `'a` is the produced
/// element's own lifetime. Every string and number the element holds is cloned
/// out of the model during the build, so the element does not borrow the
/// model and a host can drop its `RefCell` guard the moment the view is built.
pub struct EnzDownloadList<'a, 'r, Message> {
    rows: DownloadRows<'r>,
    on_action: Box<dyn Fn(DownloadId, DownloadAction) -> Message + 'a>,
    empty_text: String,
    /// Measured height of the host's scroll viewport. `None` (the eager
    /// constructor's default) builds every row; `Some(h)` realizes only the
    /// visible + overscan rows. The provider constructor starts at
    /// [`DEFAULT_VIEWPORT_HEIGHT`]. See [`EnzDownloadList::viewport_height`].
    viewport_height: Option<f32>,
    /// Current scroll offset (top of the viewport) in px — an input to the
    /// realization window, fed back by the host.
    scroll_offset: f32,
    /// Extra rows realized on each side of the visible range.
    overscan: usize,
    /// Row-builder invocation probe (`#[doc(hidden)]`, tests only).
    row_builder_probe: Option<Rc<Cell<usize>>>,
    /// Host route for a complete row's Show in Folder action.
    on_show_in_folder: Option<Box<dyn Fn(DownloadId) -> Option<Message> + 'a>>,
    /// Whether rows lead with a file-type icon.
    file_icons: bool,
    /// Writing direction override; `None` follows the locale.
    direction: Option<Direction>,
}

/// Build a download list.
///
/// `on_action` is called with the download's stable [`DownloadId`] and the
/// [`DownloadAction`] the user invoked. It is a factory rather than a
/// pre-built message per button because a list of N rows with 4 actions each
/// would otherwise construct 4N messages per frame, most of which are never
/// used; the factory runs once per real activation.
pub fn enz_download_list<'a, 'r, Message>(
    rows: impl IntoIterator<Item = DownloadRow<'r>>,
    on_action: impl Fn(DownloadId, DownloadAction) -> Message + 'a,
) -> EnzDownloadList<'a, 'r, Message> {
    EnzDownloadList {
        rows: DownloadRows::Eager(rows.into_iter().collect()),
        on_action: Box::new(on_action),
        empty_text: enzui_core::localization::text("enzui-download-list-no-downloads"),
        viewport_height: None,
        scroll_offset: 0.0,
        overscan: DEFAULT_OVERSCAN,
        row_builder_probe: None,
        on_show_in_folder: None,
        file_icons: true,
        direction: None,
    }
}

/// Build a provider-backed download list, windowed by default.
///
/// Use this constructor for a queue that retains offscreen history. The
/// provider itself is borrowed only while [`EnzDownloadList::into_element`]
/// projects the visible window; the returned element owns its row snapshots.
/// Set [`EnzDownloadList::viewport_height`] to the host's measured/fixed
/// viewport and feed scroll position back through
/// [`EnzDownloadList::scroll_offset`], just as for [`crate::virtual_list`].
///
/// The eager [`enz_download_list`] constructor remains the ergonomic path for
/// small lists and the intentional positive control for all-history building.
pub fn enz_download_list_provider<'a, 'r, Message>(
    provider: &'r dyn DownloadRowProvider,
    on_action: impl Fn(DownloadId, DownloadAction) -> Message + 'a,
) -> EnzDownloadList<'a, 'r, Message> {
    EnzDownloadList {
        rows: DownloadRows::Provider(provider),
        on_action: Box::new(on_action),
        empty_text: enzui_core::localization::text("enzui-download-list-no-downloads"),
        viewport_height: Some(DEFAULT_VIEWPORT_HEIGHT),
        scroll_offset: 0.0,
        overscan: DEFAULT_OVERSCAN,
        row_builder_probe: None,
        on_show_in_folder: None,
        file_icons: true,
        direction: None,
    }
}

impl<'a, 'r, Message> EnzDownloadList<'a, 'r, Message> {
    /// Offer a Show in Folder icon action on complete rows.
    ///
    /// Called once per realized complete row while the view is built; return
    /// the message that reveals that download's file, or [`None`] when there
    /// is nothing to show (the file was moved, or the row has no destination).
    /// The action is named in the row's accessible description beside the
    /// model's actions. Without this route no row draws it.
    #[must_use]
    pub fn on_show_in_folder(
        mut self,
        on_show_in_folder: impl Fn(DownloadId) -> Option<Message> + 'a,
    ) -> Self {
        self.on_show_in_folder = Some(Box::new(on_show_in_folder));
        self
    }

    /// Whether rows lead with a file-type icon derived from the name
    /// (default `true`). Rows narrower than 480 px drop it to keep the name
    /// readable.
    #[must_use]
    pub fn file_icons(mut self, show: bool) -> Self {
        self.file_icons = show;
        self
    }

    /// Override the writing direction. By default the list follows the locale;
    /// right-to-left puts the icon on the right and the actions on the left.
    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Override the text shown when the list is empty.
    #[must_use]
    pub fn empty_text(mut self, text: impl Into<String>) -> Self {
        self.empty_text = text.into();
        self
    }

    /// The number of rows this list will draw.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the list has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The measured height of the host's scroll viewport, in px.
    ///
    /// This is the ON switch for viewport realization. Setting it
    /// makes the view build an `Element` only for the visible +
    /// [`overscan`](Self::overscan) rows. The eager constructor leaves it unset
    /// for small-list compatibility; the provider constructor is windowed by
    /// default so retained history cannot become eager through omission.
    ///
    /// Why it matters here specifically: a download manager retains completed
    /// transfers as history (up to `DEFAULT_HISTORY_LIMIT` = 1,000), and the
    /// eager path builds a full row — an icon, two text runs, a progress bar
    /// and its icon buttons — for every one of them on every frame,
    /// while a progress tick arrives several times a second. Wrapping the
    /// result in `scrollable` clips those rows at paint time but does not stop
    /// them being constructed, so per-frame cost tracks session length rather
    /// than what is on screen.
    ///
    /// Sanitized: `NaN`, `±inf`, negative **and `0.0`** all fall back to eager
    /// rather than realizing nothing but padding, which would render an empty
    /// list; an empty visible window is invalid. A caller who genuinely wants
    /// no rows passes no rows.
    ///
    /// Pair it with [`scroll_offset`](Self::scroll_offset); without the
    /// feedback loop the window stays pinned at offset `0.0` and scrolling
    /// reveals blank space instead of the later rows.
    #[must_use]
    pub fn viewport_height(mut self, height: f32) -> Self {
        self.viewport_height = if height.is_finite() && height > 0.0 {
            Some(height)
        } else {
            None
        };
        self
    }

    /// The current scroll offset (top of the viewport) in px. Sanitized:
    /// non-finite and negative degrade to `0.0`, which is expressible intent
    /// (the top of the content) and is honoured literally.
    ///
    /// Ignored unless [`viewport_height`](Self::viewport_height) is set.
    #[must_use]
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = enzui_core::metrics::FiniteNonNeg::new_or(offset, 0.0).get();
        self
    }

    /// Extra rows realized on each side of the visible range (default
    /// [`DEFAULT_OVERSCAN`]). `0` is honoured literally, per
    /// [`RealizationWindow`].
    ///
    /// Ignored unless [`viewport_height`](Self::viewport_height) is set.
    #[must_use]
    pub fn overscan(mut self, n: usize) -> Self {
        self.overscan = n;
        self
    }

    /// The half-open range of row indices this list will materialize as
    /// `Element`s for the current configuration.
    ///
    /// Eager (`viewport_height` unset) is `0..len`. Virtualized is the visible
    /// range plus overscan, computed by the shared
    /// [`RealizationWindow`] primitive rather than by a second copy of the
    /// arithmetic.
    ///
    /// Public because it is the *same* function the benchmark and the route
    /// consult: a gate that recomputed the window itself would be measuring
    /// its own arithmetic rather than the shipped one.
    #[must_use]
    pub fn realized_range(&self) -> std::ops::Range<usize> {
        match (self.viewport_height, &self.rows) {
            (Some(vp_h), DownloadRows::Eager(rows)) => {
                let model = HeightModel::new(rows.len(), row_height());
                let anchor = Anchor::from_scroll_top(&model, self.scroll_offset);
                RealizationWindow::new(self.overscan).realize(&model, &anchor, vp_h)
            }
            (Some(vp_h), DownloadRows::Provider(provider)) => {
                let geometry = provider.geometry();
                let anchor = Anchor::from_scroll_top(geometry, self.scroll_offset);
                RealizationWindow::new(self.overscan).realize(geometry, &anchor, vp_h)
            }
            (None, _) => 0..self.rows.len(),
        }
    }

    /// Install a row-builder invocation probe (`#[doc(hidden)]`).
    ///
    /// Bumped exactly once per row this view materializes as an `Element` —
    /// the mechanism that makes "only the visible rows are built" an assertable
    /// number rather than a code-reading exercise. A window that realizes
    /// everything is indistinguishable from one that does not by any
    /// output-shape assertion; it is not indistinguishable by this counter.
    #[doc(hidden)]
    #[must_use]
    pub fn row_builder_probe(mut self, probe: Rc<Cell<usize>>) -> Self {
        self.row_builder_probe = Some(probe);
        self
    }
}

/// Byte state for a download row's accessible description.
///
/// [`crate::file_browser::format_bytes`] deliberately uses compact SI units,
/// which are ideal for values but do not name the dimension in prose (`0 B`,
/// `250 kB`). Prefixing the complete state with `bytes:` keeps both known-total
/// and no-total rows unambiguous when spoken, including the especially
/// important `indeterminate · bytes: 0 B` retry state. The painted status line
/// ([`DownloadRow::status_text`]) shows the same sizes without the prefix.
fn byte_state_text(download: &Download) -> String {
    let value = match download.total() {
        Some(total) => {
            enzui_core::localized!("enzui-download-list-of", "received" => crate::file_browser::format_bytes(download.received), "total" => crate::file_browser::format_bytes(total))
        }
        None => crate::file_browser::format_bytes(download.received),
    };
    enzui_core::localized!("enzui-download-list-bytes", "value" => value)
}

/// One row's progress cell, or [`None`] once the transfer is over.
///
/// * complete, failed, cancelled: no bar, so the text may use the space;
/// * a known fraction: a bar, accent with the busy sweep while active and a
///   quiet fill (`text_subtle`) while paused or queued;
/// * unknown length: a spinner, turning only while the transfer is active, or
///   an empty quiet track for queued work that has received nothing yet.
fn progress_cell<'a, Message: 'a>(b: &RowBuild<Message>) -> Option<Element<'a, Message>> {
    if b.phase.is_terminal() {
        return None;
    }
    let active = b.phase == Phase::Active;
    let bar = |fraction: f32| {
        let bar = enz_progress_bar(fraction).busy(active);
        if active {
            bar
        } else {
            bar.fill_role(|palette| palette.text_subtle)
        }
        .into_element()
    };
    Some(match b.fraction {
        Some(fraction) => bar(fraction),
        None if b.phase == Phase::Queued && !b.received_any => bar(0.0),
        None => enz_spinner()
            .size(SizingTokens::default().icon_sm)
            .active(active)
            .into_element(),
    })
}

/// An owned, per-layout-pass snapshot of everything one row draws.
///
/// `iced::widget::responsive` re-invokes its closure on every layout
/// pass (see `iced/widget/src/responsive.rs`), so the closure cannot
/// borrow the model or consume a one-shot: it must rebuild the row from owned
/// inputs each time. This struct is that snapshot — taken once per row while
/// the model is still borrowed, then moved into the closure.
struct RowBuild<Message> {
    name: String,
    status: String,
    failure: bool,
    file_icon: Option<crate::icon::IconSource>,
    phase: Phase,
    fraction: Option<f32>,
    received_any: bool,
    actions: Vec<DownloadAction>,
    /// The host's Show in Folder message and its label, for complete rows.
    reveal: Option<(Message, String)>,
    id: DownloadId,
    rtl: bool,
}

type ActionFactory<'a, Message> = Rc<Box<dyn Fn(DownloadId, DownloadAction) -> Message + 'a>>;

fn action_buttons<'a, Message: Clone + 'a>(
    b: &RowBuild<Message>,
    on_action: &ActionFactory<'a, Message>,
) -> iced::widget::Row<'a, Message> {
    let id = b.id;
    let mut buttons = iced::widget::Row::new()
        .spacing(action_gap())
        .align_y(Alignment::Center);
    for &action in &b.actions {
        let f = Rc::clone(on_action);
        let label = action.label();
        // Built lazily: the message exists only when the button is activated.
        let button =
            enz_toolbar_icon_maybe_with(action_icon(action), Some(Box::new(move || f(id, action))))
                .id(action_button_id(id, action))
                .aria_label(label.clone())
                .tooltip(label);
        buttons = buttons.push(button.into_element());
    }
    if let Some((message, label)) = &b.reveal {
        buttons = buttons.push(
            enz_toolbar_icon(SHOW_IN_FOLDER_ICON, message.clone())
                .id(show_in_folder_button_id(id))
                .aria_label(label.clone())
                .tooltip(label.clone())
                .into_element(),
        );
    }
    buttons.mirrored(b.rtl)
}

fn text_column<'a, Message: 'a>(b: &RowBuild<Message>) -> iced::widget::Column<'a, Message> {
    let tone = if b.failure {
        TextTone::Danger
    } else {
        TextTone::Muted
    };
    // Single elided lines: string length cannot change the row pitch.
    column![
        enz_text(b.name.clone())
            .size_px(name_size())
            .single_line_elided()
            .full_width()
            .into_element(),
        enz_text(b.status.clone())
            .size_px(detail_size())
            .tone(tone)
            .single_line_elided()
            .full_width()
            .into_element(),
    ]
    .spacing(stack_gap())
    .width(Length::Fill)
}

// One grid for every row: [icon] [text, Fill] [progress, fixed] [actions, fixed].
// A finished row has no progress cell, so its text runs on into that column
// while the actions keep their edge.
fn row_body<'a, Message: Clone + 'a>(
    b: &RowBuild<Message>,
    on_action: &ActionFactory<'a, Message>,
    width: f32,
) -> Element<'a, Message> {
    let mut cells: Vec<Element<'a, Message>> = Vec::with_capacity(4);
    if let Some(icon) = &b.file_icon
        && width >= file_icon_breakpoint()
    {
        let size = if icon.is_app_owned() {
            themed_file_icon_size()
        } else {
            file_icon_size()
        };
        cells.push(
            enz_icon_slot("")
                .icon_source(icon.clone())
                .icon_size(size)
                .slot_width(size)
                .into_element(),
        );
    }
    cells.push(text_column::<Message>(b).into());
    if let Some(cell) = progress_cell::<Message>(b) {
        cells.push(
            container(cell)
                .width(Length::Fixed(progress_column_width(width)))
                .align_y(Alignment::Center)
                .into(),
        );
    }
    // Trailing-aligned, so the last control of every row shares one edge.
    cells.push(
        container(action_buttons(b, on_action))
            .width(Length::Fixed(actions_column_width()))
            .align_x(if b.rtl {
                Alignment::Start
            } else {
                Alignment::End
            })
            .align_y(Alignment::Center)
            .into(),
    );
    iced::widget::Row::with_children(cells)
        .mirrored(b.rtl)
        .spacing(column_gap())
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .height(Length::Fixed(row_content_height()))
        .into()
}

impl<'a, Message: Clone + 'a> EnzDownloadList<'a, '_, Message> {
    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        // which rows become an `Element` this frame. Computed before the
        // destructure so it reads the shared `realized_range` the route and the
        // benchmark also call, rather than a second copy of the arithmetic.
        let realized = self.realized_range();
        let virtualized = self.viewport_height.is_some();

        let Self {
            rows,
            on_action,
            empty_text,
            row_builder_probe,
            on_show_in_folder,
            file_icons,
            direction,
            ..
        } = self;

        if rows.is_empty() {
            return SemanticsContainer::leaf(
                container(
                    enz_text(empty_text.clone())
                        .size_px(detail_size())
                        .tone(TextTone::Muted)
                        .into_element(),
                )
                .padding(row_padding())
                .width(Length::Fill),
                Semantics::new(Role::Group).label(empty_text),
            )
            .into();
        }

        let rtl = enzui_core::placement::resolve_direction(direction) == Direction::RightToLeft;
        let reveal_label = on_show_in_folder
            .as_ref()
            .map(|_| enzui_core::localization::text("enzui-download-show-in-folder"));
        let on_action = Rc::new(on_action);
        let mut keyed_rows = Vec::with_capacity(realized.len());

        // Ask the source only for the realized indices. `rows` itself still
        // reports the full logical count to the group node below, so windowing
        // changes what is PROJECTED and BUILT, never what is REPORTED.
        for index in realized.clone() {
            let Some(r) = rows.row(index) else {
                // Providers are contractually total for `0..len`. Failing
                // closed keeps a broken provider from panicking the entire UI;
                // the construction-count tests still expose the missing row.
                continue;
            };
            if let Some(p) = &row_builder_probe {
                p.set(p.get() + 1);
            }
            let id = r.download.id;
            let phase = r.download.phase;

            // The host's reveal action, offered only once the file exists.
            let reveal = match (&on_show_in_folder, &reveal_label) {
                (Some(route), Some(label)) if phase == Phase::Complete => {
                    route(id).map(|message| (message, label.clone()))
                }
                _ => None,
            };

            // Everything the row draws, snapshotted into owned/`Copy` form so
            // the `responsive` closure below can rebuild the body from scratch
            // on every layout pass without borrowing the model. `responsive`
            // re-invokes its closure each pass, so the closure must own its
            // inputs and reconstruct — never consume — them (the one-shot-
            // resource trap `responsive` is known for): the progress bar and
            // buttons are all rebuilt here.
            let build = RowBuild {
                name: r.name.to_owned(),
                status: r.status_text(),
                failure: r.is_failure(),
                // Resolved once per type and size, then a cache lookup.
                file_icon: file_icons.then(|| {
                    crate::file_icon::file_icon_source(
                        &crate::file_icon::FileIconQuery::file_name(r.name),
                        themed_file_icon_size(),
                    )
                }),
                phase,
                fraction: r.download.fraction(),
                received_any: r.download.received > 0,
                actions: r.download.actions(),
                reveal: reveal.clone(),
                id,
                rtl,
            };
            let semantics = r.semantics_with(reveal.as_ref().map(|(_, label)| label.as_str()));
            let on_action = Rc::clone(&on_action);

            // `responsive` hands the closure the width the row will actually
            // occupy this pass: the progress column's width is a share of it,
            // identical for every row of the list, and a narrow row drops the
            // file icon (see `row_body`).
            // `.height(Shrink)`: a `Responsive` defaults to `height: Fill`
            // (`iced/widget/src/responsive.rs`), which inside a
            // `column` would make one row swallow all the vertical space.
            // Shrink makes it hug the body's fixed content height.
            let body = crate::enz_responsive::enz_responsive(move |size| {
                row_body::<Message>(&build, &on_action, size.width)
            })
            .height(Length::Shrink);

            // `new`, not `leaf`: the row's own node states the name, phase,
            // percentage and actions, and the icon buttons beneath it publish
            // their own named, actionable nodes.
            let row: Element<'a, Message> = container(SemanticsContainer::new(body, semantics))
                .padding(row_padding())
                .into();
            keyed_rows.push((format!("download::{}", id.0), row));
        }
        // `DownloadQueue` ids are unique. The eager convenience constructor is
        // also used for tiny presentation-only fixtures that may intentionally
        // repeat one model row; tolerate those duplicates, which degrade only
        // the duplicate run to positional reconciliation.
        // `.width(Length::Fill)` is load-bearing, not cosmetic. `keyed_column`
        // defaults to `Length::Shrink` (matching `iced::widget::Column::new`),
        // but a `Shrink` cross axis starves each row's `Fill`-width
        // `enz_responsive` body to zero available width, so every row would
        // get the minimum progress column, lose its file icon and collapse its
        // name column at any host size. `iced::widget::column!` avoids this only because its
        // own default is `Length::Fit`, which offers fluid children the full
        // cross extent; asking for `Fill` here reproduces that geometry for the
        // eager small-list path and matches the `container(..).width(Fill)`
        // wrapper the virtualized branch already applies.
        let list = keyed_column(keyed_rows)
            .allow_duplicate_keys()
            .width(Length::Fill)
            .spacing(row_gap());

        // Unrealized extent becomes padding, not spacer siblings: padding is
        // not a child, so a host walking this subtree cannot mistake a spacer
        // rect for a row. Leading + rows + trailing sums to the provider's
        // geometry total (or `len * row_height` for eager rows), so the scroll
        // range and every row's document-space offset remain stable.
        let body: Element<'a, Message> = if virtualized {
            let leading = rows.offset_of(realized.start);
            let realized_h = rows.offset_of(realized.end) - leading;
            let trailing = (rows.total_height() - leading - realized_h).max(0.0);
            container(list)
                .width(Length::Fill)
                .padding(iced::Padding {
                    top: leading,
                    right: 0.0,
                    bottom: trailing,
                    left: 0.0,
                })
                .into()
        } else {
            list.into()
        };

        // The group node counts every TRACKED download, not the realized ones:
        // an AT user must be told the list holds 1,024 transfers even when
        // twelve are built, or virtualization would silently shrink the list
        // as far as assistive tech is concerned.
        SemanticsContainer::new(
            body,
            Semantics::new(Role::Group)
                .label(enzui_core::localization::text(
                    "enzui-download-list-downloads",
                ))
                .description(enzui_core::localized!("enzui-download-count", "count" => rows.len())),
        )
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzDownloadList<'a, '_, Message>> for Element<'a, Message> {
    fn from(list: EnzDownloadList<'a, '_, Message>) -> Self {
        list.into_element()
    }
}
