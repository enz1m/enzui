#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(missing_docs)]
//! Renderer-neutral models, policies, and runtime adapters shared by enzUI applications.
#![doc = include_str!("capability_index.md")]

#[cfg(feature = "accesskit")]
pub mod a11y_action;
pub mod accessibility_scan;
pub mod action;
pub mod action_overflow;
pub mod activation;
pub mod annotation;
pub mod app_runtime;
// `$XDG_DATA_HOME` stores durable app data and is distinct from serialized persistence tiers.
pub mod app_data;
pub mod app_identity;
pub mod async_image;
pub mod atomic_state;
pub mod autosave;
pub mod background_io;
pub mod blocking_task;
pub mod calendar_nav;
pub mod calendar_semantics;
pub mod desktop_settings;
pub mod layout_spill_scan;
/// Re-exports the AccessKit version used by the accessibility API.
#[cfg(feature = "accesskit")]
pub use accesskit;
pub use enzui_security::cancellation;
pub use enzui_security::quarantine;

pub mod capture_session;
pub mod chart;
pub mod chat_stream;
pub mod chunked_text;
pub mod civil_date;
pub mod click;
pub mod close_guard;
pub mod collect;
pub mod confirm;
pub mod context_menu;
pub mod crash_report;
pub mod date_selection;
// Relocation is Unix-only because it relies on link, unlink, directory-fsync, and inode identity.
#[cfg(unix)]
pub mod db_migration;
pub use enzui_security::decode_limits;
pub mod diagnostics_host;
pub mod diagnostics_sink;
pub mod dismissal;
pub mod document_tabs;
pub mod download;
pub mod download_destination;
pub mod drag;
pub mod drag_autoscroll;
pub mod editable_shortcuts;
pub mod editable_text_budget;
pub mod editable_text_viewport;
pub mod file_browser;
pub mod file_tree_watch;
pub mod file_watch;
pub mod focus;
pub mod focus_after_removal;
pub mod focus_traversal;
pub mod glyph_selection;
// The user's UI/prose/code family choices, a different capability from the
// renderer's installed-font discovery.
pub mod font_preferences;
pub mod form;
pub mod frame_plan;
pub mod grapheme_cell;
pub mod grid_viewport;
pub mod virtual_text;
#[cfg(feature = "rust-source-audit")]
pub use enzui_test_support::hand_roll_scan;
#[cfg(feature = "rust-source-audit")]
pub use enzui_test_support::rust_syntax;
pub mod heading_outline;
pub mod identity;
pub mod image_crop;
pub mod image_space;
pub mod image_transform;
pub mod io_class;
pub mod keyboard_locks;
#[cfg(feature = "test-frame")]
pub mod keyboard_traversal_validation;
#[cfg(feature = "shell-integration")]
pub mod launcher_badge;
pub mod list_selection;
/// Named live-region roles and politeness policies.
pub mod live_region;
pub mod local_time;
pub mod localization;
pub mod logging;
pub mod marquee;
pub mod material;
pub mod material_preferences;
pub mod media;
pub mod menu_bar;
pub mod menu_model;
pub mod menu_stack;
pub mod metrics;
pub mod mnemonic;
pub mod modal;
pub mod monotonic_ms;
pub mod native_child;
#[cfg(target_os = "linux")]
pub mod native_loop;
pub mod notification_inbox;
pub mod operation_activity;
pub mod overlay_layer;
pub mod packaging;
pub mod paged_bytes;
pub mod paged_text;
pub mod paging;
pub mod palette;
pub mod pen_input;
#[cfg(feature = "perf-counter")]
pub mod perf_frame;
#[cfg(feature = "perf-report")]
pub mod perf_report;
pub mod persist;
pub mod placement;
pub mod portal;
pub mod recovery;
pub use enzui_security::random;
/// Runs a widget operation and all chained operations using the test runtime.
#[cfg(feature = "test-frame")]
pub use enzui_test_support::chained_operation;
#[cfg(feature = "test-frame")]
pub mod reducer_task;
pub mod registered_overlap_scan;
pub mod rounded_geometry;
pub mod route;
pub use enzui_security::runtime_dir;
pub mod schedule;
pub mod scroll_chrome;
#[doc(hidden)]
pub mod scroll_stack;
pub use enzui_security::secure_dir;
pub use enzui_security::tempstage;
pub mod drop_files;
pub mod semantics;
pub mod sheet;
pub mod shortcut;
pub mod sound_theme;
pub mod spelling;
pub mod state_authority;
pub mod store_watch;
#[cfg(feature = "portal-dbus")]
pub mod system_sleep;
#[cfg(feature = "portal-dbus")]
pub mod system_status;
pub mod table;
pub mod text_fold;
pub mod text_selection;
pub mod theme_file;
#[cfg(feature = "thumbnail-cache")]
pub mod thumbnail_cache;
pub mod time_of_day;
pub mod user_file;
// Measurement tooling: behind `transition-capture` (and `test-frame`), so an
// ordinary application build does not carry it.
#[cfg(feature = "transition-capture")]
pub mod transition_capture;
#[cfg(feature = "shell-integration")]
pub mod tray;
pub mod tree_nav;
pub mod type_ahead;
pub mod undo;
pub use enzui_security::url_policy;
pub mod velocity;
pub mod virtual_grid;
pub mod virtual_sequence;
pub mod widget_runtime;
pub mod window_lifecycle;
pub mod window_placement;
pub mod wizard;
pub mod worker_bridge;
pub mod zoom_levels;
pub mod zoom_pan;

pub use action::{
    Action, ShortcutBinding, ShortcutHelpEntry, action_for_event, action_for_key,
    dispatch_shortcut, dispatch_shortcut_command, shortcut_binding_for_event, shortcut_help,
};
pub use editable_shortcuts::{
    EditableShortcutCatalog, EditableShortcutRow, RebindRefusal, ShortcutOverrides,
    ShortcutValidity,
};
pub use enzui_icons::Icon;
pub use focus::{
    FocusState, Nav, Traversal, activation_from_key, focus_next_wrapping, focus_previous_wrapping,
    navigation_from_key, traversal_from_key, traversal_operation, wrap_target,
};
pub use identity::{KeyedIds, derive_id};
pub use semantics::{NumericValue, Orientation, Role, Semantics, Toggle};
pub use shortcut::{Modifier, NamedKey, Shortcut, ShortcutKey, debug_name_is, named_key_label};
