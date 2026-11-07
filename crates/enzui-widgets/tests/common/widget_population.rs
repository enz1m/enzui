//! The population ratchet for custom `Widget`
//! implementations in this crate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Directory holding the widget crate's production sources.
pub fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Remove `#[cfg(test)] mod NAME { .. }` blocks by brace matching.
pub fn strip_cfg_test_modules(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0usize;

    while i < src.len() {
        let Some(rel) = src[i..].find("#[cfg(test)]") else {
            out.push_str(&src[i..]);
            break;
        };
        let attr = i + rel;

        // Accept only `#[cfg(test)] [pub] mod IDENT {`, allowing whitespace.
        let after = attr + "#[cfg(test)]".len();
        let mut j = after;
        let skip_ws = |j: &mut usize| {
            while *j < bytes.len() && (bytes[*j] as char).is_whitespace() {
                *j += 1;
            }
        };
        skip_ws(&mut j);
        if src[j..].starts_with("pub") {
            j += 3;
            skip_ws(&mut j);
        }
        if !src[j..].starts_with("mod") {
            // Not a test module (e.g. `#[cfg(test)]` on a single item). Keep
            // the attribute text and continue past it.
            out.push_str(&src[i..after]);
            i = after;
            continue;
        }
        j += 3;
        skip_ws(&mut j);
        while j < bytes.len() && (bytes[j] == b'_' || (bytes[j] as char).is_alphanumeric()) {
            j += 1;
        }
        skip_ws(&mut j);
        if j >= bytes.len() || bytes[j] != b'{' {
            out.push_str(&src[i..after]);
            i = after;
            continue;
        }

        out.push_str(&src[i..attr]);
        let mut depth = 0usize;
        let mut k = j;
        while k < bytes.len() {
            match bytes[k] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            k += 1;
        }
        i = (k + 1).min(src.len());
    }
    out
}

