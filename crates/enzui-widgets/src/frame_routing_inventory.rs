//! Authoritative ownership classification for production `Widget` implementations.
//!
//! This inventory is intentionally source-level and exhaustive. The integration
//! gate derives the production implementation population independently and
//! compares it with these rows, so adding a widget without choosing an ownership
//! shape is a loud failure. `Excluded` rows state why targeted single-child frame
//! forwarding is not their contract.

/// The tree-ownership shape relevant to targeted frame delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// Shares the child's `Tree` node and forwards directly.
    Transparent,
    /// Owns several alternatives but dispatches only the laid-out active child.
    ActiveChild,
    /// Owns the child tree/cache and must refresh it after normal and frame dispatch.
    OwnedChildCache,
    /// Owns frame work in addition to any descendant route.
    OwnAnimation(&'static str),
    /// Has no descendant widget subtree.
    Leaf,
    /// In-flow and overlay children have separate dispatch ownership.
    OverlayOwned,
    /// Not a total single-child/pass-through wrapper, for the stated reason.
    Excluded(&'static str),
}

/// Every production custom `Widget` in the source tree, keyed by the same
/// `module::Type` identity as the independent widget-population scanner.
#[doc(hidden)]
pub const INVENTORY: &[(&str, Shape)] = &[
    ("action_bar::ActionBarWidget", Shape::OwnedChildCache),
    (
        "adaptive_row::EnzAdaptiveRowWidget",
        Shape::Excluded("multi-child responsive layout owner"),
    ),
    ("anchored_overlay::AnchoredOverlay", Shape::OverlayOwned),
    ("annotation_overlay::EnzAnnotationOverlay", Shape::Leaf),
    (
        "app_shell::AppShellWidget",
        Shape::OwnAnimation("owns sidebar presentation animation and targeted content routes"),
    ),
    (
        "app_shell::LiveSeamPad",
        Shape::Excluded("shell-private geometry adapter covered by AppShell"),
    ),
    (
        "app_shell::MirrorRow",
        Shape::Excluded(
            "multi-child pane row; routes frames to interested children only and \
             prunes its own route list",
        ),
    ),
    (
        "app_shell::RetainedResponsiveWidth",
        Shape::Excluded("shell-private geometry adapter covered by AppShell"),
    ),
    (
        "app_shell::SeamRider",
        Shape::OwnAnimation(
            "owns a retained scene node and publishes the panel's travel curve \
             so shell chrome (hairline, stable toggle) tracks the sidebar clip \
             on every composited frame",
        ),
    ),
    (
        "activity_pulse::EnzActivityPulse",
        Shape::OwnAnimation("owes one frame for the trailing activity pulse"),
    ),
    ("aspect_ratio::EnzAspectRatio", Shape::Transparent),
    (
        "accessibility_scan_widgets::ContrastScanProbe",
        Shape::Excluded("audit-only operation/paint probe"),
    ),
    (
        "accessibility_scan_widgets::ElisionScanProbe",
        Shape::Transparent,
    ),
    (
        "accessibility_scan_widgets::HitTargetScanProbe",
        Shape::Transparent,
    ),
    (
        "accessibility_scan_widgets::MissingNameScanProbe",
        Shape::Transparent,
    ),
    (
        "autocomplete_field::AutocompleteWidget",
        Shape::Excluded("composite field and popup owner"),
    ),
    ("banner::BannerResponsive", Shape::ActiveChild),
    ("cached_subtree::EnzCachedSubtree", Shape::OwnedChildCache),
    ("calendar_spans::CalendarSpanLayer", Shape::Leaf),
    ("context_menu_host::MenuHeader", Shape::Transparent),
    (
        "image_cropper::EnzImageCropper",
        Shape::Excluded(
            "owns its image viewer as a private child; keeps the default frame \
             interest, so it stays on every frame route",
        ),
    ),
    ("layout_memo::LayoutMemo", Shape::OwnedChildCache),
    (
        "layout_spill_probe::LayoutSpillProbe",
        Shape::Excluded("audit-only operation probe"),
    ),
    (
        "capped_dialog_body::CappedDialogBody",
        Shape::OwnedChildCache,
    ),
    (
        "carousel::CarouselInput",
        Shape::Excluded("multi-child carousel interaction owner"),
    ),
    (
        "center_clamp_row::CenterClampRow",
        Shape::Excluded("multi-child geometry container"),
    ),
    (
        "chart_motion::ChartMotion",
        Shape::OwnAnimation("owns chart transition timing"),
    ),
    (
        "chart_tooltip::AlignedTooltipRows",
        Shape::Excluded("multi-row geometry container"),
    ),
    (
        "code_block::BodySemantics",
        Shape::Excluded("private fixed semantics adapter"),
    ),
    (
        "code_block::CodeBlockPulse",
        Shape::OwnAnimation("owns code-block pulse timing"),
    ),
    ("combo_box::ComboBoxWidget", Shape::OverlayOwned),
    (
        "command_palette::PaletteKeys",
        Shape::Excluded("private keyboard event owner"),
    ),
    ("context_menu_host::EnzContextMenuHost", Shape::OverlayOwned),
    ("context_target::EnzContextTarget", Shape::OwnedChildCache),
    (
        "date_picker::DismissOnOutsidePress",
        Shape::Excluded("private popup dismissal interceptor"),
    ),
    (
        "date_picker::SurfacePressShield",
        Shape::Excluded("private popup press interceptor"),
    ),
    ("decorative::Decorative", Shape::Transparent),
    ("desktop_glass::EnzDesktopGlass", Shape::OwnedChildCache),
    (
        "diagnostics_overlay::DiagnosticsOverlay",
        Shape::OverlayOwned,
    ),
    ("divider::FadedRule", Shape::Leaf),
    ("document_tab::TabHeightFill", Shape::Transparent),
    ("document_tab::TabMouseCapture", Shape::OwnedChildCache),
    ("document_tab::TitleReserveCap", Shape::Transparent),
    ("document_tab::TitleWidthPin", Shape::Transparent),
    (
        "drag_autoscroll_host::EnzDragAutoScrollHost",
        Shape::OwnAnimation("owns drag-edge autoscroll timing"),
    ),
    (
        "drag_drop_host::EnzDragDropHost",
        Shape::Excluded("drag registry and multi-target event owner"),
    ),
    (
        "drag_item_surface::EnzDragItemSurface",
        Shape::Excluded("drag source event/state owner"),
    ),
    (
        "editor_scrollbar::EnzEditorScrollbar",
        Shape::OwnedChildCache,
    ),
    (
        "elevation::ElevationAnim",
        Shape::OwnAnimation("owns elevation interpolation and forwards child routes"),
    ),
    ("progress_text::InkWave", Shape::Transparent),
    ("responsive::EnzResponsive", Shape::OwnedChildCache),
    (
        "table::FoldRow",
        Shape::Excluded("multi-child responsive table layout owner"),
    ),
    (
        "table::RowHover",
        Shape::Excluded("table-private hover adapter"),
    ),
    (
        "table::TableScrollbar",
        Shape::Excluded("table-private scrollbar interaction owner"),
    ),
    ("text::SuppressAnnounce", Shape::Transparent),
    ("exclusive_focus::ExclusiveFocus", Shape::Transparent),
    ("focus_claim::FocusClaimHost", Shape::Transparent),
    (
        "expander::BodyGate",
        Shape::Excluded("expander-private visibility gate"),
    ),
    (
        "expander::ExpanderAnim",
        Shape::OwnAnimation("owns disclosure expansion timing"),
    ),
    ("field_chrome::FieldChrome", Shape::OwnedChildCache),
    (
        "file_drop::EnzFileDropTarget",
        Shape::OwnAnimation("owes one frame to publish files the drag service handed it"),
    ),
    (
        "focus_button::FocusButton",
        Shape::Excluded("control with own activation/focus semantics"),
    ),
    ("focus_retention::FocusRetention", Shape::OwnedChildCache),
    (
        "frame_clock::EnzFrameClock",
        Shape::OwnAnimation("is the explicit caller-owned frame clock"),
    ),
    ("glow::EnzGlow", Shape::OwnedChildCache),
    ("glyph_selection::EnzGlyphSelection", Shape::OwnedChildCache),
    ("grid::EnzGrid", Shape::Excluded("multi-child container")),
    (
        "grid::KeyedGrid",
        Shape::Excluded("multi-child keyed container"),
    ),
    ("hover_control::HoverControl", Shape::Leaf),
    ("hover_group::EnzHoverGroup", Shape::OwnedChildCache),
    ("hug_width::HugToPreferred", Shape::OwnedChildCache),
    ("image_viewer::EnzImageViewer", Shape::Leaf),
    ("inert::Inert", Shape::Transparent),
    ("inert_clip::InertClip", Shape::Transparent),
    (
        "input_otp::OtpChrome",
        Shape::Excluded("private fixed OTP chrome adapter"),
    ),
    ("key_capture_chip::EnzKeyCaptureChip", Shape::Leaf),
    ("keyboard_capture::RovingGrid", Shape::OwnedChildCache),
    (
        "keyed_column::KeyedColumn",
        Shape::Excluded(
            "multi-child keyed container; routes frames to interested children \
             only and prunes its own route list",
        ),
    ),
    (
        "keyed_row::KeyedRow",
        Shape::Excluded("multi-child keyed container"),
    ),
    (
        "marquee_area::EnzMarqueeArea",
        Shape::OwnAnimation("owns marquee scroll timing"),
    ),
    ("max_height::MaxHeight", Shape::Transparent),
    ("max_width::MaxWidth", Shape::Transparent),
    (
        "menu::ActiveReveal",
        Shape::Excluded("private menu scroll-into-view owner with its own state"),
    ),
    (
        "menu::HoverReporter",
        Shape::Excluded("private menu hover interceptor"),
    ),
    (
        "menu::MenuRovingWidget",
        Shape::Excluded("private menu keyboard/focus owner"),
    ),
    ("menu::RowHighlight", Shape::Transparent),
    (
        "menu_button::DismissOnOutsidePress",
        Shape::Excluded("private overlay dismissal interceptor"),
    ),
    ("min_height::MinHeight", Shape::Transparent),
    ("list_row_surface::OuterGutter", Shape::Transparent),
    ("min_width::MinWidth", Shape::Transparent),
    ("modal_host::EscapeCapture", Shape::Transparent),
    ("modal_host::WithdrawFocus", Shape::OwnedChildCache),
    (
        "motion::CrossFade",
        Shape::OwnAnimation("owns cross-fade opacity timing"),
    ),
    (
        "motion::Motion",
        Shape::OwnAnimation("owns generic transition timing"),
    ),
    ("motion::PanelReveal", Shape::OwnedChildCache),
    (
        "navigation_outlet::BackKeyCapture",
        Shape::Excluded("navigation keyboard owner"),
    ),
    (
        "navigation_outlet::BackSwipeCapture",
        Shape::Excluded("navigation gesture owner"),
    ),
    (
        "navigation_outlet::Slide",
        Shape::OwnAnimation("owns navigation slide timing"),
    ),
    (
        "navigation_split_view::NavigationSplitViewWidget",
        Shape::Excluded("multi-child navigation layout owner"),
    ),
    (
        "offset_scrollbar::EnzOffsetScrollbar",
        Shape::Excluded("scrollbar interaction and offset owner"),
    ),
    (
        "overflow_row::OverflowRowWidget",
        Shape::Excluded("multi-child overflow layout owner"),
    ),
    (
        "pane_grid::PaneGridSemantics",
        Shape::Excluded("private fixed pane semantics adapter"),
    ),
    (
        "positioned_overlay::EnzPositionedOverlay",
        Shape::OverlayOwned,
    ),
    (
        "preferences_row::PreferencesResponsive",
        Shape::Excluded("two-child responsive geometry owner"),
    ),
    ("preferences_row::SwitchPress", Shape::Transparent),
    (
        "radio_group::RadioGroupWidget",
        Shape::Excluded("group selection and keyboard owner"),
    ),
    (
        "reflow_pair::ReflowPair",
        Shape::Excluded("two-child reflow geometry owner"),
    ),
    (
        "reorder_list::EnzReorderList",
        Shape::Excluded("multi-child reorder interaction owner"),
    ),
    ("responsive_switch::ResponsiveSwitch", Shape::ActiveChild),
    ("revision_text::RevisionText", Shape::Leaf),
    (
        "rich_text::LinkSemantics",
        Shape::Excluded("private fixed link event/semantics adapter"),
    ),
    (
        "rich_text::ProseSemantics",
        Shape::Excluded("private fixed prose semantics adapter"),
    ),
    ("rounded_clip::RoundedClip", Shape::OwnedChildCache),
    (
        "row_measure::EnzRowMeasure",
        Shape::Excluded("measurement observer with fixed child contract"),
    ),
    (
        "row_motion::RowMotion",
        Shape::OwnAnimation("owns row insertion and removal timing"),
    ),
    ("schedule::EnzSchedule", Shape::Leaf),
    ("scrim_click::ScrimClick", Shape::OwnedChildCache),
    (
        "scroll_view::KeyScrollForward",
        Shape::Excluded("private scroll keyboard adapter"),
    ),
    (
        "scroll_view::KeyboardScroll",
        Shape::Excluded("private scroll keyboard owner"),
    ),
    (
        "scroll_view::PublishGeometry",
        Shape::Excluded("private scroll operation adapter"),
    ),
    (
        "scroll_view::PublishTranslation",
        Shape::Excluded("private scroll translation adapter"),
    ),
    (
        "search_field::FocusOnMount",
        Shape::OwnAnimation("owns one-shot focus and routes its input child"),
    ),
    (
        "search_field::ResponsiveSearch",
        Shape::Excluded("composite search field and responsive overlay owner"),
    ),
    (
        "search_field::SelectAllOnFocus",
        Shape::Excluded("private one-shot selection owner"),
    ),
    (
        "secure_field::CommandHost",
        Shape::Excluded("private secure-field command owner"),
    ),
    ("selection_scope::SelectionRow", Shape::Transparent),
    (
        "selection_scope::SelectionScopeWidget",
        Shape::OwnedChildCache,
    ),
    (
        "semantics_container::SemanticsContainer",
        Shape::Transparent,
    ),
    (
        "settings_dialog::RevealSelectedTab",
        Shape::Excluded("dialog-private reveal operation adapter"),
    ),
    (
        "sheet::SheetGestureArea",
        Shape::OwnAnimation("owns sheet gesture settle timing"),
    ),
    ("slider::SliderControl", Shape::Leaf),
    ("stepper::SpinBoxControl", Shape::Leaf),
    ("stack::CollapsingFlex", Shape::OwnedChildCache),
    ("streaming_text::StreamingText", Shape::Leaf),
    ("surface_interaction::DimContent", Shape::Transparent),
    ("swallow_press::SwallowPress", Shape::Transparent),
    ("switch::Switch", Shape::Leaf),
    ("tab_pill::Seam", Shape::Leaf),
    (
        "tab_pill::IndicatorLayer",
        Shape::OwnAnimation("owns selection-indicator animation and forwards child frame interest"),
    ),
    (
        "tab_pill::TabListWidget",
        Shape::Excluded("multi-child tablist focus owner"),
    ),
    ("table_text_cell::SuppressAnnounce", Shape::Transparent),
    ("tail_follow::TailFollow", Shape::OwnedChildCache),
    (
        "text_field_semantics::TextFieldSemantics",
        Shape::OwnedChildCache,
    ),
    ("text_grid::TextGridWidget", Shape::Leaf),
    (
        "title_bar::TitlebarLayout",
        Shape::Excluded("multi-region titlebar geometry owner"),
    ),
    (
        "titlebar_drag_region::TitlebarDragRegion",
        Shape::OwnedChildCache,
    ),
    (
        "toggle::ToggleActivationRow",
        Shape::Excluded("control activation owner"),
    ),
    (
        "toolbar_divider::CollapseGuard",
        Shape::Excluded("private toolbar geometry guard"),
    ),
    (
        "toolbar_row::ThreeZoneRow",
        Shape::Excluded("multi-child toolbar geometry owner"),
    ),
    (
        "tooltip::FadeTooltip",
        Shape::OwnAnimation("owns tooltip fade timing"),
    ),
    (
        "tree_view::KeyedVirtColumn",
        Shape::Excluded("multi-child virtualized container"),
    ),
    ("tree_view::TreeViewWidget", Shape::OwnedChildCache),
    ("view_that_fits::ViewThatFits", Shape::OwnedChildCache),
    ("viewport_gate::ViewportGate", Shape::OwnedChildCache),
    (
        "window_resize_frame::WindowResizeFrame",
        Shape::OwnedChildCache,
    ),
    (
        "wrap::EnzWrap",
        Shape::Excluded("multi-child wrapping layout owner"),
    ),
    ("color_picker::SaturationValuePlane", Shape::Leaf),
    ("color_picker::Swatch", Shape::Leaf),
    (
        "focus_dismiss::BoundsTracker",
        Shape::Excluded("private focus-dismiss bounds observer"),
    ),
    (
        "focus_dismiss::DismissScope",
        Shape::Excluded("private focus dismissal boundary"),
    ),
    (
        "list_row_surface::TrailingPaint",
        Shape::Excluded("private row paint adapter"),
    ),
    (
        "scroll_column::CompleteRowViewport",
        Shape::Excluded("private viewport-completion adapter"),
    ),
    (
        "table::TableBodyViewport",
        Shape::Excluded("table-private viewport owner"),
    ),
    ("action_bar::DismissOnChildActivation", Shape::Transparent),
    (
        "form::FormWidget",
        Shape::Excluded("multi-child form layout owner"),
    ),
    (
        "item_strip::Row",
        Shape::Excluded("multi-child keyed strip"),
    ),
    (
        "keyboard_capture::KeyboardCaptureAnyFocus",
        Shape::Excluded("transparent via the kc_delegate! macro, which a source scan cannot see"),
    ),
    (
        "keyboard_capture::KeyboardPreGuard",
        Shape::Excluded("transparent via the kc_delegate! macro, which a source scan cannot see"),
    ),
    ("material_boundary::MaterialBoundary", Shape::Transparent),
    ("modal_host::ModalScrollBarrier", Shape::Transparent),
    ("tab_pill::NarrowTabViewport", Shape::OwnedChildCache),
    ("text_baseline::BaselineSeat", Shape::OwnedChildCache),
    ("time_field::TimeFieldAdapter", Shape::OwnedChildCache),
    (
        "toolbar_row::FlexLayoutProxy",
        Shape::Excluded("multi-child toolbar layout proxy"),
    ),
    ("toolbar_row::ToolbarSlot", Shape::OwnedChildCache),
    (
        "virtual_text_surface::VirtualTextSurface",
        Shape::Excluded("keeps its child tree inside its own surface state"),
    ),
    ("wheel_capture::WheelCapture", Shape::Transparent),
    (
        "zoom_wheel::EnzZoomWheel",
        Shape::Excluded("owns modifier and wheel-accumulator state beside its child"),
    ),
];