/// Strip `//`-style line comments (keeping newlines so offsets stay sane).
pub fn strip_line_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find("//") {
            Some(p) => &line[..p],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Find every `impl .. Widget<..> for Type` head in already-screened source and
/// return the implemented type names.
pub fn widget_impl_types(screened: &str) -> Vec<String> {
    let b = screened.as_bytes();
    let mut found = Vec::new();
    let mut i = 0usize;

    let is_ident = |c: u8| c == b'_' || (c as char).is_alphanumeric();

    while let Some(rel) = screened[i..].find("Widget") {
        let at = i + rel;
        i = at + "Widget".len();

        if at > 0 && is_ident(b[at - 1]) {
            continue;
        }
        if at + "Widget".len() < b.len() && is_ident(b[at + "Widget".len()]) {
            continue;
        }

        // Walk backwards over `path::` qualifiers and whitespace.
        let mut p = at;
        loop {
            while p > 0 && (b[p - 1] as char).is_whitespace() {
                p -= 1;
            }
            if p >= 2 && &screened[p - 2..p] == "::" {
                p -= 2;
                while p > 0 && is_ident(b[p - 1]) {
                    p -= 1;
                }
                continue;
            }
            break;
        }
        // Optional generic parameter list belonging to `impl`.
        if p > 0 && b[p - 1] == b'>' {
            let mut depth = 0i32;
            let mut q = p - 1;
            loop {
                match b[q] {
                    b'>' => depth += 1,
                    b'<' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                if q == 0 {
                    break;
                }
                q -= 1;
            }
            if depth != 0 {
                continue;
            }
            p = q;
            while p > 0 && (b[p - 1] as char).is_whitespace() {
                p -= 1;
            }
        }
        if !(p >= 4 && &screened[p - 4..p] == "impl" && (p == 4 || !is_ident(b[p - 5]))) {
            continue;
        }

        // Forwards: balanced `<..>` then `for IDENT`.
        let mut q = at + "Widget".len();
        while q < b.len() && (b[q] as char).is_whitespace() {
            q += 1;
        }
        if q >= b.len() || b[q] != b'<' {
            continue;
        }
        let mut depth = 0i32;
        while q < b.len() {
            match b[q] {
                b'<' => depth += 1,
                b'>' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            q += 1;
        }
        if depth != 0 {
            continue;
        }
        q += 1;
        while q < b.len() && (b[q] as char).is_whitespace() {
            q += 1;
        }
        if !screened[q..].starts_with("for") {
            continue;
        }
        q += 3;
        if q < b.len() && is_ident(b[q]) {
            continue;
        }
        while q < b.len() && (b[q] as char).is_whitespace() {
            q += 1;
        }
        let start = q;
        while q < b.len() && is_ident(b[q]) {
            q += 1;
        }
        if q > start {
            found.push(screened[start..q].to_owned());
        }
    }
    found
}

/// Scan the crate's production sources and return the sorted population as
/// `module::Type` strings.
pub fn scan_population() -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let dir = src_dir();
    let excluded = test_only_paths(&dir);
    scan_population_dir(&dir, &dir, &excluded, &mut out);
    out
}

/// The files and directories a file-backed module declaration in `file` names:
/// `(declared under #[cfg(test)], target paths)` per `mod NAME;`.
pub fn module_file_declarations(file: &Path, src: &str) -> Vec<(bool, Vec<PathBuf>)> {
    let parent = file.parent().expect("a source file has a directory");
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let module_dir = if matches!(stem, "mod" | "lib" | "main") {
        parent.to_path_buf()
    } else {
        parent.join(stem)
    };
    let mut out = Vec::new();
    let mut cfg_test = false;
    let mut path_attr: Option<String> = None;
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("#[") {
            if line.starts_with("#[cfg(test)]") || line.starts_with("#[cfg(all(test") {
                cfg_test = true;
            } else if let Some(rest) = line.strip_prefix("#[path") {
                path_attr = rest.split('"').nth(1).map(str::to_owned);
            }
            continue;
        }
        let decl = line
            .strip_prefix("pub(crate) ")
            .or_else(|| line.strip_prefix("pub "))
            .unwrap_or(line);
        if let Some(name) = decl.strip_prefix("mod ").and_then(|r| r.strip_suffix(';')) {
            let name = name.trim();
            let targets = match path_attr.take() {
                Some(path) => vec![parent.join(path)],
                None => vec![module_dir.join(format!("{name}.rs")), module_dir.join(name)],
            };
            out.push((cfg_test, targets));
        }
        cfg_test = false;
        path_attr = None;
    }
    out
}

/// Every source path under `root` that only a test build compiles: the targets
/// of `#[cfg(test)] mod NAME;`, and transitively every module file declared
/// inside one of them.
pub fn test_only_paths(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("widget src dir must be readable") {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(root, &mut files);
    // Each file with its `mod` declarations: whether test-gated, and the
    // candidate paths.
    type FileDecls = (PathBuf, Vec<(bool, Vec<PathBuf>)>);
    let decls: Vec<FileDecls> = files
        .iter()
        .map(|f| {
            let src = std::fs::read_to_string(f).expect("readable source");
            (f.clone(), module_file_declarations(f, &src))
        })
        .collect();
    let mut excluded: Vec<PathBuf> = Vec::new();
    loop {
        let before = excluded.len();
        for (file, file_decls) in &decls {
            let inside = is_under_any(file, &excluded);
            for (cfg_test, targets) in file_decls {
                if *cfg_test || inside {
                    for t in targets {
                        if !excluded.contains(t) {
                            excluded.push(t.clone());
                        }
                    }
                }
            }
        }
        if excluded.len() == before {
            return excluded;
        }
    }
}

/// Whether `path` is one of `roots` or lies inside one of them.
pub fn is_under_any(path: &Path, roots: &[PathBuf]) -> bool {
    roots
        .iter()
        .any(|root| path == root || path.starts_with(root))
}

/// Recurse `dir`, attributing every production `impl Widget<..>` to its
/// top-level module.
fn scan_population_dir(
    root: &std::path::Path,
    dir: &std::path::Path,
    excluded: &[PathBuf],
    out: &mut BTreeSet<String>,
) {
    let entries = std::fs::read_dir(dir).expect("widget src dir must be readable");
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if is_under_any(&path, excluded) {
            continue;
        }
        if path.is_dir() {
            scan_population_dir(root, &path, excluded, out);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        // The module is the first path component under `src/`: the directory
        // name for a nested file, or the file stem for a flat `src/<name>.rs`.
        let rel = path.strip_prefix(root).expect("path under src root");
        let first = rel
            .components()
            .next()
            .and_then(|c| c.as_os_str().to_str())
            .expect("module component");
        let module = first.strip_suffix(".rs").unwrap_or(first).to_owned();
        if module == "lib" {
            continue;
        }
        let raw = std::fs::read_to_string(&path).expect("readable source");
        let screened = strip_cfg_test_modules(&strip_line_comments(&raw));
        for ty in widget_impl_types(&screened) {
            out.insert(format!("{module}::{ty}"));
        }
    }
}

/// The pinned population: every production `impl Widget<..> for T` in this
/// crate, as `module::Type`, sorted.
pub const POPULATION: &[&str] = &[
    "action_bar::ActionBarWidget",
    "action_bar::DismissOnChildActivation",
    "adaptive_row::EnzAdaptiveRowWidget",
    "anchored_overlay::AnchoredOverlay",
    "annotation_overlay::EnzAnnotationOverlay",
    "app_shell::AppShellWidget",
    "app_shell::LiveSeamPad",
    "app_shell::MirrorRow",
    "app_shell::RetainedResponsiveWidth",
    "app_shell::SeamRider",
    "activity_pulse::EnzActivityPulse",
    "aspect_ratio::EnzAspectRatio",
    "accessibility_scan_widgets::ContrastScanProbe",
    "accessibility_scan_widgets::ElisionScanProbe",
    "accessibility_scan_widgets::HitTargetScanProbe",
    "accessibility_scan_widgets::MissingNameScanProbe",
    "autocomplete_field::AutocompleteWidget",
    "banner::BannerResponsive",
    "cached_subtree::EnzCachedSubtree",
    "calendar_spans::CalendarSpanLayer",
    "color_picker::SaturationValuePlane",
    "color_picker::Swatch",
    "context_menu_host::MenuHeader",
    "focus_dismiss::BoundsTracker",
    "focus_dismiss::DismissScope",
    "form::FormWidget",
    "image_cropper::EnzImageCropper",
    "item_strip::Row",
    "keyboard_capture::KeyboardCaptureAnyFocus",
    "keyboard_capture::KeyboardPreGuard",
    "layout_memo::LayoutMemo",
    "layout_spill_probe::LayoutSpillProbe",
    "capped_dialog_body::CappedDialogBody",
    "carousel::CarouselInput",
    "center_clamp_row::CenterClampRow",
    "chart_motion::ChartMotion",
    "chart_tooltip::AlignedTooltipRows",
    "code_block::BodySemantics",
    "code_block::CodeBlockPulse",
    "combo_box::ComboBoxWidget",
    "command_palette::PaletteKeys",
    "context_menu_host::EnzContextMenuHost",
    "context_target::EnzContextTarget",
    "date_picker::DismissOnOutsidePress",
    "date_picker::SurfacePressShield",
    "decorative::Decorative",
    "desktop_glass::EnzDesktopGlass",
    "diagnostics_overlay::DiagnosticsOverlay",
    "divider::FadedRule",
    "document_tab::TabHeightFill",
    "document_tab::TabMouseCapture",
    "document_tab::TitleReserveCap",
    "document_tab::TitleWidthPin",
    "drag_autoscroll_host::EnzDragAutoScrollHost",
    "drag_drop_host::EnzDragDropHost",
    "drag_item_surface::EnzDragItemSurface",
    "editor_scrollbar::EnzEditorScrollbar",
    "elevation::ElevationAnim",
    "list_row_surface::TrailingPaint",
    "material_boundary::MaterialBoundary",
    "modal_host::ModalScrollBarrier",
    "progress_text::InkWave",
    "responsive::EnzResponsive",
    "scroll_column::CompleteRowViewport",
    "tab_pill::NarrowTabViewport",
    "table::FoldRow",
    "table::RowHover",
    "table::TableBodyViewport",
    "table::TableScrollbar",
    "text::SuppressAnnounce",
    "exclusive_focus::ExclusiveFocus",
    "expander::BodyGate",
    "expander::ExpanderAnim",
    "field_chrome::FieldChrome",
    "focus_button::FocusButton",
    "focus_retention::FocusRetention",
    "frame_clock::EnzFrameClock",
    "glow::EnzGlow",
    "glyph_selection::EnzGlyphSelection",
    "grid::EnzGrid",
    "grid::KeyedGrid",
    "hover_control::HoverControl",
    "hover_group::EnzHoverGroup",
    "hug_width::HugToPreferred",
    "image_viewer::EnzImageViewer",
    "inert::Inert",
    "inert_clip::InertClip",
    "input_otp::OtpChrome",
    "key_capture_chip::EnzKeyCaptureChip",
    "keyboard_capture::RovingGrid",
    "keyed_column::KeyedColumn",
    "keyed_row::KeyedRow",
    "marquee_area::EnzMarqueeArea",
    "max_height::MaxHeight",
    "max_width::MaxWidth",
    "menu::ActiveReveal",
    "menu::HoverReporter",
    "menu::MenuRovingWidget",
    "menu::RowHighlight",
    "menu_button::DismissOnOutsidePress",
    "min_height::MinHeight",
    "list_row_surface::OuterGutter",
    "min_width::MinWidth",
    "modal_host::EscapeCapture",
    "modal_host::WithdrawFocus",
    "motion::CrossFade",
    "motion::Motion",
    "motion::PanelReveal",
    "navigation_outlet::BackKeyCapture",
    "navigation_outlet::BackSwipeCapture",
    "navigation_outlet::Slide",
    "navigation_split_view::NavigationSplitViewWidget",
    "offset_scrollbar::EnzOffsetScrollbar",
    "overflow_row::OverflowRowWidget",
    "pane_grid::PaneGridSemantics",
    "positioned_overlay::EnzPositionedOverlay",
    // Content-aware label/control layout. Swept through the entry-kind
    // `preferences_row` specimen at both inline and stacked widths.
    "preferences_row::PreferencesResponsive",
    "preferences_row::SwitchPress",
    "radio_group::RadioGroupWidget",
    "reflow_pair::ReflowPair",
    "reorder_list::EnzReorderList",
    "responsive_switch::ResponsiveSwitch",
    "revision_text::RevisionText",
    "rich_text::LinkSemantics",
    "rich_text::ProseSemantics",
    "rounded_clip::RoundedClip",
    "row_measure::EnzRowMeasure",
    "row_motion::RowMotion",
    "schedule::EnzSchedule",
    "scrim_click::ScrimClick",
    "scroll_view::KeyScrollForward",
    "scroll_view::KeyboardScroll",
    "scroll_view::PublishGeometry",
    "scroll_view::PublishTranslation",
    "search_field::FocusOnMount",
    "search_field::ResponsiveSearch",
    "search_field::SelectAllOnFocus",
    "secure_field::CommandHost",
    // Tags a virtualised row's text regions with its logical key.
    "selection_scope::SelectionRow",
    // The selection scope widget owns the gesture state machine and
    // paints selection highlights across its children's text regions.
    "selection_scope::SelectionScopeWidget",
    "semantics_container::SemanticsContainer",
    "settings_dialog::RevealSelectedTab",
    "sheet::SheetGestureArea",
    "slider::SliderControl",
    "stepper::SpinBoxControl",
    // Shared Row/Column replacement used by stack and sidebar_content;
    // it computes separator geometry only between non-empty laid-out children.
    "stack::CollapsingFlex",
    "streaming_text::StreamingText",
    "surface_interaction::DimContent",
    "swallow_press::SwallowPress",
    "switch::Switch",
    "tab_pill::IndicatorLayer",
    "tab_pill::Seam",
    "tab_pill::TabListWidget",
    "table_text_cell::SuppressAnnounce",
    "tail_follow::TailFollow",
    "text_baseline::BaselineSeat",
    "text_field_semantics::TextFieldSemantics",
    "text_grid::TextGridWidget",
    "time_field::TimeFieldAdapter",
    "title_bar::TitlebarLayout",
    "titlebar_drag_region::TitlebarDragRegion",
    "toggle::ToggleActivationRow",
    "toolbar_divider::CollapseGuard",
    "toolbar_row::FlexLayoutProxy",
    "toolbar_row::ThreeZoneRow",
    "toolbar_row::ToolbarSlot",
    "tooltip::FadeTooltip",
    "tree_view::KeyedVirtColumn",
    "tree_view::TreeViewWidget",
    "view_that_fits::ViewThatFits",
    "viewport_gate::ViewportGate",
    "virtual_text_surface::VirtualTextSurface",
    "wheel_capture::WheelCapture",
    "zoom_wheel::EnzZoomWheel",
    "window_resize_frame::WindowResizeFrame",
    "wrap::EnzWrap",
];

/// The pinned population as `&'static str` entries (what a coverage claim
/// is diffed against).
pub fn population_entries() -> Vec<&'static str> {
    POPULATION.to_vec()
}
