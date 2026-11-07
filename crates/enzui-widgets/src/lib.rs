//! enzUI widget library.
//!
//! Styled components built on iced, porting the enzUI design system.
//! Each component follows the enzUI token-driven theming approach while
//! using iced's native widget architecture.
//!
//! # Widget catalog - find a widget by what you are trying to do
//!
//! ## Naming and imports
//!
//! Every component in this catalog follows one grammar, so a name tells you
//! what it is:
//!
//! - **Canonical component module**: the unprefixed `snake_case` *concept*
//!   (`text`, `table`, `button`, `image`) - never a brand-prefixed or
//!   `_widget`-suffixed path.
//! - **Canonical visual constructor**: prefix-first `enz_snake_case`
//!   (`enz_text`, `enz_disabled_button`). The `enz_` marker is always first,
//!   never in the middle.
//! - **Canonical visual builder / type**: `EnzUpperCamelCase` (`EnzText`,
//!   `EnzButton`).
//! - **Models, policies, controllers, measurements and support helpers** use
//!   ordinary Rust names and do **not** acquire an `Enz`/`enz_` marker merely
//!   because they support a widget (`TextSize`, `CellCoord`, `ToastHost`,
//!   `FileRequest`, lifecycle policies).
//! - Every canonical tier-1 constructor and every `Enz*` builder type is
//!   reachable **both** from its module and from the crate root.
//! - **Compatibility aliases are not canonical catalog entries.** The brand-prefixed module paths (`enz_text`, `enz_table`,
//!   `image_widget`, …) and the reversed / bare constructor spellings
//!   (`disabled_enz_button`, `revision_text`) keep resolving for existing
//!   callers but are absent from this catalog and `docs/widgets.json`; reach
//!   for the canonical spelling in new code.
//!
//! The catalog below lists every tier-1 (supported public API) widget module,
//! grouped by task. Tier 1 is every `pub mod` not listed in tier 2 or tier 3
//! (see the export boundary below); the `### ` group headings partition it
//! exactly, and `docs/widgets.md` is generated from this block, group prose
//! included. To add a widget, add its `pub mod` and one bullet
//! `- [NAME](mod@NAME) - what it is for.` under its group; a test fails on a
//! tier-1 module with no entry, an entry in two groups, or a phantom entry.
//!
//! ## Finding a widget by keyword
//!
//! The group headings answer *what am I trying to do*. The table below
//! answers the other direction: *I have a name in mind, where did it ship?*
//! It is an **index**, not an incident log: every tier-1 widget module is
//! reachable here by at least one intent term that is **not** its own module
//! name - the term an app author porting from GTK4/Adwaita, SwiftUI, shadcn
//! or Kirigami would actually type (`segmented control`, `spinbox`, `empty
//! state`, `snackbar`, `split view`, `data grid`, …). Grep for the term you
//! were going to type into the file finder and the row here points at the
//! module that ships it.
//!
//! Rows cover the terms other design systems use for a widget that ships
//! under a different name here. `enz_autocomplete_field`, for example, is an
//! autocomplete dropdown rather than `typeahead`, because
//! "typeahead" already means the Nautilus type-to-jump feature elsewhere in
//! this codebase, so an author searching "autocomplete" would not find it
//!
//!
//! **The flywheel.** If you searched this table for an intent term and did
//! *not* find your widget, that is a bug in the index, not proof the widget
//! is missing - add the term you searched for as a new alias row in the same change that resolves it. The index then improves through
//! use instead of only after an audit.
//!
//! This is a **checked** table, not free prose:
//! `keyword_aliases_resolve_to_real_modules` (`tests/contracts/widget_catalog_page_discovery.rs`)
//! fails the build if any target names a `pub mod` that does not exist;
//! `every_tier1_module_has_a_non_self_keyword` - the
//! coverage floor - fails, naming the modules, if any tier-1 module becomes
//! reachable only by its own name (or by nothing).
//!
//! Closure-shaped searches are welcome too: `content(|size|)` is the common
//! spelling for content that needs the shell's resolved width, and the row
//! below routes it through its prose aliases without requiring a second outer
//! responsive host.
//!
//! | you searched for | reach for |
//! |---|---|
//! | `animate position`, `animate offset`, `implicit animation`, `animate to a new place`, `move on a spring`, `spring position`, `animated offset`, `animate opacity`, `SwiftUI animation modifier`, `withAnimation`, `animate without frame messages`, `window::frames`, `per-frame message`, `SpringAnimation in app state` | [`motion`](mod@motion) - [`motion::animated`] wraps an element that travels to the offset and opacity each view gives it, on a spring that keeps its velocity through a retarget. The widget keeps its own clock and publishes tracks, so the application sends **no per-frame messages** and its view is not rebuilt while the child moves; the render lane moves the recorded child by itself. Driving a `SpringAnimation` in app state from `window::frames()` instead rebuilds and re-lays-out the whole view on every frame. The offset is visual: layout is unchanged, pointer input and overlays follow the child. |
//! | `animate list insertion`, `list layout transition`, `layout transition`, `animate insertion`, `animate removal`, `inserted row pushes the others`, `rows jump when a row is inserted`, `animate reorder`, `FLIP`, `animateItemPlacement`, `animate item placement`, `slide rows into place` | [`keyed_column`](mod@keyed_column) - [`keyed_column(..).animate_moves()`](crate::keyed_column::KeyedColumn::animate_moves) slides the rows a change of children moves to their new places on a spring: a message arriving at the top pushes the list down over several frames instead of one, and a removed or reordered row's neighbours close the gap the same way. Layout is exact on every frame and only the paint travels, carried by the render lane; resizes are not animated, and under reduced motion rows move at once. Give the inserted row its own entrance with [`motion::animated`]`(..).enters_from(..)`. |
//! | `transition`, `expressive travel`, `overshoot travel`, `ExpressiveTravel`, `slide in`, `slide out`, `animate a panel`, `drawer animation`, `pane reveal`, `panel reveal`, `layout-driven reveal`, `inspector reservation`, `animated split pane`, `right docked inspector`, `top panel`, `bottom panel`, `edge panel`, `dock panel`, `exit animation`, `retained presentation`, `collapse the gap`, `zero-size child`, `spacing collapse`, `motion`, `duration`, `easing`, `reduced motion`, `size animation`, `scale animation`, `animate any element`, `fade`, `fade in`, `cross-fade`, `fade between`, `opacity animation`, `stagger`, `staggered animation`, `stagger a list`, `choreography`, `sequence animations`, `sequenced animation`, `cascade animation`, `one after another`, `shared animation clock` | [`motion`](mod@motion) - wrap **any element** in a named, composable `Slide` (four edges), `Size` (width/height/both), `Scale`, or renderer-composited `Fade` effect. `PanelReveal` is the caller-clock panel composition on top of them: `PanelRevealClock` publishes one `PanelMotion` to it, and `PanelRevealEdge` docks it against any of the four window edges - `Leading`/`Trailing` spend the reveal on width, `Top`/`Bottom` (via `PanelReveal::vertical`, whose resting extent is a height) on height - with pinned reservation and retained clip on either axis by default. `PanelRevealClock::layout_driven(true)` instead reserves the current eased extent so adjacent grids/tables reflow on each owner frame, including right-docked and vertical inspectors; the owner advances the clock and rebuilds during travel, while departing/closed descendants are inert. `cross_fade` retains and lays out both endpoints, reserves their maximum size to prevent a last-frame jump, and composites opaque overlap without the brightness dip of two translucent sibling groups. [`RetainedPresentation`] keeps dropped pane data soundly rebuildable through exit and rejects stale settlement after reopen. Framework stacks charge separators only between laid-out children, so any number of settled-hidden wrappers has the same geometry as those children being absent. Defaults come from the live theme's duration/easing/reduced-motion policy; idle and offscreen effects request no frames, size/Reserve motion pins its widest reservation and owes only first/final geometry commits while a retained clip carries travel, translation/scale/fade do not reflow, and a settled-hidden child leaves the widget tree. **Staggering is a documented technique here, not a missing widget:** `PanelMotion::set_target` (in the module linked above) takes the *caller's* instant, so item `i` starts at `frame + i * step` off one shared clock. There is deliberately no sequence primitive to find: owning the `PanelMotion` and offsetting one instant is the whole mechanism. |
//! | `autocomplete`, `combo`, `completion`, `suggest`, `type ahead`, `type-ahead`, `typeahead`, `autocomplete field`, `responsive autocomplete`, `collapsible autocomplete`, `autocomplete search icon`, `search with results on small screens`, `focus collapsed autocomplete` | [`autocomplete_field`](mod@autocomplete_field) - an editable text field with a filtered suggestion list underneath. **Responsive search-with-results is on by default:** below the shared, measured usability width it becomes one named search button; activation opens with focus already in the input, Escape restores the button, an active query remains selected, and routed focus to the field id opens it without leaking the hidden input into the AT tree. It consumes the same responsive-search core as [`search_field`](mod@search_field), while retaining its editable-combobox semantics and keyboard-operable results popup. **Note:** this module was formerly called `typeahead`; "type-ahead" (the Nautilus type-to-jump concept) is a different thing and is not shipped as a widget. |
//! | `popover`, `anchored panel`, `floating panel`, `attached panel`, `panel under a button`, `GtkPopover`, `popup form`, `popover focus` | [`popover`](mod@popover) - a floating panel attached to a control: `enz_popover(trigger, content, on_dismiss)` with the open state in the application (`Some(content)` while open). It sits below the trigger by default ([`EnzPopover::side`](popover::EnzPopover::side), `alignment`), flips and shifts to stay in the window (on Wayland it opens past the window edge, in a popup surface, only when it does not fit inside), can attach to a window-space rectangle instead ([`EnzPopover::anchor`](popover::EnzPopover::anchor)), and dismisses on an outside press or Escape while a press on the trigger still reaches the trigger. It publishes a non-modal `Dialog` named by `label`; [`popover::focus_operation`] moves focus to its first control when it opens. For commands use [`menu_button`](mod@menu_button); `anchored_overlay` (next row) is the low-level layer it is built on. |
//! | `popup`, `attached menu`, `virtual anchor`, `selection popover`, `selection toolbar`, `dismiss`, `dismiss popup`, `outside click`, `click outside to close`, `light dismiss`, `close on escape`, `escape to close`, `blocking scrim`, `block background scroll`, `non-dismissible scrim` | [`anchored_overlay`](mod@anchored_overlay) + [`scrim_click`](mod@scrim_click) - a positioned overlay pinned to an anchor rectangle, paired with the outside-click dismissal host. Together these are what other frameworks call a popover. **Dismissal is not the caller's to hand-roll:** `anchored_overlay::dismissible_anchored_menu` wires the outside-press backdrop (which knows the anchor's bounds, so it never covers the trigger) *and* the Escape capture in one call, and `dismissible_anchored_menu_at` accepts a window-space virtual anchor such as `ScopeSelection::bounds` instead of guessing coordinates from the containing pane. `anchored_overlay::dismiss_on_escape` is the Escape half alone. A hand-rolled full-bleed scrim eats its own trigger and a menu with neither seam cannot be closed at all. `ScrimClick::new` is deliberately **light-dismiss**: scroll keeps reaching its live background. A blocking drawer opts into `.block_scroll()` so wheel, touch-drag and unmodified PageUp/PageDown/Home/End stop at the scrim after the drawer body gets first refusal; use `ScrimClick::shield` for the same blocking behavior without advertising or publishing dismissal. |
//! | `diagnostics overlay`, `diagnostic outline`, `audit highlights`, `screen-space findings`, `child-relative findings`, `local findings` | [`diagnostics_overlay`](mod@diagnostics_overlay) - paint-only audit outlines; `DiagnosticsOverlay::findings` accepts screen coordinates and `.local_findings` resolves child-relative bounds during drawing. Neither mode clips findings to the child, so spill remains visible. |
//! | `gallery`, `image gallery`, `thumbnail strip`, `screenshot strip`, `screenshot gallery`, `carousel`, `slideshow`, `pager`, `banner carousel`, `promoted items strip`, `rotating promoted items`, `content under carousel arrows`, `carousel chrome overlaps content`, `inset carousel body`, `carousel arrows cover cards`, `carousel page height`, `stage body height`, `page slot`, `carousel body slot`, `carousel vertical chrome` | [`carousel`](mod@carousel) - accessible keyed paging with indicators, prev/next chrome, keyboard paging and optional swipe. Also the answer for a store detail page's **screenshot strip** and a store front page's **banner carousel**: the searcher who typed those exact terms found nothing, then hand-composed the same widget out of a scroll container and lost the keyed paging, the indicators and the keyboard reach. Not the same as [`banner`](mod@banner), which is a static inline notification strip - the alias `banner carousel` is included here so a query for the rotating shape lands on paging, not on the status row. The prev/next chrome is stacked **over** the page body (right for a full-bleed gallery, wrong for a page of pressable cards): `EnzCarousel::chrome_inset(true)` insets the body clear of it, so no caller hand-computes the clearance from the chrome constants. [`body_slot_height`] is the vertical counterpart: it reports the height available for page content after the widget's own padding, spacing and indicator row are subtracted. |
//! | `chromeless tab strip`, `hide tab strip scrollbar`, `tab strip`, `tab bar`, `AdwTabBar`, `dynamic tabs`, `document tabs` | [`document_tabs`](mod@document_tabs) - the widget layer over the keyed document-tab model. See [`document_tab`](mod@document_tab) for the single-cell primitive. [`EnzDocumentTabs::hide_scrollbar`](document_tabs::EnzDocumentTabs::hide_scrollbar) scrolls the strip without scrollbar chrome. |
//! | `pagination`, `numbered pagination`, `page navigator`, `page navigation`, `go to page`, `jump to page`, `paged document`, `page picker`, `page selector`, `page bar`, `numbered pager`, `document pager`, `PDF page picker`, `page 1 of N`, `first page last page`, `next previous page`, `prev next page`, `previous page next page` | [`pagination`](mod@pagination) - numbered pagination with prev/next chrome and ellipsis collapse, built in **O(visible cells)** so a million-page pager renders the same handful of buttons (page 1, page N, a bounded window around the current page, and two ellipses when the gap spans more than one page). Its cells are neutral (`Quiet`), not accent-inked. Reach for it for a bounded set of pages a reader picks among; a long document reads better as a typed page number (Reader floats previous / page field / next on a pill). The bare term `pager` deliberately still lives on the `carousel` row (a carousel is a paginated widget) and on the `text_measure` row (its `paginate_by_height_with` breaks prose into pages); reach for `enz_pagination` when you have N pre-computed pages and need the numbered strip with prev/next and ellipsis collapse. |
//! | `file picker`, `file chooser`, `file dialog`, `open dialog`, `save dialog`, `folder picker` | **not a widget** - see [`enzui_core::portal`] (different crate). File choosing goes through the XDG Desktop Portal, not a widget in this crate. An app author greps here first; the pointer above is why. |
//! | `portal file chooser`, `xdg file dialog`, `native file dialog`, `portal seam`, `file chooser`, `save as dialog`, `export file`, `open file dialog`, `chooser session`, `cancel file chooser`, `close file dialog`, `chooser cancellation`, `file chooser without portal`, `in-app file chooser`, `fallback file dialog`, `no xdg-desktop-portal`, `wlroots file dialog` | [`file_chooser`](mod@file_chooser) - [`ChooserSession`](file_chooser::ChooserSession) owns a lazy portal backend and one tagged request. Start on a user gesture, return its subscription, and pump on completion wakes. [`cancel`](file_chooser::ChooserSession::cancel) returns the tagged cancellation and settles without another pump; dropping the session cancels and settles before releasing the backend. The XDG portal supplies the native dialog; [`with_fallback`](file_chooser::ChooserSession::with_fallback) answers with the in-app [`FallbackChooser`](file_chooser::FallbackChooser) when no portal implements FileChooser (render [`view`](file_chooser::ChooserSession::view), route [`handle`](file_chooser::ChooserSession::handle)). |
//! | `find and replace`, `find replace bar`, `replace bar`, `responsive find`, `responsive replace`, `close search`, `compact find actions`, `find replace status`, `regex error announcement`, `replace status announcement`, `find replace live region`, `announce find replace`, `replacement field width`, `replacement chrome`, `matching find fields` | [`find_replace_bar`](mod@find_replace_bar) - the controlled inline editor composition: query + count, previous/next, replace disclosure, replacement field, replace/replace-all, a real named close-X, and optional search-policy/scope controls. `.replacement_width(iced::Length::Fill)` lets the replacement span the available width, wrapping actions below it; `.replacement_chrome(ReplacementFieldChrome::Search)` matches Find's fill and border roles. Defaults retain the existing 260 px standard field. Whole controls reflow through intrinsic flow breakpoints instead of clipping; both text inputs retain focus and their exact caret across width changes. Pointer close and Escape while the bar is mounted emit the same `FindReplaceEvent::Close`, while all values remain reducer-owned. There is deliberately no visible `Esc` pseudo-control. The caller-supplied status (`3 of 5`, `No matches`, `Searching…`, a regex error) publishes through the shared live-region vocabulary so assistive technology hears each change in place: polite [`Announcement::Status`](enzui_core::live_region::Announcement::Status) by default, or assertive [`Announcement::Alert`](enzui_core::live_region::Announcement::Alert) selected by the caller with `.announcement(..)` for a real error. A blank or whitespace-only status leaves the region present but silent while the query and actions remain. |
//! | `find surface`, `find bar`, `find popover`, `find next`, `match count`, `search matches`, `Ctrl+F`, `highlight matches`, `announce match count`, `live count`, `still searching`, `find results announcement` | [`find_surface`](mod@find_surface) - query + one authoritative match count + wrapping previous/next navigation in an anchored, dismissible overlay that cannot resize its host. It composes [`search_field`](mod@search_field) with [`anchored_overlay::dismissible_anchored_menu`](mod@anchored_overlay), focuses the field on open, routes Ctrl+F / Enter / Shift+Enter, and exposes stable-key navigation for the caller to paint all match coordinates with one active match. The caller owns the sole ordered match collection; the state stores only its active stable key, so streaming append cannot drift the highlight. The count publishes through the shared live-region vocabulary ([`Announcement::Status`](enzui_core::live_region::Announcement::Status) → [`Role::Status`](enzui_core::semantics::Role::Status) + [`Live::Polite`](enzui_core::semantics::Live::Polite)), so a screen reader hears result-count, navigation-index and "still searching" transitions as they happen; an empty query publishes no name, so opening the popup is silent. Replace, regex, case and whole-word remain host search policy, not permanent toggles in this presentation primitive. |
//! | `match ink`, `find match ink`, `active match`, `current match highlight`, `search highlight contrast`, `highlighted text is unreadable`, `match fill`, `selection ink`, `syntax ink over a highlight`, `range ink`, `composited ink`, `highlight overrides ink`, `multi-line syntax`, `editable syntax`, `highlight checkpoint`, `block comment`, `raw string`, `triple quoted string`, `markdown fence` | [`match_ink`](mod@match_ink) - the stateful editable-text syntax highlighter keeps multi-line comments, strings, and Markdown fences across reusable line checkpoints and resolves **glyph ink inside a find match**. A `text_editor` `background_highlights` quad is painted UNDER the glyphs, so a coloured match fill cannot change the syntax ink above it: an app's accent-filled current match measured **1.02:1** in light and **1.74:1** in dark, with accent-coloured keywords sitting on the accent fill. No fill alone can fix that - the syntax inks sit just above 4.5:1 on `background` (light `tone_success_fg` at 4.52:1), so any tint toward them drops one below the floor. Wire [`MatchInkHighlighter`](match_ink::MatchInkHighlighter) with [`format_for_editor_ink`](match_ink::format_for_editor_ink) instead of [`syntax::SyntaxHighlighter`], hand it the painted [`MatchRun`](match_ink::MatchRun)s, and paint the quads from [`FindMatchTokens`](enzui_theme::FindMatchTokens) - one authoritative fill/ink pair. Outside a match the resolution is byte-identical to the plain syntax path; only matched runs are resolved, so document syntax colours are never flattened. **Overlap policy:** the runs may overlap in any order and precedence is resolved *per covered byte* - the strongest emphasis of all runs covering a byte wins, so an `Active` run owns every byte it covers even when an overlapping `Inactive` run starts earlier; inactive-only bytes stay inactive, emitted segments tile each syntax run with no gaps/duplicates on UTF-8 boundaries, and equal emphases are deterministic. |
//! | `titlebar search`, `glass search`, `ghost search`, `transparent search field`, `collapse to icon`, `search popover`, `results list`, `omnibox`, `quick find`, `incremental search`, `search floats in the middle of the titlebar`, `search bar should hug the window controls`, `cap a fill width`, `bounded fill` | [`search_field`](mod@search_field) - [`enz_titlebar_search`] is the 26 px titlebar field preset: `Length::Fill` up to `TITLEBAR_SEARCH_MAX_WIDTH` (340 px), measured narrowing, collapse to the shared 28 px titlebar-affordance box + 16 px glyph, then a viewport-centred card below the trigger. Activation never expands back into the slot that proved too narrow. Optional `filters` and rich/multi-line `results` slots follow the field in focus order; pass [`suggestion_list`](mod@suggestion_list) when listbox/arrow-key semantics are wanted, or arbitrary rows when the caller owns those semantics. Ordinary pane/form search keeps [`enz_search_field`]'s `Md` + inline defaults. `EnzSearchField::max_width` is a **bounded fill** (`Length::Fill.max(cap)`), not a fill share the field paints part of: the row allocates at most the cap and hands the remainder back, so a capped field in a trailing-aligned action slot stays beside the window controls instead of painting 340 px adrift in the middle of a wide band. [`EnzSearchField::chrome`](search_field::EnzSearchField::chrome) accepts [`Ghost`](search_field::SearchFieldChrome::Ghost) for transparent resting inner paint with live-theme hover and focus rings; pair it with `.material(Regular)` to place the field on glass. |
//! | `typed context menu`, `context menu command enum`, `which row was right clicked`, `context menu target`, `menu returns command`, `row menu`, `right click a row`, `no string menu ids`, `update an open menu`, `live menu rows`, `stable menu row key` | [`context_menu_host`](mod@context_menu_host) - [`ContextMenu<Target, Command>`](context_menu_host::ContextMenu): `open(target, at, items)` with typed [`ContextMenuItem<Command>`](context_menu_host::ContextMenuItem)s (icons, shortcuts, destructive, submenus); `update(&event)` returns the chosen `(target, command)` and the focus task; `.key(..)` names a row and `reconcile(items)` changes a showing menu's rows, keeping the highlight. |
//! | `context menu`, `right click menu`, `menu at cursor`, `popup at point`, `click position`, `menu host only while open`, `scroll jumps to top on right click`, `mount the menu host unconditionally`, `restore context menu focus`, `tree row context focus`, `reaction menu`, `context menu header`, `interactive menu header`, `context menu accessory` | [`context_menu_host`](mod@context_menu_host) - the batteries-included right-click menu. Its optional `.header(factory)` places caller-owned interactive controls above root commands, supplies the showing stamp for stale-action checks, and enables Tab/Shift+Tab wrapping within the deepest surface. It owns the keyboard funnel (Escape, arrows, Home/End skipping disabled), the outside-press dismissal with consumption, focus restoration to the invoker, and the routable `ShowContextMenu` AT node. Open [`ContextMenuState`](mod@context_menu_host) with `open_with_restore`, then apply events with `apply_event_restoring`: the framework retains the target, interprets dismissal causes, and batches restoration with the app's activation task. Apps should not call `focus_restore_operation_to` themselves; that is the custom-host escape hatch. A tree row uses `FocusRestoreTarget::tree_row_path`, because the embedded row is not a separate Tab stop. Two entry points: `enz_point_context_menu_host` when the trigger is a POINT your app hit-tested (a virtualised row, a calendar cell) - the common case - and `enz_context_menu_host` when the trigger is a single static child it can measure itself. Pair either with `enz_menu` rows via `enzui_core::menu_model::MenuModel`. **Mount the point host every frame, not only while the menu is open:** the conditional mount changes the tree shape above your page, so iced discards the state tree below it and the scrolled list you right-clicked jumps back to the top. `enz_optional_point_context_menu_host` takes the menu as an `Option` for exactly that call site and is an inert, layout-identical pass-through while closed. Reach for `positioned_overlay` only when you want the bare placement layer with no menu semantics at all. |
//! | `accessibility action`, `AT action`, `AT-SPI action`, `screen reader click`, `screen reader activates a button`, `screen reader sets a value`, `assistive technology action`, `apply a routed action`, `routed action to widget operation`, `accessibility wake`, `a11y wake`, `drain accessibility actions`, `AT increment`, `expand from a screen reader`, `scroll into view request` | [`a11y_actions`](mod@a11y_actions) - the application half of the accessibility action path, and the reason no app writes a per-kind match of its own: [`apply_accessibility_wake`](a11y_actions::apply_accessibility_wake) drains the batch a wake receipt names and turns every action in it into the widget operation the pointer and keyboard paths already run, each scoped to the exact window that published the target node (two windows built from one `view` share widget ids, so an unscoped operation would move focus in both). [`apply_pending_accessibility_actions`](a11y_actions::apply_pending_accessibility_actions) is the ordinary-turn drain, and [`apply_accessibility_action`](a11y_actions::apply_accessibility_action) applies a single action for an app that batches work of its own beside it. Every `AccessibilityActionKind` is covered here, so a new arm reaches every app at once, and the per-widget carriers it drives (`activation`, `value_action`, [`context_menu_action`](mod@context_menu_action)) stay framework-internal. |
//! | `menu surface`, `menu rows`, `dropdown surface`, `elevated menu`, `menu container` | [`menu`](mod@menu) - the elevated, rounded, bordered menu-surface **container** (rows + chrome), not the *when* or the *placement*. Pair it with [`context_menu_host`](mod@context_menu_host) for right-click menus, [`menu_button`](mod@menu_button) for a controlled trigger, or [`titlebar_actions`](mod@titlebar_actions) for the narrow-band kebab overflow. |
//! | `popup outside the window`, `menu past the window edge`, `tooltip outside window`, `keep popups inside the window`, `xdg_popup`, `tiling window manager popup`, `popup overflow` | [`anchored_overlay`](mod@anchored_overlay) - overlays (menus, lists, tooltips, popovers, context menus) stay inside the window when they fit there and leave it, in a Wayland popup surface, only when they do not: [`PopupOverflow::WhenNeeded`](crate::anchored_overlay::PopupOverflow::WhenNeeded). [`set_default_popup_overflow`](crate::anchored_overlay::set_default_popup_overflow) changes that for the whole app, and `.popup_overflow(..)` on an [`AnchoredOverlay`](crate::anchored_overlay::AnchoredOverlay::popup_overflow) or [`FadeTooltip`](crate::tooltip::FadeTooltip::popup_overflow) for one: `Allowed` places it past the edge whenever its preferred side is there, `Never` keeps it inside. |
//! | `positioned surface`, `bare overlay`, `floating surface`, `pinned to a point`, `non menu popup` | [`positioned_overlay`](mod@positioned_overlay) - the bare placement layer (clamp / flip / outside-press and Escape dismissal with causes) with **no menu navigation semantics**: no arrows, active row, focus restoration or menu AT node. Reach for it when the surface is a floating card, palette panel or preview, not a menu; when it IS a menu, reach for [`context_menu_host`](mod@context_menu_host) instead. |
//! | `custom overlay`, `overlay layout`, `write an overlay`, `overlay with its own layout`, `legacy overlay`, `overlay node` | [`overlay_adapter`](mod@overlay_adapter) - the adapter for hand-written overlays. Upstream iced's `Overlay` trait has no `layout`: an overlay is laid out when it is created. Implement `overlay_adapter::LegacyOverlay` (the layout-taking shape) and return `overlay_adapter::element(overlay, renderer, window)` from `Widget::overlay`; the adapter lays it out once against the window and keeps the node, which `Overlay::node` reports to tests. A wrapper that only forwards another overlay implements `Overlay` directly. |
//! | `overflow toolbar`, `overflow bar`, `adaptive toolbar`, `command overflow`, `toolbar overflow menu`, `action bar`, `fold buttons into a menu`, `More button`, `right align the More button`, `align action bar`, `trailing action bar`, `right align action bar`, `stationary overflow collector`, `always visible overflow menu`, `overflow-only action`, `pin toolbar action`, `inline action limit`, `expanded to compact action`, `compact action rung`, `collapse toolbar`, `kebab next to a button`, `kebab at the far end`, `gap before the kebab`, `kebab only bar`, `name the overflow trigger`, `collector accessible name`, `collector tooltip`, `icon menu trigger` | [`action_bar`](mod@action_bar) - the shared measured command-overflow widget. It folds commands progressively, can try a declared compact inline rung before overflow, builds keyed icon+text menu rows from `enzui_core::Action`, and has explicit collector presentation/visibility/placement plus measured-vs-uniform folding and an inline limit. Per-command `Descriptor` policy pins, folds, or keeps an action overflow-only. `TrailingCluster` keeps surviving inline actions beside a stationary trailing collector; `Trailing` pins only the collector. A `Collector::Kebab` uses the toggle message for its trigger; set [`.on_dismiss(..)`](action_bar::EnzActionBar::on_dismiss) to a distinct idempotent close message for Escape, outside press, and row activation. Pair `.overflow_trigger_id(..)` with [`overflow_focus_restore_operation`] in both reducers when dismissal must restore focus. [`EnzActionBar::fit_content`](action_bar::EnzActionBar::fit_content) makes a bar whose actions all live in the menu hug its kebab, so it sits beside the control before it instead of at the far end of an empty slot. [`EnzActionBar::trigger_label`](action_bar::EnzActionBar::trigger_label) and [`trigger_tooltip`](action_bar::EnzActionBar::trigger_tooltip) name a collector that stands for a specific menu, such as an import icon, in every collector form. |
//! | `kebab`, `kebab menu`, `three dot`, `three dot menu`, `titlebar overflow`, `titlebar kebab`, `titlebar actions`, `titlebar trailing actions`, `vertical ellipsis`, `hamburger for titlebar`, `dismiss overflow menu`, `close kebab menu`, `outside click dismiss`, `light dismiss`, `collapse toolbar` | [`titlebar_actions`](mod@titlebar_actions) - the standalone/custom-band preset of [`action_bar`](mod@action_bar): measured folding, the standard toolbar-icon kebab, trailing placement, keyed menu rows, and shared dismissal. Use the toggle message for the trigger and `.on_dismiss(..)` for an idempotent close after activation, Escape, or outside press. Inside [`title_bar::EnzTitleBar`](mod@title_bar), use its budgeted [`.actions(..)`](title_bar::EnzTitleBar::actions) slot instead of pushing this adapter as generic content. |
//! | `time grid`, `week grid`, `day grid`, `calendar view`, `week view`, `agenda`, `multi-day`, `event span`, `all-day band`, `date range`, `spanning event`, `selected day column`, `selected column`, `highlight a column`, `column emphasis`, `secondary ruler label`, `secondary timezone label`, `name the second ruler`, `timezone ruler header`, `drag to create an event`, `drag selection preview`, `time range preview`, `half hour lines`, `grid lines outside working hours`, `full width event`, `lone event width`, `selected event ring`, `hour label clipping`, `schedule gutter width` | [`schedule`](mod@schedule) - the day/week time-grid surface with events. Visible hour gutters grow from the configured minimum (30px in compact mode) to fit the effective font and formatted labels on both rulers. A zero gutter outside compact mode hides the rulers. Mark one column the caller's selected day with `ScheduleColumn::selected(true)`: it paints a stronger accent wash plus a stronger header, composable with `.today(true)` so *today*, *selected* and *today+selected* are all legible (`ColumnEmphasis`) - do not draw your own 1px column outline. Name the optional second time ruler with `ScheduleConfig::secondary_label`, drawn as that gutter's header and single-line elided to its width. **Drawing a range on empty grid previews itself**: past the 2px create deadband the widget paints the rectangle it would commit, labelled with its own time range, read back from `ScheduleDrag::preview` so the pixels and the published `Previewing` proposal are one value - do not paint a host-side ghost from `Previewing`. The hour and half-hour ruler runs the **whole** visible day; inside the off-hours band it uses `ScheduleTokens::off_hours_rule` / `off_hours_minor_rule`, re-derived against that recessed plane at the same measured contrast, so an evening slot is as locatable as a midday one. Event width is not `1 / lane_count`: an event expands over the adjacent lanes that are free for its whole interval (`LaidOutEvent::lane_span`), so the only event in a slot fills the column even when a later collision widened its cluster, and the lane gap is charged only where a lane boundary is actually crossed. A selected block's ring is `schedule::selection_ring_for` - that event's own ink pulled back to the 3:1 indicator floor at `ScheduleTokens::selection_ring_width` - not the theme accent, so a red event does not wear a blue frame. |
//! | `rich text`, `formatted text`, `styled text`, `spans`, `secondary link activation` | [`rich_text`](mod@rich_text) - the **immutable display** model (styled [`RichSpan`](mod@rich_text)s, semantic tone, links). Renderer for it comes via [`block_list`](mod@block_list), [`reading_pane`](mod@reading_pane), [`code_block`](mod@code_block) and [`streaming_text`](mod@streaming_text); reach for the model when you are producing spans, and for one of those widgets when you are laying them out. [`LinkHandlers`](rich_text::LinkHandlers) routes primary/keyboard activation and secondary link presses with typed payloads and window-space points. |
//! | `rich text editor`, `wysiwyg`, `editor`, `RichEditor`, `html editor`, `formatted text input`, `formatting toolbar`, `bold italic`, `underline text`, `strikethrough text`, `paragraph alignment`, `align text`, `centre a paragraph`, `justify text`, `alignment control`, `fill editor height`, `composer editor height`, `bulleted list`, `numbered list`, `insert link`, `hyperlink in editor`, `highlight colour`, `text background colour`, `clear formatting`, `rich editor font`, `base document font`, `rich editor text size`, `canvas text font`, `rich editor typography`, `selected font family`, `selected text size`, `pending typing size`, `mixed inline typography`, `editor only`, `shared formatting toolbar`, `transparent rich editor`, `borderless document editor`, `document canvas` | [`rich_edit`](mod@rich_edit) - start with [`RichEditor`]: one value in application state, `editor.view(Message::Edit)` to show it, `editor.update(action)` for each action and `editor.to_html()` to save. Below it sits the **renderer-neutral edit model + controller** (per-byte formatting, undo/redo, coalescing, pending typing attributes) plus [`enz_rich_edit`] composite widget (an icon formatting toolbar + text area bridged through [`RichEditAction`]), [`enz_rich_edit_toolbar`](rich_edit::enz_rich_edit_toolbar), and [`to_html`](rich_edit::to_html)/[`rich_text_to_html`](rich_edit::rich_text_to_html) HTML emitters. The bare `enz_rich_edit` requests [`DEFAULT_EDITOR_HEIGHT`](rich_edit::DEFAULT_EDITOR_HEIGHT); use [`enz_rich_edit_builder`] + [`EnzRichEdit::editor_height`](rich_edit::EnzRichEdit::editor_height) to make the editor fill a bounded dialog. [`EnzRichEdit::font`](rich_edit::EnzRichEdit::font) pins a base family while preserving application regular weight and inline formatting; [`font_role`](rich_edit::EnzRichEdit::font_role) and [`font_from_theme`](rich_edit::EnzRichEdit::font_from_theme) match the text area catalog contract. [`text_size_px`](rich_edit::EnzRichEdit::text_size_px) pins absolute logical pixels; unpinned sizes follow desktop scaling. Hosts persist base font and size separately from HTML formatting. [`SetFont`](rich_edit::RichEditAction::SetFont) and [`SetTextSize`](rich_edit::RichEditAction::SetTextSize) apply selected-range or pending-caret overrides, persisted inline in HTML and undo history. [`active_font`](rich_edit::RichEditController::active_font) and [`active_text_size`](rich_edit::RichEditController::active_text_size) report shared or pending values; clearing an override restores base inheritance. Native shaping owns per-run row metrics, caret, selection, wrapping and intrinsic height. [`show_toolbar(false)`](rich_edit::EnzRichEdit::show_toolbar) removes the toolbar and its spacing; compose [`enz_rich_edit_toolbar_builder`](rich_edit::enz_rich_edit_toolbar_builder) once above the document with `.alignment(..).actions(..).colors(..)` for the full formatting surface. [`editor_variant(Plain)`](rich_edit::EnzRichEdit::editor_variant) selects a transparent square editor with no idle, hover or focus border while preserving native glyphs, caret, selection and hit geometry. [`editor_tokens`](rich_edit::EnzRichEdit::editor_tokens) supplies shared text-area inks and padding, overriding page tokens in either builder order; `.paper(..)` still selects authored-ink contrast against the host backdrop. Different from [`markdown`](mod@markdown) (a read-only CommonMark renderer that takes a `&str`) and from [`text_area`](mod@text_area) (multi-line plain-text input, no per-character formatting). Bold, italic, code, tone, underline and strike-through paint in the editable surface itself and not only in the model and the emitted HTML, so the toolbar state, the document and the glyphs the user is editing cannot disagree. Paragraph alignment is offered by [`EnzRichEdit::paragraph_alignment`](rich_edit::EnzRichEdit::paragraph_alignment) and routed by [`SetParagraphAlign`](rich_edit::RichEditAction::SetParagraphAlign) - a single-select group chosen from [`RichEditController::active_paragraph_align`](rich_edit::RichEditController::active_paragraph_align), showing nothing for a selection whose paragraphs disagree. The composite toolbar also offers bulleted and numbered lists ([`ToggleList`](rich_edit::RichEditAction::ToggleList); Enter continues a list and ends it on an empty item), links through a URL popover, one colour button whose popover holds text colours and highlight (marker) colours, and clear formatting; all of them round-trip through [`to_html`](rich_edit::to_html) and [`from_html`](rich_edit::from_html) as `<ul>`/`<ol>`, `<a href>` and `background-color`. The editable surface moves the glyphs, and the caret, the selection rectangles and the hit test move with them, so the toolbar state, the document, the emitted HTML and the glyphs the user is editing cannot disagree. |
//! | `markdown`, `commonmark`, `md`, `document renderer`, `markdown table`, `GFM table`, `markdown image`, `image in markdown`, `image resolver`, `heading size`, `nested list`, `manual Markdown blocks`, `deep Markdown tree`, `stack-safe Markdown drop`, `link context menu`, `quiet link`, `link presentation`, `link underline policy`, `link hover colour`, `link focus colour`, `right-click link`, `task list`, `checklist`, `todo list`, `markdown checkbox`, `tick a task` | [`markdown`](mod@markdown) - a read-only CommonMark renderer: `enz_markdown(&str).into_element()`. Not an editor - it is one-way source-to-element. [`enz_markdown_blocks`](markdown::enz_markdown_blocks) also accepts caller-built block trees: parse and render share a 64-level nesting cap and visible truncation marker, while rendering stays flat and destruction is iterative even for a deeper manual Quote/List tail. The alpha [`EnzMarkdownBlock`](markdown::EnzMarkdownBlock) enum is deliberately non-exhaustive, so downstream matches require a wildcard and future block/safety-marker variants are not another source break. [`EnzMarkdown::on_link_context`](markdown::EnzMarkdown::on_link_context) supplies the URL under a secondary press and its window-space point without triggering primary activation or a selection menu. Images resolve through [`EnzMarkdown::image_resolver`](markdown::EnzMarkdown::image_resolver) (the framework fetches nothing; an unresolved image shows its alt text); GFM tables keep column alignment and publish as a grid with column headers; headings scale with `body_size` unless [`EnzMarkdown::heading_scale`](markdown::EnzMarkdown::heading_scale) pins them. GFM task items (`- [ ]`) show a checkbox named by the item text; [`EnzMarkdown::on_task_toggled`](markdown::EnzMarkdown::on_task_toggled) lets the reader tick it and reports the marker's source range, which [`TaskToggle::apply`](markdown::TaskToggle::apply) rewrites. Pair with [`reading_pane`](mod@reading_pane) for long-form layout. Shared [`rich_text`](mod@rich_text) link presentation tokens provide rest/hover/focus tones and Always/HoverFocus/Never underline policy through `EnzMarkdown::link_presentation`, while preserving link actions, selection and focus rings. |
//! | `preview`, `read only rich text`, `article view`, `long form` | [`reading_pane`](mod@reading_pane) - long-form document renderer for prose blocks. For MiB-scale immutable chapters, [the revision-aware constructor](reading_pane::enz_revisioned_reading_pane) uses [`revision_text`](mod@revision_text) so an unrelated warm relayout validates content in O(1) without flattening or scanning it. See also [`block_list`](mod@block_list) (headings/paragraphs/quotes/lists) and [`markdown`](mod@markdown) (for a CommonMark source). |
//! | `revision text`, `immutable text snapshot`, `rope text`, `large text relayout`, `lazy text materialization` | [`revision_text`](mod@revision_text) - opt-in identity + monotonic revision text layout. A [snapshot](revision_text::TextSnapshot) materializer runs only on a cold miss or changed revision; ordinary `iced::widget::text(&str)` remains the exact-comparison safe default. |
//! | `large file`, `paged text`, `lazy text`, `large immutable text`, `random access text`, `virtualized text`, `virtualised text`, `line cache`, `line index coverage`, `partial line index`, `sparse tail`, `source byte range`, `paged scrollbar`, `byte seek`, `huge log viewer`, `read only large document`, `visible rows`, `actual allocation`, `viewport state`, `paged viewport`, `large document font`, `paged monospace`, `large viewer text size`, `bottom align the tail`, `empty space after End`, `sparse tail alignment`, `spacer above the viewer`, `keyboard navigation stops after End`, `binary preview`, `hex viewer`, `ASCII byte preview`, `byte address` | [`paged_text_viewer`](mod@paged_text_viewer) - visible-row-only **random-access** viewer over `enzui_core::paged_text` for a large, already-complete UTF-8 document (a file on disk, a captured log): integrated line numbers, distinct source-byte and partial line-index status, document semantics, keyboard/page/wheel navigation, byte-progress overlay scrollbar with bounded absolute seeks, and bounded overscan. It seeks anywhere in an immutable body; for text that is still *arriving* token-by-token use `streaming_text` instead. It never flattens through an editor or the `revision_text` snapshot path. The realised row window is resolved from the viewer's **actual allocation** under `Length::Fill`, so no app subtracts titlebar, banner or status chrome, and the accessible range announces only the on-screen rows and never the clipped overscan; [`PagedTextViewportState`] publishes that resolved geometry read-only for update-time navigation, and a fully fixed size keeps the deterministic eager path. Use [`.font_from_theme(theme)`](paged_text_viewer::EnzPagedTextViewer::font_from_theme) for the monospace catalog role and [`.text_size_px(px)`](paged_text_viewer::EnzPagedTextViewer::text_size_px) when the viewer must follow an editor zoom. **The tail gap is the viewer's, not the caller's:** a short terminal frame is bottom-aligned inside the viewer's own body ([`tail_alignment_rows_at`](paged_text_viewer::EnzPagedTextViewer::tail_alignment_rows_at) reports the reserved rows), so mount the viewer as one unconditional child. A spacer the host mounts only at the tail changes the tree shape above the document, iced discards the subtree state below it, and the keyboard focus that routes Home, Page Up, Page Down and the arrows goes with it - leaving a document that reaches its end and can never be paged back. [`enz_paged_byte_viewer`] reuses this read-only shell with bounded fixed-width hex/ASCII rows, original byte-address status/navigation and horizontal overflow; prepare `enzui_core::paged_bytes::PagedByteSource` off-thread and retain its typed state/viewport. |
//! | `typography`, `text primitive`, `emphasize a range`, `bold the matched word`, `search match emphasis`, `emphasis span`, `font role`, `type scale`, `styled text primitive`, `shaping`, `kerning`, `ligatures`, `typography policy`, `ASCII width changes`, `no ligatures`, `basic shaping`, `advanced shaping`, `code text shaping` | [`enz_text`](mod@text) - the token-driven typography primitive every other widget renders text through; [`text::TextShaping`] defaults to compatibility `Auto`; prose can request stable `Advanced` shaping to match rich text, while `Basic` remains an explicit controlled-code/chrome policy. [`label`](mod@label) and [`text_pair`](mod@text_pair) forward explicit policies. Prefer this over raw `iced::widget::text`. |
//! | `text color ignored`, `container text_color does nothing`, `text_color no-op`, `set text colour on container`, `inherit text colour`, `ink does not inherit`, `declared ink never lands`, `on_accent not applied`, `text stays the wrong colour` | [`inherited_ink`](mod@inherited_ink) - the rule, and the [`InkProbe`](inherited_ink::InkProbe) that measures it on painted pixels. A container's `text_color` is one **inherited channel**, read only by a leaf that declined to choose: `iced::widget::text` reads it, [`enz_text`](mod@text) does **not** (it pins its own ink, which is what `TextTone`/`subtle_on` are), and an [`icon`](mod@icon) never does (an `svg` has no text channel at all). So a `container` around an `enz_text` whose style sets `text_color: Some(x)` is dead code - the shape that shipped a calendar's *today* marker at **2.63:1**. Pin the run with [`.color(x)`](text::EnzText::color), or opt the run into the channel with [`.inherit_color(true)`](text::EnzText::inherit_color). |
//! | `pixel gate cannot fail`, `vacuous colour assertion`, `assert a colour on pixels`, `icon tint gate`, `glyph is not the accent`, `probe found no ink`, `which palette did the frame paint`, `stock iced theme in a test`, `inconclusive not ok` | [`color_presence`](mod@color_presence) - headless color counts with a rendered control, repeated-subject check, and explicit palette provenance. [`ColorProbe`](color_presence::ColorProbe) uses one enzUI theme; present and absent verdicts compare matching pixels with a configurable floor. Inconclusive results reject invalid controls, foreign palettes, or unstable renders. |
//! | `subtle`, `weak ink`, `muted contrast`, `caption contrast`, `selected row text`, `selected tree row subtitle`, `section header`, `group label`, `non-text contrast`, `icon contrast`, `large text contrast`, `3:1`, `ink on fill`, `legible ink`, `on-fill contrast`, `arbitrary fill`, `calendar colour`, `tag colour` | [`enz_text`](mod@text) with [`.subtle_on(backdrop)`](text::EnzText::subtle_on) or [`.muted_on(backdrop)`](text::EnzText::muted_on) - resolve tertiary or secondary ink against the surface it actually lands on, especially `surface_selected`; raw `text_subtle` / `text_muted` remain unchanged. For a custom [`tree_view`](mod@tree_view) body, read the actual accent-soft selection or panel backdrop from [`EnzTreeView::resolved_row_fill`](tree_view::EnzTreeView::resolved_row_fill), not `surface_selected`. Qualifying large text uses [`EnzText::subtle_on_with_floor`](text::EnzText::subtle_on_with_floor) with `enzui_theme::ContrastFloor::LargeText`; a heading role by itself does not qualify. A meaningful graphical icon governed by WCAG SC 1.4.11 uses [`EnzIcon::tint_theme_subtle_on_with_floor`](icon::EnzIcon::tint_theme_subtle_on_with_floor) with `enzui_theme::ContrastFloor::NonText` so it stops at 3:1 instead of over-promoting to body-text contrast. The same subtle-ink stopgap exists for [icons](icon::EnzIcon::tint_theme_subtle_on), [icon labels](icon_label::EnzIconLabel::subtle_on), [text-grid cells](text_grid::TextGridCell::subtle_on), and a [section/group header that owns its label](section_header::EnzSectionHeader::subtle_on), and a [window title's owned subtitle](window_title::EnzWindowTitle::subtle_on) (also [`.muted_on(backdrop)`](window_title::EnzWindowTitle::muted_on), which the *default* subtitle tone needs: raw `text_muted` measures only 4.35:1 on the light preset's `surface_alt` - the fill a default [`header_bar`](mod@header_bar) paints - and 3.68:1 on `surface_selected`). A parent fill is not available to these leaves through iced theme context, so ordinary `.tone(Subtle)` / `.tone(Muted)` remain byte-identical and callers over a non-default backdrop opt in. For text or glyph ink painted on an **opaque caller-owned fill** (calendar/tag colours), use `tokens::ink_for_with_floor` so success proves a semantic `enzui_theme::ContrastFloor` and failure is explicit; `tokens::ink_for` remains the best-effort compatibility resolver. |
//! | `disabled ink`, `disabled contrast`, `transparent disabled control`, `disabled on backdrop`, `disabled_on` | [`menu_button`](mod@menu_button) and [`sidebar_row`](mod@sidebar_row), plus their shared internal button primitive, expose `.disabled_on(backdrop)` for the three widget-owned disabled inks that can land on a transparent control over an unknown parent. The composing caller must pass the opaque fill it actually owns; leaving the builder unset preserves the tested refusal to guess `surface` or `background`. Filled controls already resolve against their own disabled fill. |
//! | `persisted highlight`, `annotation ink`, `highlight marker ink`, `yellow marker`, `highlighter pen`, `reading highlight`, `highlight over paper`, `translucent highlight fill`, `composited highlight`, `annotation on paper`, `Text highlight ink`, `Editor highlight ink`, `Span::highlight ink` | [`tokens::annotation_glyph_ink`] in [`tokens`](mod@tokens) - the single foreground role for prose sitting inside a persisted annotation highlight (a user's "yellow marker", a `RichText` span's `.highlight = Some(bg)`). A persisted marker is caller-owned data (Foliate highlighter colours, a `text_editor` background), often translucent, so the pixel painted is the fill composited over the reading paper; prose ink chosen against the paper is **not** the ink the reader experiences. On the shipped Dark reading paper (`#222222`) plus a `yellow` marker (`#FFE066` at 0.55 alpha) the composite is a warm gold near `#9C8B47`, and the theme's light prose ink `#E0E0E0` measures **2.47:1** against it - below WCAG AA. The helper composites the fill over the paper, keeps `prose_ink` byte-identical when it already clears the floor (an annotation is not license to relight legible prose), and falls back to the higher-contrast of pure black/white against the composite otherwise. Not a chrome-palette resolver: the reading paper is a per-book theme choice independent of the app palette, and the paper is caller-owned data so the helper cannot invent a backdrop. |
//! | `tone ink`, `success ink`, `warning ink`, `error ink`, `danger text`, `status text on card`, `tone on backdrop`, `tone_on`, `semantic tone contrast`, `tone_success_fg on surface_alt`, `tone_warning_fg on surface_alt`, `tone_error_fg on elevated` | [`enz_text`](mod@text) with [`.tone_on(TextTone, backdrop)`](text::EnzText::tone_on) for Success, Warning, or Danger resolves the audited `tone_*_fg` role against the opaque surface the run actually paints on, backed by [`Palette::tone_on`](enzui_theme::Palette::tone_on) and [`SemanticTone`](enzui_theme::SemanticTone). The raw palette role is retained byte-identical when it already clears the 4.5:1 body-text floor - including a customised `tone_*_fg` supplied by a downstream palette. A shipped-preset role that misses the floor on a non-default surface (light Success/Warning on `surface_alt`, dark Error on `surface_elevated`) is lifted on its OkLab hue and chroma; a hue whose chroma cannot reach the floor against `backdrop` degrades to the honest best-available colour, and callers who need the three-state outcome should read `Palette::tone_on(_).meets_floor()` directly. Existing `.tone()` callers for Success, Warning, or Danger keep the raw token, matching the additive contract of the subtle/muted/disabled family. |
//! | `wrap text`, `word wrap`, `multi line text`, `text is cut off`, `text truncated`, `sentence disappears`, `missing end of sentence`, `elide`, `ellipsis`, `truncate label`, `single line label`, `stop text wrapping`, `keep row height fixed` | [`enz_text`](mod@text) wraps by **default** (`Wrapping::Word`, matching iced), so prose reflows instead of silently losing its tail at narrow widths. For the *label* case - a table cell, a chip, a status pill, anything whose row height must not move - call [`.no_wrap()`](text::EnzText::no_wrap). If a truncated label is itself a **defect** you want reported, use [`.single_line_elided()`](text::EnzText::single_line_elided) instead: same geometry, plus an `AccessibilityCheck::Elision` audit node. Note [`icon_label`](mod@icon_label) deliberately still defaults to single-line (its label is a *name* in a row; wrapping blew a file-list row from 18 px to 144 px - see its `wrap` docs). |
//! | `page heading`, `screen title`, `h1 h2 h3`, `heading level`, `aria-level`, `document outline`, `skim by heading`, `next heading`, `title that is a heading`, `large heading` | [`enz_text`](mod@text) with `.heading(HeadingLevel::..)` - the **semantics-only** heading claim. It publishes `Role::Heading` + `aria-level` so "next heading" / "list headings" find the text, and changes **no pixel**: keep your `.size(TextSize::Title1/2/3)`, tone and weight exactly as they are. This is the answer for the ~50 title-rung headings that cannot adopt [`section_header`](mod@section_header) without shrinking to an 11 px caption. Levels are **named rungs** (`Page`/`Section`/`Subsection`/`Detail`), never numbers copied off the type ramp - a large number or a big host name is a **value**, not a heading. The discipline that keeps this honest is `enzui_core::heading_outline`, a per-frame audit (one Page rung, no level skips, every heading named, and a density limit); wire `HeadingOutline::assert_sound` into your per-screen a11y sweep. |
//! | `title subtitle`, `caption row`, `icon title subtitle`, `list item label`, `SwiftUI Label`, `icon and text`, `symbol label`, `leading icon text`, `icon label`, `label style`, `file label`, `desktop artwork`, `handle-based label` | [`label`](mod@label) - **the** icon + title (+ optional subtitle) label, one model with icon-only / title-only / both display (SwiftUI's one `Label` varied by `labelStyle`). `enz_label` presets the form/preferences shape (shrink-wrapped, square icon frame, muted glyph, subtitle); `enz_icon_label` presets the row shape (fills width, intrinsic icon, single-line elided) and [`icon_label`](../crates/enzui-widgets/src/icon_label.rs)'s `EnzIconLabel`/`IconLabelTone`/`IconLabelDisplay`/`IconLabelA11y` are aliases of the canonical builder and enums. Either preset reaches every capability - tones, glyph escape hatch, icon slot, wrapping, scale forwarding, decorative a11y, subtitle. `.icon_source(IconSource)` accepts bundled names or cached SVG/raster handles; artwork retains its colors and the label owns one aggregate accessible name. Not for a value column ([`labeled_content`](../crates/enzui-widgets/src/labeled_content.rs)), protected trailing metadata ([`text_pair`](../crates/enzui-widgets/src/text_pair.rs)), heading semantics ([`section_header`](../crates/enzui-widgets/src/section_header.rs)) or window chrome ([`window_title`](../crates/enzui-widgets/src/window_title.rs)). |
//! | `label value`, `key value row`, `detail row`, `field value`, `label value gap`, `compact label spacing`, `LabeledContent`, `labeled content` | [`labeled_content`](mod@labeled_content) - a form label-and-value row (label beside value, or stacked in compact mode); use it for computed read-only properties instead of hand-building a preferences row. Values accept owned strings, and [`.row_spacing(..)`](labeled_content::EnzLabeledContent::row_spacing) controls the between-cell gap in either arrangement. |
//! | `leading trailing text`, `metadata row`, `title with metadata`, `truncating pair`, `text pair`, `protected trailing`, `metadata that must not wrap`, `no-wrap timestamp`, `trailing badge` | [`text_pair`](mod@text_pair) - single-line leading text plus protected trailing metadata. [`enz_text_pair`] takes a trailing **string** (end-elided); [`enz_text_pair_element`] takes a trailing **`Element`** (an icon + date, a badge, a status dot, a no-wrap timestamp) whose reserved width is *protected*, not elided - the leading side yields first. |
//! | `group heading`, `group label`, `list header`, `caps header`, `settings header`, `caption band`, `all caps label`, `section header`, `sticky header`, `sticky section header`, `pinned list header`, `header stays in view` | [`section_header`](mod@section_header) - the 11 px muted all-caps **caption** above a form group or sidebar rail. When the caption uses the tertiary tone over a non-default fill, call [`.subtle_on(backdrop)`](section_header::EnzSectionHeader::subtle_on) so the widget-owned label resolves legible ink. To keep a list section's caption in view while its rows scroll, call [`.sticky_on(backdrop)`](section_header::EnzSectionHeader::sticky_on) and make it the first child of that section's own column. **Not a page heading**: it is deliberately smaller and fainter than body text, so using it to introduce a page section inverts the hierarchy. For that, see the `page section` row - that card publishes its bold title (17 px at baseline text scale) as a `HeadingLevel::Section` heading (aria-level 2), so heading navigation reaches it directly. |
//! | `page section`, `visual grouping`, `group content`, `section title`, `page section heading`, `stacked sections` | [`card`](mod@card) with `CardTone::Flat` - the bold title (17 px at baseline text scale) with **no** box: `enz_card(body).title("Repository").tone(CardTone::Flat)`. This is the answer for *"a top-level section of a page"*. The title publishes a `HeadingLevel::Section` heading (aria-level 2) so screen-reader heading navigation reaches it; other tones stay ordinary labels because a `Default`/`Elevated`/`Outlined`/`Sunken` card is *an entity*, not a section of the outline. It is **not** `section_header`, which is an 11 px muted *caption* for a form-group rail - That swap is a hierarchy inversion: the heading comes out smaller and fainter than the body text under it. It is **not** a bordered card either: stacking `Outlined` cards down a page draws a border around every section and separates nothing. For a *settings* pane reach for `preferences_group`, which is this same card with the settings rhythm and is flat by default. |
//! | `monospace grid`, `tui output`, `terminal text grid`, `character grid`, `dense text`, `text grid`, `Nerd Font glyph`, `Powerline icon`, `private-use glyph`, `oversized terminal icon` | [`text_grid`](mod@text_grid) - a single-row dense text grid for TUI-style output. A nonzero [`TextGridTokens::glyph_bleed_x`](text_grid::TextGridTokens::glyph_bleed_x) opts terminal callers into extra horizontal paint room; known Nerd Font folder symbols (including Starship's U+F07C) receive a bounded 1.5× font size and wider clip, while unrelated private-use icons and ordinary text keep the configured font size and every cell keeps its original advance. |
//! | `terminal performance`, `line damage`, `damage tracking`, `row revision`, `unchanged row`, `skip reshaping`, `fast terminal redraw` | [`text_grid`](mod@text_grid) - [`.content_revision(..)`](text_grid::EnzTextGrid::content_revision) lets a row the caller marks unchanged reuse last frame's shaped glyphs without reading its cells; one-byte ASCII cells resolve through a direct table either way, and [`TextGridCell::ch`](text_grid::TextGridCell::ch) builds a cell without allocating. |
//! | `cursor`, `terminal cursor`, `cursor style`, `caret`, `beam`, `underline`, `underline cursor`, `bar cursor`, `block cursor`, `squiggle`, `spellcheck underline`, `error underline`, `search match marker` | [`text_grid`](mod@text_grid) - [`TextGridCellDecoration`] via [`TextGridCell::decoration`](text_grid::TextGridCell::decoration) paints an `Underline` or a `LeadingBar` (beam/bar cursor) over a cell in **its own ink**, leaving the glyph, its colour, the background and the row's accessible label alone. A **block** cursor is not a decoration - it is the cell's existing `bg`/`color` swap. Without the decoration API a grid can only recolour a cell, so a block is the only cursor it can paint whatever the user chose. |
//! | `select text in a grid`, `drag to select cells`, `cell under the pointer`, `terminal selection`, `click a cell`, `text selection`, `select a line`, `copy text from a grid`, `mouse in a terminal`, `pointer to cell`, `column under the mouse`, `URL click to open`, `OSC 8`, `link under the cursor`, `hover a cell` | [`text_grid`](mod@text_grid) - pointer routing in **cell** coordinates: [`.on_cell_press(..)`](text_grid::EnzTextGrid::on_cell_press), [`.on_cell_drag(..)`](text_grid::EnzTextGrid::on_cell_drag), [`.on_cell_release(..)`](text_grid::EnzTextGrid::on_cell_release) and [`.on_cell_hover(..)`](text_grid::EnzTextGrid::on_cell_hover) report a [`TextGridPointer`] - a saturating `column`, a signed `row_offset` in whole cell heights, and a [`TextGridEdge`] that distinguishes *on the last column* from *past the end of the line*, so drag-select to the edge is expressible. The widget **reports; it does not own a selection** - it knows one row and nothing about scrollback, reflow or wrapped lines, so the model belongs to the app. A grid with a press/drag handler also reports an I-beam cursor, and [`.cell_interaction(..)`](text_grid::EnzTextGrid::cell_interaction) overrides that per column (a pointer over an OSC-8 link). Do **not** reach for `selection_scope` here (deliberately unlinked, so this row's intents stay attributed to `text_grid` alone): it collects text regions by walking children for a labelled semantics node, so it wraps text-widget children, not a paint grid. |
//! | `emoji cell`, `flag emoji`, `grapheme cluster row`, `text to grid cells`, `string to cells`, `split text into cells`, `combining mark cell`, `ZWJ emoji cell`, `double width column`, `CJK two columns`, `cursor column not char count` | [`text_grid`](mod@text_grid) - [`cluster_cells`](text_grid::cluster_cells) builds a grid row from a `&str` with **one cell per grapheme cluster**, each reporting the columns it occupies; [`cluster_columns`](text_grid::cluster_columns) is the matching column count for placing a cursor. Reach for these instead of `text.chars().map(..)`, which tears clusters apart: a US flag is one cluster of two `char`s, so a per-`char` split paints the letters **US** rather than a flag, and a `chars().count()` cursor drifts right on combining marks and left on wide glyphs. |
//! | `gutter`, `line numbers`, `code gutter`, `editor gutter`, `wrapped line numbers`, `logical line gutter`, `embedded gutter`, `embedded surface`, `transparent gutter`, `gutter without background`, `flat gutter`, `windowed gutter`, `large document gutter`, `gutter survives resize`, `restore gutter scroll` | [`line_number_gutter`](mod@line_number_gutter) - a line-number gutter that stays aligned under word wrapping. A scrolled wrapped editor must pass the editor-owned first logical line / content-space top through [`GutterConfig::window`](crate::line_number_gutter::GutterConfig::window), even for a small document: independently shaping the whole prefix and applying a sparse editor offset can put logical line 130 beside label 34/66/77. Pass the same viewport's absolute offset through [`GutterConfig::scroll_offset`](crate::line_number_gutter::GutterConfig::scroll_offset), so a responsive re-parent starts at the mapped window instead of zero. The seed applies only to fresh state; the shared `.scroll_id(..)` continues to drive live scrolling. |
//! | `measure a row`, `row height`, `how tall is this row`, `measured row height`, `variable row height`, `correct row estimate`, `measure laid out widget`, `intrinsic height`, `actual height`, `report my height`, `list row heights` | [`row_measure`](mod@row_measure) - the producer the variable-height virtual-list measurement-correction loop (`enzui-core`'s `virtual_sequence`) was missing: it lays a row out a second time with no height ceiling and publishes the height the imposed slot hid. Use it through [`chat_list`](mod@chat_list)'s `.on_measure(..)`, and feed the batch to [`reconcile_measurements`](chat_list::reconcile_measurements). Do **not** reach for `iced::widget::Sensor::on_resize` here - it reports the size the row was *given*, which inside a virtual list is the guess you are trying to correct. For text alone, [`text_measure`](mod@text_measure) is exact and cheaper. |
//! | `measure text`, `text height`, `intrinsic text size`, `size text`, `wrapped line count`, `how many rows`, `grow text area`, `auto-grow text`, `fit to content`, `auto size text`, `content height`, `rows for text`, `paginate prose`, `page break`, `pager`, `break offset`, `text fits page` | [`text_measure`](mod@text_measure) - font-backed text height, height-derived visual-line estimates, and byte break offsets through the renderer's shaper. `enz_text_break_offset_with` returns where the first visual line after a line budget begins, while `paginate_by_height_with` returns lossless page ranges, emitting an over-height legal span when needed to make progress. This is also the row-count answer a growing [`text_area`](mod@text_area) needs: `enz_text_measure_with(text, width, typo).line_count` estimates the number of *wrapped rows*, which `Content::line_count` (logical `\n` lines) is not. For an auto-growing composer prefer [`text_area`](mod@text_area)'s own [`.auto_grow(..)`](text_area::EnzTextArea::auto_grow), which uses native editor layout; [`.row_count(..)`](text_area::EnzTextArea::row_count) forwards the area's complete font, shaping, wrapping, and line-height descriptor here for manual sizing. |
//! | `baseline`, `text baseline`, `bold text sits lower`, `bold label misaligned`, `mixed weight row`, `current breadcrumb shifted down`, `heavier run one pixel lower`, `weights do not line up`, `align text with its neighbour`, `share a baseline` | [`text_baseline`](mod@text_baseline) - [`seat_on_baseline_of`](text_baseline::seat_on_baseline_of) seats a run of one weight on the painted baseline another font occupies at the same size. Vertical centring aligns *line boxes*, and the baseline inside a line box comes from the face's own ascent/descent, so a bold run beside regular neighbours paints a pixel off while reporting an identical rectangle - which is why a bounds assertion cannot see it. Measure a single run's own baseline with [`baseline_offset_px`](text_baseline::baseline_offset_px). Where both faces already agree the seat is a no-op and changes no pixel. |
//! | `window heading`, `bold title subtitle`, `page title text`, `window title`, `sidebar title`, `sidebar heading`, `title heading`, `title heading level`, `title aria-level` | [`window_title`](mod@window_title) - a bold title with an optional muted subtitle. Over a non-default fill the subtitle's weak ink opts in with [`.muted_on(backdrop)`](window_title::EnzWindowTitle::muted_on) or [`.subtle_on(backdrop)`](window_title::EnzWindowTitle::subtle_on). A title used as **pane / sidebar chrome** is document structure, not window chrome: name its rung with [`.heading(HeadingLevel::..)`](window_title::EnzWindowTitle::heading) and the title's own label node is promoted in place - one node, no wrapper, no extra focus stop, identical pixels. Off by default, so a window title stays a plain label. Read the module's window-title / sidebar-heading / page-heading / muted-caption table before picking: the muted 11 px all-caps band above a group of rows is [`section_header`](mod@section_header), and adopting it for a pane title inverts the visual hierarchy. |
//! | `button radius`, `square button corners`, `button group radius` | [`button`](mod@button) - unpinned buttons resolve `radius.xs` through `radius.xl` by size (3/5/6/10/12 px by default); palette-only overrides leave corners live. `EnzButton::size_tokens` pins all geometry. [`button_group`](mod@button_group) outer corners use live `radius.md` (6 px); `.corner_radius(px)` pins them. Inner group corners stay square, and neither family changes layout when radii change. |
//!
//! | `push button`, `cta`, `primary button`, `action button`, `active filter`, `filter chip`, `filter pill`, `choice chip`, `toggle chip`, `multi-select filter`, `visible selected state`, `plain button`, `neutral button`, `neutral text action`, `no chrome button`, `chromeless button`, `flat button`, `flat at rest`, `text button`, `quiet button`, `borderless destructive`, `quiet danger`, `destructive text button`, `hover fill no border`, `ghost button`, `hover text color`, `ink brightens on hover`, `flat at rest brightens on hover`, `circular button`, `round button`, `fab`, `send button`, `circular icon button`, `path button`, `middle ellipsis`, `preserve filename`, `keep path suffix`, `elide path middle`, `accessible description`, `describe a button`, `tooltip for a screen reader`, `hint on a button`, `what does this button do`, `click to copy value` | [`button`](mod@button) - the default styled button component. [`enz_filter_chip`] is the compact multi-select filter form: outlined when inactive, accent-filled when active, and `aria-selected` in both states. Button labels end-elide by default; for a filesystem path or other identity whose suffix disambiguates it, call [`EnzButton::ellipsis`](crate::button::EnzButton::ellipsis) with `iced::widget::text::Ellipsis::Middle` so the constrained control retains both the prefix and filename. For a quiet text-only **in-app action** with no hover feedback at all, use `ButtonKind::Plain`: it has neutral `palette.text` ink and no fill or border in any state. When that same neutral, borderless action should also **light up under the pointer** - a "See all", a quiet row action - use `ButtonKind::Quiet`: transparent at rest, neutral `palette.text` ink, no border, with a hover/pressed wash derived from the shared overlay tokens. `Quiet` is the text-button twin of `ToolbarIconVariant::Standard` on the icon side. These variants exist because `plain button`, `no chrome button`, `chromeless button`, `neutral text action`, `hover fill no border` each returned zero hits while `text button` resolved to the accent-inked `ButtonKind::Ghost`, which is a blue link. `ButtonKind::Ghost` remains the accent-ink alternative; override `ButtonTokens::ghost_hover_fg` / `ghost_pressed_fg` when that accent ink should brighten on hover. The danger ladder has three rungs, not two: `ButtonKind::Destructive` is the filled red block and `ButtonKind::DestructiveOutlined` is *always* bordered in the danger colour, so neither can join a **borderless** action row (a Reply / Reply All / Forward / Block / More family where Block reads as destructive through ink, not a different pill). `ButtonKind::QuietDestructive` is that missing rung - the danger twin of `ButtonKind::Quiet`: transparent with zero border at rest, the shared overlay hover/pressed wash, and contrast-audited danger ink (`ButtonTokens::quiet_destructive_fg` = `tone_error_fg`, legible on the app surface where the raw `destructive` fill is not) - and it publishes the SAME destructive disposition to assistive technology as the other two, because the ink is the only warning left once the chrome is gone. If activation navigates to a URI, use `enz_link`, not either button kind: it publishes link semantics. For a **circular** icon button on the full styled button (a composer send affordance, a FAB) rather than the titlebar `toolbar_icon`, declare it icon-only and pin square geometry so `radius = height/2` produces a circle: `.icon_position(ButtonIconPosition::Only).size_tokens(ButtonSizeTokens { padding_x: (h - icon_size) / 2.0, radius: h / 2.0, ..base }).aria_label("Send")` - a pinned `size_tokens.padding_x` now wins over the icon-only default so the box stays square instead of collapsing to a pill, and an icon-only button must carry an `aria_label` since it has no text for a name. When the affordance is not in the name - a `Plain` button whose label is a datum and whose press copies it - add [`EnzButton::aria_description`](crate::button::EnzButton::aria_description): it publishes the sentence a hovering pointer gets from a [`tooltip`](mod@tooltip) as the node's accessible description, so a keyboard or screen-reader user receives it too (a tooltip bubble is mounted only while hovering, so its text reaches the tree at no other time). Do not fold the hint into `aria_label`: the name is what AT matches on and what voice control speaks to click the control. |
//! | `joined buttons`, `segmented buttons`, `button bar`, `split button row` | [`button_group`](mod@button_group) - an action cluster of joined buttons. For a single-choice segmented control use [`view_switcher`](mod@view_switcher). |
//! | `hyperlink`, `anchor`, `text link`, `link button`, `href` | [`link`](mod@link) - a styled hyperlink for URI navigation, with link semantics. The term `link button` is recorded here because a caller who wanted "a button that looks like a link" got zero hits: reach for [`link`](mod@link) when activation navigates to a URI, and for [`button`](mod@button) with `ButtonKind::Plain` when it is a quiet in-app action that sends an application message. |
//! | `tappable surface`, `selectable surface`, `hover press surface`, `button-like surface`, `right click a pressable`, `right click a card`, `pressable context menu`, `pressable on_context`, `secondary click a pressable`, `grid tile`, `grid cell open on Enter`, `tile Enter opens`, `tile on_enter`, `open focused tile`, `Enter opens focused tile`, `pressable Enter distinct from Space`, `AT default action opens tile` | [`pressable`](mod@pressable) - a clickable surface with button-like hover/press/selected behaviour that must not look like a button. For a **right-click / context menu** on a pressable, compose with [`context_target`](mod@context_target): `enz_pressable` deliberately has no `on_context` of its own (unlike [`list_row_surface`](mod@list_row_surface) and [`sidebar_row`](mod@sidebar_row), which are row-shaped and publish the routable `ShowContextMenu` AT node at row scope), so `enz_context_target(enz_pressable(..), Msg::OpenMenu)` is the intended composition and the `iced::Point` it reports is already in the window/viewport space [`context_menu_host`](mod@context_menu_host) anchors on - no coordinate translation, and no risk of the low-level pressable silently swallowing the row-level hook. **Enter vs Space:** `.on_enter(..)` is the grid-tile half of the typed split - pointer click and Space keep firing the tile's activation ("select") while Enter and a screen reader's default action publish the `on_enter` message ("open"), so a grid opens the *focused* tile without a global key subscription guessing which tile that is. It is an override of an already-enabled tile (one with no activation stays disabled), and an `embedded` tile leaves Enter to its owning roving-focus composite. The row-shaped twin is [`list_row_surface`](mod@list_row_surface)`::on_enter`. |
//! | `make clickable`, `wrap as button`, `content follows hover`, `hover disc`, `pressed content`, `round shutter`, `status aware content`, `tap wrapper`, `clickable content` | [`clickable`](mod@clickable) - wraps arbitrary content as a button (role, focus, Enter/Space, hover, minimum hit target). Content can read the control's live state with `enz_clickable_with_status`: move the supplied handle into a child style/draw callback and call `.get()` there. It retains content geometry and delegates hover, press, keyboard and accessibility to the clickable. |
//! | `accessible name from content`, `name from content`, `name-from-content`, `derived accessible name`, `synthesised accessible name`, `synthesized accessible name`, `automatic accessible name`, `nameless button`, `nameless actionable`, `nameless_actionable`, `aria label on pressable`, `aria label on clickable`, `pressable aria label`, `clickable aria label`, `takes_name_from_content`, `accessible name of a button-like surface`, `how does a button get its accessible name`, `where does the aria name come from` | [`pressable`](mod@pressable) + [`clickable`](mod@clickable) - a button-shaped wrapper takes its accessible name from its wrapped text through the shared name-from-content derivation ([`Semantics::takes_name_from_content`](enzui_core::semantics::Semantics::takes_name_from_content) for `Role::Button`), and the loose `Role::Label` copy the text child would otherwise contribute is suppressed. An icon- or graphic-only wrapper publishes no text for the collector to derive from and lands per frame in [`SemanticsCollector::nameless_actionable`](enzui_core::collect::SemanticsCollector::nameless_actionable) so a headless audit reddens instead of a screen reader hearing only "button"; set [`EnzPressable::aria_label`](pressable::EnzPressable::aria_label) or [`EnzClickable::aria_label`](clickable::EnzClickable::aria_label) to name it. A caller-supplied [`EnzPressable::semantics`](pressable::EnzPressable::semantics) descriptor wins over `aria_label`, and the nameless-actionable record covers that override path too. |
//! | `icon button`, `icon-only button`, `icon only action`, `round action leaf`, `toolbar button`, `round icon button`, `titlebar icon button`, `titlebar button`, `settings icon in titlebar`, `show hide panel button`, `expand collapse button`, `disclosure button`, `reveal a region`, `aria-expanded`, `announce collapsed`, `ink-only toolbar icon`, `chromeless icon hover`, `icon brightens on hover`, `no hover fill` | [`toolbar_icon`](mod@toolbar_icon) - a circular icon-only button for titlebars / toolbars, and the **canonical form of the crate's one round icon-action leaf**: collapsed search, `toolbar_icon`, [`sidebar_toggle_button`](mod@sidebar_toggle_button) and [`window_control_button`](mod@window_control_button) share the 28 px square box from [`TitlebarAffordanceTokens`](toolbar_icon::TitlebarAffordanceTokens) and the existing 16 px [`icon::DEFAULT_ICON_SIZE`] glyph scale, while the shared engine owns `radius = side / 2`, status-aware fill/ink, contrast-aware focus ring and pointer / Enter / Space / routed-AccessKit activation. The global minimum hit target is an independent floor applied after the preferred control size; it is never the glyph size. Start here unless you specifically need the sidebar-disclosure chip or window chrome. Use `.variant(ToolbarIconVariant::InkOnly)` for chrome that must not grow a hover/pressed fill: the SVG moves from muted to primary ink while the background stays transparent, and the policy remains live-theme reactive. Call `.active(bool)` for a *toggle*: it both paints the selected fill and publishes the state (`enz_button(..).selected(..)` is aria-only and does not change the fill). Call `.expanded(bool)` when the button **reveals a region** rather than turning something on: that publishes `aria-expanded`, so AT announces "Show sidebar, collapsed" and - with an `.id(..)`, which is what lets the action route - advertises `Action::Expand` so AT can open it. The two are different axes and picking `active` for a disclosure makes the Linux adapter announce a ToggleButton. This matters most when the region is **absent** from the tree while closed (the structural-visibility rule): the disclosure is then the region's only representation, and without it the frame publishes no trace of it at all. Group several inside [`titlebar_icon_group`](mod@titlebar_icon_group). |
//! | `sidebar toggle`, `collapse sidebar button`, `show sidebar button`, `hamburger sidebar`, `hamburger`, `hamburger menu`, `hamburger button` | [`sidebar_toggle_button`](mod@sidebar_toggle_button) - a circular icon-only chip that toggles the sidebar: the **sidebar-disclosure preset** of the shared icon-action leaf [`toolbar_icon`](mod@toolbar_icon) is the canonical form of. It adds only defaults - the `navigation.sidebar_left` glyph, the `"Toggle sidebar"` tooltip and accessible name, and an opaque `surface_hover` → `surface_selected` chip fill instead of the toolbar's translucent overlay. It publishes **no** `aria-expanded`: for a control that announces the region it reveals, use `enz_toolbar_icon(..).expanded(..)`, which is what [`app_shell`](mod@app_shell) uses for the real shell disclosure. |
//! | `window button`, `chrome button`, `round window chip` | [`window_control_button`](mod@window_control_button) - an icon-only round chip for window chrome: the **window-control preset** of the shared icon-action leaf [`toolbar_icon`](mod@toolbar_icon) is the canonical form of. It adds neutral chip chrome, a *required* action name (`"Close"` / `"Minimize"` / …, with no glyph-name fallback), and a stable id normally supplied by the [`window_controls`](mod@window_controls) cluster - which owns platform action ordering, visibility and per-action ids, never leaf paint. |
//! | `second window`, `secondary window`, `multi-window`, `multiple windows`, `detached window`, `preview window`, `daemon`, `app does not quit`, `app keeps running`, `window exit policy`, `close last window`, `window resize`, `resize event`, `window width`, `track window size`, `remember window size`, `restore window size`, `listen_with`, `undecorated window`, `borderless window`, `frameless window`, `no window decorations`, `content window`, `floating close button`, `dialog window`, `modal dialog window`, `transient window`, `parent window`, `transient for` | [`window_lifecycle`](mod@window_lifecycle) - the exit policy for an app with more than one window, plus [`app_shell`](mod@app_shell)'s `# A second window` section, which is what commits you to `iced::daemon` and lists the four traps (a daemon never exits; `window::latest()` silently becomes "the newest window"; `resize_events()` is app-wide; a shell-less window needs `enz_secondary_window_settings`). A second window that draws no titlebar but supplies its own floating close control takes the third constructor, [`enz_content_window_settings`](crate::app_shell::enz_content_window_settings) (`decorations: false`), and owes the compositor frame's three jobs back: a close control, a drag region to move, and a resize frame at the window edge (that constructor names the exact primitives). For **which resize route to be on**, that section's `event::listen_with` subsection is the answer: an app already using `event::listen_with` will never find `iced::window::resize_events()` by reading its own source, and the `_id` underscore in its own closure is trap 4 already present in its code. To **persist** a window size across restarts see [`enzui_core::window_placement`], whose `PlacementTracker::record_width` is the width-only route for a shell app that never reads a height, and whose `initial_unbounded` is the honest answer before a window's first resize - do not invent a monitor size. A dialog window that belongs to another takes [`enz_dialog_window_settings`](crate::app_shell::enz_dialog_window_settings)`(parent, modal, size)`: the desktop keeps it above its parent, and a modal one blocks the parent's input until it closes. |
//! | `suggestion list`, `suggestion popup`, `option list`, `list option`, `rich option row`, `rich result row`, `leading trailing option`, `three line option`, `search result row`, `title subtitle option`, `two line row`, `two line search result`, `at mention list`, `slash command list`, `mention popup`, `completion list`, `inline suggestions`, `caret anchored list`, `listbox`, `spotlight`, `search results` | [`suggestion_list`](mod@suggestion_list) - a standalone, keyboard-navigable suggestion list (the *list half* of [`autocomplete_field`](mod@autocomplete_field), extracted for use with any input). Both [`SuggestionOption::subtitle`](suggestion_list::SuggestionOption::subtitle) and [`AutocompleteOption::subtitle`](autocomplete_field::AutocompleteOption::subtitle) render a simple title + subtitle. For a sender/title, protected right-aligned metadata, and a subject + bounded two-line preview in one wrapping paragraph (continuations restart at the detail-line origin), use [`SuggestionOption::rich_result`](suggestion_list::SuggestionOption::rich_result) or [`AutocompleteOption::rich_result`](autocomplete_field::AutocompleteOption::rich_result): both carry the same [`OptionRowContent`](suggestion_list::OptionRowContent), retain one accessible name and add no nested focus stop. A list inside a parent popup can call [`EnzSuggestionList::embedded`](suggestion_list::EnzSuggestionList::embedded) to drop this list's **own** popup fill/border/padding. Individual options are always borderless rows: they rest with no fill and no outline in either mode, keeping active/hover/focus-visible treatment, so a popup shows one outer edge instead of one box per option. There is no per-option card tone. For the combined single-line-field + list use [`autocomplete_field`](mod@autocomplete_field); for slash commands or `@`-mentions in a multi-line [`text_area`](mod@text_area) use this. |
//! | `menubar`, `menu bar`, `File Edit View menus`, `application menu bar`, `F10 menu`, `top menu`, `main menu`, `submenu`, `cascading menu`, `menu with submenus` | [`menu_bar`](mod@menu_bar) - the desktop menu bar: an application keeps a [`MenuBarModel`](enzui_core::menu_bar::MenuBarModel) of [`MenuBarMenu`](enzui_core::menu_bar::MenuBarMenu)s (each a menu model, plus child models for submenus) and applies each event; F10 opens the first menu from anywhere in the window, arrows move through menus and submenus, Enter chooses, Escape closes, and moving over another title while a menu is open switches to it. Rows show shortcuts, check and radio marks and submenu chevrons from the model. |
//! | `dropdown button`, `menu trigger button`, `split menu button`, `button with menu`, `menu button`, `flat menu button`, `quiet menu trigger`, `kebab menu button`, `three dots menu`, `more menu`, `overflow menu icon`, `toolbar kebab`, `kebab trigger` | [`menu_button`](mod@menu_button) - the controlled menu-button host. Its `MenuButtonKind` picks the trigger chrome: `Default` fills a pill, `ActionBar(size)` derives a filled Secondary chip on a button size tier, `Flat` is transparent in every state (the `ButtonKind::Plain` analogue - no hover wash, for a trigger inside a surface that already paints its own hover), `Quiet(size)` is the `ButtonKind::Quiet` analogue: transparent at rest with the shared overlay wash on hover **and** press, neutral ink, and height/radius/padding derived from the button size tokens - so a `More` trigger and the `Quiet` buttons beside it share one borderless family, and `Toolbar` delegates its complete trigger to the canonical [`toolbar_icon`](mod@toolbar_icon) leaf. [`enz_kebab_menu_button`] is the one-call controlled-menu preset with the vertical-ellipsis icon, a caller-overridable accessible name, 28 px control and 32 px toolbar slot. The `pressed_background` token lets any non-toolbar trigger express a distinct pressed rung. |
//! | `input`, `text box`, `single line input`, `entry`, `text input`, `right click copy paste`, `cut copy paste menu`, `text context menu`, `undo in text field` | [`text_field`](mod@text_field) - a styled single-line text input component with undo/redo and a built-in right-click edit menu ([`edit_menu`](text_field::EnzTextField::edit_menu) turns it off). For multi-line use [`text_area`](mod@text_area). For an editable value with no resting outline, see the `inline edit` row below. |
//! | `inline edit`, `editable value`, `numeric value editor`, `quiet field`, `rename in place`, `inline rename`, `edit in place`, `edit a label` | [`text_field`](mod@text_field) - to rename a row or title where its label stood, `.variant(FieldVariant::Inline)` draws no body, outline or focus ring and no horizontal inset, so the caret and selection alone mark the edit (an error still draws its edge). For an editable value that sits inline beside other content, with no resting outline: an exact-entry number next to a slider, a value cell in a settings row. Call [`EnzTextField::quiet(true)`](text_field::EnzTextField::quiet), which omits the **resting** edge and nothing else - hover, the 2 px keyboard focus ring and the error border paint exactly what an ordinary field paints in those states, and the accessible name and edit semantics, caret and selection, trailing in-field actions and `enabled`/`read_only` behaviour are unchanged, all still resolved from the live theme. A field earns that presentation only by being **named** (a visible `label` or [`.aria_label(..)`](text_field::EnzTextField::aria_label)) **and** painting its own text inside the box (a non-empty value, or a placeholder while the value is empty); a field missing either keeps the ordinary outline, so this cannot turn an unidentified field into invisible chrome. A [`leading`](text_field::EnzTextField::leading) or [`trailing`](text_field::EnzTextField::trailing) slot does not qualify: an Apply or Clear affordance identifies its own action, not the empty input beside it. The canonical composition is a quiet [`.read_only(true)`](text_field::EnzTextField::read_only) readout that becomes a quiet editable field carrying its Apply/Cancel affordances in the trailing slot, plus [`.adjustable_value(..)`](text_field::EnzTextField::adjustable_value) so a numeric editor publishes a spin-button contract to assistive technology. `quiet` never changes the fill: pair it with [`FieldVariant::Outlined`](tokens::FieldVariant) for a resting cell that reads as page surface. Do not reach for [`.palette(..)`](text_field::EnzTextField::palette) to erase a border - that hands the field a whole palette of shared colour roles and restyles every widget in the subtree. |
//! | `password field`, `masked input`, `password input`, `reveal password`, `secure field`, `show password toggle`, `secure field reveal` | [`secure_field`](mod@secure_field) - a masked text input with a trailing reveal/hide toggle. The eye publishes a labelled **pressed** toggle (`Show password` / `Hide password` plus aria-pressed) and the AT node stays a valueless `PasswordInput` in both states, so a visual reveal never declassifies the secret. `revealed` is caller-owned; inside a [`form_dialog`](mod@form_dialog) that owner is the session, not the app. Use [`enz_secure_field_with`](secure_field::enz_secure_field_with) / [`EnzSecureField::on_toggle_with`](secure_field::EnzSecureField::on_toggle_with) / [`EnzSecureField::on_submit_with`](secure_field::EnzSecureField::on_submit_with) when the message comes from a host mapping, so the mapping runs at event time instead of once per view pass. |
//! | `search box`, `search input`, `filter box`, `search bar`, `focus search field`, `autofocus search`, `focus on open`, `search field`, `keep a text field focused across a rebuild`, `search loses focus after the first letter`, `loses focus on first character`, `field loses focus when results appear`, `focus lost on rebuild`, `caret jumps to end`, `retain focus`, `preserve caret` | [`search_field`](mod@search_field) - a search-flavoured text input. **Focus survives a rebuild by default:** typing the first character usually flips the caller's view from an empty-query branch to a results branch, so an ancestor tag changes, `Tree::diff` discards the field's subtree, and the second character lands nowhere. `enz_search_field` reclaims its own focus **and its exact caret** through the crate's internal `focus_retention` seam - no shell, no `.id(..)` and no reducer arm required, and it holds when the caller wraps the field in a new container. `.retain_focus(false)` opts out. Any *other* text input in the same position still needs [`app_shell`](mod@app_shell)'s checkpoint and a stable `.id(..)`. **Focus contract (preferred):** for a search surface a shortcut toggles into view, call `.focus_on_mount(true)` - the field claims focus the first frame it is mounted and never again, the opening transition only, with no reducer arm to forget. **Manual alternative:** give the field an `.id(id)` and return `iced::widget::text_input::focus(id)` from the `update` arm that opens the surface. Either way, focus on the **opening transition only** - a toggle-shut or a re-entry while the field is already visible must not re-focus, or it steals the caret position mid-type. Do NOT set `focus_on_mount` on an always-visible field: it grabs focus at startup. |
//! | `otp`, `one time code`, `pin entry`, `verification code`, `2fa code`, `input otp` | [`input_otp`](mod@input_otp) - a segmented one-time-code / PIN entry. |
//! | `chip input`, `tag input`, `token field`, `tags field`, `recipients field`, `keyword entry`, `multi value input`, `comma separated input` | [`chip_input`](mod@chip_input) - `enz_chip_input(chips, draft, on_event)` with [`chip_input::apply_chip_event`]: Enter or a separator (comma by default, typed or pasted) turns the draft into a chip once, Backspace in an empty focused draft removes the last chip, and each chip has a remove button named `Remove <chip>`. Chips wrap inside text-field chrome; the field is a `Group` named by `label`. |
//! | `emoji picker`, `emoji chooser`, `insert emoji`, `emoji keyboard`, `emoji search`, `recent emoji`, `smiley picker`, `emoji shortcode` | [`emoji_picker`](mod@emoji_picker) - `enz_emoji_picker(query, category, recents, on_event)` (feature `emoji-picker`): a search field (name words and `:shortcode:`, Enter picks the first match), category tabs and a grid of emoji buttons named by their Unicode names; `EmojiRecents` keeps the recently picked ones for the Recent tab. Emoji newer than Unicode 15.0 are left out unless `.newest_unicode(..)` says the fonts have them. |
//! | `font chooser`, `font picker`, `font dialog`, `choose a font`, `font family list`, `font selector`, `typeface picker`, `installed fonts` | [`font_chooser`](mod@font_chooser) - `enz_font_chooser(&state, &choice, on_event)`: a search field and a virtualised list of the installed families (each drawn in its own font), the chosen family's styles, a size stepper and a preview; `FontChooserState::apply` folds the events into the state and the `FontChoice`, whose `font()` is the `iced::Font`. `FontChooserState::monospace_only(true)` lists fixed-width families for an editor or terminal. |
//! | `drag selected text`, `drop text into field`, `text drag and drop`, `move text by dragging`, `drag text between apps`, `drop target text field` | [`text_drag`](mod@text_drag) - nothing to wire: `app_setup::run` installs it on Wayland. A press inside a selection and a drag starts a native text drag (moved, or copied with Ctrl); text dropped on a field or editor is inserted at the drop point through the field's edit callback; moving inside one field is one edit. `AppServices::text_drag(false)` turns it off for an application that owns `NativeDragRuntime` itself. |
//! | `mnemonic`, `access key`, `accelerator letter`, `alt underline`, `underlined letter`, `alt shortcut on button`, `keyboard access key` | [`mnemonic`](mod@mnemonic) - mark the letter with an underscore (`"_Save"`): [`EnzButton::mnemonic(true)`](button::EnzButton::mnemonic) and the [`menu_bar`](mod@menu_bar) underline it while Alt is held and activate on Alt and the letter; [`with_mnemonic`](mnemonic::with_mnemonic) adds the activation to any control, and `EnzMenuRow::mnemonic` underlines a row. |
//! | `spinbox`, `spin button`, `number stepper`, `decimal stepper`, `float stepper`, `decimal spinbox`, `fractional value`, `increment decrement`, `plus minus number`, `numeric stepper`, `unsigned count`, `u32 stepper`, `caption beside stepper`, `value touching the button`, `stepper value padding`, `type a number`, `editable spinbox`, `number out of range` | [`stepper`](mod@stepper) - a bounded spinbox with ± buttons over an `i32`, `u32`, `f32` or `f64`: `enz_stepper(value, ..)` (a decimal keeps the places its step needs, or `.decimals(n)`). Click the value to type one; Enter or clicking away commits, clamped, and `.range_error(true)` marks a typed value outside the range. `.aria_label(..)` names a control whose row already provides its caption. The centred readout splits `StepperTokens::value_inset` evenly between the ± segments. |
//! | `range slider`, `two thumb slider`, `dual slider`, `min max slider`, `price range`, `double slider`, `slider with two handles` | [`range_slider`](mod@range_slider) - `enz_range_slider(range, (low, high), on_change)`: two thumbs on one track with the fill between them. The thumbs never cross; each is its own Tab stop and its own `Slider` node (`<label> minimum`, `<label> maximum`) whose limits are the range's end and the other thumb. Arrows, Page Up/Down and Home/End move the focused thumb; a press moves the nearer thumb and drags it; right-to-left text puts the minimum on the right. Tokens and snapping are [`slider`](mod@slider)'s. |
//! | `range`, `scrubber`, `volume slider`, `value slider`, `discrete slider`, `stepped slider`, `snap points`, `step marks`, `stop indicators`, `hide stop dots`, `seek bar hover time`, `hover value preview`, `value under pointer` | [`slider`](mod@slider) - a custom, focusable slider primitive. `on_drag_start` and `on_release` distinguish pointer ownership from keyboard or routed changes, so media scrubbers can retain a preview until release. |
//! | `check box`, `tick box`, `boolean checkbox`, `tri-state checkbox`, `mixed checkbox`, `indeterminate checkbox`, `select all checkbox`, `partially checked` | [`checkbox`](mod@checkbox) - a rounded-square checkbox, unchecked, checked or mixed ([`EnzCheckbox::mixed`](checkbox::EnzCheckbox::mixed), `aria-checked=mixed`). |
//! | `switch`, `on off switch`, `toggle switch`, `mode switch`, `icon toggle`, `toggle icon`, `glyph switch`, `icon-bearing switch` | [`toggle`](mod@toggle) - a styled toggle/switch component (GtkSwitch / SwiftUI `Toggle`). For a symbolic glyph that travels inside the thumb, call [`EnzToggle::thumb_icon`](toggle::EnzToggle::thumb_icon); the glyph is decorative, while the same switch remains the sole activation target and checked semantic node. The thumb and glyph mirror together in RTL, resolve contrasting ink from the live theme, and share the switch's reduced-motion, disabled and read-only behavior. |
//! | `radio`, `single radio`, `option button` | [`radio_button`](mod@radio_button) - a single, standalone radio button. For a set of options use [`radio_group`](mod@radio_group). |
//! | `radio buttons`, `segmented control`, `option group`, `single choice`, `mode switch group`, `radio group`, `typed radio group`, `keyed radio choices`, `enum radio group`, `radio group of values not indices`, `radio choice`, `radio group hide heading`, `inline labelled radio group`, `radio group accessible name` | [`radio_group`](mod@radio_group) - a styled radio button group; the accessible single-choice control (1 tab stop, not N). [`enz_radio_group_keyed`] is the typed form: one collection of [`RadioChoice`]s (key, value, label) that takes the selected value and emits the selected value, so an enum-valued setting needs no parallel arrays or index adapters. Identity follows the key, so a reorder, a filter or two choices with one label keep every option addressing itself, and a selected value the collection does not offer stays unselected. [`EnzRadioGroup::label_visible(false)`](radio_group::EnzRadioGroup::label_visible) drops the painted heading and keeps the accessible name and identity, for a form that lays the caption beside the group; [`EnzRadioGroup::aria_label`](radio_group::EnzRadioGroup::aria_label) names a group with no heading text. |
//! | `shortcut recorder`, `keybinding input`, `record shortcut`, `capture chord`, `hotkey input` | [`key_capture_chip`](mod@key_capture_chip) - a controlled chip that records a keyboard chord. |
//! | `edit shortcuts`, `rebind key`, `rebind keys`, `shortcut settings`, `keybinding conflict`, `shortcuts editor`, `keybinding editor`, `editable shortcuts`, `reset shortcuts` | [`editable_shortcuts`](mod@editable_shortcuts) - a SettingsPage-compatible pane that borrows one authoritative resolved shortcut catalog, reuses [`key_capture_chip`](mod@key_capture_chip), names conflict/validation feedback, supplies per-action reset, and exposes Reset all as a pinned Settings footer action. The app owns action names and reserved-chord policy; `enzui_core::EditableShortcutCatalog` owns overrides, live resolution, conflict checks and persistence. |
//! | `key binding`, `dispatch shortcut`, `global shortcut`, `shortcut dispatcher`, `hotkey handler`, `alternate shortcut`, `shortcut alias`, `shortcut catalog`, `contextual shortcut` | **not a widget** - see [`enzui_core::shortcut`] and its `ShortcutBinding` catalog model (different crate). `ShortcutBinding` is the authoritative catalog row when one command has primary + alternate chords, section/description help metadata, or a live-context gate; `dispatch_shortcut_command` matches it and `shortcut_help` derives one help row per command. The older `Action` / `dispatch_shortcut` pair deliberately remains the simpler one-chord/one-message API. A hand-rolled chord that ignores `repeat` re-fires on every OS key repeat. |
//! | `measured form`, `shared label track`, `align labels`, `label column`, `two column form`, `QFormLayout`, `field growth`, `wrap long rows`, `stack label over field`, `form measured labels` | [`form`](mod@form) - a label-aligned form that **co-measures every label** into one shared label track (a caller assigns no pixel width and measures no translated label), gives the field track the rest, and stacks label-over-field below a width breakpoint ([`FormWrap`], the analogue of Qt's `RowWrapPolicy`). [`form_multi_field`] puts several fields beside one label; [`form_row`] is the full-width escape hatch. Labelled rows publish a `Role::Group` named by the caption. This is the measured counterpart to the spacing-only [`form_layout`](mod@form_layout). |
//! | `form`, `settings form`, `label aligned form`, `data entry form`, `form layout`, `spacing only form`, `form row gap` | [`form_layout`](mod@form_layout) - a **spacing-only** vertical stack of opaque form rows (SwiftUI `Form`): it owns the row gap and the form inset, and each row owns its own label alignment. Reach for [`form`](mod@form) when the labels across rows should share one measured column. |
//! | `form modal`, `dialog form`, `modal form`, `input dialog`, `form dialog`, `password field in a form`, `form secure field reveal`, `show password in a dialog`, `two forms at once`, `field id collision`, `namespace field ids`, `focus the first invalid field` | [`form_dialog`](mod@form_dialog) - a dynamic form presented over EnzModalHost. A [`FieldKind::Secure`](enzui_core::form::FieldKind::Secure) field renders the real [`secure_field`](mod@secure_field) with its reveal toggle, and the reveal state is **session-owned** ([`FormSession::is_revealed`](enzui_core::form::FormSession::is_revealed) / [`toggle_reveal`](enzui_core::form::FormSession::toggle_reveal)): hidden on open, reset by `retarget`, isolated per presentation, and reported to the app as `FormDialogEvent::RevealToggled(FieldId)` - an event that carries the id and never the secret, so a reveal adds no route for the value to reach a host's `Message` log (`Edited` still carries typed text; that is the field's job). Secure edits and prefills use [`FieldValue::Secure`](enzui_core::form::FieldValue::Secure): clones of events, sessions, and submissions share one protected allocation, every formatter redacts it, and the allocation is wiped when the final owner drops. A secure field immediately moves a compatibility `FieldValue::Text` input into protected custody and never stores the ordinary variant, while ordinary text fields keep their existing independent-clone behavior. For tall forms, `.max_height(..)` keeps title/actions pinned and scrolls only the field body through [`capped_dialog_body`](mod@capped_dialog_body) - a viewport-derived cap (subtract the modal surface's own padding) that the narrow-viewport gate holds. A [`FieldKind::Choice`](enzui_core::form::FieldKind::Choice) field round-trips its `ChoiceOption` value, not its label: the drafted value is resolved to a position each frame, so two options sharing a label keep the stored one marked. The field-id namespace has one owner - set it with `FormPresenter::with_field_id_scope` and render through `EnzFormDialog::presenter`, and a rejected submit focuses the same ids the fields were drawn with; configuring `field_id_scope` on the dialog alone leaves the presenter aiming at ids no control carries. |
//! | `settings group`, `preferences card`, `AdwPreferencesGroup`, `settings section`, `preferences group`, `compact preferences group`, `settings group heading weight` | [`preferences_group`](mod@preferences_group) - a settings group (title + description) card. [`EnzPreferencesGroup::compact`](preferences_group::EnzPreferencesGroup::compact) uses Body/Semibold headings; semantic title size and weight setters and forwarded typography scales affect the shared card header. |
//! | `settings row`, `AdwActionRow`, `AdwSwitchRow`, `switch row`, `click row to toggle`, `preferences row`, `adw row`, `responsive settings row`, `stacked row`, `restack label over control`, `preferences row size`, `settings field size`, `field size tier`, `accessible id`, `owned display text`, `repeated setting identity`, `settings validation`, `stable node identity` | [`preferences_row`](mod@preferences_row) - a labelled settings row (a `switch` row toggles from a press anywhere on it, focusing the switch) that accepts owned display text and automatically restacks its complete label block over a trailing control when the control's own minimum width no longer fits beside an equally usable label column. Use [`.accessory_width(..)`](preferences_row::EnzPreferencesRow::accessory_width) and [`.label_min_width(..)`](preferences_row::EnzPreferencesRow::label_min_width) to declare compact slot/readable-caption budgets; unset policies preserve the default equal-column threshold. Use [`.switch_hover(false)`](preferences_row::EnzPreferencesRow::switch_hover) for a quiet switch row that retains row activation and focus. Use [`.id_prefix(..)`](preferences_row::EnzPreferencesRow::id_prefix) with a stable model key for repeated same-labelled rows; [`.error(..)`](preferences_row::EnzPreferencesRow::error) forwards validation to built-in entry/password fields. Entry and password rows default to `TextFieldSize::Sm`; [`.field_size(..)`](preferences_row::EnzPreferencesRow::field_size) selects the shared tier. |
//! | `settings picker`, `settings row with a menu`, `choose an option in preferences`, `value button`, `pick a value`, `week start` | [`preferences_row`](mod@preferences_row) - the `value_menu` kind is the GNOME/macOS settings picker: a title, a gap, and a text-only button showing the current value that opens a menu of the options with the current one marked. It is **not** a [`combo_box`](mod@combo_box) (no field chrome) and **not** a [`radio_group`](mod@radio_group) (no vertical list filling the pane). Reach for it instead of nesting a radio group in a single-row [`preferences_group`](mod@preferences_group), which draws the setting's caption twice. Dismissal is the shared [`anchored_overlay`](mod@anchored_overlay)`::dismissible_anchored_menu` (outside-press + Escape); open state is the caller's single `Option<key>`, so opening one row closes the rest; the trigger publishes one `Role::ComboBox` node named by the setting with the current choice as its accessible value. A **numeric** setting is a different shape - use `.control(`[`enz_stepper`]`(..))`, not a menu of every value. See [`EnzPreferencesRow::value_menu`](preferences_row::EnzPreferencesRow::value_menu). |
//! | `dropdown`, `select`, `picker dropdown`, `choose one`, `combobox`, `duplicate labels`, `same label twice`, `selected option identity`, `keep the right option selected`, `dropdown separator`, `option groups` | [`combo_box`](mod@combo_box) - an enzUI-styled dropdown selector. For a free-text editable variant use [`autocomplete_field`](mod@autocomplete_field). Selection identity is positional on both halves: a display label can be shared by two options, so `EnzComboBox::selected_index` names the selected one on the way in and `EnzComboBox::on_select_index` reports it on the way out. Resolve a stored key to a position each frame and the mark, the announced value and the row an opening list highlights follow that option across a reorder or a relabel; an index past the last option selects nothing, and `selected_index(None)` restores the label-only path, which marks the first option carrying the label. `EnzComboBox::separator_before(index)` paints a divider before an original option, preserving selection stops and filtered option identity. |
//! | `month view`, `month grid`, `date grid`, `mini calendar`, `mini month`, `sidebar month`, `event dot`, `event marker`, `days with events`, `busy day`, `dense calendar`, `compact day cell`, `multi-day`, `event span`, `all-day band`, `date range`, `spanning event`, `span over my own grid`, `fill-sized grid`, `fill-width calendar`, `non-square cell`, `cell spacing`, `grid hairline`, `right click a day`, `day context menu`, `day state`, `today marker`, `today and selected`, `selected day`, `day hover`, `circular day cell`, `highlight a day`, `flat calendar`, `no card border`, `borderless calendar`, `pill header`, `month year popover`, `day number baseline`, `header wraps at narrow width`, `month title two lines` | [`calendar`](mod@calendar) - the reusable month surface. For a field that opens it use [`date_picker`](mod@date_picker). Per-day host data ("this day has events") goes through `.markers(..)` / `.event_days(..)`, which both **paints and announces** it - do not stack your own dot on a cell, because a decorative dot is invisible to a screen reader. Multi-day events go through `.event_spans(..)`, which owns continuous band paint, clipping, stacking, one-event semantics, and paints the event **title** on the band's leading segment (end-elided to fit; the label is dropped, announce-only, when the band is too short for legible text on a dense grid). A dense grid sets `.cell_size(..)`; below 24px the widget declares the WCAG target-size departure to the a11y audit rather than hiding it. If your month grid is hand-rolled and **fill-sized** rather than fixed-square, do not re-implement the bands: stack this module's `calendar_span_overlay` over it, which derives its own cell rect from its resolved bounds and keeps the same title, elision, ink and segmentation rules. **Day states are the widget's own, by default:** hover, today, selected and today+selected each paint the SAME circular day surface, told apart by tone, fill weight and numeral weight and never by a border - see `calendar::DayEmphasis`. Do not add an app-local style branch for them; an app's mini-month did, and shipped a rectangular hover beside two discs plus an extra black outline on today+selected. **A sidebar mini-month is this widget too**: `.fill_width(true)` makes each day fill its column (the day surface becomes a stadium of the cell's height - the circle rule extended, not a second treatment), `.cell_spacing(..)` opens a hairline between days without moving the grid pitch, and `.on_day_context(..)` reports the day plus the pointer point for a right-click, with Menu / Shift+F10 parity anchored at the cursor cell. Pair that point with [`context_menu_host`](mod@context_menu_host)'s point entry; the calendar publishes the gesture and never the menu's contents. The month/year header is a **single end-elided line** at every width, because a sidebar closing animation passes through widths that no month name fits and a header that reflows moves everything under it. |
//! | `date field`, `pick date`, `date input`, `calendar popover`, `date picker`, `inline date label`, `leading date label` | [`date_picker`](mod@date_picker) - a date field that opens a calendar popover (SwiftUI `DatePicker`). Its label is visible by default; when a form paints the same accessible label beside the trigger, `.label_visible(false)` suppresses only the duplicate caption and keeps the `DateInput` name. |
//! | `time picker popover`, `pick a time from a list`, `time dropdown`, `half hour slots`, `date and time picker`, `date time picker`, `datetime picker`, `event start`, `appointment time` | [`time_picker`](mod@time_picker) - `enz_time_picker(draft, value, open, on_event)`: a [`time_field`](mod@time_field) with a clock button that opens a list of times at an interval (30 minutes by default), starting at the time nearest the value with the value checked; arrows and Enter choose, Escape or an outside press close and return focus to the button. [`time_picker::enz_date_time_picker`] puts a [`date_picker`](mod@date_picker) and a time picker side by side as one named group. |
//! | `time input`, `time of day`, `edit a time`, `type a time`, `click and type a time`, `start time`, `end time`, `hh:mm`, `am pm`, `12 hour clock`, `24 hour clock`, `clock input`, `time spinner`, `time picker`, `time entry` | [`time_field`](mod@time_field) - one focusable control that is typed into and stepped: `Role::SpinButton`, `editable` and `adjustable` on the same node, so an assistive technology is not offered two overlapping targets. Text is read and written by the shared core formatter `enzui_core::time_of_day::TimeFormat`, so the caller's hour cycle, separator and day-period labels govern both directions and a typed `9:07` is not rounded by a 15-minute arrow step. Do not pair [`stepper`](mod@stepper) with a hand-rolled `hh:mm` splitter - that is the shape this replaces, and it gets `12:00 AM` wrong. A time of day is cyclic, so the field wraps at midnight and takes no minimum or maximum; "end after start" is the caller's rule, because the pair is date-aware and a cross-midnight event is legitimate. For a date use [`date_picker`](mod@date_picker). |
//! | `date range`, `range picker`, `from to date`, `start end date`, `two date range` | [`date_range_picker`](mod@date_range_picker) - a field that opens a calendar for a two-click range. |
//! | `color chip`, `colour square`, `swatch`, `color box`, `no colour`, `clear colour`, `none swatch`, `color swatch` | [`color_swatch`](mod@color_swatch) - a clickable coloured square with selection border. |
//! | `color picker`, `colour picker`, `continuous colour picker`, `color wheel`, `hue strip`, `saturation value plane`, `sv plane`, `hsv picker`, `pick an arbitrary colour`, `choose colour`, `hex field with preview`, `alpha slider`, `eyedropper` | [`color_picker`](mod@color_picker) - the **continuous** colour editor: a pointer- and keyboard-driven saturation/brightness plane, a hue strip, an optional alpha strip, a live swatch and a precise hex field. Reach for it when the user must choose an *arbitrary* colour; the `color_swatch_grid` row below is the fixed-palette inventory and cannot express a value that is not already in the palette. There is no eyedropper: screen colour pickup needs a compositor portal this framework does not wrap. |
//! | `colour palette`, `swatch grid`, `palette grid`, `palette inventory`, `fixed palette`, `choose a palette colour` | [`color_swatch_grid`](mod@color_swatch_grid) - a wrapping palette-inventory grid of coloured swatches with captions (a **fixed** inventory of authored swatches; for an arbitrary value see the `color_picker` row above). |
//! | `workspace colour`, `workspace tag`, `tab tint`, `workspace accent`, `folder colour`, `circular colour chooser`, `tint chooser`, `compact workspace palette`, `multi-row accent chooser` | [`workspace_accent_chooser`](mod@workspace_accent_chooser) - a named radio chooser of circular swatches, with `.columns(4)` for two compact rows, for picking one of the 7 shipped [`WorkspaceAccentKey`](enzui_theme::WorkspaceAccentKey) tints (plus a *Default* chip). It uses the framework colour-swatch paint with radio-group semantics so the AT surface hears "Workspace colour" not "#3B82F6". Pair with [`EnzDocumentTab::workspace_accent`](document_tab::EnzDocumentTab::workspace_accent) to colour that workspace's tabs coherently - the tab paints the accent as a soft, contrast-audited wash over its own state fill, so ink contrast survives. |
//! | `command menu`, `action search`, `ctrl k`, `quick open`, `command bar`, `fuzzy command`, `command palette` | [`command_palette`](mod@command_palette) - a query input + live-filtered command list (shadcn `Command`). |
//! | `three panel mail`, `three pane header`, `full-bleed titlebar background`, `secondary titlebar surface`, `application shell`, `floating titlebar over photos`, `titlebar scrim`, `titlebar shade`, `legible titlebar over video`, `app frame`, `window shell`, `app scaffold`, `app shell`, `sidebar header`, `sidebar titlebar`, `sidebar title and actions`, `sidebar chrome`, `sidebar disclosure tooltip`, `sidebar toggle inset`, `sidebar reflow`, `docked sidebar reflow`, `smooth sidebar under load`, `sidebar stutters when busy`, `responsive sidebar`, `retained responsive width`, `live width propagation`, `window resize edge`, `resize corner`, `automatic CSD resize`, `CSD resize`, `resize opt-out`, `client decoration resize`, `drag resize window` | [`app_shell`](mod@app_shell) - the layered application shell: five presets over a rounded background layer. [`.titlebar_background_with_geometry(..)`](app_shell::EnzAppShell::titlebar_background_with_geometry) paints full-band list/reader surfaces beneath foreground toggle clearance and window controls, with shell clipping and no decoration input, focus or accessibility targets. See `reference_three_pane` for a navigation/list/reader composition. A floating titlebar over photos or video takes [`.titlebar_scrim(true)`](app_shell::EnzAppShell::titlebar_scrim) for a legibility shade, and fades with `.chrome_visible(..)`. Docked sidebar motion reflows the content beside it on every frame like a live window resize, so trailing content stays at the window's edge; a frame the UI thread is late with holds the last layout, and the slide resumes where it held rather than jumping or showing a layout moved rigidly. Overlay drawers remain compositor-only. Put title/actions above the sidebar in its `sidebar_titlebar` slot; the band follows the sidebar into an open compact drawer. The compact drawer covers the whole window, titlebar included: it takes the titlebar's tone, its header moves the window and carries the toggle, a named sidebar (`.sidebar_label(..)`) without a `sidebar_titlebar` shows its name there, and the window controls stay live above the scrim. `ShellLayout::FloatingSidebar` floats the sidebar as an inset, raised card with the same header, a toggle chip at its corner while closed, and an equal inset on both sides however narrow the window. The drawer and the floating card take the sidebar's material when its `PaneStyle` names one (`ShellTokens::sidebar.material(MaterialStyle::Frosted)`), else the titlebar's. The shell disclosure shows its accessible name after the default hover delay; `.sidebar_toggle_tooltip(false)` suppresses the hint while preserving its accessible name and expanded state; `.tooltip_delay(..)` overrides the delay of shell-owned controls; `.sidebar_toggle_side_inset(px)` shares the inset across sidebar and titlebar placements. Set `ShellTokens::toggle_anchor(ToggleAnchor::LeadingBar)` for a fixed leading disclosure. Every client-decorated shell automatically owns invisible native resize edges/corners: iced binds presses to the exact current window and consults its authoritative `resizable`, maximized, and fullscreen state, so no app message, id, subscription, reducer, `on_window_gesture`, or mirrored flags are needed. `window::Settings { resizable: false, ..enz_window_settings(..) }` is the sole ordinary opt-out. |
//! | `native fullscreen`, `fullscreen window`, `enter fullscreen`, `exit fullscreen`, `toggle fullscreen`, `F11`, `Escape fullscreen`, `fullscreen video`, `shell window state`, `window id helper`, `shell resize adoption`, `ShellWindowState`, `ShellWindowEvent`, `adopt resize edges`, `window state subscription`, `runtime window id`, `track window id`, `window opened id`, `window::latest replacement`, `replace window latest`, `preview window steals resize`, `second window retargets titlebar`, `which window does the titlebar command` | [`app_shell`](mod@app_shell) - `app_shell::window_state` is the compatibility subscription + reducer for policy-bearing chrome commands (close/minimize/etc.), **pinned to one window**; automatic edge resize does not need it. `ShellWindowState::new()` binds the first `Opened` (a single-window app); a multi-window app uses `ShellWindowState::deferred()` plus `bind(id)` (or `for_window(id)`) so a preview or secondary window that opens or focuses first can never claim the shell. `Focused`, `Resized` and `Rescaled` never bind an identity, compositor replies carry the id they were asked about, and while unbound `run_command` issues nothing rather than guessing a window - the `window::latest()` failure. `set_fullscreen(bool)` requests native fullscreen; `toggle_fullscreen()` queries the bound window's current mode rather than trusting a mirrored flag. Native fullscreen restores the window's previous normal/maximized state. `DecorationsChanged` observes same-size transitions. The shell automatically presents primary content without chrome, gaps or rounded corners in fullscreen; `.fullscreen_chrome_visible(true)` retains application chrome. F11/Escape bindings are opt-in application policy; see `examples/reference_floating_media.rs`. Keep one state per window. |
//! | `enz_app_shell width`, `content width`, `content size closure`, `content width closure`, `width-aware shell content` | [`app_shell`](mod@app_shell) - use `enz_app_shell_sized` when the primary content needs its own resolved pane width to choose a toolbar mode, column count, or responsive layout. The callback receives `ShellGeometry::content_width` after edge insets and an inline sidebar, and shares the shell's within-cycle layout cache; do not wrap the entire shell in `responsive` merely to recover the outer window width. The plain `enz_app_shell` remains the width-independent form. It publishes a width and **nothing else**: for the resolved sidebar presentation use `enz_app_shell_with_geometry` (see the slot-geometry row below). |
//! | `slot geometry`, `sidebar presentation in a slot`, `titlebar knows the sidebar`, `sidebar presentation in the titlebar`, `presentation-sensitive titlebar`, `is the sidebar an overlay right now`, `shell geometry in a closure`, `slot closure geometry`, `footer knows the sidebar`, `drawer header vs inline header`, `page with no sidebar`, `route without a sidebar`, `hide the sidebar on one page`, `animate the sidebar collapse on navigation`, `sidebar disappears harshly` | [`app_shell`](mod@app_shell) - every shell slot has a `_with_geometry` form that is handed the resolved `ShellGeometry`: `sidebar_with_geometry`, `titlebar_with_geometry`, `sidebar_titlebar_with_geometry`, `subtitlebar_with_geometry`, `footer_with_geometry`, `secondary_content_with_geometry`, and `enz_app_shell_with_geometry` for the primary content slot. Use it whenever a slot's content depends on how the shell resolved the sidebar at this width - a titlebar search ladder that collapses once navigation moves into a drawer, a drawer header that drops an inline-only affordance. Do **not** infer the presentation from the slot's own assigned width (two presentations produce the same slot width) and do **not** re-resolve a breakpoint against an app-held window width: that is a second collapse authority, wrong on the first visible frame, and the `.forced()` pin that keeps it in step is an app maintaining a copy of a decision the shell already made. The value is the shell's window-level resolution, identical in every slot of one frame; no field reports the receiving slot's own box. `enz_app_shell_sized` remains the width-only content form and `ShellGeometry::content_width` is the same number. For a destination that has **no sidebar at all**, keep the `.sidebar(..)` slot wired and say so with [`.sidebar_available(false)`](crate::app_shell::EnzAppShell::sidebar_available): the shell resolves the sidebar to `Hidden`, publishes no toggle for a pane this route does not own, and treats the change as navigation intent so its own docked reveal plays the collapse. Dropping the slot instead (what every app did) removes the subtree, so there is nothing left to animate and the whole content column teleports sideways by `sidebar_width + gap` in one frame. |
//! | `sidebar gap`, `gap around panes`, `space around sidebar`, `window margin`, `inset panes`, `floating sidebar look`, `rounded sidebar corners`, `not flush`, `flush sidebar`, `sidebar touches window edge`, `full height rail`, `edge inset` | [`app_shell`](mod@app_shell) - **not a widget**: `ShellTokens::inset_panes()` is the one-call opt-out from the flush sidebar default. Flush (sidebar against the window edge, seam corners square) is the DEFAULT on the two seam layouts; `inset_panes()` restores the gapped, fully rounded arrangement, and `floating_cards()` is the same geometry plus the card repaint. `gap` still steers both distances under either. |
//! | `sidebar divider`, `divider between sidebar and content`, `seam line`, `seam colour`, `seam width`, `hairline between panes`, `remove the divider`, `divider looks bad in dark`, `dark stripe between panes`, `pane boundary line` | [`app_shell`](mod@app_shell) - the sidebar/content boundary is the shell's, not an app-placed [`enz_divider`](mod@divider): a hand-placed rule cannot be a child of the content pane (that pane starts below the titlebar, so the line stops at the band), and a `BorderStyle::Thin` on either pane draws all four sides. A seam layout **paints the hairline by default** in the theme's divider colour - `SeamStyle::Flat`'s reserved-but-transparent slot let the window backdrop show through the one-pixel gap, which reads as a hard near-black stripe between two lighter panes in dark. Change it with `ShellTokens::seam_thickness` (the content pane loses exactly that width, and `ShellGeometry::content_width` reports it) and `ShellTokens::seam_color`; `seam(SeamStyle::Flat)` keeps the slot unpainted and `seam(SeamStyle::None)` removes the rule (and, with it, the flush arrangement). The rule is drawn only while the sidebar is **inline** - an overlay drawer has no boundary to mark. |
//! | `flat page`, `flush page`, `no card`, `borderless page`, `full-bleed content` | [`app_shell`](mod@app_shell) - use `ShellTokens::flat_page()` when page content is the shell background rather than a floating card. It blends and squares the content pane, closes titlebar→content and subtitlebar→content to the measured 0 px, preserves the outer window radius/clip, and deliberately leaves content→footer on the ordinary pane gutter. `flush_page()` is the geometry-first alias. Do not use it for `floating_cards()` or `inset_panes()`. |
//! | `page inset`, `page padding`, `content padding`, `top padding`, `heading inset` | [`app_shell`](mod@app_shell) - the shell-level page-top authority is `ShellTokens::page_inset`. Its default is **0 px** so a framework update cannot double-pad existing app literals; while adopting, remove the private top padding and use `.recommended_page_inset(&theme.spacing)` (**8 px** on the stock scale, `PAGE_INSET_TOKEN = SpaceToken::Sm`), or `.page_inset(other_token, &theme.spacing)` for an explicit semantic override. The rule behind 8 is testable: space above a page heading must not exceed heading→first-row space. Side padding may remain 16 px; a titlebar already supplies vertical separation, so top and sides need not match. [`content_pane`](mod@content_pane) preserves its old symmetric default for compatibility; when it sits inside an adopted shell, call `.flush_header(true)` so only the shell supplies top spacing. Its 16 px side/bottom constant is derived from the shared 8 px decision and gated against drift. |
//! | `RTL sidebar`, `right-to-left sidebar`, `sidebar mirroring`, `layout direction`, `RTL layout`, `mirror sidebar`, `sidebar on the right`, `bidi shell`, `RTL drawer`, `RTL toggle anchor` | [`app_shell`](mod@app_shell) - `ShellTokens::direction` mirrors **navigation placement**, not text alignment: at `Direction::RightToLeft` the sidebar pane, the seam, the collapse toggle and its anchor/clearance, the overlay drawer, the floating activity surface and the seam-square pane corners all move to the trailing (right) edge, and `ShellGeometry::content_inset` reports the sidebar extent on `right` instead of `left`. Default `None` resolves direction from an active localization scope, or from the environment (`ENZUI_TEXT_DIRECTION`, then the POSIX locale chain) outside a scope, so an RTL locale mirrors with no app change. **Visual order mirrors; logical order does not** - focus traversal and the accessibility tree keep sidebar-before-content so Tab and AT reading order are direction-independent. |
//! | `application id`, `application_id`, `Wayland application id`, `Wayland app id`, `xdg_toplevel app id`, `set_app_id`, `platform_specific.application_id`, `desktop application identity`, `desktop entry basename`, `desktop-entry basename`, `.desktop file basename`, `reverse-DNS name`, `taskbar window group`, `taskbar grouping`, `AT-SPI application name`, `apply_to_window_settings`, `secondary window identity`, `content window identity` | [`app_shell`](mod@app_shell) - [`DesktopApplicationIdentity`](app_shell::DesktopApplicationIdentity) is the validated application-boundary descriptor for `iced::window::Settings::platform_specific.application_id` (the Wayland `xdg_toplevel` id and the desktop-entry basename). Construct it from the installed `.desktop` file's basename without `.desktop`, then use its settings methods so every window (primary, detached secondary, undecorated content) carries the same identity: [`apply_to_window_settings`](app_shell::DesktopApplicationIdentity::apply_to_window_settings) preserves any other settings, [`window_settings`](app_shell::DesktopApplicationIdentity::window_settings) / [`window_settings_for`](app_shell::DesktopApplicationIdentity::window_settings_for) build primary settings for one or several layouts, [`secondary_window_settings`](app_shell::DesktopApplicationIdentity::secondary_window_settings) builds server-decorated secondary settings, and [`content_window_settings`](app_shell::DesktopApplicationIdentity::content_window_settings) builds undecorated content-window settings. The value is validated (non-empty, no `.desktop` suffix, no `/` or `\`, no NUL, ASCII `A-Za-z0-9._-` only) with a typed [`DesktopApplicationIdentityError`](app_shell::DesktopApplicationIdentityError); reverse-DNS naming is recommended by the Desktop Entry specification but not required by this type, and this descriptor neither registers a `.desktop` entry nor owns a D-Bus well-known name. Calling the free `enz_window_settings` / `enz_secondary_window_settings` / `enz_content_window_settings` constructors without this descriptor leaves iced's empty `application_id`, which lets Wayland compositors and taskbars fall back to the binary name and group unrelated windows together. |
//! | `split view`, `master detail`, `sidebar layout`, `two pane`, `three column`, `AdwNavigationSplitView` | [`navigation_split_view`](mod@navigation_split_view) - the adaptive sidebar + content navigation shell (SwiftUI `NavigationSplitView`) that collapses to a drawer. It is not a user-resizable tiled pane tree; for that use `pane_grid`. For the collapse-rule state see [`nav_split`](mod@nav_split). |
//! | `pane grid`, `paned`, `GtkPaned`, `two pane splitter`, `split pane`, `tiled panes`, `splitter`, `resizable panes`, `sash`, `terminal splits`, `pane tree`, `pane gap color`, `pane rectangle`, `pane bounds`, `pane size`, `how big is my pane`, `size content to pane`, `pty columns per pane`, `pane allocation`, `pane activation`, `keyed panes`, `draggable pane header`, `draggable titlebar`, `pane drop`, `pane rearrangement`, `pane snapshot`, `pane restore` | [`pane_grid`](mod@pane_grid) - recursive user-resizable tiled panes through [`enz_pane_grid`], with theme-derived seams, pane minimums, pointer + keyboard resizing, focus rings, and named `Role::Splitter` semantics. The app owns [`EnzPaneGridState`] and feeds [`EnzPaneGridEvent`] back to it; it never composes the native pane grid or `SeparatorModel` itself. Pointer moves update split geometry, while `PointerResizeFinished` is the one release boundary for expensive content reflow and clears pointer-active paint while retaining logical focus for a following arrow key. Use `.seam_color(..)` when rounded pane cards need their containing surface - not the standard border - to show through the gap. Sizing content in its own units (a PTY's columns, a virtualised row window) needs the pane's real pixel allocation, not `ratio * extent`: the seam comes out of the region before the ratio divides it and the first child's extent is rounded. [`EnzPaneGridState::pane_rects`](crate::EnzPaneGridState::pane_rects) / [`pane_rect`](crate::EnzPaneGridState::pane_rect) predict rectangles from the state's tokens and clamp them within the supplied extent. [`enz_pane_grid_with_content`] supplies current native allocations during layout, with an [`EnzPaneContent`] builder that reserves controls before eliding the full accessible title. Its decorative header is draggable; interactive content belongs in `.controls(..)`. Opt into [`EnzPaneGrid::on_activate`](pane_grid::EnzPaneGrid::on_activate) for keyed presses even when a child captures input, and [`on_drag`](pane_grid::EnzPaneGrid::on_drag) for [`PaneDragEvent`] destinations. Apply dropped events with [`EnzPaneGridState::drop_pane`](pane_grid::EnzPaneGridState::drop_pane), or use [`swap_panes`](pane_grid::EnzPaneGridState::swap_panes); [`snapshot`](pane_grid::EnzPaneGridState::snapshot) / [`restore`](pane_grid::EnzPaneGridState::restore) persist unique application keys and topology. Unlike `navigation_split_view`, this tree does not collapse into a navigation drawer. |
//! | `router outlet`, `route outlet`, `active route view`, `navigation stack`, `navigation outlet` | [`navigation_outlet`](mod@navigation_outlet) - projects the active route of a route path with a header, back navigation and a transition. |
//! | `segmented control`, `selected segment`, `which tab is active`, `icon only tabs`, `icons only switcher`, `mode switch`, `toggle group`, `view switcher`, `switcher label size`, `switcher label weight`, `translucent selected tab`, `material selected indicator`, `glass selected pill`, `compact dialog tabs`, `AdwViewSwitcher`, `Picker segmented`, `do these tabs fit`, `tab strip width`, `measure tab labels`, `compact tabs`, `stacked icon`, `bottom navigation`, `compact rung`, `narrow width`, `animated selection`, `sliding indicator`, `moving thumb`, `selection ease`, `settings tabs motion`, `switcher motion`, `abrupt tab fill` | [`view_switcher`](mod@view_switcher) - a segmented single-choice tab switcher; the accessible alternative to a row of N buttons. **Titlebar presentation:** `.collapse_inactive_labels(true)` retains the active label, and `.icons_only(true)` hides all icon-tab labels while preserving names, tooltips and identity. A titlebar can use measured widths to select these inline presentations with `SwitcherPolicy::compact_fallback` disabled. Collapsed strips keep unequal natural cell widths. **Compact rung:** [`SwitcherTokens::strip_width`](view_switcher::SwitcherTokens::strip_width) is the guard: at or above that measured width the switcher keeps its wide inline icon + label presentation; below it the widget itself falls back once to equal-width cells with each icon stacked over its label, so callers must not re-derive a breakpoint or elide one sibling to subsidise another. Call `strip_width` directly only when choosing a different navigation primitive altogether. **Moving indicator:** the selection fill slides between the previous and the new cell over the theme's `motion-normal` duration and easing; while it travels each label is painted per pixel region - selected ink where the thumb is under it, unselected ink where the track is - so no frame drops below the resting contrast. Selection semantics and activation commit immediately, reduced motion snaps, and a settled strip requests no frames - and asking for none is only half of it: the strip's own outer widget publishes frame interest and forwards a presented frame to the interested branch alone, instead of taking iced's defaults and fanning every frame in the window out over every cell, label and icon (`tab_pill::indicator_frame_routing` measures that over the real outer widget; full route *absence* additionally waits on the per-cell widgets adopting frame interest). Utility dialogs can pair `.size(Sm).label_size(12.0).label_weight(TabLabelWeight::AlwaysRegular)` for compact labels without changing track geometry or theme colours; see the module guidance. `.selection_indicator(TabSelectionStyle::TranslucentAccent)` is a tinted pill with no outline and an accent label; `NeutralMaterial` requests a Regular lens and uses an opaque neutral fallback inside another material. `FilledAccent` remains the default. Accessibility refusals make the accent wash opaque. |
//! | `busy tab`, `spinner in a tab`, `tab activity indicator`, `loading tab`, `install in progress tab`, `tab busy state` | [`view_switcher`](mod@view_switcher) - [`SwitcherPage::with_busy(true)`](crate::view_switcher::SwitcherPage::with_busy) shows a small spinner after the page's label, inside its tab, and reports the tab busy to assistive technology without adding text to its name. The switcher sizes and fits the tab with it, in stretched and intrinsic strips, mirrored in right-to-left text; `TabPillItem::busy` is the same on a tab list. |
//! | `inline segmented`, `inline tabs`, `tab switcher`, `inline mode switch`, `joined segments`, `segmented tabs in content`, `animated segmented indicator`, `segmented control motion` | [`inline_view_switcher`](mod@inline_view_switcher) - the **inline-segmented preset** of the one switcher engine: joined flat cells, no outer track. It is `pub type EnzInlineViewSwitcher = EnzViewSwitcher` over the shared `SwitcherPage`, so badge, per-page disabled state, id callbacks, unwired strips, `selection_bounds` and scrolling are available in both presentations. Choose between the two presentations with `SwitcherStyle` / `SwitcherPolicy`, both re-exported from [`view_switcher`](mod@view_switcher). It shares [`view_switcher`](mod@view_switcher)'s moving selection indicator. |
//! | `tab strip pills`, `detached tab hover`, `round tab hover`, `joined tab hover`, `pill tabs`, `tab list`, `filter pills`, `tab pill`, `icon tab tooltip`, `icon-only tab hover name`, `tab tooltip` | [`tab_pill`](mod@tab_pill) - tab-strip primitives: one pill, and the list that joins them. An icon-only tab ([`TabPillItem::aria_label`](tab_pill::TabPillItem::aria_label) with an empty label) shows its accessible name as a hover tooltip by default; [`TabPillItem::tooltip`](tab_pill::TabPillItem::tooltip) overrides it. [`EnzTabList::hover_shape`](tab_pill::EnzTabList::hover_shape) and [`EnzViewSwitcher::hover_shape`](view_switcher::EnzViewSwitcher::hover_shape) accept [`Detached`](tab_pill::TabHoverShape::Detached) for rounded per-cell hover and press washes at zero gap; hit bounds and spacing stay unchanged, selected cells receive no wash, and hover is withheld during selection motion. |
//! | `breadcrumbs`, `path bar`, `location bar`, `crumb trail`, `editable path`, `editable location`, `click to edit path`, `Ctrl+L location`, `breadcrumb changes size when editing`, `breadcrumb style changes when editing`, `breadcrumb mode switch`, `path field continuity`, `path completion`, `directory suggestions`, `breadcrumb autocomplete` | [`breadcrumb`](mod@breadcrumb) - clickable navigation segments with an optional caller-owned editable location mode. Editable idle/edit presentations share the complete field rectangle and chrome; swapping modes changes the text model, not the surrounding control size or style. `EnzBreadcrumb::autocomplete_editor` composes caller-fed typed directory suggestions with the same edit identity, cancellation and trailing controls; the caller owns queries, ranking and I/O. |
//! | `sidebar column`, `sidebar body`, `nav sidebar`, `side panel content`, `sidebar content`, `section rhythm`, `sidebar section rhythm`, `flat sidebar sections`, `sidebar sections that do not collapse`, `sidebar section gap`, `sidebar section caption`, `mailbox list`, `favorites and smart mailboxes`, `grouped sidebar rows`, `places and devices` | [`sidebar_content`](mod@sidebar_content) - a padded vertical column with sidebar-specific defaults, and the framework's **sidebar section rhythm**. [`.push_section("Favorites")`](crate::sidebar_content::EnzSidebarContent::push_section) pushes the shipped `section_header` caption (`enz_section_header`) with a *larger, tokenized* gap before it (the tokenized [section-gap default](crate::sidebar_content::DEFAULT_SECTION_GAP) = 20 px = `Space9`, vs the 4 px row gap) and insets it onto the [row labels' rail](crate::sidebar_row::default_label_inset_x), so the boundary between two groups outweighs the boundary between two rows inside one. It stays **flat**: no card, no border, no box around a group - for a group that *folds as a unit* (one header, one body, a chevron) the widget is `expander` (its own row below). |
//! | `nav row`, `sidebar item`, `sidebar list row`, `navigation item`, `right click a sidebar row`, `sidebar row with app icon`, `sidebar row with favicon`, `sidebar row with brand mark`, `workspace favicon in sidebar`, `sidebar row custom icon`, `sidebar row image`, `hover reveal`, `reveal on hover`, `row hover action`, `trailing on hover`, `hover accessory`, `stale row highlight`, `deselected row flashes`, `row selection flash`, `instant row selection`, `sidebar row reduced motion`, `per row reduced motion` | [`sidebar_row`](mod@sidebar_row) - a sidebar list row. Its `on_context` forwards the framework's secondary-press hook, so a mailbox/workspace right-click menu needs no hand-wrapped [`context_target`](mod@context_target). A rail of N rows built from a `Fn(..) -> Message` should use its `enz_sidebar_row_lazy` constructor, which does not construct N messages per view pass. An **app-owned** leading icon (a workspace favicon, a provider brand mark) plugs into the same slot via [`.icon_source(..)`](crate::sidebar_row::EnzSidebarRow::icon_source), so a row with a custom glyph stays a proper `Role::ListItem` instead of being hand-rolled out of `list_row_surface` and losing its semantics. A trailing control can use [`.trailing_visibility(TrailingVisibility::OnHoverOrFocus)`](crate::sidebar_row::EnzSidebarRow::trailing_visibility): its width stays reserved and its node stays mounted/focusable for AT while only resting paint is suppressed; row hover or keyboard focus on the row/action reveals it, and `OnHoverFocusOrSelected` additionally keeps the *selected* row's action painted after the pointer leaves. **Selection hand-off:** the row's selected fill cross-fades from the previously selected colour by default, which is right for a toggle and wrong for navigation - a route change rebuilds the *deselected* row with a fresh animation, so it flashes the old highlight for a few frames while the new row lights up. [`.reduced_motion(true)`](crate::sidebar_row::EnzSidebarRow::reduced_motion) is the per-instance override that commits selection paint instantly for that rail without touching the theme-wide policy. |
//! | `headerbar`, `top bar`, `AdwHeaderBar`, `app header`, `header bar` | [`header_bar`](mod@header_bar) - a top-of-window header bar with title + actions. |
//! | `large title`, `page title header`, `hero header`, `screen title`, `page header` | [`page_header`](mod@page_header) - a large-title page header with optional actions. |
//! | `pane title`, `pane header`, `section header with action`, `header with trailing action`, `title with count`, `left aligned title header`, `18px header`, `section band` | [`pane_header`](mod@pane_header) - the title-rung header between the 11 px section caption and the 28 px page hero: a left-aligned ~18 px title with an arbitrary trailing/leading `Element` and an inline subtitle/count, publishing `Role::Heading` at a chosen rung. |
//! | `titlebar`, `csd titlebar`, `window titlebar`, `client side decoration`, `app titlebar`, `titlebar chrome`, `window drag`, `drag to move window`, `title bar`, `responsive titlebar`, `titlebar overflow model`, `titlebar overflow row id`, `titlebar menu row namespace`, `double row titlebar`, `titlebar wraps to two rows`, `resize hides titlebar icons`, `collapse titlebar buttons`, `titlebar heading`, `heading titlebar`, `sidebar header heading`, `pane header heading`, `modal titlebar`, `drawer titlebar overlap`, `titlebar occupancy`, `inert titlebar controls`, `titlebar pattern`, `titlebar height`, `titlebar padding`, `back button in titlebar`, `two close buttons`, `who draws the window controls`, `titlebar search in the middle` | [`title_bar`](mod@title_bar) - the client-side-decoration titlebar: ordered slots, drag regions, per-window gestures. **The one titlebar pattern:** in the shell's titlebar slot, `enz_title_bar()` takes the shell's height and padding and draws no window controls of its own (the shell's `.on_window_gesture(..)` gives the window its controls); put a back button in [`.leading(..)`](title_bar::EnzTitleBar::leading), the title in `.title(..)` at the leading edge, a search field in `.center(..)` and the commands in `.actions(..)` as [`ActionItem::icon`](action_bar::ActionItem::icon). The `reference_*` examples show it in each shell layout, and `enzui_test_support::titlebar_adoption` checks an app for it. [`.modal_navigation(..)`](title_bar::EnzTitleBar::modal_navigation) yields ordinary slots and band paint while retaining window exits; A bar fed to a pane's header slot (the shell's `sidebar_titlebar`) keeps its caption in the accessibility outline with [`.title_heading(HeadingLevel::..)`](title_bar::EnzTitleBar::title_heading); a **window** titlebar names no rung and is unchanged. Its `.actions(..)` slot is the band's **measured** overflow model: trailing commands fold whole into one `⋯` menu against the width the band actually has, the window controls never fold, and the band never becomes two rows. Use its action message to toggle the trigger and [`.action_dismiss(..)`](title_bar::EnzTitleBar::action_dismiss) for an idempotent close after activation, Escape, or outside press. Call [`.action_menu_id_prefix(..)`](title_bar::EnzTitleBar::action_menu_id_prefix) when automation or accessibility requests address folded rows; the stable, tree-unique caller namespace prevents equal action ids in separate titlebars from colliding. |
//! | `titlebar tab`, `tab in titlebar`, `browser tab in titlebar`, `document tab titlebar band`, `tab centreline`, `variable-width document tabs`, `fold document tabs`, `tab overflow list`, `list all documents` | [`titlebar_tab`](mod@titlebar_tab) - [`enz_titlebar_tab_row`] is the variable-width browser/editor strip: it measures a caller-overridable filename sample, keeps the active closable tab visible, folds only the documents that no longer fit legibly, and exposes every document through a correctly named overflow menu. For one cell, `enz_titlebar_document_tab` binds a [`document_tab`](mod@document_tab) and neighbouring [`window_controls`](mod@window_controls) chip to the same painted height and centreline. |
//! | `titlebar cluster`, `titlebar widgets row`, `title bar group` | [`titlebar_group`](mod@titlebar_group) - a basic horizontal cluster of titlebar widgets; a back-compat alias that delegates to the `Ghost` variant of [`titlebar_icon_group`](mod@titlebar_icon_group). |
//! | `titlebar icon cluster`, `icon group titlebar`, `titlebar icons`, `titlebar toolbar` | [`titlebar_icon_group`](mod@titlebar_icon_group) - the container for a custom titlebar icon cluster: grouping rhythm, centre alignment and a rounded clip mask. Wrap [`toolbar_icon`](mod@toolbar_icon) children - and, outside [`title_bar::EnzTitleBar`](mod@title_bar), the chrome-free [`titlebar_actions`](mod@titlebar_actions) preset - in this. |
//! | `close minimize maximize`, `window buttons`, `traffic lights`, `min max close`, `window controls`, `window close button`, `minimize button`, `maximize button` | [`window_controls`](mod@window_controls) - the titlebar close/minimize/maximize group. [`title_bar`](mod@title_bar) builds it for you once `on_gesture` is wired. |
//! | `toolbar`, `action row zones`, `start center end row`, `toolbar cluster`, `toolbar row` | [`toolbar_row`](mod@toolbar_row) - a start / center / end zone toolbar cluster row. Use `.center_title(..)` for a single-line end-elided title; `.center(..)` accepts custom content. |
//! | `header plus content`, `toolbar scaffold`, `header content shell`, `toolbar view` | [`toolbar_view`](mod@toolbar_view) - a header bar + content region shell composite. |
//! | `toolbar separator`, `vertical hairline`, `toolbar rule`, `toolbar divider` | [`toolbar_divider`](mod@toolbar_divider) - an inset vertical hairline for toolbar item groups. |
//! | `toolbar gap`, `toolbar spacer`, `space between toolbar items` | [`toolbar_spacer`](mod@toolbar_spacer) - a spacer between toolbar item groups. |
//! | `statusbar`, `bottom bar`, `status strip`, `footer bar`, `status bar`, `status bar item`, `status bar button`, `statusbar segment`, `status bar segment`, `status bar action`, `status bar separator`, `status bar stat`, `stat cell`, `caption over value`, `stacked stat`, `activity monitor bar`, `summary bar stat`, `embedded status bar`, `embedded surface`, `transparent status bar`, `status bar without background`, `status bar inheriting host surface`, `status bar radius`, `rounded status strip`, `evenly spaced status items`, `distribute items`, `equal columns`, `equal width stat columns`, `aligned stat columns`, `caption over value alignment` | [`status_bar`](mod@status_bar) - a bottom status strip with optional interactive items. The `status bar *` terms are recorded because several separate searches typed them looking for a **clickable segment** - a status-bar item with focus / hover / press / activation - and found nothing, so an app hand-rolled one out of `Outlined`/`Xs` buttons and a literal pipe character as a separator. `enz_status_bar_strip` with `StatusBarItem::action` is that primitive. A **stacked caption-over-value stat cell** (`StatusBarItem::stat("Processes", "460")`, the macOS Activity-Monitor shape) is `StatusBarItem::stat`: an app reached for it, found the strip one variant short, and hand-rolled the whole bar - losing single-line elision and the `TextElision` audit finding until a visual sweep photographed a sliced glyph. Both stat lines are elided runs, so the cell publishes a `TextElision` finding, and a strip containing a stat sizes its band to the two-line height without changing height when a value appears or disappears. The strip rounds its own corners with `EnzStatusBarStrip::radius` (`status bar radius` / `rounded status strip`) - a `RadiusSize` token so an embedded strip can match a rounded host instead of squaring against it - and spreads *n* items evenly with `EnzStatusBarStrip::distribute(Distribution::Even)` (`evenly spaced status items` / `distribute items`), computing the gap from its own width so a caller no longer inserts and re-guesses spacers. `Distribution::EqualColumns` is the other spread and the one a **summary bar of stats** wants: `Even` distributes the *gaps* between cells of unequal width, so no two cells share a pitch and the narrowest one ends up nearest an edge (an app's process count read as right-aligned); `EqualColumns` gives every non-separator item an equal share of the band and centres it in that share, so the columns have one pitch and each caption sits over its own value. |
//! | `content region`, `padded pane`, `main content pane`, `body pane`, `content pane` | [`content_pane`](mod@content_pane) - a full-bleed pane with padded vertical content layout. |
//! | `edge to edge bands`, `full bleed stack`, `band layout`, `stacked bands`, `band stack` | [`band_stack`](mod@band_stack) - a transparent full-bleed vertical stack of self-styled edge-to-edge bands (toolbar + body + status). |
//! | `hbox`, `horizontal row`, `row with spacing`, `h stack row`, `adaptive row` | [`adaptive_row`](mod@adaptive_row) - a horizontal row with configurable spacing. |
//! | `aspect ratio`, `fixed ratio`, `keep ratio`, `16 9 frame` | [`aspect_ratio`](mod@aspect_ratio) - a frame that keeps a fixed width-over-height ratio, deriving height from the resolved width. |
//! | `centre`, `center content`, `vertically center`, `horizontally center` | [`center`](mod@center) - thin axis-specific centring recipes (both axes, vertical only, horizontal only). |
//! | `max width`, `readable width`, `constrain width`, `limit width` | [`clamp`](mod@clamp) - cap child width to a readable-width max, centred in the parent. |
//! | `flex wrap`, `wrap layout`, `chip flow`, `tag flow` | [`flow`](mod@flow) - a flex-wrap container. See also [`wrap`](mod@wrap) for a fixed-cell model-driven variant. |
//! | `flow child`, `wrap item`, `flow cell`, `flow item` | [`flow_item`](mod@flow_item) - a fixed-height child for a flow / wrap layout. |
//! | `fixed grid`, `column grid`, `LazyVGrid`, `grid layout` | [`grid`](mod@grid) - a fixed-column grid layout. For a responsive column ramp use [`responsive_grid`](mod@responsive_grid). |
//! | `adaptive grid`, `responsive columns`, `size class grid`, `auto columns`, `responsive grid`, `category grid`, `category tiles`, `tile grid` | [`responsive_grid`](mod@responsive_grid) - an own-width responsive grid with a size-class column ramp. Also the answer for a **category grid** - a store or launcher landing surface's grid of category tiles: the searcher who typed `category grid` found nothing and hand-rolled columns from a `row!` of cards, losing the size-class breakpoints. |
//! | `responsive grid cell`, `grid item host` | [`responsive_grid_item`](mod@responsive_grid_item) - a fixed-width, fixed-height cell host for a responsive grid. |
//! | `vstack`, `hstack`, `vertical stack`, `column stack` | [`stack`](mod@stack) - tokenized vertical / horizontal stack primitives. |
//! | `flexible space`, `filler`, `push apart`, `expanding gap` | [`spacer`](mod@spacer) - a fill-both-axes invisible spacer (SwiftUI `Spacer`). |
//! | `badge inventory`, `chip inventory`, `fixed cell wrap`, `tag cloud` | [`wrap`](mod@wrap) - a model-driven, fixed-cell badge/chip inventory. For free flex-wrap use [`flow`](mod@flow). |
//! | `cached subtree`, `retained subtree`, `retained pane`, `skip rebuild`, `do not rebuild an unchanged pane`, `invalidation token`, `revision`, `dirty subtree`, `subtree cache`, `memoize a pane`, `lazy pane`, `one label rebuilds the whole window` | [`cached_subtree`](mod@cached_subtree) - an identity-backed retained boundary: a [`SubtreeHandle`] held in *application* state names the pane and carries an explicit revision, and while that revision (plus the tracked limits / font generation / style generation) is unchanged the pane's `view` closure is **not called**, its subtree is **not diffed**, and its retained `iced::advanced::layout::Node` is replayed instead of laid out. Invalidation propagates to attached ancestors while siblings stay warm. Paint still traverses: the fork lacks a renderer capture/replay hook, documented in the module. Not the same thing as [`enz_responsive`](mod@responsive) (one closure, one frame, one width) or [`virtual_list`](mod@virtual_list) (realisation, not retention). |
//! | `cached responsive`, `responsive cached`, `enz_responsive`, `cached width container` | [`enz_responsive`](mod@responsive) - a width-aware container with layout caching: the closure runs at most once per distinct width per frame. Drop-in replacement for `iced::widget::responsive` for expensive closures. |
//! | `responsive fallback`, `pick first that fits`, `ViewThatFits`, `adaptive layout choice` | [`view_that_fits`](mod@view_that_fits) - renders the first candidate layout that fits, with the losers absent from the tree (SwiftUI `ViewThatFits`). |
//! | `scrolling toolbar`, `pinned ends row`, `overflow scroll row`, `overflow row` | [`overflow_row`](mod@overflow_row) - a horizontal bar with fixed leading + scrollable content band + pinned trailing slot. |
//! | `surface`, `panel background`, `base container`, `styled container`, `theme radius`, `corner style`, `square corners`, `material`, `material surface`, `liquid glass`, `glass`, `glass panel`, `frosted glass`, `backdrop blur`, `blur behind`, `blur the background`, `background blur`, `see through panel`, `translucent panel`, `vibrancy`, `acrylic`, `lens`, `refraction`, `reduced transparency`, `protected inset` | [`surface`](mod@surface) - the tokenized visual surface with no imposed inner layout. The default `radius.md` and explicit semantic `radius_size` follow the live theme; pixel radius pins and forwarded radius scales opt out. [`EnzSurface::radius_resolved`](surface::EnzSurface::radius_resolved) shares one per-draw shape across chrome, material geometry, clipping, and pointer hits. [`card`](mod@card) is the common preset, using `radius.lg` unless pinned. **Sampled-backdrop materials live here too:** `EnzSurface::material` takes a semantic `enzui_theme::MaterialStyle` (`Regular`, `Clear`, `Lens`, `Subtle`, `Frosted`, `Optical`, `FlatFrosted`, `Solid`) and the renderer filters the content already painted behind the panel - not a translucent fill over a guess. The boundary is ordered, so the surface's own content is never part of its backdrop, and an unsupported backend, a reduced-transparency or increased-contrast preference, a nested material or a `Solid` style each paint the opaque panel instead of reporting a filter that did not happen. Reduced motion keeps the filter and drops the refraction and dispersion that make scrolling content warp at the rim. The preferences are transported, not guessed: reduced motion arrives through the live theme's motion policy and the other two through `enzui_core::material_preferences` (the freedesktop settings portal), joined per dimension with any user override, so a desktop that answers nothing leaves every material enabled and says so. Text-bearing panels keep the theme's protected inset (`MaterialInk::Protected`, the default, which raises padding to it); a compact icon-only control opts out with `MaterialInk::Strong` and owns the consequence. The look follows [ybouane/liquidglass](https://github.com/ybouane/liquidglass): shoutout to its author, whose slab model and presets the stock styles are built on. |
//! | `desktop glass`, `glass over the desktop`, `glass sidebar`, `see the wallpaper`, `blur behind the window`, `transparent window glass`, `compositor glass`, `window backdrop`, `frosted window frame`, `glass window frame`, `backdrop blur frame`, `frosted shell` | [`desktop_glass`](mod@desktop_glass) - glass that shows the desktop and the windows behind the app, painted by the compositor: [`EnzSurface::backdrop`](surface::EnzSurface::backdrop) with `Backdrop::Window`, or [`PaneStyle::behind_window`](app_shell::PaneStyle::behind_window) for a shell pane, inside an [`enz_desktop_glass`] host bound with [`desktop_glass::bind`]. Where the compositor offers no `enzui-material-v1`, the pane keeps in-app glass; reduced transparency makes it solid. A whole window frame (the shell's backdrop around and between its panes) turns to glass with [`ShellTokens::frame_material`](app_shell::ShellTokens::frame_material), and the theme file's `window.frame` can turn it on or off for every app: `app_setup` binds the window itself, the panes stay filled and are cut out of the glass so the compositor spends nothing under them, and the frame stays opaque where the compositor paints no glass. |
//! | `crash notice`, `crashed last time`, `quit unexpectedly`, `crash report banner`, `show crash report` | [`crash_notice`](mod@crash_notice) - [`enz_crash_notice`](crash_notice::enz_crash_notice) is the warning banner for the newest of `enzui_core::crash_report::unseen_reports` at start-up, with Show Report and dismiss; mark the report seen on either. |
//! | `print`, `PDF export`, `save as PDF`, `render pages to PDF`, `print preview document`, `page layout for printing` | [`print_pdf`](mod@print_pdf) - [`PdfDocument`](print_pdf::PdfDocument) flows headings, paragraphs and images over A4 or Letter pages with the app's fonts and writes vector, searchable PDF bytes; hand them to `enzui_core::portal::print` or save them. |
//! | `wizard`, `assistant`, `step by step setup`, `onboarding steps`, `GtkAssistant`, `QWizard`, `multi-step form` | [`wizard`](mod@wizard) - [`enz_wizard`](wizard::enz_wizard) over an `enzui_core::wizard::Wizard`: reached steps in the list are clickable, Next is disabled until the page's answers allow it and becomes Finish on the last step; `wizard::apply` moves the state for each `WizardEvent`. |
//! | `tab overview`, `tab grid`, `all tabs`, `AdwTabOverview`, `tab switcher grid`, `tab previews` | [`tab_overview`](mod@tab_overview) - [`enz_tab_overview`](tab_overview::enz_tab_overview) wraps `TabCard`s (title, preview element, selected) into a grid with a close button each and a new-tab card, reporting `TabOverviewEvent`s. |
//! | `notification inbox`, `notification center`, `notification history`, `past notifications`, `unread notifications`, `bell menu` | [`notification_inbox`](mod@notification_inbox) - [`enz_notification_inbox`](notification_inbox::enz_notification_inbox) draws an `enzui_core::notification_inbox::Inbox` (newest first, unread count for a badge, limit); `notification_inbox::apply` handles open (mark read), dismiss and Clear All. |
//! | `editable rating`, `rate this`, `star input`, `give stars`, `rating input`, `review stars input` | [`rating_input`](mod@rating_input) - [`enz_rating_input`](rating_input::enz_rating_input) is a row of focusable star buttons ("n of max stars"); pressing the current star clears the rating. The display-only score is [`rating`](mod@rating). |
//! | `exclusive accordion`, `accordion group`, `only one section open`, `one open at a time`, `FAQ list` | [`accordion`](mod@accordion) - [`enz_accordion`](accordion::enz_accordion) stacks [`expander`](mod@expander)s with one `open` index; opening one closes the others. |
//! | `svg`, `svg image`, `themed svg`, `illustration`, `symbolic picture`, `accessible svg`, `svg alt text` | [`svg_image`](mod@svg_image) - [`enz_svg`](svg_image::enz_svg) draws an SVG handle at a chosen size; `.symbolic(true)` paints it in the theme's text colour, `.label(..)` makes it an image to screen readers (decorative without). Icons from the icon set are [`icon`](mod@icon). |
//! | `canvas`, `custom drawing`, `accessible canvas`, `canvas alt text`, `drawing with description` | [`canvas_view`](mod@canvas_view) - [`enz_canvas`](canvas_view::enz_canvas) draws a `canvas::Program` and publishes an image with `.label(..)` and `.description(..)`, or nothing when decorative. |
//! | `system bell`, `beep`, `alert sound`, `visual bell`, `error bell`, `xdg_system_bell` | [`system_bell`](mod@system_bell) - [`system_bell::ring`] asks the compositor to ring the bell for a window (`xdg_system_bell_v1`), played as the user configured it; where there is none it resolves to `BellError::Unsupported` and the app may play the sound theme's `bell` event. |
//! | `content type`, `game mode`, `video playback window`, `tearing`, `allow tearing`, `low latency presentation`, `colour space`, `color management`, `HDR` | [`surface_hints`](mod@surface_hints) - [`surface_hints::content_type`] and [`surface_hints::tearing`] set `wp_content_type_v1` and `wp_tearing_control_v1` state on a window's surface and resolve to `HintError::Unsupported` where the compositor lacks the protocol. The colour space needs no call: the renderer draws sRGB and Mesa's Vulkan WSI describes it through `wp_color_management_v1`; HDR output is not supported. |
//! | `flat frosted`, `flat tinted blur`, `flat tinted backdrop blur`, `flat backdrop blur`, `flat glass`, `frosted panel`, `non-refracting material`, `glass button`, `glass control`, `make a widget glass`, `glass on any widget`, `tinted glass`, `controls on glass`, `glass menu`, `glass toolbar`, `fixed glass ink`, `adaptive glass`, `idle backdrop update`, `glass title bar`, `titlebar island`, `controls over video` | [`glass`](mod@glass) - [`enz_glass`] wraps any widget in liquid glass of its own shape, and widgets with a panel of their own take `.material(style)`: buttons, cards, banners, toasts, notification cards, menus, text and search fields, badges, toolbar icons, view switchers, header bars, title bars, modal surfaces and the stock dialogs; menu buttons, context menus and action bars take `.menu_material(style)` (for example [`EnzButton::material`](button::EnzButton::material)). A titlebar floating over a picture puts its trailing actions and the close button on one glass pill with [`EnzAppShell::titlebar_island`](app_shell::EnzAppShell::titlebar_island). Glass also works inside overlays, so a popup menu samples the page beneath it. Glass keeps the theme's foreground and fill whatever passes beneath (media `Clear` glass keeps light ink); [`EnzGlass::ink`](glass::EnzGlass::ink) with `GlassInk::Adaptive` opts into ink that follows the backdrop's brightness, and [`EnzSurface::glass_ink`](surface::EnzSurface::glass_ink) provides the same override. Content inside a glass surface is drawn with the glass palette, so its buttons, fields and segments turn translucent by default; tokens pinned by the caller are kept. `MaterialStyle::FlatFrosted` filters the painted backdrop with theme tint and diffusion, without a lens, rim lighting or default shadow. Use flat blur for text-bearing panels in a flat UI; choose `Regular` or `Lens` for optical glass. `enz_glass(content).style(MaterialStyle::FlatFrosted).tint_opacity(0.45)` needs no token recipe; `EnzSurface::material_tint_opacity` provides the same local control. Flat geometry permits a zero protected inset; optical styles retain their protected clearance. Both follow the existing radius, clipping, ordered backdrop and opaque accessibility fallback paths. |
//! | `grain`, `noise`, `film grain`, `paper texture`, `grainy surface`, `grainy sidebar`, `grainy dialog`, `speckle`, `noise texture`, `texture overlay`, `svg texture`, `halftone`, `print look`, `feTurbulence` | [`grain`](mod@grain) - a paper-like speckle on any surface: [`EnzSurface::grain`](surface::EnzSurface::grain) lays it on a surface's fill under its content, masked to its corners; a shell pane takes [`PaneStyle::grain`](app_shell::PaneStyle::grain) (a grainy sidebar, titlebar or drawer); dialogs (`EnzModalSurface::grain` and the stock confirm, save-changes, form, settings and shortcuts dialogs), [`EnzCard::grain`](card::EnzCard::grain) and popovers take `.grain(..)`; [`enz_grain`](grain::enz_grain) draws it over any other element. A [`Grain`](enzui_theme::Grain) is `Copy` and comes from `enzui-theme`: presets `Grain::default()`, `subtle()`, `paper()` (strong, over the content) and `film()` (shimmering at 24 fps), and knobs for intensity, density, scale, tone (light and dark specks, or one), inks or one tint, opacity, the built-in noise's frequency, octaves and seed, tile size, tiling and placement (behind or over the content). `GrainSpec::Theme` follows the theme's `EnzTheme::grain`. Any SVG can be a grain with `Grain::svg(document)`: its brightness becomes specks, or with `GrainMapping::Artwork` it is drawn in its own colours; `GrainTiling::Mirror` makes a source that does not repeat seamlessly tile without seams. Tiles are rasterised once per screen scale and cached, so a moving grainy surface costs an image draw per visible tile; film grain redraws its window at its frame rate. High-contrast palettes draw no grain. |
//! | `glass in any shape`, `glass silhouette`, `glass numerals`, `glass clock`, `glass digits`, `glass glyphs`, `glass text outline`, `glass pill shape`, `login clock glass`, `lock screen clock`, `clear glass shape`, `frosted glass shape`, `adaptive tone glass`, `glass over dark wallpaper` | [`adaptive_glass`](mod@adaptive_glass) - [`enz_adaptive_glass`](adaptive_glass::enz_adaptive_glass) fills a [`GlassSilhouette`](adaptive_glass::GlassSilhouette) (from SVG or an alpha raster) with flat liquid glass: each connected component is its own lens bending the backdrop towards its centre, counters stay clear, and one tone for the whole shape lifts towards white over dark backdrops and stays translucent over bright ones. `AdaptiveGlass::Clear` and `Frosted` are the login-clock recipes; `AdaptiveGlass::material` returns them for a custom primitive. Reduced motion settles the optics; reduced transparency or a renderer without optical materials fills the shape with a plain colour. For glass behind a rectangular widget use [`glass`](mod@glass). |
//! | `surface with stack`, `panel with column`, `container column`, `surface stack` | [`surface_stack`](mod@surface_stack) - a surface with a built-in vertical stack. |
//! | `group hover`, `hover group`, `hover scope`, `tint a group of rows`, `section hover`, `hover the whole section`, `hover over the whole control`, `nested hover`, `pointer inside group`, `hover a set of rows` | [`hover_group`](mod@hover_group) - a container that tints its background while the pointer is anywhere inside it, resolved **at draw time** from `(theme, hovered)` so it never becomes app state. A nested [`list_row_surface`](mod@list_row_surface) keeps its own, stronger per-row hover painted on top, so the two scopes compose (a sidebar section tints as one unit while the row under the pointer stays the click target). Reach for this instead of `iced::widget::mouse_area` + `on_enter`/`on_exit` tracking hover in your own state, which needs an ordering guard against boundary flicker and does not survive scrolling. |
//! | `activity pulse`, `auto hide controls`, `hide chrome when idle`, `pointer activity`, `idle pointer`, `mouse moved`, `show controls on move`, `throttle pointer moves`, `immersive viewer chrome`, `hide titlebar at rest`, `idle timer`, `fade chrome over video` | [`activity_pulse`](mod@activity_pulse) - reports pointer activity over its content at most once per interval (and once after it stops), so auto-hiding chrome re-arms its timer without an update per pointer event. [`.on_idle(..)`](activity_pulse::EnzActivityPulse::on_idle) reports the rest itself, with no app timer: drive the shell's `.chrome_visible(..)` from the two messages and a floating titlebar fades out at rest and back on movement. |
//! | `card`, `tile`, `panel card`, `content card`, `panel`, `featured`, `featured app`, `featured tile`, `featured card`, `hero`, `hero card`, `promoted card`, `spotlight card`, `card text scale`, `card typography` | [`card`](mod@card) - a styled surface for a discrete entity or featured/hero item. Use `Flat` for a page section; `page_header` is a title, not a promoted item. Unpinned title/description/footer follow desktop text scale through `Title3`/`Body`/`Caption`; `.typography(theme.typography)` forwards a custom ramp. Explicit token/rhythm size pins win; caller-owned body content is unchanged. |
//! | `clipping pane`, `clip container`, `content clip` | [`pane`](mod@pane) - a content-clipping pane. |
//! | `group box`, `GroupBox`, `titled group`, `frame with title`, `fieldset`, `group content`, `visual grouping`, `page section` | [`group_box`](mod@group_box) - a titled sunken content grouping (SwiftUI `GroupBox`). An app author who typed "group content" / "visual grouping" / "page section" and got nothing then hand-rolled the widget; the aliases point at `group_box`, and the composes-with row records that it presents through `card`. |
//! | `accordion`, `disclosure section`, `AdwExpanderRow`, `collapsible section`, `reveal section`, `expander with app icon`, `expander custom icon`, `expander brand mark`, `expander favicon`, `collapsible sidebar section`, `collapsible sidebar sections`, `next section follows the collapse`, `content below moves while collapsing`, `sidebar group`, `sidebar section`, `sidebar with sections`, `grouped sidebar`, `workspace section`, `folder header`, `header with new button`, `channel group`, `explorer section`, `finder tag group` | [`expander`](mod@expander) - a flat-by-default disclosure with visible-card and structure-removing plain variants (accordion / collapsible): **one header, one body, folding as a single unit**. This is the **sidebar-*section*** widget - reach for it when a sidebar is a stack of named groups (Workspaces, Favourites, a channel list) each of which collapses together. For a scrollable tree of hierarchical *rows* at arbitrary depth instead, that is `tree_view` (its own row below). For a **flat** sectioned rail whose groups do not fold - *Favorites*, then *Smart Mailboxes*, then one group per account - that is `sidebar_content`'s [`.push_section(..)`](crate::sidebar_content::EnzSidebarContent::push_section) section rhythm (its own row above). An **app-owned** header icon (a provider brand mark, an account avatar) plugs into the same slot via [`.icon_source(..)`](crate::expander::EnzExpander::icon_source) so the section header stays labelled and toggleable rather than being hand-rolled. For a **collapsible sidebar section** (a workspace header + its child rows tinting as one unit, with a trailing action beside the chevron): [`.hover_scope(HoverScope::Group)`](crate::expander::EnzExpander::hover_scope) wraps the whole expander in an [`EnzHoverGroup`](crate::hover_group) so the group tints as one; [`.header_action(Element)`](crate::expander::EnzExpander::header_action) puts a `+`/kebab/pill beside the chevron whose activation does **not** toggle the section (the inner button captures its own press). |
//! | `disclosure`, `expand collapse`, `chevron toggle`, `show more`, `DisclosureGroup` | [`disclosure`](mod@disclosure) - an expand/collapse toggle row (SwiftUI `DisclosureGroup`). |
//! | `separator`, `rule`, `hr`, `horizontal line`, `faded divider`, `fade divider ends` | [`divider`](mod@divider) - a styled divider/separator. Use `.fade_ends(true)` or `DividerTokens::fade_ends` to fade each horizontal, vertical or labeled segment at its ends. |
//! | `icon box`, `fixed icon area`, `icon bounding box`, `icon slot`, `artwork slot` | [`icon_slot`](mod@icon_slot) - a fixed-dimension bounding box that centres an icon. `.icon_source(IconSource)` also accepts SVG/raster handles and preserves artwork colors. |
//! | `scroll`, `scrollable`, `scroll area`, `scroll anchor`, `keep the page in view while the content reflows`, `document jumps when the sidebar animates`, `preserve scroll position on resize`, `scroll container`, `scroll view`, `scrollbar track inset`, `scrollbar safe area`, `rounded pane scrollbar`, `scrollbar corner clipping`, `scroll chrome clearance`, `scroll under titlebar`, `independent scroll track padding`, `minimum scrollbar thumb`, `usable scrollbar thumb`, `scrollbar grab length`, `scrollbar over custom background`, `scrollbar over app-painted backdrop`, `scrollbar contrast on custom canvas` | [`scroll_view`](mod@scroll_view) - a general single-child scrollable region. Every framework scroll surface (including virtual list/grid/table and editor adapters) clamps the thumb's long axis to [`ScrollbarTokens::min_thumb_length`](enzui_theme::ScrollbarTokens::min_thumb_length) when the track can fit it, clamps again to short tracks, and remaps travel so start/end remain exact. When a thumb fills a short track it has no travel and deliberately ignores drags instead of changing the offset. When the app paints its own canvas behind the surface (an app's reading backgrounds, a terminal), [`EnzScrollView::backdrop`](scroll_view::EnzScrollView::backdrop) resolves the overlay thumb ink for contrast against *that* colour rather than the palette's assumed surface.  [`EnzScrollView::scrollbar_insets`](scroll_view::EnzScrollView::scrollbar_insets) shortens painted tracks and pointer bands independently of content padding, with separate top/bottom and left/right ends; virtual lists and grids expose the same builder. In a geometry-aware shell content closure, [`ShellGeometry::scrollbar_safe_area`](app_shell::ShellGeometry::scrollbar_safe_area) derives clearance from resolved floating chrome, shell spacing and primary pane corners while content continues beneath it; secondary slots use [`ShellGeometry::secondary_scrollbar_safe_area`](app_shell::ShellGeometry::secondary_scrollbar_safe_area) for their own corner style. [`EnzScrollView::anchor`](scroll_view::EnzScrollView::anchor) keeps a logical position (a fit-width document's page) in place while the content reflows, applying and drawing the offset in the same layout, so a sidebar animating beside the document never paints a frame at a stale offset. |
//! | `overlay scrollbar`, `offset scrollbar`, `caller-owned offset`, `scrollbar without a gutter`, `scroll thumb over canvas`, `scrollbar over caller offset`, `paint a scrollbar over a virtualised body`, `scrollbar chrome over an offset the caller owns`, `report the wheel do not apply it`, `veto the wheel`, `alternate screen wheel`, `terminal scrollbar`, `transcript scrollbar` | [`offset_scrollbar`](mod@offset_scrollbar) - [`enz_offset_scrollbar`] paints tokenised overlay scrollbar chrome over a child whose scroll offset the **caller** owns. Reach for it when the offset drives content *selection* (a virtualised row window, a transcript slice) so it must exist before layout - nesting a [`scroll_view`](mod@scroll_view) would give two owners of one number. It is **layout-transparent** (no reserved gutter - the child's measured width is byte-identical with and without the chrome - the terminal's PTY column count depends on it), reports the wheel notch through `.on_wheel(delta)` **without applying it** so the caller can veto it and forward it to a PTY on an alternate screen, publishes an **absolute** offset through `.on_scroll_to(offset)` on drag, and resolves the thumb ink against a caller-supplied [`.backdrop(color)`](offset_scrollbar::EnzOffsetScrollbar::backdrop) rather than the palette's assumed surface. For a scrollable region whose offset the widget owns, reach for [`scroll_view`](mod@scroll_view) instead. |
//! | `editor scrollbar`, `code scrollbar`, `bare editor`, `frameless`, `plain variant`, `scrollbar for my own editor`, `text editor scrollbar`, `app-owned editor chrome`, `scrollbar over a text_editor`, `scroll a bare text editor`, `logical line scroll`, `restore wrapped scroll`, `chunked editor overscan` | [`editor_scrollbar`](mod@editor_scrollbar) - [`enz_editor_scrollbar`] paints the auto-hidden overlay thumb over a **caller-owned** `iced::widget::text_editor` whose scroll offset the *editor* owns. Reach for it when an app builds its own editor chrome - a **bare** `iced::widget::text_editor`, a **frameless**/**plain variant** field with a distinct font-size ladder or a line-number gutter - so it cannot adopt [`text_area`](mod@text_area) without reshaping its text, yet still needs a scrollbar. It reads the editor's real `scroll_metrics()` by default and drives an absolute `Action::ScrollTo` through the caller's own reducer, keeping the editor the sole scroll owner. A viewport-materialized editor may supply summarized full-document geometry with `.metrics(...)` while retaining that same action route. To restore a requested wrapped line in a materialized slice, use `iced::widget::text_editor::Action::ScrollToLine` with the model-local logical line and a pixel displacement within its paragraph; retain leading overscan without estimating the paragraph height from the projection. For a fully-chromed field that builds this adapter for you, use [`text_area`](mod@text_area) (its `TextAreaVariant::Plain` gives the frameless field); when the **caller** owns the offset instead of the editor, use [`offset_scrollbar`](mod@offset_scrollbar). |
//! | `scrolling column`, `vertical scroll stack`, `top anchored scroll`, `scroll column` | [`scroll_column`](mod@scroll_column) - a top-anchored scrollable vertical stack. |
//! | `scrolling card`, `scroll inside card`, `bordered scroll`, `scroll card` | [`scroll_card`](mod@scroll_card) - a flat scrolling surface with an opt-in border. |
//! | `horizontal scroll`, `side scroll`, `scroll strip`, `carousel scroll strip` | [`horizontal_scroll_view`](mod@horizontal_scroll_view) - a fixed-height horizontal scrolling strip. |
//! | `data grid`, `data table`, `spreadsheet`, `resize column`, `column width`, `resizable columns`, `fit column to contents`, `column resize handle`, `DataTable`, `virtualised table`, `virtualized table`, `table viewport`, `whole row viewport`, `fixed-height table rows`, `complete table row`, `partial row clip`, `responsive allocation`, `chrome budget`, `actual allocation`, `selectable table`, `row selection`, `selected table row`, `row activation`, `clickable table row`, `rounded table row`, `table scrollbar`, `responsive table`, `fold columns`, `hide columns on narrow screens`, `row detail`, `expandable row`, `column priority` | [`enz_table`](mod@table) - a bounded 2D virtualised data grid (shadcn `DataTable`). [`EnzTable::row_selection`](crate::table::EnzTable::row_selection) paints selection once across the assembled row (including cell gaps) and publishes selected state on `GridRow`; [`EnzTable::on_row_activate`](crate::table::EnzTable::on_row_activate) moves hover, press and activation onto that same rounded row while context targets may remain in cell content. Because frozen and scrolling panes cannot share one physical interaction owner, row activation requires `frozen_columns(0)` (the default); the unsupported composition is rejected instead of publishing duplicate half-row actions. Its caller-owned scroll offset remains the single owner while tokenised overlay scrollbar chrome auto-hides until the default 15 px grab band is pointed at or dragged. `Length::Fill` tables derive realization and scrollbar geometry from their actual allocation; for fixed-height rows the effective body is the largest whole-row band that fits, leaving any fractional remainder quiet while the outer allocation and scrollbar track stay unchanged. [`TableViewportState`] exposes that coherent effective viewport read-only for update-time scroll clamping or keyboard reveal, so endpoints reach the final complete row and apps do not subtract parent chrome. Fixed-size tables remain deterministic. `virtualized table` is included so the American `-z` spelling lands on the table row instead of returning zero hits. **Responsive column priority and the row-detail fold** are [`EnzTable::column_fold`](crate::table::EnzTable::column_fold): declare the order columns yield in and how a cell projects to text, and the table measures its own allocation, drops the least important tracks, and republishes every dropped value in a per-row detail region where it keeps its accessible name, its Tab stop and (with [`ColumnFold::on_activate`](crate::table::ColumnFold::on_activate)) an activation. Opt-in: a table that declares nothing behaves exactly as before. Do **not** hand-roll a per-app breakpoint that hides columns - that is app-level column policy: e.g. four columns of process data with nowhere to reach them. `expandable row` here is a **column** fold; for a hierarchy of rows that expand into children, that is `tree_view` (its own row below). **Resizable columns** are [`EnzTable::on_column_resize`](crate::table::EnzTable::on_column_resize): a drag strip with the resize cursor on each header's trailing edge, double-click to fit the header and realised cells, <kbd>Alt+Shift+Left</kbd>/<kbd>Right</kbd> on the active column, and a [`ColumnResize`](crate::table::ColumnResize) event the app writes into its column widths and stores. |
//! | `sortable header`, `column header`, `table header` | [`table_header_cell`](mod@table_header_cell) - a sortable column-header cell. |
//! | `table cell`, `text cell`, `data cell` | [`table_text_cell`](mod@table_text_cell) - a single-line text cell for a data table. |
//! | `virtual list`, `virtualised list`, `virtualized list`, `virtualisation`, `virtualization`, `infinite scroll`, `windowed list`, `large list`, `owned provider`, `owned row provider`, `view-local rows`, `rows built in view`, `Rc provider`, `retained geometry`, `retained variable heights`, `reuse prefix sums`, `filtered list rows`, `projected list rows`, `page up`, `keyboard scrolling`, `home end`, `virtual list keyboard`, `viewport bounds`, `measured viewport`, `track layout bounds`, `last row reveal` | [`virtual_list`](mod@virtual_list) - realises visible rows and retains keyed row state across insertion, removal and reordering. PageUp/PageDown/Home/End use viewport distances and preserve focus, with edge handoff matching [`scroll_view`](mod@scroll_view). `enz_virtual_list_with_geometry` accepts borrowed or `Rc` sequence geometry so unchanged models avoid full height enumeration and prefix reconstruction. [`ProviderHandle`](crate::virtual_list::ProviderHandle) accepts a borrowed provider or an owned `Rc` for view-local rows; retain the provider in application state when possible. Wrap the allocated viewport in crate-root [`track_bounds`] with [`LayoutBounds`], then pass its measured height to [`virtual_list::reveal_offset`]. The cell updates after layout without emitting messages. |
//! | `scroll to item`, `scroll to row`, `reveal a row`, `jump to item`, `scroll into view`, `jump to start`, `scroll to index`, `restore scroll after insert`, `keep scroll position after prepend`, `anchor scroll to content`, `measured row heights`, `correct estimated row heights` | [`virtual_scroll`](mod@virtual_scroll) - the scroll-offset policy every virtual surface needs, in one place: [`reveal_offset_at`](crate::virtual_scroll::reveal_offset_at) (`Start`/`Center`/`End`/minimum-movement `Nearest`, clamped to the extent) and [`reconcile_measurements`](crate::virtual_scroll::reconcile_measurements) (apply measured heights without the reader's row jumping). Reach them through [`virtual_list`](mod@virtual_list) (`reveal_offset`, `capture_anchor`, `resolve_anchor`) or [`chat_list`](mod@chat_list) (`reveal_offset`, `ChatAnchor`), which add the key→index half. |
//! | `virtual grid`, `photo grid`, `thumbnail grid`, `icon grid virtualised`, `file grid`, `virtual grid keyboard`, `grid page up`, `reflow anchor`, `keyed anchor`, `column change scroll`, `keep item visible` | [`virtual_grid`](mod@virtual_grid) - a model-backed virtual spatial grid for photo, icon and file-thumbnail collections. It owns the same PageUp/PageDown/Home/End policy as [`scroll_view`](mod@scroll_view), paging vertically by rows so a four-column grid advances four times as many items as a one-column list. When a responsive grid changes column count, call `.reflow_anchor(anchor_cell, previous_columns_cell)`: stable model keys compensate the scroll offset so the same item remains visible, while the app owns and retains both cells. |
//! | `single instance`, `one instance`, `second launch`, `open in running app`, `already running`, `forward files to running instance`, `DBusActivatable`, `raise existing window`, `unique application`, `explicit launch intent`, `profile launch` | [`single_instance`](mod@single_instance) - `SingleInstance::start(app_id)` in `main`: `Owner` runs with its launch intent, `Forwarded` exits; the owner's `subscription(..)` and `drain(window)` hand over later launches and raise the window. `start_with_intent(app_id, &intent)` preserves desktop activation tokens for application-parsed profile/window launches. Feature `application-dbus`. |
//! | `file browser`, `file list`, `file manager`, `directory listing`, `file picker pane`, `folder browser`, `browse files`, `file explorer`, `right click a file`, `file context menu` | [`file_browser`](mod@file_browser) - a file listing over a host-owned model, with real per-type icons and natural sort as defaults. For the **system** dialog see [`file_chooser`](mod@file_chooser). |
//! | `drag files out`, `file source`, `drag photos`, `drag attachment`, `export drag`, `file thumbnail drag`, `drag data out`, `custom MIME source`, `native row reorder` | [`file_drag`](mod@file_drag) - `enz_file_drag_source(content, paths)` starts a native Copy drag at the desktop threshold, keeps ordinary clicks and waits for destination completion. Add app metadata with `.with_data`; `enz_data_drag_source(content, owner_key, payload)` offers ordinary MIME data. Key generated sources to their asset/session. File managers may opt into Move; the widget never deletes source paths. |
//! | `drop files`, `file drop`, `drop target`, `drag files in`, `drag from file manager`, `drop files onto window`, `dropped paths`, `native file drop`, `FileDropped`, `accept dropped files`, `drop receipt`, `finish drop after copy`, `refuse a drop`, `stale drop`, `drop generation`, `drop target replaced` | [`file_drop`](mod@file_drop) - `enz_file_drop_target(content, on_drop)` turns files dropped on a region into one message with their paths; no runtime to connect, and text drag and drop keeps working. The innermost target under the drop point takes the files. `enz_file_drop_target_with_receipt` hands over a `FileDrop` instead, so the source hears success only when the app calls `finish()` after staging the files. |
//! | `file metadata cell`, `Git status column`, `file browser header corners` | [`file_browser`](mod@file_browser) - insert already-loaded cells with [`file_browser::FileMetadataColumn`], position them before a built-in column and round only the header with [`file_browser::EnzFileBrowser::header_radius`]. |
//! | `large folder`, `virtualized file list`, `thousands of files` | [`file_browser`](mod@file_browser) - lend a [`scroll_view::ScrollTranslation`] through [`file_browser::EnzFileBrowser::scroll`] to build only the rows or tiles near the viewport. |
//! | `trash`, `move to trash`, `empty trash`, `permanent delete`, `restore from trash`, `xdg-mime default`, `default application`, `mime association`, `open with`, `network mount`, `attach network location`, `network share`, `mount smb`, `recent files`, `recently used`, `recently-used`, `xbel` | Native-platform capabilities are indexed in the core capability guide. Default-application lookup and updates use `enzui_services::mime_apps::MimeApps`. File selection uses [`enzui_core::portal`]. Complete Trash transactions, network-location attachment and recently-used history are not supplied by the framework; blocking host integrations use an application-owned [`enzui_core::blocking_task::BlockingTaskOwner`]. |
//! | `file type icon`, `mime icon`, `icon for a file`, `file extension icon`, `document icon`, `themed file icon`, `icon theme`, `desktop icon theme`, `mimetype icon`, `folder icon`, `places icon`, `device icon`, `drive icon`, `attachment icon` | [`file_icon`](mod@file_icon) - [`enz_file_icon`] draws a file's icon from the desktop icon theme: a MIME type, file name or directory and a size in, the theme's `image-png` / `x-office-document` / `folder` artwork out (MIME name, then shared-mime-info's generic icon, the family `*-x-generic`, then `application-x-generic`), cached per type and size. Falls back to the bundled Lucide glyph (`enzui_icons::file_type::icon_for_mime`) when the theme has none, and `.themed(false)` always draws the glyph for a monochrome list. [`themed_icon`](file_icon::themed_icon) looks up places and devices by theme name (`folder-documents`, `drive-harddisk`). [`download_list`](mod@download_list) uses it for you. |
//! | `byte size`, `file size`, `humanise bytes`, `human readable size`, `format bytes`, `KB MB GB`, `bytes to string`, `size string`, `df -h`, `storage total`, `download size` | not a widget: [`format_bytes`] (re-exported at the crate root as `enzui_widgets::format_bytes`) - one `u64` → a **decimal / SI** size string (`842 B`, `1.50 MB`, `4.20 GB`), matching `df -h` and macOS Finder. Use the shared SI formatter; do not re-derive a `1024`/`1000` ladder per app. [`file_browser`](mod@file_browser) uses it for the Size column. |
//! | `tree`, `outline`, `file tree`, `directory tree`, `folder tree`, `folder tree in sidebar`, `hierarchy`, `OutlineGroup`, `nested list`, `expandable rows`, `hierarchical rows`, `roving sidebar`, `custom sidebar row`, `two line sidebar row`, `title subtitle row`, `sidebar row with avatar`, `sidebar row with status dot`, `sidebar row with badge`, `custom row content`, `no card border`, `transparent panel`, `flat tree`, `embedded tree`, `sidebar tree without card`, `tree without panel fill`, `hover reveal`, `reveal on hover`, `row hover action`, `trailing on hover`, `hover accessory`, `keep sidebar scroll`, `restore tree scroll on reopen`, `scroll survives collapse`, `sidebar scrolled up on reopen`, `tree scroll offset persists across collapse`, `stable tree scrollable id`, `seed tree scroll offset` | [`tree_view`](mod@tree_view) - a keyboard-navigable, scrolling tree of pre-flattened **rows** at arbitrary depth (SwiftUI `OutlineGroup`). This is the hierarchical-*rows* widget; for a sidebar that is a stack of named *sections* each folding as one unit, reach for `expander` (its own row above) with `.hover_scope`/`.header_action` instead. **Also the custom-sidebar-row widget**: `.row_content(..)` hands the row *body* to the app (a title/subtitle stack, an avatar, a status dot, an unread pill) while the widget keeps the one Tab stop, the roving cursor, virtualization and the `TreeItem` semantics. Use `.row_trailing(..).row_trailing_visibility(TrailingVisibility::OnHoverOrFocus)` for a quiet per-row action: the tree's roving keyboard focus reveals the current row while the action stays mounted and independently focusable. Reach for it instead of a `column!` of [`list_row_surface`](mod@list_row_surface) rows, which is N Tab stops with no arrow keys - the cliff this control avoids. **Scroll survives collapse:** a sidebar the shell drops on collapse structurally unmounts the tree, so its internal scrollable state is thrown away. Adopt [`EnzTreeView::scroll_id`](crate::tree_view::EnzTreeView::scroll_id) for a caller-stable identity that `scroll_to` operations survive across reopen, and pair with [`EnzTreeView::scroll_offset`](crate::tree_view::EnzTreeView::scroll_offset) fed back from [`EnzTreeView::on_scroll`](crate::tree_view::EnzTreeView::on_scroll) so the reopened tree lands at the captured offset instead of at zero. Applies to the eager path as well as the virtualized one - the seed is consumed once per fresh scrollable state, so passing it on every view is safe. |
//! | `tree chevron`, `expand tree row`, `tree toggle`, `tree disclosure` | [`tree_disclosure`](mod@tree_disclosure) - an expand/collapse chevron for tree rows. |
//! | `list row`, `selectable row`, `list item surface`, `clickable row`, `row with button`, `row actions`, `accessory`, `custom row`, `hover action`, `hover reveal`, `reveal on hover`, `row hover action`, `trailing on hover`, `hover accessory`, `selected row action`, `keep row action visible when selected`, `action on the active row`, `quiet row commands`, `row with trailing action`, `row with custom content`, `sidebar row with menu`, `right click a row`, `row context menu`, `secondary click`, `right click a list item`, `centered list row`, `centered row action`, `centre icon and label`, `add row centered`, `row content alignment`, `row align_x`, `row content is centered`, `row text not left aligned`, `search results are centered`, `shrink content in a full width row`, `list gutter`, `collection gutter`, `row inset`, `list padding`, `outer inset`, `row hover paint reaches pane edge`, `breathing room around list row`, `list outer gutter`, `open focused row`, `Enter opens focused entry`, `Enter versus Space`, `distinct open action`, `AT default action opens`, `screen reader open row`, `on_enter row`, `keyboard open action`, `row Enter open Space select` | [`list_row_surface`](mod@list_row_surface) - a focusable, keyboard-activatable row surface with hover / pressed / selected / disabled states. Its `.trailing(..)` slot supports [`TrailingVisibility::OnHoverOrFocus`](crate::list_row_surface::TrailingVisibility): the action remains mounted, width-reserved, focusable, accessible and independently clickable while only its resting paint is hidden; live row hover or keyboard focus on the row/action reveals it, while [`TrailingVisibility::OnHoverFocusOrSelected`](crate::list_row_surface::TrailingVisibility) adds the row's *model selection* to that reveal set - the shape a Trash/queue row needs when its restore and delete commands must stay visible on the item the user just picked, instead of vanishing the moment the pointer moves to press one. Its `on_context` is the framework-owned **secondary-press** hook: it reports the window-space cursor point, publishes a routable `ShowContextMenu` AT node, and hugs the row so one row cannot swallow the right-clicks below it - hand-wrapping [`context_target`](mod@context_target) around a finished row is the shape it absorbs. Pair the reported point with [`context_menu_host`](mod@context_menu_host)'s point entry. **Content spacing:** rows default to a 12px horizontal content inset and vertical centering. Content that already owns its inset uses [`.content_padding(0.0)`](crate::list_row_surface::EnzListRowSurface::content_padding); [`.content_align_y(Alignment::Start)`](crate::list_row_surface::EnzListRowSurface::content_align_y) opts out of centering. Both options are available on [`ListRowSurfaceTokens`]. Horizontal alignment defaults to the leading edge of the content band; [`.content_align_x(Alignment::Center)`](crate::list_row_surface::EnzListRowSurface::content_align_x) centers a composed icon and label as one group. **Outer-edge gutter:** a symmetric horizontal inset (`4.0` px, `spacing.xs`) is applied by default via a `container.padding` wrap around the composed `focus_button`, so the row's hover / pressed / selected pill and its focus ring stop short of the pane edge AND the child element is laid out at `outer - 2 * gutter`. This is the shared answer to the shape an app hit: without it every app invents its own container-level padding, and every one has to remember to also rebase width-dependent decisions (elision, responsive collapse) on the reduced width - a gutter the collection's own measurement does not know about is the vacuity this widget's default forbids. Opt out with [`EnzListRowSurface::outer_gutter(0.0)`](crate::list_row_surface::EnzListRowSurface::outer_gutter) for a full-bleed row inside a container that already pads; read the live value via [`ListRowSurfaceTokens::outer_gutter_of`](crate::list_row_surface::ListRowSurfaceTokens::outer_gutter_of) for width-dependent layout math that must consume exactly what the row will apply. **Enter vs Space:** `.on_enter(..)` splits the keyboard/AT primary off the pointer primary - pointer click and Space still fire `on_activate` ("select"), while Enter and a screen reader's default action publish the `on_enter` message ("open"), so the row opens the entry the focus ring is actually on instead of a global key subscription reconstructing "which row is focused" (a shape this API rejects). It is an override of an enabled row: a row built with no activation stays disabled and `on_enter` is inert there, and an `embedded` row leaves Enter/Space to the owning roving-focus composite. Grid parity lives on [`pressable`](mod@pressable)`::on_enter`. |
//! | `mail row`, `message row`, `excerpt row`, `title excerpt footer row`, `preview row` | [`preview_row`](mod@preview_row) - a title + trailing + excerpt + footer list row. |
//! | `drag to reorder`, `sortable list`, `reorderable list`, `move rows`, `reorder list`, `reorderable sidebar`, `drag a clickable row`, `reorder rows that are buttons`, `drag interactive rows`, `reorder pinned items`, `drag versus click`, `reorder grid`, `sortable grid`, `drag to reorder tiles`, `page sorter`, `wrapped reorder`, `drag handle`, `reorder grip`, `reorder groups`, `drag a group by its header`, `composite reorder row` | [`reorder_list`](mod@reorder_list) - a keyed list with drag-to-reorder. [`.grid(columns)`](crate::reorder_list::EnzReorderList::grid) wraps it into rows of equal columns: the gap opens cell by cell, dragged tiles follow the pointer freely, and arrow keys move along rows and between them. **Rows that are themselves clickable** (a sidebar place, a pinned bookmark) need [`.drag_through_children(true)`](crate::reorder_list::EnzReorderList::drag_through_children): a nested [`list_row_surface`](mod@list_row_surface) (or any other focusable button surface) captures the press, so by default the list never sees the gesture begin and the rail is un-reorderable. With it, the press is still the child's - a release inside the deadband activates the row exactly as before - but travel past the deadband starts the reorder session and the list consumes the terminal release, so one drag never both reorders and opens. **Composite rows** (a workspace group: a header plus nested clickable or draggable rows) use [`.drag_handles(true)`](crate::reorder_list::EnzReorderList::drag_handles) and wrap the header in [`reorder_handle`]: only the handle drags the whole row, and everything else in it keeps its own gestures. |
//! | `downloads`, `download manager`, `downloads panel`, `download queue`, `transfers list`, `download list`, `virtualized download history`, `windowed download history`, `download row provider`, `download row`, `show in folder`, `aligned progress column` | [`download_list`](mod@download_list) - the view over the core download model. Every row shares one grid: an optional file-type icon, the name over one muted status line (`730 MB of 2.00 GB · Paused`; only `Failed · <cause>` is coloured), a progress bar at the same x and width in every row (accent while active, quiet while paused or queued, none once finished), and quiet toolbar-icon actions with tooltips and accessible names in a fixed trailing column, including an optional host-routed Show in Folder (`EnzDownloadList::on_show_in_folder`). No phase chips, no text buttons. Use [`enz_download_list_provider`] for retained/high-cardinality history: it asks the provider for only the visible window and reconciles rows by stable [`DownloadId`](enzui_core::download::DownloadId). Keep [`enz_download_list`] for intentionally small eager lists. |
//! | `operation status`, `activity surface`, `active operations`, `background task`, `job status`, `what is running`, `transfer status`, `progress popover`, `sidebar status`, `sidebar footer status`, `floating status`, `copy progress`, `file operation progress`, `operation percentage`, `fractional completion`, `dimensionless progress`, `rate limit progress paint`, `coalesce transfer progress`, `compact activity status`, `downloading status row`, `sidebar progress row` | [`activity_surface`](mod@activity_surface) - persistent status over the core `operation_activity` model in two presentations. At the bottom of the shell's sidebar the default is **compact**: one sidebar-style row (an icon, `3 downloading`, the aggregate percentage) with a thin aggregate bar, which routes the host's `on_overflow` message (one provider's panel or all activity) when activated. Elsewhere, or with `ActivityPresentation::Detailed`, it is the panel: bounded attention-ordered rows with honest determinate/indeterminate progress, including dimensionless fractional completion rendered and announced as an operation percentage (finite values clamp to 0–100%, non-finite values fail closed, and homogeneous fractions aggregate with equal operation weight rather than pretending to be bytes or items); per-row pause/cancel/retry; one provider-level `OpenPanel` action instead of one per operation; a keyboard/AT-reachable `+N more` action whose [`ActivityOverflowTarget`](enzui_core::operation_activity::ActivityOverflowTarget) is derived from the actual hidden rows (one provider or the all-activity destination); and exactly one `Role::Status` region. Mixed units never receive a fabricated aggregate. Placed by `EnzAppShell::activity`, not by the app. Streaming producers must pass descriptors through [`ActivityPaintThrottle`](enzui_core::operation_activity::ActivityPaintThrottle) before emitting UI messages; that bounds whole-shell paints while retaining the latest sample. A [`toast`](mod@toast) is transient and a [`status_bar`](mod@status_bar) is the focused view's chrome; this is the union of live work. |
//! | `shortcuts list`, `keyboard shortcuts`, `hotkey list`, `keybindings list`, `shortcut help`, `shortcut dialog rows`, `help entry`, `shortcut catalog rows`, `alias badge`, `shortcut list` | [`shortcut_list`](mod@shortcut_list) - a sectioned list of keyboard-shortcut rows. Rendering a whole **shortcut catalog** (`enzui_core::ShortcutBinding` / [`shortcut_help`](enzui_core::shortcut_help)) is one call - [`shortcut_catalog_entries`] - because [`ShortcutHelpEntry`](enzui_core::ShortcutHelpEntry) implements [`ShortcutCatalogRow`] itself; do **not** write a per-app newtype, and do not re-decide how an alias is badged ([`shortcut_help_badge`] joins a row's distinct chords with `" or "` and drops a chord whose key was already shown, so `?`/`Shift+?` reads `?`). |
//! | `chat`, `message list`, `conversation transcript`, `virtual chat`, `chat list` | [`chat_list`](mod@chat_list) - a provider-backed, variable-height virtual chat transcript. |
//! | `chat scroll`, `bubbles surface`, `conversation view`, `chat transcript` | [`chat_transcript`](mod@chat_transcript) - a scrolling surface for chat bubbles. |
//! | `scroll to bottom`, `jump to latest`, `stick to bottom`, `follow tail`, `auto scroll to newest`, `pin to newest message`, `open at newest message`, `tail follow`, `log tail`, `unread count` | [`tail_follow`](mod@tail_follow) - the shared scroll-anchoring primitive behind both chat surfaces. It opens a transcript at the **newest** message, follows content appended after first layout, and **releases** the pin while the reader is scrolled up so an arriving message never yanks them back. Both [`chat_transcript`](mod@chat_transcript) and [`chat_list`](mod@chat_list) enable it by default; `.follow_tail(false)` opts out for an archival surface or a host that drives the offset itself. |
//! | `message bubble`, `chat bubble`, `speech bubble`, `message timestamp`, `timestamp under a message`, `message metadata`, `message footer`, `delivery state`, `sent receipt`, `read receipt`, `edited marker`, `retry a message`, `per message actions`, `token count on a message`, `model name on a message` | [`chat_bubble`](mod@chat_bubble) - a rounded chat message bubble. It owns **both** metadata lanes: the author caption above ([`.author(..)`](crate::chat_bubble::EnzChatBubble::author)) and the footnote below ([`.timestamp(..)`](crate::chat_bubble::EnzChatBubble::timestamp) for a string, [`.footnote(Element)`](crate::chat_bubble::EnzChatBubble::footnote) for delivery state / an "edited" marker / a retry button / a token count). Do **not** compose either as a sibling: the bubble's horizontal edge is a function of [`ChatRole`], so `column![bubble, stamp]` takes the column's default `Start` and puts a right-aligned user bubble's own stamp on the far side of the row. Through [`chat_list`](mod@chat_list) it is `ChatMessage::with_timestamp`, because a virtual list realises its own rows and leaves the host no sibling to wrap at all. |
//! | `composer`, `chat input`, `message input`, `prompt input`, `chat composer`, `send button`, `send stop button`, `auto grow textarea`, `growing input`, `mic in input`, `chat text box`, `ctrl enter to send`, `send on ctrl+enter`, `enter inserts newline`, `quick reply` | [`composer`](mod@composer) - the chat / prompt **input**: the bottom half of a chat UI the output family had no counterpart for. Owns the geometry: auto-grow with a row cap, Enter/Shift+Enter, a reserved in-field trailing slot, the action rail beneath, and a send button that swaps to stop on a busy flag. **Read its `# Composing a composer` section first** - a composer is a stack of parts, not one widget, and `2 * TextAreaTokens::padding` under-reserves the input height. The model / effort dropdowns and the mid-turn status strip are app vocabulary you pass in as `Element`s. Enter sends by default; [`EnzComposer::submit_key`](composer::EnzComposer::submit_key) with [`SubmitKey::CommandEnter`](text_area::SubmitKey::CommandEnter) sends on Ctrl/Cmd+Enter and keeps Enter for newlines, as a mail quick reply wants. |
//! | `monotone curve`, `smooth curve without overshoot`, `curved line chart`, `scrolling time window`, `live chart`, `real-time history`, `continuously scrolling chart`, `area chart`, `filled line chart`, `area graph`, `transparent chart`, `chart without background`, `hide vertical axis`, `horizontal grid only`, `smooth chart`, `animated chart`, `interpolate chart`, `tween chart`, `easing chart`, `gradient area chart`, `smooth spline`, `rounded peaks`, `area chart axis unit`, `area y axis label`, `area tick formatter`, `gutter`, `axis gutter`, `y gutter`, `axis label`, `y axis label`, `y axis unit`, `axis unit label`, `tick label width`, `clipped tick label`, `wide axis label` | [`area_chart`](mod@area_chart) - a Canvas-based Cartesian area chart host whose areas fade from the edge toward the baseline. Charts sit on the host surface with horizontal grid lines only; [`.plot_background(true)`](crate::area_chart::EnzAreaChart::plot_background) paints a plot panel and [`.y_axis_line(true)`](crate::area_chart::EnzAreaChart::y_axis_line) adds the left vertical rule. **Name the y unit once** with [`.y_axis_label("Kbps")`](crate::area_chart::EnzAreaChart::y_axis_label) rather than appending it in the tick formatter: repeating the unit on every tick is what overflows the 40px [`Y_GUTTER`](crate::chart_geometry::Y_GUTTER) and gets a label clipped at the canvas origin, and only the axis label is announced to a screen reader. [`.y_gutter(56.0)`](crate::area_chart::EnzAreaChart::y_gutter) is the escape hatch for labels still wide after that; it is clamped so a caller cannot reserve the plot away. [`.curve(ChartCurve::Monotone)`](crate::area_chart::EnzAreaChart::curve) draws a monotone cubic through every sample that never overshoots a segment, and `ChartCurve::Smooth` a smooth curve near the samples that rounds a lone spike and never leaves the data's range (both presentation only; the data table and summary keep the samples). [`.live(window, sampled_at)`](crate::area_chart::EnzAreaChart::live) scrolls a fixed time window continuously, with points as ages in seconds (newest at `0`): the window ends one sample interval ago so each new sample slides in from the right edge, the render lane carries the slide between samples (the UI thread draws once per sample), and motion stops under reduced motion, off-screen and once data is stale, with no app timer or message. |
//! | `cartesian chart`, `continuous chart`, `mixed line area point chart`, `chart layers`, `chart mark policy` | [`cartesian_chart`](mod@cartesian_chart) - the shared continuous-Cartesian host for line, area and point marks. Axes, domains, decimation, cache identity, semantics, data tables and optional interactions live on one [`EnzCartesianChart`]; choose a [`CartesianMark`](crate::cartesian_chart::CartesianMark) instead of copying chart chrome. Existing line/area/scatter builders remain compatibility presets. |
//! | `bar chart`, `column chart`, `bar graph`, `bar chart axis unit`, `bar y axis label`, `bar tick formatter`, `bar axis gutter`, `bar y gutter`, `bar axis label` | [`bar_chart`](mod@bar_chart) - a Canvas-based vertical bar chart host. Name the value-axis unit once with [`.y_axis_label(..)`](crate::bar_chart::EnzBarChart::y_axis_label), and widen the 40px label gutter with [`.y_gutter(..)`](crate::bar_chart::EnzBarChart::y_gutter). **Not the answer for a rating / review histogram**: a rating histogram is one horizontal bar per bucket with a `"N star"` label, and the right shape is a `column!` of [`progress_bar`](mod@progress_bar) - see the `rating histogram` row for the reasoning. |
//! | `line chart`, `line graph`, `trend line`, `time series chart`, `axis unit`, `y axis label`, `tick formatter`, `line tick formatter`, `gutter`, `axis gutter`, `y gutter`, `axis label`, `y axis unit`, `tick label width` | [`line_chart`](mod@line_chart) - a Canvas-based Cartesian line chart host; its y tick formatter receives both value and step. Prefer [`.y_axis_label(..)`](crate::line_chart::EnzLineChart::y_axis_label) - the unit named once on the axis, spoken to assistive tech - over appending it to every tick; [`.y_gutter(..)`](crate::line_chart::EnzLineChart::y_gutter) widens the 40px label gutter when the labels are genuinely wide. |
//! | `pie chart`, `donut chart`, `ring chart` | [`pie_chart`](mod@pie_chart) - an accessible, bounded pie / donut chart. |
//! | `sparkline`, `mini graph`, `mini chart`, `micro chart`, `inline chart`, `tiny chart`, `trend`, `trend line mini`, `history strip`, `mini history`, `smooth sparkline`, `area sparkline`, `animated sparkline`, `interpolate sparkline` | [`sparkline`](mod@sparkline) - a frameless, axis-less mini trend chart: bottom-anchored full-bleed bars, or with `.style(SparkStyle::Area)` a smooth line over a fading fill, with **no** frame and **no** axis gutter, the compact counterpart to the seven axis-bearing [`bar_chart`](mod@bar_chart)/[`line_chart`](mod@line_chart) Cartesian hosts. Its newest live sample eases in without running one sample behind. It publishes a `Role::Image` node carrying the current value + trend ("CPU, 34%, rising"), announces `no data` for an empty series rather than a misleading zero, and downsamples a series longer than the pixel width into peak-preserving buckets. An app was hand-rolling five per frame out of bare `container` bars, publishing no accessibility node and reading as a 9%-ink broken thumbnail. |
//! | `scatter plot`, `scatter chart`, `xy plot`, `point cloud chart`, `scatter axis unit`, `scatter y axis label`, `scatter tick formatter`, `scatter axis gutter`, `scatter y gutter` | [`scatter_chart`](mod@scatter_chart) - a Canvas-based Cartesian scatter chart host. Its y unit belongs on [`.y_axis_label(..)`](crate::scatter_chart::EnzScatterChart::y_axis_label), not on every tick; [`.y_gutter(..)`](crate::scatter_chart::EnzScatterChart::y_gutter) widens the 40px gutter. |
//! | `legend`, `chart key`, `series legend`, `chart legend` | [`chart_legend`](mod@chart_legend) - a chart legend - a row of coloured swatch + label pairs. |
//! | `chart tooltip`, `data point tooltip`, `hover value` | [`chart_tooltip`](mod@chart_tooltip) - a floating tooltip for chart data points. |
//! | `animated image`, `gif`, `animated gif`, `apng`, `animated webp`, `play an animation`, `loading animation`, `sticker` | [`animated_image`](mod@animated_image) - [`AnimatedFrames::decode`](animated_image::AnimatedFrames::decode) reads a GIF, APNG or animated WebP into frames and delays once (off the UI thread for large files); `enz_animated_image(&frames)` plays them on their delays with the [`image`](mod@image) fit modes. It schedules a wake only for the next frame, pauses when scrolled out of view, when the window stops drawing or with `.playing(false)`, and under reduced motion shows the first frame and never wakes. Alt text publishes one `Image` node. |
//! | `animated svg`, `svg animation`, `smil`, `animate`, `animateTransform`, `animateMotion`, `css keyframes svg`, `lottie-like icon`, `animated icon`, `svg spinner`, `loading spinner svg`, `svgz` | [`animated_svg`](mod@animated_svg) - [`AnimatedSvg::parse`](animated_svg::AnimatedSvg::parse) reads an SVG (or `.svgz`) once; `enz_animated_svg(&svg)` plays its SMIL (`animate`, `set`, `animateTransform`, `animateMotion`; syncbase, repeat and pointer-event timing) and CSS animations (`@keyframes`, every timing function, `transform-origin`/`transform-box`). A frame is rasterised only when the picture changes, step changes wake the app only at the step, finished or hidden pictures ask for no frames, and a loop's frames are cached after the first pass. `.playing(false)` pauses; reduced motion shows the first frame. A label publishes one `Image` node. |
//! | `image`, `picture`, `photo`, `img`, `thumbnail aspect ratio`, `round the picture not the box`, `no letterbox`, `preview at its own shape` | [`image_widget`](mod@image) - a raster image with fit, radius, and placeholder background. For a thumbnail among others of different shapes, [`.hug_picture()`](crate::image_widget::EnzImage::hug_picture) treats the slot as a maximum: the picture keeps its shape, centred, with its own corners rounded and no letterbox backdrop. For async loading use [`async_image`](mod@async_image). |
//! | `remote image`, `lazy image`, `network image`, `loading image`, `async image` | [`async_image`](mod@async_image) - renders the lifecycle of an async image request - loading skeleton, decoded image, or failure stamp. |
//! | `zoomable image`, `pan zoom image`, `image inspector`, `image viewer` | [`image_viewer`](mod@image_viewer) - an inspectable image region with a controlled transform. |
//! | `image cropper`, `avatar cropper`, `profile picture crop`, `crop handles`, `fixed aspect crop` | [`image_cropper`](mod@image_cropper) - a controlled source-pixel crop with resize handles over shared zoom/pan. |
//! | `video player`, `movie`, `video surface`, `player`, `streaming frames`, `video frame flicker`, `black flash between frames`, `image allocate`, `frame presenter`, `bounded frame uploads`, `latest-frame upload`, `show decoded frames` | [`video`](mod@video) - a video surface plus transport, driven by a media snapshot and a decoded frame. [`FramePresenter`](video::FramePresenter) retains the shown frame until a newer allocation completes; failed allocations use renderer upload-on-draw. [`LatestFramePresenter`](video::LatestFramePresenter) admits one allocation and retains only the newest pending frame. Clear on source replacement and run the typed follow-up task returned by completion. |
//! | `transport controls`, `play pause bar`, `playback controls`, `player controls`, `shuffle`, `next track`, `previous track`, `repeat mode`, `playlist transport`, `elapsed total time`, `hide time labels`, `live badge`, `media controls` | [`media_controls`](mod@media_controls) - a transport-control bar driven by a media snapshot; reach for this instead of rebuilding play/pause. Opt into playlist transport (shuffle, prev, next, repeat) via `.playlist_controls(..)`, and choose the time readout (elapsed/total, `● LIVE`, or none) with `.time_display(..)`. |
//! | `profile picture`, `user icon`, `gravatar`, `AdwAvatar`, `initials circle` | [`avatar`](mod@avatar) - a styled avatar/user-icon component. |
//! | `icon`, `symbol`, `glyph`, `lucide`, `app icon`, `custom icon`, `brand mark`, `vendor mark`, `provider logo`, `favicon`, `workspace favicon`, `file type icon svg`, `png icon`, `ico icon`, `raster icon`, `svg icon handle`, `icon from handle`, `icon from image`, `app-owned icon`, `icon source`, `IconSource`, `symbolic svg`, `monochrome logo`, `tinted svg`, `logo follows text colour`, `SymbolicSvg` | [`icon`](mod@icon) - the bundled Lucide icon widget. **App-owned images** (a workspace favicon, a provider brand mark, an .ico/.png file-type glyph) plug into the same slot via [`IconSource`]: a `String`/`&str` picks a bundled Lucide name, an `svg::Handle` plugs an app-owned SVG, an `image::Handle` plugs a raster (`.ico`/`.png`). Bundled names tint to follow the parent, app-owned handles render untinted so a brand mark keeps its own colours; `IconSource::SymbolicSvg(handle)` opts a monochrome app glyph into the parent's tint in every slot. Use `.icon_source(..)` on [`sidebar_row`](mod@sidebar_row), [`expander`](mod@expander) and [`menu`](mod@menu) items, or [`enz_icon_from_handle`] / [`enz_icon_from_image`] directly. |
//! | `qr code`, `qr`, `barcode qr` | [`enz_qr`](mod@qr) - a themed QR code widget. |
//! | `annotate`, `draw on image`, `markup overlay`, `drawing surface`, `annotation overlay` | [`annotation_overlay`](mod@annotation_overlay) - a reusable annotation surface over a picture - draw, preview, commit, hit-test. |
//! | `selectable text`, `text selection`, `select text`, `copy text`, `drag to select`, `read only selection`, `selection scope`, `cross widget selection`, `selection change`, `selected text`, `contextual action on selection`, `selection bounds`, `annotate a selection`, `selection to context action`, `on selection change`, `selection starts in blank line`, `text cursor over link`, `right click selected text`, `preserve selection on right click`, `selection context menu`, `selection context invocation`, `selection pointer anchor`, `keyboard selection menu`, `viewport selection anchor`, `stable document selection`, `keyboard text selection`, `shift arrow selection`, `focusable document`, `selection focus owner`, `bidi selection`, `selection geometry performance`, `duplicate selection regions`, `virtual list selection`, `selection across unmounted rows`, `copy unmounted rows`, `SelectionDocument`, `rich virtual rows`, `RenderedRows`, `render-derived selection document` | [`selection_scope`](mod@selection_scope) - focusable cross-widget read-only text selection with pointer and shaped keyboard range navigation; only the focused scope owns copy/select/context shortcuts. It wraps content so drag-selecting across multiple text children produces one continuous selection and one clipboard payload. As one read-only document owner, pointer selection focuses it only after descendants decline the press, Tab/Shift+Tab and `.id(..)` participate in normal traversal/restoration, window blur releases keyboard ownership, and Ctrl+A/C/Escape/Menu affect only the focused scope. Shift+Arrow extends over shaped grapheme carets (including combining and emoji clusters), Ctrl/Alt+Shift+Arrow uses Unicode word boundaries, Up/Down follows visual lines, and Home/End selects to a visual line edge (Ctrl selects to a document edge) while retaining bidi/wrap caret affinity. A fresh drag starts only inside a real text region (blank paragraph gaps cannot synthesize a first/last-line selection), while an active drag may cross gaps; double-click selects the Unicode word/whitespace/punctuation run under the pointer, and button, checkbox, switch and radio captions and actionable list-row contents remain accessible but are excluded from selectable document text; a table cell or other container named by the text it shows contributes that text once, while inline links remain selectable. Interactive descendants keep their pointer cursor instead of being misreported as selectable prose. `on_selection_change` publishes the selected text, its document-space range (paragraph + char offset), and the on-screen bounding rectangle for anchoring a contextual action. `on_context` preserves its selection-only callback contract; [`on_context_invocation`](selection_scope::EnzSelectionScope::on_context_invocation) additionally reports a [`SelectionContextInvocation`] with the exact viewport pointer point for a secondary press or translated selection bounds for Menu-key and routed AT requests. Both capture before an outer context target can duplicate the request, and `context_id` enables AT routing; `document_key` preserves and reindexes the range across unrelated descendant churn only when both endpoint texts are unique in the old and new region sets; a missing or ambiguous repeated endpoint clears the selection. Shaped paint geometry is assigned only after filtering and reading-order sorting, and a live highlight recollects rectangles during layout, so excluded controls, same-line runs, and width-only reflow cannot shift or stale the painted range. Active geometry is joined by collection index instead of repeated bounds/text scans, preserving duplicate text and identical bounds in linear work; unchanged source strings reuse their prior allocation. Over a virtual list, `.document(..)` keeps a selection whose endpoint rows unmount and copies rows never mounted: rows carry `enz_selection_row` keys, and `RenderedRows::new` builds the document from the rows' own widgets (Markdown, code, tables, quotes) so unmounted rows copy exactly as mounted ones. In a `view` function, `.rendered_rows(keys, row)` builds it with the list's own row builder and the scope keeps it. |
//! | `glyph selection`, `select text in a PDF`, `select text on an image`, `page text selection`, `pdf text selection`, `select words on a page`, `ocr selection`, `double click word on page`, `rotated page selection`, `select text on a rotated PDF` | [`glyph_selection`](mod@glyph_selection) - text selection over content that is a picture (a PDF raster with a text layer): press-drag from caret to caret in reading order, double-click a word, triple-click a line, click to clear; paints one highlight per line in the document selection fill. A page drawn turned by a quarter turn keeps its upright glyph layer through [`EnzGlyphSelection::rotation`](glyph_selection::EnzGlyphSelection::rotation). Not [`marquee_area`](mod@marquee_area), which selects whole items under a band. |
//! | `rubber band select`, `rubber band`, `marquee selection`, `drag select`, `box select`, `region select`, `selection rectangle`, `crop region`, `zoom to rect`, `band rectangle`, `marquee area` | [`marquee_area`](mod@marquee_area) - a rubber-band drag selection over positioned content, reporting both the covered items and the drawn band rectangle (widget-local coords). |
//! | `stacked dialogs`, `dialog over dialog`, `nested dialogs`, `dialog stack`, `several dialogs open`, `dialog hand off`, `replace dialog with another`, `which dialog is on top`, `modal focus set`, `close dialog from a button`, `dialog cancel returns focus`, `busy dialog ignores escape` | [`modal_host`](mod@modal_host) - [`DialogStack<Key>`](modal_host::DialogStack): open dialogs by key with the control focus returns to; Escape closes the topmost, closing one closes those opened over it, `hand_off` swaps one for the next without returning focus in between; `dismiss(key, cause)` closes one from its own Save/Cancel/Close button and returns focus to its invoker; `set_policy` makes a busy dialog refuse Escape and the scrim; `view(page, dialog, on_event)` renders them, and `view_with` configures each host (placement, label). |
//! | `modal`, `dialog host`, `overlay host`, `present dialog`, `modal layer`, `modal host`, `ModalFocusLifecycle`, `scoped_to`, `dialog exit`, `modal animation`, `animate dialog close`, `retain dismissed dialog` | [`modal_host`](mod@modal_host) - the layer that owns modality and theme-driven exit motion: scrim, background inertness, Escape, dismissal routing, and visual retention until settlement. Because an iced `Element` may borrow app state, [`ModalPresentation`] retains the app's small presentation model rather than an unsound prior-frame element; the outgoing subtree becomes inert and leaves accessibility immediately, then leaves the widget tree when motion settles. Every dialog is presented through it. |
//! | `modal container`, `dialog surface`, `modal box`, `modal surface` | [`modal_surface`](mod@modal_surface) - a flat modal container surface. |
//! | `dialog`, `dialog body`, `dialog actions`, `dialog footer`, `pinned footer`, `pinned actions`, `action row`, `buttons scroll away`, `dialog buttons that scroll away` | [`capped_dialog_body`](mod@capped_dialog_body) - the opt-in layout for a hand-built [`modal_surface`](mod@modal_surface): fixed header/footer chrome around one scrolling body. It caps the whole stack and derives the body's room from the **measured** chrome, never a guessed footer height. |
//! | `confirmation`, `are you sure`, `yes no dialog`, `AlertDialog`, `confirm delete`, `confirm dialog` | [`confirm_dialog`](mod@confirm_dialog) - the canonical \"are you sure?\" preset over EnzModalHost (shadcn `AlertDialog`). |
//! | `save changes`, `save before closing`, `unsaved changes dialog`, `save discard cancel`, `three action dialog`, `save all discard`, `close with unsaved changes`, `discard changes prompt` | [`save_changes_dialog`](mod@save_changes_dialog) - the canonical three-action \"Save changes before closing?\" preset over EnzModalHost. Right-aligned standard-size Discard / Cancel / Save All in one fixed order: the safe Save All is trailing so a bare Enter fires it, the destructive Discard is leading so Enter never reaches it, and Escape / scrim are Cancel. |
//! | `about dialog`, `about window`, `about box`, `application info`, `credits`, `version dialog`, `license dialog`, `release notes`, `what's new`, `report an issue` | [`about_dialog`](mod@about_dialog) - the application's About dialog hosted over its view: header (icon, name, developer, version), Website and Report-an-Issue buttons reported as [`AboutDialogEvent::OpenUrl`](about_dialog::AboutDialogEvent::OpenUrl), and scrolling What's New, Credits and Legal sections; every label is localized. |
//! | `shortcuts window`, `keyboard shortcuts dialog`, `help shortcuts`, `shortcuts dialog`, `shortcuts page`, `keyboard shortcuts settings page` | [`shortcuts_dialog`](mod@shortcuts_dialog) - keyboard-shortcut help as either content-only (`into_content`) or a modal surface hosted over EnzModalHost. |
//! | `settings dialog`, `settings window`, `preferences window`, `settings screen`, `preferences dialog`, `settings tabs`, `settings sidebar`, `preferences sidebar`, `settings pages`, `dialog action bar`, `pinned dialog actions`, `reset button in settings`, `settings footer buttons`, `settings sheet`, `settings bottom sheet`, `adaptive settings`, `settings presentation`, `tiling settings`, `lazy settings page`, `settings page factory`, `hidden settings page rebuild`, `compact settings`, `settings title typography`, `small preferences dialog`, `settings page scroll`, `save button in settings`, `settings page owns scroll` | [`settings_dialog`](mod@settings_dialog) - Adaptive settings surface with measured Tabs, Sidebar, or Stacked navigation. Stacked keeps a compact rail above the selected pane unless the caller supplies drill state and a Back route. Supports pinned global actions, lazy page factories, content-fit single pages, and centred-dialog or bottom-sheet presentation. [`EnzSettingsDialog::compact`](settings_dialog::EnzSettingsDialog::compact) selects Small geometry with Title3/Semibold text; title size, weight and typography ramp are independently configurable. Native multi-page navigation keeps a stable height cap; fitting the selected body requires None navigation and caller-owned selection. The preset scrolls page bodies, so a body reports its natural height; Save and similar commands go in the pinned footer, and [`SettingsPage::owns_scroll`](settings_dialog::SettingsPage::owns_scroll) hands the pane to a body that scrolls itself (example `settings_dialog`). |
//! | `bottom sheet`, `drawer sheet`, `Sheet`, `detent sheet`, `resizable sheet`, `grabber`, `snap points`, `scroll to expand`, `swipe to resize` | [`sheet`](mod@sheet) - [`SheetPresenter`] is the batteries-included detent controller: signed two-way resizing, distinct runtime snap points, an operable grabber, top-edge scroll handoff, and theme/reduced-motion settling. Route each [`SheetPresenterEvent`] through [`SheetPresenter::update`](crate::sheet::SheetPresenter::update); its typed outcome reports detent changes or a drag dismissal for the host's existing modal path. [`enz_sheet`] remains the source-compatible static/low-level surface. Present either over [`EnzModalHost`] (shadcn `Sheet`). |
//! | `lightbox`, `full screen`, `full screen modal`, `takeover`, `dimmed overlay`, `fullscreen dialog` | [`full_screen_overlay`](mod@full_screen_overlay) - a dimming scrim + centred content + close button, hosted over EnzModalHost. It stays inside its existing window; for native output-filling fullscreen use [`ShellWindowState::set_fullscreen`](crate::app_shell::ShellWindowState::set_fullscreen). |
//! | `menu item`, `dropdown item`, `menu entry`, `menu item with app icon`, `menu item custom icon`, `menu item brand mark`, `menu item favicon`, `menu row icon source`, `menu row` | [`menu_row`](mod@menu_row) - a single row in a menu / dropdown popup. An **app-owned** leading icon (a provider brand mark on a "Sign in with …" row, a recent-workspace favicon) plugs into the same slot via [`.icon_source(..)`](crate::menu_row::EnzMenuRow::icon_source); an `enz_menu` item exposes the same setter on its wrapper. |
//! | `right click surface`, `context capture`, `secondary click target` | [`context_target`](mod@context_target) - a right-click capture surface for context menus. See [`context_menu_host`](mod@context_menu_host) for the full menu. |
//! | `tooltip`, `tooltip delay`, `hover delay`, `dwell time`, `default tooltip delay`, `hover hint`, `hover tooltip`, `title attribute`, `wrap with tooltip`, `add tooltip to element`, `themed iced::widget::tooltip` | [`tooltip`](mod@tooltip) - a styled tooltip wrapper that takes any `Element` + an owned `impl Into<String>` tip. Ordinary hints wait 500 ms; [`set_default_tooltip_delay`] changes the app default and `.tooltip_delay(..)` overrides a control. [`enz_tooltip_with_delay`] configures a wrapper; raw [`FadeTooltip::new`](crate::tooltip::FadeTooltip::new) stays immediate unless `.delay(..)` is set. Named icon-only controls get hints automatically, visible captions do not, and `.tooltip("")` suppresses a hint without removing the accessible name. Reduced motion preserves the delay, and pointer exit, click, scroll, deactivation or tree cancellation withdraws the hint. This is the general form; the tooltip on `enz_toolbar_icon(..).tooltip(..)` and `enz_sidebar_toggle_button(..).tooltip(..)` composes it internally. Reach for it when a widget without its own `.tooltip(..)` builder needs a themed hover tip - never a raw `iced::widget::tooltip`, whose tip is unstyled iced text on the surface. |
//! | `inline banner`, `notification strip`, `AdwBanner`, `inline message`, `alert bar`, `InlineMessage`, `bare tone glyph`, `no filled tone chip`, `bare badge`, `chip badge`, `banner badge style`, `banner without background chip` | [`banner`](mod@banner) - an inline status banner (title + body + tone + optional action + optional dismiss). The default leading glyph is the filled tone chip - the look every existing caller already ships; opt into a bare tone-inked glyph with `EnzBanner::badge_style(BannerBadgeStyle::Bare)` for a banner mounted inside chrome that already carries the tone (for example, `EnzAppShell::notifications`), where a filled chip reads as a second, louder statement of the same signal. |
//! | `durable actionable status`, `portal failure banner`, `shell notifications band`, `shell notifications slot`, `notifications slot`, `notifications band`, `notifications strip`, `application-global banner`, `shell-owned banner`, `stack banners at the top`, `shell owned status area` | [`app_shell`](mod@app_shell) - `EnzAppShell::notifications` is a shell-owned band between the titlebar and the content, for durable actionable status (portal failures, permission errors, external-change nags). Same `Fn(&ShellGeometry) -> Option<Element>` shape as `EnzAppShell::activity`, so an absent slot and a `None`-returning slot pay nothing. Prefer this over hand-mounting `enz_banner` in an app's content column: stacking, single live region, and inset become the shell's concern rather than every adopter's. A banner mounted through this slot should also opt into `BannerBadgeStyle::Bare`. |
//! | `toast`, `snackbar`, `transient notification`, `Sonner`, `toast notification` | [`toast`](mod@toast) - a transient toast surface (shadcn `Sonner` / snackbar). This is one toast's chrome; for how a toast is *shown, timed and dismissed* - and how its text reaches a screen reader as one live region - see its host, [`toast_host`](mod@toast_host). A stand-alone toast announces at its own tone; `.announcement(..)` overrides that term, and is what the host uses to keep exactly one region on screen. |
//! | `state notice`, `persistent notice`, `notice that stays`, `sticky toast`, `toast that does not expire`, `error notice in the corner`, `retry notice`, `app status notice`, `notice stack`, `speak one notice` | [`notice_stack`](mod@notice_stack) - notices about the application's own condition (a failed save, an operation waiting for Retry) as toasts that stay while their cause holds; rebuild the list from state in `view` and pass it to [`enz_notice_stack`]. Exactly one notice is a live region, the first error and otherwise the first notice, and the rest are silent, so a stack is announced once; a notice without `on_dismiss` has no close control. Transient events go through [`toast_host`](mod@toast_host). |
//! | `auto-dismiss`, `stacking`, `toast queue`, `compact toast`, `toast typography`, `toast presentation`, `snackbar queue`, `toast host`, `snackbar host`, `dismiss toast`, `toast timeout`, `how to dismiss a toast`, `notification stack`, `transient notification stack`, `coalesce toasts`, `announce status`, `status announcement`, `screen reader status`, `live status`, `speak a status message`, `toast placement`, `where does the toast go`, `bottom inset`, `safe area toast`, `toast over bottom nav`, `toast above tab bar`, `toast clears mini player`, `toast covers navigation`, `float a toast`, `toast overlay`, `bottom right notification`, `error notification`, `sync failed notice`, `operation failed notice`, `banner above the title bar`, `dismissable error`, `toast layer`, `toast resets scroll`, `page rebuilt when a toast appears` | [`toast_host`](mod@toast_host) - the controller that owns a toast's whole lifecycle: auto-dismiss timers, stacking a queue of toasts, coalescing duplicates and an overflow/waiting budget. [`ToastHost::view_with`](toast_host::ToastHost::view_with) customizes each visible toast's typography, width or material using its configured builder and queued specification; the host retains keyed state, hover timing and shared announcements. The host itself is not an element; drive `ToastHost` from `update`/`subscription` the way you drive `file_chooser`. A default `ToastHost::new()` publishes exactly one `enzui_core::live_region::Announcement` region for the message it is currently showing - polite for advisory tones, assertive for `Warning`/`Error` - updated in place on a replacement or a queue advance and empty once that toast leaves, with the stacked toasts under it published `Announcement::Silent` so retained notifications are readable but never re-spoken. Do not pair it with a second hand-rolled live region; `ToastHost::announcements(ToastAnnouncements::Decorative)` is the opt-out and is for decorative text only. Placement: [`ToastHost::view`](toast_host::ToastHost::view) returns the toast column and owns no position, so the mount is what positions it. [`enz_toast_layer`] mounts it: it floats the column bottom-centre over a whole presentation, and keeps the presentation in one place in the tree whether or not a toast is up (stacking only while a toast shows re-parents the whole app on the first one, resetting every widget's state). [`enz_toast_overlay`] is the bare placement it uses, and [`ToastOverlayPlacement::bottom_inset`](toast_host::ToastOverlayPlacement::bottom_inset) keeps it clear of an app's own bottom chrome - a tab bar, a mini player, a status strip - which an overlay flush with the window edge covers. The default is flush and lays out identically to the container a host owner writes by hand, so an app with no bottom chrome adopts it without moving a pixel. [`ToastOverlayPlacement::trailing`](toast_host::ToastOverlayPlacement::trailing) floats the column in the bottom-trailing corner instead: the place for notices about the app's own work (a failed sync, a file it could not open), which otherwise end up as banners stacked above the title bar. |
//! | `status indicator`, `online dot`, `presence dot`, `status light`, `status dot` | [`status_dot`](mod@status_dot) - a compact semantic status indicator. |
//! | `empty state`, `placeholder`, `no results`, `AdwStatusPage`, `PlaceholderMessage`, `nothing here`, `status page`, `suggested action`, `action emphasis`, `call to action` | [`status_page`](mod@status_page) - a centred empty-state hero view (GTK `AdwStatusPage`). Its primary CTA defaults to the **suggested** emphasis (accent-filled) and shares the same `ActionEmphasis` vocabulary [`banner`](mod@banner) uses; opt down with `.primary_action_emphasis(ActionEmphasis::Neutral)`. |
//! | `loading spinner`, `busy indicator`, `activity indicator`, `indeterminate loader` | [`spinner`](mod@spinner) - a rotating 8-dot indeterminate loader. For content placeholders use [`skeleton`](mod@skeleton). |
//! | `skeleton`, `shimmer`, `loading placeholder`, `content placeholder`, `ghost loading` | [`skeleton`](mod@skeleton) - a shimmering loading placeholder (shadcn `Skeleton`). |
//! | `progress text`, `status text`, `text wave`, `shimmer label`, `working text`, `responding text`, `single accessible progress label` | [`progress_text`](mod@progress_text) - a moving palette ink band over one Advanced-shaped Caption or Body label. Uses visible-only frame scheduling, reduced-motion policy, and optional shared phase handles. |
//! | `animate`, `animate text`, `self animating`, `self-animating`, `animation without a tick`, `no subscription animation`, `pulse`, `pulsing label`, `breathing`, `throb`, `shimmer text`, `shimmer any widget`, `working label`, `in progress label`, `still alive`, `frame clock`, `animation clock`, `frame timer`, `elapsed seconds`, `per frame phase`, `animate without rebuilding the view` | [`frame_clock`](mod@frame_clock) - wrap **any** element so it animates on its own from `RedrawRequested` timestamps. You get elapsed seconds in a handle your style closure reads; it stops under reduced motion, when scrolled off screen, and when you say it is inactive. This is the answer instead of an `iced::time::every` subscription driving a `Message::Tick` that rebuilds your whole view every frame. If you only need "something is happening", reach for [`spinner`](mod@spinner) first - it already self-animates. |
//! | `progress`, `progress indicator`, `loading bar`, `determinate progress`, `ProgressView`, `progress bar` | [`progress_bar`](mod@progress_bar) - a styled progress indicator (SwiftUI `ProgressView` / GtkProgressBar). |
//! | `gauge`, `dial`, `radial meter`, `level meter`, `Gauge` | [`gauge`](mod@gauge) - a circular, token-driven gauge showing a value within a range (SwiftUI `Gauge`). |
//! | `rating`, `star rating`, `stars`, `review score`, `score`, `rating histogram`, `review histogram`, `histogram`, `rating breakdown`, `per-star bars`, `star distribution`, `bar per star` | [`rating`](mod@rating) is the **aggregate** star-score glyph - a display-only fractional star row (Amazon / Steam / GNOME Software convention, half-star quantised, `Role::Image` node with a value / max / step 0.5 numeric domain, `unrated` state announced explicitly with no fabricated zero on the AT bus). For the **histogram** (one bar per bucket, `"N star"` label + numeric value_text on each) reach for a `column!` of [`progress_bar`](mod@progress_bar) - each horizontal bar represents one bucket. Input (the user *sets* a rating) is a separate widget - see [`rating`](mod@rating)'s `# Display, not input` section. |
//! | `badge`, `tag`, `pill`, `count badge`, `label chip`, `Badge` | [`badge`](mod@badge) - a styled badge/tag/pill component. |
//! | `keycap`, `key chip`, `shortcut key`, `kbd`, `key badge` | [`key_badge`](mod@key_badge) - a monospace keyboard-shortcut chip. |
//! | `drag and drop`, `dnd`, `drop onto breadcrumb`, `breadcrumb drop target`, `drop target by widget id`, `cross list drag`, `draggable`, `drop target`, `file row drag`, `folder drop target`, `virtual grid drag`, `drop-only region`, `directional drop highlight`, `selection drag ownership`, `grab cursor`, `grabbing cursor`, `drag preview`, `drop highlight bounds` | [`drag_drop_host`](mod@drag_drop_host) + [`drag_item_surface`](mod@drag_item_surface) - one shared session host plus bounded keyed row/tile registration for onto-item/current-directory drops. Mark destination-only geometry with `DragSurfaceItem::drag_source(false)`; for targets inside content that lays itself out (a breadcrumb's crumbs, via `EnzBreadcrumb::crumb_ids`), `EnzDragItemSurface::items_at_widgets` takes each item's rectangle from the child widget with its id; custom `.drop_target_highlight(..)` follows only the negotiated item and replaces the generic border. Captured child gestures are excluded by default; use `.allows_captured_press(true)` only for rows whose pointer capture merely establishes focus. A **hit rectangle is not a paint rectangle**: feedback is confined to the registering surface, and `DragSurfaceItem::feedback_bounds(..)` narrows it further for a row that draws smaller than it is grabbed by, so a pinned grab width never washes the pane beside the rail, or shows where a drop will land when that differs from where it is aimed (the half of a pane a dropped item will take). `.drop_target_outline(color, radius)` rounds and outlines the custom highlight, for targets large enough to cover content. The live drag is reported through the pointer shape (grab/grabbing), not a rectangle trailing the cursor. |
//! | `autoscroll on drag`, `edge scroll drag`, `scroll while dragging` | [`drag_autoscroll_host`](mod@drag_autoscroll_host) - wires auto-scrolling into a real drag path. |
//! | `zoom wheel`, `Ctrl+scroll zoom`, `Ctrl+wheel zoom`, `zoom with the mouse wheel`, `wheel zoom instead of scroll`, `zoom a grid` | [`zoom_wheel`](mod@zoom_wheel) - wraps a scrolling view so Ctrl+wheel over it publishes a `ZoomStep` instead of scrolling it; it claims the wheel before the content, because iced's scrollable scrolls whatever the modifiers. Pair it with `enzui_core::zoom_levels::ZoomLadder` and Ctrl+plus/minus/0 in the app's shortcut table. |
//! | `shadow`, `drop shadow`, `elevation`, `raise`, `z depth` | [`elevation`](mod@elevation) - a semantic elevation wrapper with 0-5 shadow levels; prefer over hand-written `iced::Shadow`. |
//! | `glow`, `halo`, `aura`, `coloured glow` | [`glow`](mod@glow) - a soft blurred halo around content; tint follows the theme accent and fades with distance. |
//! | `gradient`, `linear gradient`, `gradient fill`, `gradient surface`, `gradient background` | [`gradient_background`](mod@gradient_background) - a configurable linear-gradient surface. |
//! | `block list`, `prose blocks`, `block sequence`, `monospace list`, `code block font`, `theme font family` | [`block_list`](mod@block_list) - a sequence of prose blocks (heading / paragraph / blockquote / list-item / rule). The name a caller half-remembers; the renderers it feeds are on the `rich text` and `preview` rows. The prose and code families come from the theme catalog: `.font_role(FontRole)` picks the role for prose blocks (default [`FontRole::Ui`](enzui_theme::FontRole)), `.font_from_theme(&EnzTheme)` reads the role's family, and `EnzBlockKind::Code` always resolves through `FontRole::Monospace` - no `Font` setter is exposed. |
//! | `button group` | [`button_group`](mod@button_group) - the prose name for the joined-button action cluster; the intent terms (`joined buttons`, `segmented buttons`, `split button row`) are on its row above. For a single-choice segmented control use `view_switcher`. |
//! | `code block`, `code snippet`, `fenced code` | [`code_block`](mod@code_block) - a flat-by-default code-snippet display with an opt-in frame. It renders `markdown` fenced blocks; those long-form renderers are on the `rich text` and `markdown` rows. |
//! | `combo box` | [`combo_box`](mod@combo_box) - the prose name for the enzUI dropdown selector; the intent terms (`dropdown`, `select`, `combobox`) are on its row above. For a free-text editable variant see `autocomplete_field`. |
//! | `context target` | [`context_target`](mod@context_target) - the prose name for the right-click capture surface. Prefer a widget's own `on_context` hook (`sidebar_row`, `list_row_surface`) over hand-wrapping this. |
//! | `document tab`, `single tab`, `tab cell` | [`document_tab`](mod@document_tab) - the single-cell tab primitive. For the full keyed tab strip see the `tab strip` row (`document_tabs`). |
//! | `image widget` | [`image_widget`](mod@image) - the prose name for the raster image with fit, radius and placeholder background. For a source that loads asynchronously see `async_image`. |
//! | `positioned overlay` | [`positioned_overlay`](mod@positioned_overlay) - the prose name for the bare placement layer; the intent terms (`positioned surface`, `bare overlay`, `floating surface`) are on its row above. When the surface IS a menu, reach for `context_menu_host` instead. |
//! | `radio button` | [`radio_button`](mod@radio_button) - the prose name for a single, standalone radio button. For a set of options (one Tab stop, not N) use `radio_group`. |
//! | `reading pane` | [`reading_pane`](mod@reading_pane) - the prose name for the long-form document renderer; the intent terms (`preview`, `article view`, `long form`) are on its row above. |
//! | `row measure` | [`row_measure`](mod@row_measure) - the prose name for the variable-height row-measurement producer; the intent terms (`measure a row`, `variable row height`, `report my height`) are on its row above. |
//! | `sidebar row` | [`sidebar_row`](mod@sidebar_row) - the prose name for the sidebar list row; the intent terms (`nav row`, `sidebar item`, `navigation item`) are on its row above. |
//! | `lazy virtual text accessibility`, `large editable document screen reader`, `screen reader large document`, `accessible text editor node`, `edit box accessibility`, `virtual text provider`, `accessibility beyond the viewport`, `AT reads off screen text`, `gigabyte document accessibility`, `publish an editor to accesskit` | [`virtual_text_surface`](mod@virtual_text_surface) - the **published AT node** for an editor whose text is served lazily by `enzui_core::virtual_text`. It wraps an app's existing editor element, paints nothing of its own, and publishes one editable-text node with a stable widget `Id`; the runtime seam resolves that id to the real `accesskit::NodeId` in the tree the window actually published, so an assistive technology can read, select and replace at an arbitrary offset in a document far larger than any tree could carry. Without it there is no node to bind a provider to, and a provider registered against an invented id answers on an object no client can reach by traversal. It is **not** a viewer, and `paged_text_viewer` is not its sibling: that widget *renders* the visible rows of a large **read-only** document and truthfully announces only those rows to an AT, while this one renders nothing, targets an **editable** document, and is the route by which text *outside* the rendered window is reachable at all. Use `paged_text_viewer` to show a huge log; wrap the editor in `virtual_text_surface` when an AT must reach the whole document. |
//! | `streaming text`, `typewriter text`, `token stream`, `incremental text`, `appended text`, `live log line` | [`streaming_text`](mod@streaming_text) - wrapping text that grows by **incremental append** (a token stream, a typewriter reveal, a live log line landing), with a blinking caret at the tail. For a large, already-complete body you seek around use `paged_text_viewer` instead. The caret only blinks off an `iced::time::every` subscription; with none it renders correctly and never blinks. |
//! | `text area`, `textarea`, `multiline input`, `multi line input`, `monospace editor`, `code editor font`, `editor font family`, `editor scrollbar`, `text editor scroll`, `scrollable editor`, `text content width`, `wrap width`, `padding aware width`, `content width excluding padding`, `editor text size`, `pixel text size`, `line height`, `editor padding`, `paste hook`, `focused paste`, `paste image into composer`, `rich clipboard paste`, `edit delta`, `text diff after edit`, `what changed`, `which bytes changed`, `mirror the document`, `document snapshot per keystroke`, `search index after edit`, `avoid Content::text()`, `incremental editor model`, `bulk edit`, `replace ranges`, `undoable replace all` | [`text_area`](mod@text_area) - a multi-line text editor. [`.on_paste(message)`](text_area::EnzTextArea::on_paste) notifies the host before a focused, writable editor requests native text paste, so richer clipboard formats can be handled without replacing plain-text insertion or submit bindings. Its standard auto-hidden overlay scrollbar reads iced's real laid-out viewport/content/offset and controls the editor through an absolute `Action::ScrollTo`; it never nests a second scroll owner. With wrapping on, text reflows and horizontal chrome is never shown. With wrapping off, horizontal chrome appears only when laid-out content exceeds the viewport width. For an auto-growing composer call `.auto_grow(min_rows, max_rows)` rather than sizing a box from `Content::line_count`. For a single-line input see the `input` row (`text_field`). **What changed:** `Content::perform` returns `Option<EditDelta>` - the exact ordered insert/delete items, with `Position` endpoints and the changed text - so a caller keeping its own model, search index or save snapshot no longer calls `Content::text()` after every keystroke (that walks and copies the whole document). `EditDelta::apply` replays a delta onto a `String` for convenience; a store that needs bounded work per edit applies the same items to a rope. **Bulk replacement:** `Edit::Replace(Arc<ReplaceBatch>)` applies sorted, disjoint `Replacement` ranges against the pre-edit document as one history transaction. Invalid plans are atomic no-ops; undo restores both the exact bytes and the pre-batch cursor, and the returned delta carries changed bytes rather than a flattened document. Non-edit actions (caret, drag, scroll) and no-op edits return `None`. **Large-document pointer work:** a click or drag that lands inside the laid-out editor reuses the already-shaped viewport, so it no longer runs cosmic-text's whole-buffer dirty scan (that scan cost 1.34 ms per drag on a 100 000-line document and is now zero); a pointer on the far edge or outside the editor keeps the conservative path, so caret reveal and drag autoscroll are unchanged, and code holding a cosmic `Buffer` directly still has its externally invalidated lines discovered and re-shaped. **Fonts:** call [`.font_role(FontRole::Monospace)`](text_area::EnzTextArea::font_role) + [`.font_from_theme(theme)`](text_area::EnzTextArea::font_from_theme) rather than pinning `iced::Font::MONOSPACE` at the call site - the family then tracks the theme's `FontCatalog`, the same shape `block_list` and `enz_text` already offer. **Pixel metrics:** [`.text_size_px(14.0)`](text_area::EnzTextArea::text_size_px) overrides the `FieldSize` ramp with an exact px value (the `FieldSize` still drives chrome radius/label size); [`.line_height(1.3)`](text_area::EnzTextArea::line_height) pins the editor's line height. [`.text_content_width(outer)`](text_area::EnzTextArea::text_content_width) returns the width text actually wraps against (outer minus 2× padding), and [`.resolved_padding()`](text_area::EnzTextArea::resolved_padding) exposes the per-side padding. |
//! | `float an image`, `wrap text around image`, `text flow around picture`, `image beside paragraph`, `exclusion region`, `float left`, `float right`, `inline image in editor`, `picture in a note` | [`text_float`](mod@text_float) - [`TextFloat::new(line, side, size, element)`](text_float::TextFloat::new) passed to [`EnzTextArea::float`](text_area::EnzTextArea::float) or [`EnzRichEdit::float`](rich_edit::EnzRichEdit::float): the editor lays each wrapped row in the gap the float leaves at its height (a row with no usable gap moves below it), draws the element in the text and scrolls it with the text; a press on the float is the float's, and drags still select across it. The caller owns the element and moves its line with the edits above it. |
//! | `time field` | [`time_field`](mod@time_field) - the prose name for the editable time-of-day spin button; the intent terms (`time input`, `edit a time`, `start time`) are on its row above. |
//! | `custom syntax parser`, `caller highlighter`, `source preview grammar`, `read only source preview` | [`text_area`](mod@text_area) - [`.highlight_with`](text_area::EnzTextArea::highlight_with) accepts any iced line parser and a live-theme token formatter while retaining field semantics, selection, font configuration and scrolling. Grammar bundles remain optional application dependencies; `.syntax` and `.syntax_label` restore the lightweight framework lexer. |
//! | `spell check`, `spelling`, `misspelled word`, `red squiggly underline`, `spelling suggestions`, `add to dictionary`, `hunspell`, `enchant`, `spell checker in editor` | [`text_area`](mod@text_area) - [`.spell_check(&check)`](text_area::EnzTextArea::spell_check) takes an `enzui_core::spelling::SpellCheck` (built over `enzui_services::spell_check::for_locale()`, feature `spell-check`: Enchant, or Hunspell dictionaries read directly) and draws a wavy error-coloured underline under each misspelled word; a right click on one puts its suggestions, Add to Dictionary and Ignore above the edit menu, and a chosen suggestion replaces the word through the ordinary edit callback. Words with digits, abbreviations, links and addresses are skipped. |
//! | `text area radius`, `square editor corners`, `multiline corner style` | [`text_area`](mod@text_area) - field corners follow live `radius.sm`/`md`/`lg` (5/6/10 px by default), including the focus ring. `.radius(px)` pins corners independently of palette and text-area tokens; `Plain` stays square. |
//! | `editor key bindings`, `custom copy in editor`, `host owned selection`, `bounded document editor`, `projected editor`, `line numbers`, `line number gutter`, `drag selection callback`, `on_drag_update`, `key_binding` | [`text_area`](mod@text_area) - a host whose document is larger than the editor shows keeps the shared field: [`.key_binding(..)`](text_area::EnzTextArea::key_binding) is consulted before the field's own bindings and also receives the edit menu's commands (the menu carries them out as their shortcuts), so Copy, Cut and Select All can act on the host's whole selection; [`.on_drag_update(..)`](text_area::EnzTextArea::on_drag_update) reports each drag-selection step; [`.gutter(..)`](text_area::EnzTextArea::gutter) shows line numbers from the host's `line_offset`. |
//!
//! | `search field radius`, `combo box radius`, `autocomplete radius` | [`search_field`](mod@search_field), [`combo_box`](mod@combo_box), [`autocomplete_field`](mod@autocomplete_field) - expanded fields follow live `radius.sm`/`md`/`lg` by size (5/6/10 px by default). `.radius(px)` pins the field, not the palette or layout. Popup surfaces follow `radius.lg`, option rows follow `radius.md`, and compact search triggers remain circular. Search result surfaces also retain the field radius as their minimum. |
//!
//! | `text field`, `text field radius`, `square input corners`, `input purpose`, `on-screen keyboard layout`, `numeric keyboard` | [`text_field`](mod@text_field) - the single-line text input; for multi-line use `text_area`. Its chrome follows live `radius.sm`/`md`/`lg` by size tier (5/6/10 px by default). `.radius(px)` pins corners; palette-only overrides leave radius live. Other geometry still uses fixed size presets. `.input_purpose(..)` tells the input method what the field holds (number, PIN, phone, email, URL, name, date, time), so on-screen keyboards show the right layout. |
//! | `secure field radius`, `password field corners` | [`secure_field`](mod@secure_field) - the masked field shares the text field's corner policy: live `radius.sm`/`md`/`lg` by size tier, pinned by `.radius(px)`. |
//! | `text measure` | [`text_measure`](mod@text_measure) - the prose name for font-backed text height, height-derived line counts, and byte break offsets; the intent terms (`measure text`, `text height`, `wrapped line count`, `page break`) are on its row above. |
//! | `titlebar group`, `title bar cluster` | [`titlebar_group`](mod@titlebar_group) - the prose name for the basic titlebar-widget cluster; the intent terms (`titlebar cluster`, `titlebar widgets row`) are on its row above. |
//! | `toolbar icon`, `refresh button`, `spinning refresh`, `refresh spinner`, `busy button`, `syncing indicator`, `button shows progress` | [`toolbar_icon`](mod@toolbar_icon) - the prose name for the circular icon-only titlebar/toolbar button; the intent terms (`icon button`, `toolbar button`, `titlebar button`) are on its row above. [`EnzToolbarIcon::busy`](toolbar_icon::EnzToolbarIcon::busy) shows the spinner in place of the glyph while the control's work runs, with the details in its tooltip: a refresh that needs no banner. |
//! | `tree view` | [`tree_view`](mod@tree_view) - the prose name for the keyboard-navigable tree of flattened rows; the intent terms (`tree`, `outline`, `OutlineGroup`) are on its row above. For a folding sidebar *section* use `expander` instead. |
//! | `window lifecycle` | [`window_lifecycle`](mod@window_lifecycle) - the prose name for the multi-window exit-policy `iced::Task` binding; the intent terms (`second window`, `multi-window`, `close last window`) are on its row above. |
//! | `nav split` | [`nav_split`](mod@nav_split) - the collapse-rule *state* (inline pane / overlay drawer / hidden) for the adaptive navigation shell. For the shell widget itself see the `split view` row (`navigation_split_view`). |
//! | `collapse search on small screens`, `search collapses to an icon`, `search icon button`, `search popup`, `responsive search`, `narrow search`, `search in a popover`, `expanding search box`, `focus search field id when collapsed`, `routed focus to a collapsed search`, `Ctrl+F opens the collapsed search`, `focus(search_field_id) returns None`, `find_focused reports None for a collapsed search field` | [`search_field`](mod@search_field) - measured responsive search, **on by default**. It measures whether the field can still do its job against its own assigned width - search glyph + **six wide query characters** + clear slot + the selected size's padding - rather than consulting an app/window breakpoint or requiring the whole placeholder to fit. Verbose placeholder prose continues through the normal ellipsis path at usable widths. Below that content-derived usability threshold it renders one search icon button whose box follows [`TitlebarAffordanceTokens`](toolbar_icon::TitlebarAffordanceTokens) and whose painted glyph reads the same 16 px [`icon::DEFAULT_ICON_SIZE`] as its titlebar siblings, independently of the expanded field-prefix ladder. The icon carries the same accessible name and native keyboard shortcut as the input, a non-empty query keeps its selected fill so a hidden filter remains visible, activation expands inline with focus already in the input, and Escape closes and restores focus to the icon. Value and exact caret survive collapse/reopen. **The collapsed field is also addressable by routed focus:** a caller's `iced::advanced::widget::operation::focusable::focus(field_id)` (or a `text_input::*` op) walks the responsive wrapper, reaches the input under the icon, and the wrapper opens synchronously so the next rendered tree carries the expanded `Role::TextInput`. Apps that publish an `.id(..)` on the field keep working after the shipped default collapses their layout width. Use `.responsive(false)` only when the field already lives inside another search surface (for example [`command_palette`](mod@command_palette)), which owns its own responsive presentation. |
//! | `masonry`, `masonry grid`, `ragged grid`, `variable height tile grid`, `staggered grid`, `waterfall layout`, `Pinterest layout` | **not a widget - and no framework capability exists**. Nothing in this crate lays out a grid of variable-height tiles the ragged way Pinterest and Adwaita's `AdwWrapLayout` do; [`wrap`](mod@wrap) is a **fixed-cell** chip/badge inventory (identical cell heights) and [`flow`](mod@flow) is a flex-wrap **row** container (each row aligns to its tallest child, so tiles do not slot upward into gaps). Neither is masonry, and forcing either into that shape produces a broken thumbnail grid. The searcher who typed `masonry` recorded a **zero** hit in the index (not one wrong hit) so this row exists to say so honestly: if you actually need masonry, write it once as a new tier-1 widget with a tests entry, not per-app. Until then, prefer [`responsive_grid`](mod@responsive_grid) (equal-height cells with size-class breakpoints), whose visual is what most of these searches actually want. |
//! | `workspace`, `open folder`, `open a folder as a workspace`, `root folder`, `opened folder`, `workspace root`, `workspace as a place` | **not a widget** - an app concept, not a framework primitive. "Workspace" in an editor-style app is the user-owned root a *host* opens; the framework ships **decoration for one** (`workspace favicon in sidebar` on [`sidebar_row`](mod@sidebar_row), `workspace section` on [`expander`](mod@expander)) but no *workspace primitive*. That is on purpose - what "a workspace" is (a directory tree? an untitled project? a saved session?) is app vocabulary, and a framework workspace would either be the intersection (nothing useful) or the union (an untyped soup). For a **sidebar section** grouping a workspace's rows, reach for [`expander`](mod@expander) with `.hover_scope(HoverScope::Group)`; for a **single workspace row** (name + favicon + right-click), reach for [`sidebar_row`](mod@sidebar_row) with `.icon_source(..)` and `.on_context(..)`. For the folder tree *inside* an opened workspace, reach for [`tree_view`](mod@tree_view). Recorded here so a search that ended in zero hits lands on a decision - which of the three near-misses does your case want - rather than in silence. |
//! | `file watcher`, `watch file`, `reload on change`, `external change`, `file changed on disk`, `stale open file` | **not a widget - the capability is in core**: [`enzui_core::file_watch`]. For a **bounded set of open documents** - an editor's tabs, the book an app has open, the file the preview pane shows - each file stat's on an `iced::time::every` tick and the outcome is a typed `FileWatchOutcome` ladder (`NoBaseline` / `Unchanged` / `Modified` / `Replaced` / `Deleted` / `Appeared` / `ProbeFailed` / `Superseded`). `Superseded` means a probe from a dropped/reopened watcher, or one older than a completion already applied, arrived late: it is discarded before baseline mutation and never requires attention. Admission is monotone in *completions* (a per-watcher high-water mark), not restricted to the latest issued request, so overlapping probes on a slow backend are not starved. Identity is `(dev, ino) + mtime + ctime + len` on Unix, so an atomic save reports `Replaced` and a same-size in-place write can still be detected when either timestamp advances. After loading a document, seed the identity captured from that open file with `FileWatcher::with_baseline`; an unseeded first probe cannot prove that the file stayed unchanged since the load. `FileWatcher::acknowledge` suppresses the app's **own** save and invalidates older probes. The filesystem backend follows a final symlink to its effective target, treats a dangling link as absent, and reports special targets as `ProbeFailed`. Submit `FileWatcher::prepare_probe()` with `ProbeOp::dispatch` through an application-owned [`enzui_core::blocking_task::BlockingTaskOwner`]; an async wrapper alone does not offload `stat(2)`. For event-driven directory membership changes, see the `inotify` row below. |
//! | `inotify`, `kqueue`, `FSEvents`, `notify-rs`, `watch directory`, `watch a directory`, `filesystem event`, `fs event`, `directory watcher`, `workspace watcher` | [`enzui_core::file_tree_watch`] supplies bounded, event-driven directory membership watches, with a native Linux inotify backend. Subscribe to a finite set of visible folders, handle overflow by refreshing listings, and release watches when they are no longer needed. This is not an unbounded recursive workspace watcher. For polling a bounded set of open documents, use [`enzui_core::file_watch`]. |
//!
//! Capabilities in **other crates** that a widgets-crate reader will search
//! for and not find here are listed alongside.
//!
//! ## Composes with
//!
//! Composition rows name containers and their collaborators. Use these links
//! to find the widgets needed inside a surface; related module documentation
//! links collaborators back to their containers. The machine-readable catalog
//! in `docs/widgets.json` carries the same collaborator arrays in table order.
//!
//! | widget | composes with | why |
//! |---|---|---|
//! | [`title_bar`](mod@title_bar) | [`window_controls`](mod@window_controls), [`titlebar_icon_group`](mod@titlebar_icon_group), [`toolbar_icon`](mod@toolbar_icon), [`titlebar_actions`](mod@titlebar_actions) | A titlebar is five widgets, not one. The band owns ordered slots, the drag region and reserved controls width; it builds [`window_controls`](mod@window_controls) itself once `on_gesture` is wired. Trailing commands belong in the band's budgeted [`.actions(..)`](title_bar::EnzTitleBar::actions) slot, never in a pushed standalone [`titlebar_actions`](mod@titlebar_actions). Read `# Composing a titlebar` in the module header for the layer-by-layer map. |
//! | [`titlebar_icon_group`](mod@titlebar_icon_group) | [`toolbar_icon`](mod@toolbar_icon), [`titlebar_actions`](mod@titlebar_actions) | The cluster **container** for custom composition: variant fill/border, 8 px grouping rhythm, fixed 32 px height, centre alignment and a rounded clip mask. Its children are [`toolbar_icon`](mod@toolbar_icon)s; a standalone [`titlebar_actions`](mod@titlebar_actions) remains chrome-free and may be wrapped here only outside [`title_bar::EnzTitleBar`](mod@title_bar). This is the exact edge whose absence let an author ship a titlebar with no icon cluster at all. |
//! | [`modal_host`](mod@modal_host) | [`modal_surface`](mod@modal_surface) | The host owns *modality* - scrim, background inertness, Escape, dismissal routing - and nothing about what the dialog looks like. [`modal_surface`](mod@modal_surface) is the container that dialog content sits on; every shipped preset ([`confirm_dialog`](mod@confirm_dialog), [`shortcuts_dialog`](mod@shortcuts_dialog)) builds a surface and hands it to the host as its `dialog` argument. A host with hand-rolled content gets no radius, padding, elevation or close affordance. |
//! | [`modal_surface`](mod@modal_surface) | [`capped_dialog_body`](mod@capped_dialog_body) | A plain surface intentionally accepts arbitrary content and changes no existing geometry. When that content is a dialog with a scrolling body and fixed action footer, wrap it in [`enz_capped_dialog_body`]: it measures title/actions, gives only the remainder to the body, and keeps the footer inside the surface clip at short viewports. |
//! | [`card`](mod@card) | [`section_header`](mod@section_header), [`preferences_group`](mod@preferences_group) | Three jobs share this family, and picking the wrong one is a recorded hazard. A **card** says *"this is a thing"* - a discrete entity with its own actions. A **page section** is the same widget with `CardTone::Flat`: the bold title (17 px at baseline text scale), no fill, no border (that tone paints nothing at all, so it genuinely inherits its parent). [`section_header`](mod@section_header) is the *third* job - an 11 px muted caption for a form-group rail - and is what goes **inside** a card body to caption a sub-group; swapping it in as a page heading makes the heading smaller than the body text under it. [`preferences_group`](mod@preferences_group) is the settings-rhythm specialisation of this same card, flat by default. |
//! | [`preferences_group`](mod@preferences_group) | [`preferences_row`](mod@preferences_row) | The group is the settings **card** (title + description + the flat/bordered rhythm) and takes its body as one `content` element; the rows are what goes in that body. The group supplies no rows and the row supplies no heading, so a settings pane needs both. |
//! | [`navigation_split_view`](mod@navigation_split_view) | [`sidebar_content`](mod@sidebar_content), [`sidebar_row`](mod@sidebar_row) | The split view owns only the *collapse decision* (inline pane / overlay drawer / hidden) and takes its `sidebar` as an arbitrary `Fn() -> Element` closure - it ships no navigation chrome whatsoever. [`sidebar_content`](mod@sidebar_content) is the padded column with the sidebar padding/spacing token defaults, and [`sidebar_row`](mod@sidebar_row) the `Role::ListItem` nav row with focus ring and keyboard activation. Hand-rolling the closure's contents is how a sidebar loses its list semantics. |
//! | [`sidebar_row`](mod@sidebar_row) | [`list_row_surface`](mod@list_row_surface) | `sidebar_row` is the constrained sibling: icon, label, count, selected, leading indent - a closed set. It is built on [`list_row_surface`](mod@list_row_surface) internally. When a sidebar row needs a **trailing control** (a kebab menu, a remove button, a toggle), drop to `list_row_surface` and lay out your own content inside it - the trailing action then sits **inside** the row’s hover/selected pill rather than floating outside it. Both carry `on_context`, so a right-click menu is **not** a reason to drop down. |
//! | [`tree_view`](mod@tree_view) | [`tree_disclosure`](mod@tree_disclosure) | The tree view is a keyboard-navigable vertical list of pre-flattened rows; the chevron every branch row leads with is not part of the view, it is [`tree_disclosure`](mod@tree_disclosure). Reach for the chevron directly only when you are laying out a row the view cannot host (a custom disclosure column, an outline in another surface); otherwise the view builds it for you when a row has children. The view is the disclosure's sole consumer, and the reader arriving at `tree_disclosure` has no other route to the widget it is for. |
//! | [`markdown`](mod@markdown) | [`reading_pane`](mod@reading_pane), [`code_block`](mod@code_block) | The Markdown renderer turns CommonMark source into a laid-out document by delegating the two hard sub-problems: [`reading_pane`](mod@reading_pane) draws prose blocks (headings, paragraphs, blockquotes, lists) with the reading-body ink ladder, and [`code_block`](mod@code_block) renders fenced code with syntax colours. Reach for one of the two directly when you already have blocks or a snippet in hand and do not need a CommonMark parse. |
//! | [`chat_bubble`](mod@chat_bubble) | [`block_list`](mod@block_list), [`streaming_text`](mod@streaming_text) | A chat bubble's *chrome* (avatar column, role tint, tail, timestamp) is what this module owns; the message body is drawn by [`block_list`](mod@block_list) when the reply is a finished sequence of prose blocks and by [`streaming_text`](mod@streaming_text) when a token stream is still landing (wrapping text with a blinking caret). Passing plain text works, but a chat surface with a Markdown reply or a live LLM stream needs both siblings. |
//! | [`chat_list`](mod@chat_list) | [`chat_bubble`](mod@chat_bubble), [`tail_follow`](mod@tail_follow) | The list is the scrolling transcript container - empty-state handling, per-role spacing, and the virtualized realisation window. Each row is a [`chat_bubble`](mod@chat_bubble); the list ships no bubble chrome of its own. Its scroll **anchoring** is [`tail_follow`](mod@tail_follow): it opens at the newest message and follows appended ones, releasing the pin while the reader is scrolled up. |
//! | [`chat_transcript`](mod@chat_transcript) | [`chat_bubble`](mod@chat_bubble), [`tail_follow`](mod@tail_follow) | Same relationship as [`chat_list`](mod@chat_list): each turn is rendered as a [`chat_bubble`](mod@chat_bubble), and scroll anchoring is delegated to the shared [`tail_follow`](mod@tail_follow) primitive rather than owned here - so both chat surfaces open at the newest message and release the pin identically. Paging is the underlying [`scroll_column`](mod@scroll_column)'s. |
//! | [`shortcuts_dialog`](mod@shortcuts_dialog) | [`shortcut_list`](mod@shortcut_list), [`capped_dialog_body`](mod@capped_dialog_body) | `into_content()` returns the fill-size title + shortcut list for embedding; `into_surface(..)` adds the capped card and Close button; `into_host_with(..)` adds modality. The preset shares one content builder between its embedded and surface forms so they cannot drift. |
//! | [`shortcut_list`](mod@shortcut_list) | [`key_badge`](mod@key_badge) | Each row's leading fixed-width slot is a [`key_badge`](mod@key_badge) chord glyph, the same chip you would use anywhere a chord is displayed inline. The list ships the section-header rhythm and the row-height defaults; the badge ships the chord rendering. |
//! | [`command_palette`](mod@command_palette) | [`search_field`](mod@search_field) | The palette's query input is [`search_field`](mod@search_field), not a plain [`text_field`](mod@text_field) - that is what gives the palette its clear-glyph, its Escape semantics and the correct placeholder tone. Wiring it up manually re-derives all three. |
//! | [`date_picker`](mod@date_picker) | [`calendar`](mod@calendar), [`date_field`](mod@date_field) | The picker is the field + popup pairing: the field chrome is shared through [`date_field`](mod@date_field), and the popup body is a [`calendar`](mod@calendar) - the pure keyboard-navigable month grid. Reach for the calendar directly when you already own the popup shell (a form, a sidebar) and only need the grid. |
//! | [`date_range_picker`](mod@date_range_picker) | [`date_picker`](mod@date_picker), [`calendar`](mod@calendar), [`date_field`](mod@date_field) | A range picker is a coordinated pair of date pickers over a shared [`calendar`](mod@calendar); it re-uses [`date_picker`](mod@date_picker)'s dismissal policy and calendar-owner id derivation so the two popups behave identically at both ends of the range. Do not hand-roll the pairing - the coordination is where the accessibility bugs live. |
//! | [`download_list`](mod@download_list) | [`progress_bar`](mod@progress_bar) | Each row's determinate progress readout is a [`progress_bar`](mod@progress_bar) in a fixed-width column; reach for the bar directly when you have one download to show and no need for the list's per-item chrome. |
//! | [`menu_button`](mod@menu_button) | [`menu`](mod@menu), [`menu_row`](mod@menu_row) | The controlled menu-button host: the trigger is a chip that opens an anchored surface, the surface is [`menu`](mod@menu) (elevation, dismissal, focus routing) and each item is a [`menu_row`](mod@menu_row) (icon slot, keyboard accelerator column, `MenuRowTone`). All three names appear in a call site - the trigger only decides *when* to show a menu, not what is in it. |
//! | [`titlebar_actions`](mod@titlebar_actions) | [`menu`](mod@menu) | In a custom band outside [`title_bar::EnzTitleBar`](mod@title_bar), `titlebar_actions` folds overflowed commands into a vertical-ellipsis kebab and shows them in a [`menu`](mod@menu) via an anchored overlay. An [`EnzTitleBar`] caller must use its [`.actions(..)`](title_bar::EnzTitleBar::actions) slot so the fold shares the band's controls budget. |
//! | [`context_menu_host`](mod@context_menu_host) | [`menu`](mod@menu), [`context_menu_action`](mod@context_menu_action) | The host wires right-click and the [`Action::ShowContextMenu`](crate::context_menu_action) alias to open a [`menu`](mod@menu) surface at the pointer (or, from AT, at the target's centre). The menu is where items appear; the host owns the *when* and the assistive-technology hook. |
//! | [`context_target`](mod@context_target) | [`context_menu_action`](mod@context_menu_action) | A right-click hit region that publishes a [`ContextMenuRequest`](crate::context_menu_action::ContextMenuRequest) into the same routing the AT action opens. Reach for [`context_menu_action`](mod@context_menu_action) directly only when you are implementing a target that is not a rectangle. |
//! | [`page_header`](mod@page_header) | [`view_that_fits`](mod@view_that_fits), [`window_title`](mod@window_title) | Wide screens land the actions in a trailing row; a narrow screen folds them under the title. The width-driven pick is [`view_that_fits`](mod@view_that_fits); the title + subtitle typography is [`window_title`](mod@window_title). Hand-rolling either is how a header ships an untranslated action bar or a title that wraps in the wrong place. |
//! | [`header_bar`](mod@header_bar) | [`center_clamp_row`](mod@center_clamp_row), [`window_title`](mod@window_title) | A titled bar whose start / end zones must not overlap its centred title. The zone layout is [`center_clamp_row`](mod@center_clamp_row) - a three-slot bar that clamps the middle child against the *measured* widths of the two side zones so a wide localized action reserves exactly its width. The title + subtitle typography is [`window_title`](mod@window_title). |
//! | [`toolbar_view`](mod@toolbar_view) | [`header_bar`](mod@header_bar) | The port of SwiftUI's `.toolbar` presents its title and actions through a [`header_bar`](mod@header_bar); the wrapper is where the placement decisions live, the bar itself is the presentation. |
//! | [`navigation_outlet`](mod@navigation_outlet) | [`header_bar`](mod@header_bar) | The routed outlet's back-button + title + action-slot row is a [`header_bar`](mod@header_bar); the outlet decides *which* header to show for the active route, the bar renders it. |
//! | [`preview_row`](mod@preview_row) | [`text_pair`](mod@text_pair) | The row's leading title + trailing metadata pairing is a [`text_pair`](mod@text_pair): single-line leading text with a protected trailing metadata slot. Reach for `text_pair` when you need the same shape without the row chrome. |
//! | [`video`](mod@video) | [`media_controls`](mod@media_controls), [`aspect_ratio`](mod@aspect_ratio), [`skeleton`](mod@skeleton) | The transport strip is [`media_controls`](mod@media_controls), the frame ratio is [`aspect_ratio`](mod@aspect_ratio), and the pre-load placeholder is a [`skeleton`](mod@skeleton) at the same ratio (so the frame does not resize when the first frame lands). |
//! | [`media_controls`](mod@media_controls) | [`slider`](mod@slider) | The scrubber is [`slider`](mod@slider), not a hand-rolled progress bar - that is what carries the focus ring, the keyboard step / page-step keys and the drag-scrub semantics. |
//! | [`group_box`](mod@group_box) | [`card`](mod@card) | The borderless-by-default tonal group container is presented through [`card`](mod@card) (surface tokens, rhythm); reach for `card` directly when you need the card's visuals without a titled group. |
//! | [`tab_pill`](mod@tab_pill) | [`horizontal_scroll_view`](mod@horizontal_scroll_view) | A tab strip too wide for its band is not clipped, it *scrolls*: the pill row is wrapped in a [`horizontal_scroll_view`](mod@horizontal_scroll_view) with scroll-fade edges so a strip that overflows still reveals the tabs beyond it. |
//! | [`inline_view_switcher`](mod@inline_view_switcher) | [`tab_pill`](mod@tab_pill), [`view_switcher`](mod@view_switcher) | The inline segmented switcher is a row of [`tab_pill`](mod@tab_pill)s - reach for the pill directly for a single-widget selection cue; reach for the switcher when you have a small set of exclusive views to swap. It **is** [`view_switcher`](mod@view_switcher)'s engine under [`SwitcherPolicy::INLINE_SEGMENTED`](view_switcher::SwitcherPolicy::INLINE_SEGMENTED): one page model, one selection/keyboard/a11y owner, two presentation presets. Pick the preset by where the control sits (toolbar chrome vs inline in content), not by which features you need. |
//! | [`view_switcher`](mod@view_switcher) | [`tab_pill`](mod@tab_pill) | Same shape as [`inline_view_switcher`](mod@inline_view_switcher): the segmented row is [`tab_pill`](mod@tab_pill)s; the switcher wraps them with the exclusive-selection routing. |
//! | [`expander`](mod@expander) | [`toggle`](mod@toggle) | The disclosure summary carries a [`toggle`](mod@toggle) as its expand/collapse control when the caller asks for a switch (rather than a chevron) - the expander is the container, the toggle is the affordance. |
//! | [`toggle`](mod@toggle) | [`switch`](mod@switch) | The public toggle is a [`switch`](mod@switch) plus the row-shape defaults and the aria-label handling; reach for `switch` directly only when you need the bare pill outside of an operable-toggle context. |
//! | [`toolbar_icon`](mod@toolbar_icon) | [`icon`](mod@icon) | The circular icon-only button paints its glyph with an [`icon`](mod@icon), routed through [`tint_with_status`](icon::EnzIcon::tint_with_status) so `hover_color` / `pressed_color` reach the glyph. `icon` is generic (67 consumers) and is otherwise deliberately off the composes-with list - a per-widget edge to it would flood the table. This one edge is specific and justified: an author adding a status-reactive tint to *another* icon-bearing widget needs to learn, from `icon`'s own docs, that [`tint_resolved`](icon::EnzIcon::tint_resolved) receives only `&Theme` and so cannot express hover; setting the tokens without the routing change looks exactly like the fix not landing. |
//!
//! ## Choose between siblings
//!
//! Use this table to choose among widgets that share an intent but differ in
//! scroll ownership, accessibility semantics, or the surrounding chrome they
//! provide. Each row links the available siblings and explains when to use them.
//!
//! | family | siblings | the question that decides | pick |
//! |---|---|---|---|
//! | shell | [`app_shell`](mod@app_shell), [`navigation_split_view`](mod@navigation_split_view) | Are you writing the **top-level view of an application**, or an embedded/isolated split whose host owns the titlebar and window chrome? | [`app_shell`](mod@app_shell) is the default and what all apps use - it owns titlebar, sidebar, content, window chrome, the collapse rule and the drawer semantics in one composition. Reach for [`navigation_split_view`](mod@navigation_split_view) **only** for an embedded pane, a secondary window whose host draws the chrome, or a test harness rendering the split in isolation; composing it beside a hand-rolled titlebar and a bare [`window_controls`](mod@window_controls) cluster duplicates the chrome this family already draws. |
//! | titlebar-band | [`title_bar`](mod@title_bar), [`header_bar`](mod@header_bar), [`toolbar_view`](mod@toolbar_view), [`page_header`](mod@page_header), [`navigation_outlet`](mod@navigation_outlet) | Is the band the **window titlebar** (drag region, reserved controls, split at the sidebar seam), the **toolbar** above the content pane, the header of a **routed page**, or the outlet of a **navigation stack**? | [`title_bar`](mod@title_bar) is the window band inside [`app_shell`](mod@app_shell): it owns the drag region and the [`.actions(..)`](title_bar::EnzTitleBar::actions) budget, and shipping a titlebar with **no icon cluster** by using standalone [`titlebar_actions`](mod@titlebar_actions) as generic titlebar content is a recorded defect. [`toolbar_view`](mod@toolbar_view) is the SwiftUI-`.toolbar` port for placement decisions; [`header_bar`](mod@header_bar) is the plain titled bar the others build on. [`page_header`](mod@page_header) is a **page**'s header with responsive action folding; [`navigation_outlet`](mod@navigation_outlet) is a **routed** back-button + title + action row. Reach for `header_bar` directly only when you own the placement yourself. |
//! | segmented-control | [`view_switcher`](mod@view_switcher), [`inline_view_switcher`](mod@inline_view_switcher), [`tab_pill`](mod@tab_pill), [`radio_group`](mod@radio_group) | Are you switching between **whole views/pages**, offering an inline segmented control **within content**, drawing a single **tab pill**, or presenting a semantic **radio-button group** for a form value? | [`view_switcher`](mod@view_switcher) is the toolbar-chrome view-swap. [`inline_view_switcher`](mod@inline_view_switcher) is [`view_switcher`](mod@view_switcher)'s engine under [`SwitcherPolicy::INLINE_SEGMENTED`](view_switcher::SwitcherPolicy::INLINE_SEGMENTED) - pick the preset by where the control sits (toolbar vs inline), not by which features you need. [`tab_pill`](mod@tab_pill) is a single pill for a one-widget selection cue; hand-rolling a row of pills instead of using a switcher loses the exclusive-selection routing and the keyboard model. [`radio_group`](mod@radio_group) is the **form-value** exclusive control (the mode-switch role an app once emulated with two buttons), never a view swap: its semantic role is `RadioGroup`, so screen readers hear a group of radios rather than tabs. |
//! | button-emphasis | [`button`](mod@button), [`confirm_dialog`](mod@confirm_dialog), [`form_dialog`](mod@form_dialog) | Is the button a dialog's or form's **default action**, an ordinary **neutral** action, or the **way out**? | On a [`button`](mod@button), [`ButtonKind::Accent`] is the default or suggested action; [`confirm_dialog`](mod@confirm_dialog) and [`form_dialog`](mod@form_dialog) use it for confirm and submit. [`ButtonKind::Primary`] is a neutral filled button for an action that is not the default, and on a dialog surface it does not stand out. Cancel and close are [`ButtonKind::Quiet`]. |
//! | chat-surfaces | [`chat_list`](mod@chat_list), [`chat_transcript`](mod@chat_transcript) | Do you need a **virtualised, host-scrolled** transcript (long histories, tens of thousands of messages) or a **presentational** transcript column that composes cleanly inside another surface? | [`chat_list`](mod@chat_list) is the virtualised list - it has **no self-scrolling mode**, so the `on_scroll` → `update` → `.scroll_offset(..)` round trip is the only route (over 10,000 messages: unwired realises 0..=9 at every offset; wired 120..=134). [`chat_transcript`](mod@chat_transcript) delegates its scrolling to [`scroll_column`](mod@scroll_column) with the shared [`tail_follow`](mod@tail_follow) anchor and is what you want for short histories, LLM previews, and any place the host is already inside a scroll container. Both open at the newest message and release the pin when the reader scrolls up; the choice is scroll ownership, not chrome. |
//! | card-container | [`card`](mod@card), [`group_box`](mod@group_box), [`preferences_group`](mod@preferences_group), [`section_header`](mod@section_header) | Is this a **discrete entity** with its own identity or actions, a **page section** stacked with other sections, a **settings-rhythm group** of preference rows, or a **caption above a rail of controls**? | [`card`](mod@card) with `CardTone::Default` (or `Outlined` when the border genuinely separates it from an *adjacent* pane) says *"this is a thing"*; a **page section** is the same widget with `CardTone::Flat` - a bold heading (17 px at baseline text scale), no fill, no border. [`group_box`](mod@group_box) is the borderless-by-default tonal group presented through [`card`](mod@card); [`preferences_group`](mod@preferences_group) is the settings-rhythm specialisation of the same card, flat by default. [`section_header`](mod@section_header) is the **third** job - an 11 px muted caption for a form-group rail - and it lives inside a card body to caption a sub-group; swapping it in as a page heading makes the heading smaller than the body text under it. A border that appears around every section separates nothing: name what a border separates before adding one. |
//!
//! ### Text and typography
//!
//! [`enz_text`](mod@text) supplies plain text with the framework's type scale,
//! semantic weights and tone tokens. This group also contains label/value
//! pairings, captions, measurement and whole-document renderers; custom text
//! surfaces keep their own layout and rendering policies.
//!
//! [`section_header`](mod@section_header) is a muted caption band (11 logical
//! pixels at the default scale). For a titled page-section container, use
//! [`card`](mod@card)'s Flat tone, whose title publishes a Section heading.
//!
//! - [`text`](mod@text) - token-driven typography primitive; unpinned families and semantic weights follow application defaults. `font_role` selects a catalog role, `weight_rung` selects a semantic face policy, and `TextShaping` selects Auto or explicit Advanced/Basic shaping.
//! - [`label`](mod@label) - the canonical icon + title (+ optional subtitle) label; `enz_label` presets the form shape.
//! - [`icon_label`](mod@icon_label) - the SwiftUI `Label`-style row **preset** of [`label`](mod@label): `EnzIconLabel` is an alias for `EnzLabel`.
//! - [`labeled_content`](mod@labeled_content) - form "label | value" row (or stacked in compact mode).
//! - [`text_pair`](mod@text_pair) - single-line leading text + protected trailing metadata.
//! - [`section_header`](mod@section_header) - muted all-caps caption band above a form group or sidebar rail (11 logical pixels at the default scale). For a page-section heading, use `card`'s Flat tone with its `HeadingLevel::Section` title.
//! - [`streaming_text`](mod@streaming_text) - wrapping text with a blinking caret while streaming. **Wiring:** the widget owns no clock, so the caret only blinks if the caller flips `.caret_visible(..)` from an `iced::time::every(CARET_HALF_CYCLE)` subscription; with no subscription the indicator renders correctly and never blinks.
//! - [`markdown`](mod@markdown) - a token-aware Markdown document renderer with bounded parse/render nesting, a flat render adapter, and stack-safe destruction for caller-built trees.
//! - [`reading_pane`](mod@reading_pane) - long-form document renderer, including an opt-in revision-aware immutable-snapshot path for large chapters.
//! - [`revision_text`](mod@revision_text) - identity + monotonic-revision plain text that materializes immutable content only on a cache miss; construct it with [the public constructor](revision_text::enz_revision_text).
//! - [`paged_text_viewer`](mod@paged_text_viewer) - visible-row viewer for very large random-access UTF-8 text, with virtual line numbers and distinct source-byte / partial line-index status; an overlay scrollbar emits bounded source-byte seeks without indexing the whole document; the row window is derived from the widget's actual allocation and `PagedTextViewportState` publishes it for update-time navigation, and a short terminal frame is bottom-aligned inside the viewer's own body so the host mounts it as one unconditional child.
//! - [`virtual_text_surface`](mod@virtual_text_surface) - publishes an app's own editor as one editable accessible text node whose contents are served lazily by `enzui_core::virtual_text`, so an assistive technology can read, select and replace at arbitrary offsets in a document too large to publish eagerly. It wraps the editor and changes no pixels; the widget id it publishes is the id the runtime seam resolves to the real `accesskit::NodeId` of the tree the window published. Distinct from [`paged_text_viewer`](mod@paged_text_viewer), which renders the visible rows of a large read-only document and announces only those rows.
//! - [`block_list`](mod@block_list) - sequence of prose blocks (heading / paragraph / blockquote / list-item / ordered-item / rule / spacer).
//! - [`code_block`](mod@code_block) - flat-by-default code-snippet display.
//! - [`match_ink`](mod@match_ink) - editor highlighter that resolves readable glyph ink inside a find match, so an accent-filled current match cannot swallow syntax-coloured text.
//! - [`text_grid`](mod@text_grid) - a single-row dense text grid for TUI-style output.
//! - [`line_number_gutter`](mod@line_number_gutter) - a line-number gutter that stays aligned under word wrapping. A scrolled wrapped editor must pass the editor-owned first logical line / content-space top through [`GutterConfig::window`](crate::line_number_gutter::GutterConfig::window), even for a small document: independently shaping the whole prefix and applying a sparse editor offset can put logical line 130 beside label 34/66/77. Pass the same viewport's absolute offset through [`GutterConfig::scroll_offset`](crate::line_number_gutter::GutterConfig::scroll_offset), so a responsive re-parent starts at the mapped window instead of zero. The seed applies only to fresh state; the shared `.scroll_id(..)` continues to drive live scrolling.
//! - [`text_measure`](mod@text_measure) - font-backed wrapped text height, height-derived line counts, and byte break offsets.
//! - [`window_title`](mod@window_title) - a bold title with an optional muted subtitle.
//! - [`wizard`](mod@wizard) - a step-by-step assistant: the step list, the current page and Cancel, Back, Next or Finish.
//! - [`selection_scope`](mod@selection_scope) - focusable cross-widget read-only text selection with pointer and shaped keyboard range navigation; only the focused scope owns copy/select/context shortcuts.
//!
//! ### Buttons and actions
//!
//! Things the user clicks to *cause* something. [`button`](mod@button) is the
//! default; the rest are shape variants (round chips for chrome, pills for
//! suggestions) or containers that arrange several actions.
//! [`pressable`](mod@pressable) is the escape hatch when you need
//! button-like hover/press/selected behaviour on a surface that must not look
//! like a button.
//!
//! - [`button`](mod@button) - a styled button component, including [`enz_filter_chip`] for compact filters with visible and accessible toggle state.
//! - [`button_group`](mod@button_group) - action cluster of joined buttons.
//! - [`link`](mod@link) - a styled hyperlink component.
//! - [`pressable`](mod@pressable) - a clickable surface with button-like hover/press/selected behaviour, without being a button.
//! - [`clickable`](mod@clickable) - wraps arbitrary content as a button (role, focus, Enter/Space, hover, minimum hit target).
//! - [`action_bar`](mod@action_bar) - an action row that folds whole commands into an overflow menu instead of clipping them. **Wiring:** hold the menu state in the app, pass `.overflow_open(..)`, and use the toggle message only for the trigger. Set [`.on_dismiss(..)`](action_bar::EnzActionBar::on_dismiss) to a distinct message that unconditionally closes the menu; it is published after a row command and for Escape/outside dismissal. For a named `Collector::Kebab`, call [`overflow_focus_restore_operation`] with the prior open state and trigger id when either reducer closes the menu.
//! - [`toolbar_icon`](mod@toolbar_icon) - circular icon-only button for titlebars / toolbars. `.icon_source(..)` accepts bundled names (tinted) or application SVG/raster artwork (untinted and contained in the glyph box); `.aria_label(..)` is required.
//! - [`sidebar_toggle_button`](mod@sidebar_toggle_button) - circular icon-only chip that toggles the sidebar.
//! - [`window_control_button`](mod@window_control_button) - icon-only round chip for window chrome.
//! - [`menu_bar`](mod@menu_bar) - a desktop menu bar (File, Edit, View) built from a menu model, with submenus, F10 and arrow-key navigation, and access keys.
//! - [`mnemonic`](mod@mnemonic) - access keys: Alt and an underlined letter activate a control (`"_Save"`).
//! - [`menu_button`](mod@menu_button) - the controlled menu-button host.
//!
//! ### Text input and forms
//!
//! Widgets that read a value back out of the user. Each is *controlled*: it
//! takes the current value or editor content plus a message constructor. The
//! application applies reported edits; widgets retain interaction state such
//! as focus and cursor position. The containers at the end of the
//! group ([`form_layout`](mod@form_layout),
//! [`preferences_group`](mod@preferences_group)) handle label alignment so
//! rows do not each hand-roll it.
//!
//! - [`text_field`](mod@text_field) - a styled text input component.
//! - [`text_area`](mod@text_area) - multi-line text editor with a standard auto-hidden overlay scrollbar driven by the editor's real scroll metrics (one scroll owner; wrapping on never shows horizontal chrome, while wrapping off shows it only for real horizontal overflow). **Sizing to content:** for an auto-growing composer call [`.auto_grow(min_rows, max_rows)`](text_area::EnzTextArea::auto_grow) - it grows one *wrapped row* at a time and then scrolls; do **not** size a box from `Content::line_count`, which counts *logical* `\n` lines and leaves a wrapped paragraph clipped in a one-row box. To measure rows yourself use [`.row_count(width)`](text_area::EnzTextArea::row_count), which forwards that area's font, size, line height, advanced shaping, and wrapping policy to [`text_measure`](mod@text_measure). [`text_area::row_count_at`] is the default-font, word-wrapped convenience form. **Slots:** [`.leading(el)`](text_area::EnzTextArea::leading) / [`.trailing(el)`](text_area::EnzTextArea::trailing) reserve an edge slot (trailing bottom-aligned) that shrinks the editor *width* so an inline mic/counter/send never overlaps the text.
//! - [`rich_edit`](mod@rich_edit) - a rich-text editor with a formatting toolbar; keep a [`RichEditor`] and read and write HTML.
//! - [`text_float`](mod@text_float) - an element floated in a [`text_area`](mod@text_area) or [`rich_edit`](mod@rich_edit) document, like an image placed against a paragraph: [`TextFloat`](text_float::TextFloat) sits at the top of a line against the left or right edge, the paragraph flows beside it and continues at the full width below, and it scrolls with the text while the content, caret and undo history stay one document.
//! - [`composer`](mod@composer) - the chat / prompt input: auto-grow with a row cap, Enter/Shift+Enter, an in-field trailing slot, an action rail, and a send button that swaps to stop on a busy flag. It owns the input **geometry** apps were measuring by hand.
//! - [`secure_field`](mod@secure_field) - masked text input with a trailing reveal/hide toggle.
//! - [`search_field`](mod@search_field) - search-flavoured text input. **Focus contract:** `.focus_on_mount(true)` (preferred) or `.id(id)` + `text_input::focus(id)` on the opening transition only; a toggle-shut must not re-focus.
//! - [`find_replace_bar`](mod@find_replace_bar) - responsive inline query/navigation/replacement composition with one close route.
//! - [`find_surface`](mod@find_surface) - anchored query, count and wrapping previous/next navigation over a caller-owned stable-key match set.
//! - [`input_otp`](mod@input_otp) - segmented one-time-code / PIN entry; narrow hosts compress cells and gaps while keeping the complete code visible and retaining field height.
//! - [`stepper`](mod@stepper) - bounded integer or decimal spinbox with ± buttons.
//! - [`chip_input`](mod@chip_input) - a text field that turns typed or pasted text into removable chips (tags, recipients).
//! - [`emoji_picker`](mod@emoji_picker) - an emoji chooser with search by name and shortcode, category tabs and recently used emoji (feature `emoji-picker`).
//! - [`font_chooser`](mod@font_chooser) - the installed font families (each name in its own font), their styles, a size and a live preview.
//! - [`text_drag`](mod@text_drag) - selected text drags out of fields, editors, selection scopes and glyph layers; read-only document selections use Copy. Custom canvases expose selections with `offer_read_only_selection`. Dropped text goes into fields where it lands (installed by `app_setup::run` on Wayland).
//! - [`time_field`](mod@time_field) - a click-and-type time of day that the arrow keys also step, as one `SpinButton` node. Text is read and written by [`enzui_core::time_of_day::TimeFormat`], so the hour cycle and day-period labels govern both directions; a caller that needs "end after start" owns that rule, because a time of day is cyclic and this field wraps at midnight rather than clamping.
//! - [`time_picker`](mod@time_picker) - a time field with a list of times to pick from, and a combined date-and-time picker.
//! - [`slider`](mod@slider) - a custom, focusable slider primitive.
//! - [`range_slider`](mod@range_slider) - a two-thumb slider choosing a low and high value that never cross.
//! - [`checkbox`](mod@checkbox) - a styled checkbox component.
//! - [`toggle`](mod@toggle) - a styled toggle/switch component with an optional in-thumb icon.
//! - [`radio_button`](mod@radio_button) - single, standalone radio button.
//! - [`radio_group`](mod@radio_group) - a styled radio button group.
//! - [`key_capture_chip`](mod@key_capture_chip) - a controlled chip that records a keyboard chord. **Wiring:** `armed` is an **input** the host owns - activating the chip only publishes `KeyCapture::Started`, and until the caller feeds `.armed(true)` back the chip records nothing.
//! - [`editable_shortcuts`](mod@editable_shortcuts) - a SettingsPage-compatible editable shortcut pane over one authoritative resolved catalog, with capture, readable refusal/save feedback, per-action reset, and a pinned Reset all helper.
//! - [`form`](mod@form) - a label-aligned form that **co-measures** its labels into one shared label track (no caller pixel widths) and stacks label-over-field below a breakpoint; the measured counterpart to the spacing-only [`form_layout`](mod@form_layout).
//! - [`form_layout`](mod@form_layout) - spacing-only vertical stack of opaque form rows; each row owns its own label alignment (reach for [`form`](mod@form) when labels should line up).
//! - [`form_dialog`](mod@form_dialog) - a dynamic form presented over EnzModalHost.
//! - [`preferences_group`](mod@preferences_group) - settings group (title + description) card.
//! - [`preferences_row`](mod@preferences_row) - labelled settings row (Adw-style).
//!
//! ### Selection and pickers
//!
//! Choosing one value out of many, where the set is too large or too visual
//! for a radio group. Anything that opens a popover lives here rather than
//! under overlays, because the popover is an implementation detail of the
//! picker.
//!
//! - [`combo_box`](mod@combo_box) - enzUI-styled dropdown selector.
//! - [`autocomplete_field`](mod@autocomplete_field) - an editable combo box: a text field the user types into, with a filtered suggestion list underneath it; options can add a second line with [`AutocompleteOption::subtitle`](autocomplete_field::AutocompleteOption::subtitle) or a three-line result with [`AutocompleteOption::rich_result`](autocomplete_field::AutocompleteOption::rich_result).
//! - [`calendar`](mod@calendar) - the reusable month surface.
//! - [`canvas_view`](mod@canvas_view) - a canvas drawing published to screen readers as a labelled image.
//! - [`date_picker`](mod@date_picker) - a date field that opens a calendar popover.
//! - [`date_range_picker`](mod@date_range_picker) - a field that opens a calendar for a two-click range.
//! - [`color_swatch`](mod@color_swatch) - a clickable coloured square with selection border.
//! - [`color_swatch_grid`](mod@color_swatch_grid) - wrapping palette-inventory grid of coloured swatches with captions.
//! - [`color_picker`](mod@color_picker) - the continuous colour editor: saturation/brightness plane, hue strip, optional alpha, live swatch and hex field.
//! - [`workspace_accent_chooser`](mod@workspace_accent_chooser) - a named radio chooser with configurable columns for picking a workspace tint.
//! - [`command_palette`](mod@command_palette) - query input + live-filtered command list.
//! - [`suggestion_list`](mod@suggestion_list) - a standalone, keyboard-navigable suggestion list: the *list half* of [`autocomplete_field`](mod@autocomplete_field), for use with a multi-line [`text_area`](mod@text_area) or any custom input; [`SuggestionOption::subtitle`](suggestion_list::SuggestionOption::subtitle) provides a two-line option and [`SuggestionOption::rich_result`](suggestion_list::SuggestionOption::rich_result) adds protected trailing metadata plus a bounded preview without changing listbox semantics.
//!
//! ### Navigation and app shell
//!
//! The chrome around your content: what a window is made of before any of your
//! own widgets appear. Start at
//! [`navigation_split_view`](mod@navigation_split_view) - it is the adaptive
//! sidebar/content shell every reference app is built on - then add a
//! titlebar, header bar and status strip from this group rather than composing
//! rows by hand, because these carry the drag regions and window-control
//! semantics a client-side-decorated window needs.
//!
//! The shell splits in two: [`navigation_split_view`](mod@navigation_split_view)
//! renders, while [`nav_split`](mod@nav_split) owns the decision of *when* a
//! sidebar collapses to an overlay drawer. Reach for the widget to build a
//! layout; read the state module to understand or test the collapse rule.
//!
//! - [`app_shell`](mod@app_shell) - the layered application shell: five presets over a rounded background layer; docked sidebar motion continuously reflows retained content like a live window resize, while overlay drawers remain compositor-only.
//! - [`navigation_split_view`](mod@navigation_split_view) - the adaptive navigation shell.
//! - [`single_instance`](mod@single_instance) - one instance per user (`application-dbus`): `SingleInstance::start(app_id)` owns the application or forwards a later launch's files to it; the owner drains them with the task that raises its window.
//! - [`navigation_outlet`](mod@navigation_outlet) - projects the active route of a route path with a header, back navigation and a transition.
//! - [`view_switcher`](mod@view_switcher) - segmented tab switcher.
//! - [`inline_view_switcher`](mod@inline_view_switcher) - segmented view-switcher with tab semantics.
//! - [`tab_pill`](mod@tab_pill) - tab-strip primitives: one pill, and the list that joins them.
//! - [`document_tab`](mod@document_tab) - single tab cell for browser / editor tab strips.
//! - [`document_tabs`](mod@document_tabs) - the widget layer over the keyed document-tab model.
//! - [`breadcrumb`](mod@breadcrumb) - Navigation breadcrumbs with an editable mode whose idle/edit presentations keep one outer field rectangle and chrome.
//! - [`pagination`](mod@pagination) - numbered pagination with prev/next chrome and ellipsis collapse, built in O(visible cells) so a million-page pager renders the same handful of buttons. Cells are neutral (`Quiet`), not accent-inked. **Wiring:** pass a lazy `enz_button_with` message factory (`impl Fn(usize) -> Message`); the factory is invoked once per real activation and never at view-build time, so an M-page pager is not M closures deep.
//! - [`sidebar_content`](mod@sidebar_content) - a padded vertical column with sidebar-specific defaults.
//! - [`sidebar_row`](mod@sidebar_row) - sidebar list row. **Wiring:** a rail whose selection follows the current *route* wants `.reduced_motion(true)`, or the row being deselected replays its old highlight for a few frames after navigation.
//! - [`header_bar`](mod@header_bar) - top-of-window header bar with title + actions.
//! - [`page_header`](mod@page_header) - a large-title page header with optional actions.
//! - [`pane_header`](mod@pane_header) - the title-rung header between the section caption and the page hero: a left-aligned ~18 px title with an arbitrary trailing/leading element and an inline subtitle/count.
//! - [`pane_grid`](mod@pane_grid) - recursive user-resizable tiled panes for terminal splits and editor groups. Build the app's [`PaneTree`], keep [`EnzPaneGridState`] beside it, render with [`enz_pane_grid`], and feed [`EnzPaneGridEvent`] back through the state. This is not [`navigation_split_view`](mod@navigation_split_view): that is an adaptive sidebar + content shell that collapses to a drawer, while this preserves a recursive pane tree and exposes splitter/sash interaction.
//! - [`title_bar`](mod@title_bar) - the client-side-decoration titlebar: ordered slots, drag regions, per-window gestures. **Wiring:** the `.actions(..)` overflow slot does not own its menu state - pass `.overflow_open(..)`, use the actions message to toggle the trigger, and set [`.action_dismiss(..)`](title_bar::EnzTitleBar::action_dismiss) to a distinct idempotent close message for row activation, Escape, and outside press.
//! - [`titlebar_actions`](mod@titlebar_actions) - the standalone/custom-band preset of the shared measured action-overflow engine: progressive folding, standard toolbar-icon kebab and trailing placement. Inside [`title_bar::EnzTitleBar`](mod@title_bar), use its budgeted [`.actions(..)`](title_bar::EnzTitleBar::actions) slot instead. **Wiring:** pass the app's open state to `.overflow_open(..)`, use the toggle message for the trigger, and set `.on_dismiss(..)` to a distinct idempotent close message for activation, Escape, and outside press.
//! - [`titlebar_group`](mod@titlebar_group) - basic horizontal cluster of titlebar widgets.
//! - [`titlebar_icon_group`](mod@titlebar_icon_group) - the container for a custom titlebar icon cluster: grouping rhythm, centre alignment on a fixed band height, a variant fill/border and a rounded clip mask. Wrap [`toolbar_icon`](mod@toolbar_icon) children - and, outside [`title_bar::EnzTitleBar`](mod@title_bar), the chrome-free [`titlebar_actions`](mod@titlebar_actions) preset - in this.
//! - [`titlebar_tab`](mod@titlebar_tab) - a variable-width document-tab row with measured legibility folding and a complete-document overflow menu, plus the single-cell titlebar composition that binds [`document_tab`](mod@document_tab) to the neighbouring [`window_controls`](mod@window_controls) height and centreline.
//! - [`window_controls`](mod@window_controls) - the trailing close/minimize/maximize chip cluster of a titlebar, with caller-driven order, visibility and enabled state. [`title_bar`](mod@title_bar) builds it for you; reach for it directly only for window chrome outside that band.
//! - [`toolbar_row`](mod@toolbar_row) - start / center / end zone toolbar cluster row.
//! - [`toolbar_view`](mod@toolbar_view) - header bar + content region shell composite.
//! - [`toolbar_divider`](mod@toolbar_divider) - an inset vertical hairline for toolbar item groups.
//! - [`toolbar_spacer`](mod@toolbar_spacer) - a spacer between toolbar item groups.
//! - [`status_bar`](mod@status_bar) - bottom status strip.
//! - [`content_pane`](mod@content_pane) - full-bleed pane with padded vertical content layout.
//! - [`band_stack`](mod@band_stack) - transparent full-bleed vertical stack of self-styled edge-to-edge bands (toolbar + body + status), the opposite shape of `content_pane`.
//! - [`window_lifecycle`](mod@window_lifecycle) - **not a widget**: the `iced::Task` binding for the multi-window exit policy. A `daemon` - the only way to have two windows - does not stop when its last window closes, so closing the main window must produce an explicit `iced::exit()` or the process survives invisibly. Pair with its `WindowLedger`, and read [`app_shell`](mod@app_shell)'s `# A second window` section first.
//!
//! ### Layout and spacing
//!
//! Arrangement primitives with no visual of their own. They exist so spacing
//! comes from the token scale instead of magic numbers, and so the responsive
//! decisions ([`view_that_fits`](mod@view_that_fits),
//! [`responsive_grid`](mod@responsive_grid)) key off *component* width rather
//! than window width.
//!
//! - [`adaptive_row`](mod@adaptive_row) - horizontal row with configurable spacing.
//! - [`aspect_ratio`](mod@aspect_ratio) - a frame that keeps a fixed width-over-height ratio, deriving height from the resolved width.
//! - [`center`](mod@center) - thin axis-specific centring recipes (both axes, vertical only, horizontal only).
//! - [`clamp`](mod@clamp) - cap child width to a readable-width max, centre in the parent.
//! - [`flow`](mod@flow) - flex-wrap container.
//! - [`flow_item`](mod@flow_item) - fixed-height child for a flow / wrap layout.
//! - [`grid`](mod@grid) - fixed-column grid layout.
//! - [`responsive_grid`](mod@responsive_grid) - own-width responsive grid with size-class column ramp.
//! - [`responsive_grid_item`](mod@responsive_grid_item) - fixed-width, fixed-height cell host for a responsive grid.
//! - [`stack`](mod@stack) - tokenized vertical / horizontal stack primitives.
//! - [`spacer`](mod@spacer) - fill-both-axes invisible spacer.
//! - [`wrap`](mod@wrap) - model-driven, fixed-cell badge/chip inventory.
//! - [`cached_subtree`](mod@cached_subtree) - identity-backed retained subtree boundary: an unchanged pane is not rebuilt, not diffed and not laid out again.
//! - [`responsive`](mod@responsive) - cached width-aware container; runs its closure at most once per distinct width per frame.
//! - [`view_that_fits`](mod@view_that_fits) - renders the first candidate layout that fits, with the losers absent from the tree.
//! - [`overflow_row`](mod@overflow_row) - a horizontal bar with fixed leading + scrollable content band + pinned trailing slot.
//!
//! ### Containers, surfaces and scrolling
//!
//! Things that give content a background, a border, a radius or a viewport.
//! [`surface`](mod@surface) is the unopinionated base (visual only, no imposed
//! inner layout); [`card`](mod@card) is the common preset. If content can
//! exceed its region, wrap it in one of the scroll members rather than letting
//! it overflow - an unbounded child inside a bounded region is the most common
//! layout defect.
//!
//! - [`surface`](mod@surface) - tokenized visual surface with no imposed inner layout.
//! - [`desktop_glass`](mod@desktop_glass) - glass over the desktop and the windows behind the app, painted by the compositor.
//! - [`glass`](mod@glass) - puts any widget on liquid glass of its own size and radius; what sits inside defaults to glass.
//! - [`grain`](mod@grain) - lays a paper-like speckle (built-in noise or any SVG) on a surface, a shell pane or any element.
//! - [`adaptive_glass`](mod@adaptive_glass) - fills any shape, such as clock numerals or a pill, with liquid glass whose tone adapts to the backdrop.
//! - [`surface_stack`](mod@surface_stack) - a surface with a built-in vertical stack.
//! - [`svg_image`](mod@svg_image) - an SVG picture that can follow the theme's ink and has an accessible label.
//! - [`surface_hints`](mod@surface_hints) - tell the compositor a window's content type and whether it may tear.
//! - [`system_bell`](mod@system_bell) - ring the desktop's alert sound or flash for a window.
//! - [`tab_overview`](mod@tab_overview) - all open tabs as a grid of preview cards to pick, close or add one.
//! - [`hover_group`](mod@hover_group) - a container that tints its background while the pointer is anywhere inside it (draw-time group hover, composes with a nested row's own hover).
//! - [`activity_pulse`](mod@activity_pulse) - rate-limited pointer-activity reports for auto-hiding chrome; no layout, paint or semantics of its own.
//! - [`card`](mod@card) - a styled container/card surface.
//! - [`pane`](mod@pane) - content-clipping pane.
//! - [`group_box`](mod@group_box) - a titled sunken content grouping.
//! - [`expander`](mod@expander) - flat/card/plain disclosure section.
//! - [`disclosure`](mod@disclosure) - expand/collapse toggle row.
//! - [`divider`](mod@divider) - a styled divider/separator.
//! - [`icon_slot`](mod@icon_slot) - fixed-dimension bounding box that centres an icon.
//! - [`scroll_view`](mod@scroll_view) - general single-child scrollable region; all default and virtual surfaces inherit the tokenized minimum thumb length with exact endpoint mapping.
//! - [`scroll_column`](mod@scroll_column) - top-anchored scrollable vertical stack. `.complete_rows(row_height)` retains the parent's actual allocation while limiting paint/input to its largest whole-row inner viewport; `.free_form_viewport()` keeps the ordinary unquantized allocation, and neither policy snaps pixel scroll offsets.
//! - [`scroll_card`](mod@scroll_card) - flat scrolling surface with optional border.
//! - [`horizontal_scroll_view`](mod@horizontal_scroll_view) - fixed-height horizontal scrolling strip.
//! - [`offset_scrollbar`](mod@offset_scrollbar) - overlay scrollbar chrome painted over a child whose scroll offset the caller owns; layout-transparent, wheel reported not applied, thumb ink resolved against a caller-supplied backdrop.
//! - [`editor_scrollbar`](mod@editor_scrollbar) - [`enz_editor_scrollbar`] paints the same auto-hidden overlay thumb over a caller-owned, internally-scrolling `iced::widget::text_editor` whose scroll offset the *editor* owns. Reach for it when an app owns its own editor chrome - a distinct font-size ladder, a line-number gutter, a bespoke frame - so it cannot adopt [`text_area`](mod@text_area) without reshaping its text, yet still needs a scrollbar. It reads the editor's real `scroll_metrics()` and drives an absolute `Action::ScrollTo` through the caller's own reducer, so the editor stays the sole scroll owner; for the standard fully-chromed field use [`text_area`](mod@text_area), which builds this adapter for you, and when the *caller* owns the offset use [`offset_scrollbar`](mod@offset_scrollbar) instead.
//! - [`tail_follow`](mod@tail_follow) - opens a scroll region at the newest content and follows appends, releasing while the reader is scrolled up.
//!
//! ### Lists, tables and virtualisation
//!
//! Collections. The virtualised members
//! ([`virtual_list`](mod@virtual_list), [`virtual_grid`](mod@virtual_grid),
//! [`enz_table`](mod@table), [`chat_list`](mod@chat_list)) realise only
//! the visible window, so they are what you want past a few hundred rows; the
//! row widgets are the cells you put inside them.
//!
//! - [`table`](mod@table) - a bounded 2D virtualised data grid; `Length::Fill` realization uses the actual allocation, fixed-height bodies reserve only complete row slots, and `TableViewportState` publishes the effective viewport for update-time navigation. `.on_row_activate(..)` makes hover, press and selection one rounded row surface instead of one transient highlight per cell. It requires the default unfrozen layout because split panes cannot honestly expose one row action.
//! - [`table_header_cell`](mod@table_header_cell) - sortable column-header cell.
//! - [`table_text_cell`](mod@table_text_cell) - single-line text cell for a data table.
//! - [`virtual_list`](mod@virtual_list) - a scrolling list that realises only the visible items. Use `.self_scrolling(..)` to synchronize idle scrolling without application messages, or feed `.on_scroll(..)` back through `.scroll_offset(..)`; without either, realization stays at the supplied offset.
//! - [`virtual_grid`](mod@virtual_grid) - a model-backed virtual spatial grid for photo, icon and file-thumbnail collections. Use `.self_scrolling(..)` for message-free synchronization, or feed `.on_scroll(..)` back through `.scroll_offset(..)`; without either, realization stays at the supplied offset.
//! - [`file_browser`](mod@file_browser) - a host-owned listing with per-type icons and natural sort. [`file_browser::FileMetadataColumn`] adds fixed-width host cells to its responsive prefix; [`file_browser::EnzFileBrowser::header_radius`] rounds only the header.
//! - [`tree_view`](mod@tree_view) - a keyboard-navigable, scrolling tree of pre-flattened rows.
//! - [`tree_disclosure`](mod@tree_disclosure) - expand/collapse chevron for tree rows.
//! - [`list_row_surface`](mod@list_row_surface) - a focusable, keyboard-activatable row surface with hover / pressed / selected / disabled states. Its trailing action can be revealed on hover/focus, or on hover/focus/selection, without ever leaving the layout or the accessibility tree.
//! - [`preview_row`](mod@preview_row) - title + trailing + excerpt + footer list row.
//! - [`print_pdf`](mod@print_pdf) - lay out headings, paragraphs and images on pages and write them as a vector PDF for printing or export.
//! - [`keyed_column`](mod@keyed_column) - a column reconciled by key, so a row keeps its state when rows are inserted or removed above it. **Wiring:** `.animate_moves()` slides the rows a change moves, instead of jumping.
//! - [`reorder_list`](mod@reorder_list) - a keyed list with drag-to-reorder. **Wiring:** rows that are themselves clickable need `.drag_through_children(true)`, or the child captures every press and nothing is draggable.
//! - [`download_list`](mod@download_list) - the view over the core download model.
//! - [`shortcut_list`](mod@shortcut_list) - sectioned list of keyboard-shortcut rows.
//! - [`chat_list`](mod@chat_list) - provider-backed, variable-height virtual chat transcript. **Wiring:** the realised window follows `.scroll_offset(..)`, **not** the inner scrollbar, and this widget has no self-scrolling mode - plumb `.on_scroll(..)` back through `update` into `.scroll_offset(..)` or the same first window is realised forever (measured: messages 0..=9 at every offset). That is the *realisation window*; the **scroll position** is separate and needs no wiring - [`tail_follow`](mod@tail_follow) opens the list at the newest message and follows appended ones by default. **Variable heights:** a height table built from an estimator is a permanent guess - pass `.on_measure(..)` and feed the reported `RowMeasurement`s to `reconcile_measurements`, which corrects the table AND the scroll offset.
//! - [`row_measure`](mod@row_measure) - report a virtualised row's **intrinsic** height so a variable-height list can correct its guess. This is the producer `VariableHeightModel::apply_measurements` was waiting for; `iced::widget::Sensor::on_resize` reports the *imposed* slot instead, which in a virtual list is the estimate being corrected.
//! - [`chat_transcript`](mod@chat_transcript) - scrolling surface for chat bubbles.
//! - [`chat_bubble`](mod@chat_bubble) - a rounded chat message bubble.
//! - [`schedule`](mod@schedule) - the day/week time-grid surface.
//! - [`carousel`](mod@carousel) - accessible keyed paging with indicators, prev/next chrome, keyboard paging and optional swipe.
//!
//! ### Charts and data visualisation
//!
//! Canvas-backed plots plus the two pieces of shared chrome
//! ([`chart_legend`](mod@chart_legend),
//! [`chart_tooltip`](mod@chart_tooltip)) they all compose. Series colours come
//! from the theme palette, so a chart re-themes with the rest of the app.
//!
//! - [`area_chart`](mod@area_chart) - a Canvas-based Cartesian area chart host; it sits on the host surface with horizontal grid lines only, and `.plot_background(true)` / `.y_axis_line(true)` add a plot panel and the left vertical rule.
//! - [`bar_chart`](mod@bar_chart) - a Canvas-based vertical bar chart host.
//! - [`cartesian_chart`](mod@cartesian_chart) - the shared continuous-Cartesian host with line, area and point mark policies.
//! - [`line_chart`](mod@line_chart) - a Canvas-based Cartesian line chart host.
//! - [`pie_chart`](mod@pie_chart) - an accessible, bounded pie / donut chart.
//! - [`scatter_chart`](mod@scatter_chart) - a Canvas-based Cartesian scatter chart host.
//! - [`sparkline`](mod@sparkline) - a frameless, axis-less mini trend chart (sparkline).
//! - [`chart_legend`](mod@chart_legend) - a chart legend - a row of coloured swatch + label pairs.
//! - [`chart_tooltip`](mod@chart_tooltip) - a floating tooltip for chart data points.
//!
//! ### Media and imagery
//!
//! Pictures, video, audio and the surfaces drawn over them. The playback
//! widgets are driven by a *snapshot* of media state rather than owning a
//! decoder, so the same transport bar works over any backend - which is why a
//! music app should reach for [`media_controls`](mod@media_controls) instead
//! of rebuilding one.
//!
//! - [`image`](mod@image) - raster image with fit, radius, and placeholder background.
//! - [`animated_image`](mod@animated_image) - a GIF, APNG or animated WebP that plays, pausing when hidden and holding its first frame under reduced motion.
//! - [`animated_svg`](mod@animated_svg) - an SVG playing its SMIL and CSS animations, drawing and waking only when the picture changes.
//! - [`async_image`](mod@async_image) - renders the lifecycle of an async image request - loading skeleton, decoded image, or failure stamp.
//! - [`image_viewer`](mod@image_viewer) - an inspectable image region with a controlled transform.
//! - [`image_cropper`](mod@image_cropper) - a controlled crop selection over the image viewer.
//! - [`video`](mod@video) - a video surface plus transport, driven by a media snapshot and a decoded frame.
//! - [`media_controls`](mod@media_controls) - a transport-control bar driven by a media snapshot.
//! - [`avatar`](mod@avatar) - a styled avatar/user-icon component.
//! - [`icon`](mod@icon) - bundled Lucide icon widget.
//! - [`file_icon`](mod@file_icon) - a file's icon from the desktop icon theme, with the bundled glyph as fallback.
//! - [`qr`](mod@qr) - a themed QR code widget.
//! - [`annotation_overlay`](mod@annotation_overlay) - a reusable annotation surface over a picture - draw, preview, commit, hit-test.
//! - [`glyph_selection`](mod@glyph_selection) - text selection over a page raster's glyph boxes.
//! - [`marquee_area`](mod@marquee_area) - rubber-band drag selection over positioned content.
//!
//! ### Overlays, menus and dialogs
//!
//! Anything that renders *above* the page. [`modal_host`](mod@modal_host) is
//! the layer that owns modality - scrim, background inertness, Escape,
//! dismissal routing - and every dialog in this group is presented through it
//! rather than re-implementing those four things. Menus are the non-modal
//! half: they dismiss on outside click and do not make the page inert.
//!
//! - [`modal_host`](mod@modal_host) - the layer that owns modality and exit motion - scrim, background inertness, Escape, dismissal routing, and visual retention until settlement. **Several dialogs:** [`DialogStack<Key>`](modal_host::DialogStack) keeps an app's open dialogs and their focus - Escape closes the topmost, closing one closes those over it, `hand_off` replaces one with the next - and renders them over the page with every host level kept mounted. **Exit wiring:** store modal data in [`ModalPresentation`], keep rebuilding its retained value, pass `is_open()` to `EnzModalHost::presentation_open`, and route `on_exit_settled_with` back to `finish_exit(key)`. The exiting subtree is paint-only: no focus stop, dialog role, Escape capture or background inertness. **Focus wiring:** prefer the owned lifecycle [`ModalFocusLifecycle`] - `ModalFocusLifecycle::open(scope_key, invoker)` captures the invoker and returns the initial-focus operation, bind the host to it with `EnzModalHost::scoped_to(&lifecycle)` so the surface scope has one source and cannot drift from the traversal target, and `reduce(event)` answers every `ModalHostEvent` with framework-owned focus work: Tab traversal **scoped to this presentation** (never the unscoped app traversal that leaks focus behind the scrim), invoker restoration at *logical* dismissal (not after exit), and `close_for_host()` for a host-driven/model-unavailable close that must not steal focus. The low-level `initial_focus_operation(scope)` / `scoped_focus_operation(event, scope)` / `restore_focus_operation(event, invoker)` stay public as an escape hatch for a custom host; a host wired with none of them presents a dialog with focus nowhere (measured: zero focused nodes after build).
//! - [`modal_surface`](mod@modal_surface) - flat modal container surface.
//! - [`capped_dialog_body`](mod@capped_dialog_body) - pinned header/action chrome around one scrolling dialog body; derives the body cap from measured chrome.
//! - [`confirm_dialog`](mod@confirm_dialog) - the canonical "are you sure?" preset over EnzModalHost.
//! - [`save_changes_dialog`](mod@save_changes_dialog) - the three-action "Save changes before closing?" preset over EnzModalHost.
//! - [`popover`](mod@popover) - a floating panel attached to a control or a window-space rectangle, holding any content; dismissed by an outside press or Escape.
//! - [`about_dialog`](mod@about_dialog) - the application's About dialog: icon, name, version, developer, website and issue links, release notes, credits and legal text, localized.
//! - [`accordion`](mod@accordion) - sections of which at most one is open.
//! - [`shortcuts_dialog`](mod@shortcuts_dialog) - keyboard-shortcut help as fill-size content, a card surface, or a hosted modal.
//! - [`settings_dialog`](mod@settings_dialog) - the Settings preset over EnzModalHost: dialog → tabs → preference rows, arbitrary pane bodies.
//! - [`sheet`](mod@sheet) - [`SheetPresenter`] composes detents, signed drag, viewport/insets, accessible grabber, scroll handoff, and theme-motion settling; route its events through [`SheetPresenter::update`](crate::sheet::SheetPresenter::update). [`enz_sheet`] remains the static low-level surface presented over EnzModalHost.
//! - [`full_screen_overlay`](mod@full_screen_overlay) - dimming scrim + centred content + close button, hosted over EnzModalHost.
//! - [`menu`](mod@menu) - elevated menu-surface container.
//! - [`menu_row`](mod@menu_row) - a single row in a menu / dropdown popup.
//! - [`context_menu_host`](mod@context_menu_host) - turns a right-click invocation into a positioned, navigable, dismissible menu. [`ContextMenu<Target, Command>`](context_menu_host::ContextMenu) is the usual way in: open it on a row with typed [`ContextMenuItem<Command>`](context_menu_host::ContextMenuItem)s, mount it with `host(page, ..)`, and `update` hands back `(target, command)` with focus returned to the row.
//! - [`context_target`](mod@context_target) - right-click capture surface for context menus.
//! - [`crash_notice`](mod@crash_notice) - the start-up banner saying the app crashed last time and a report was saved.
//! - [`positioned_overlay`](mod@positioned_overlay) - places a surface at an app-supplied point, clamped to the viewport, reporting caused outside-press or Escape dismissal without making the page inert.
//! - [`tooltip`](mod@tooltip) - a styled tooltip wrapper.
//! - [`file_chooser`](mod@file_chooser) - the (`portal-dbus`-gated) seam to the XDG Desktop Portal file chooser, with an opt-in in-app fallback for hosts without one. **Wiring:** `FileRequest::start` only *opens* the request - the caller must also return [`completion_subscription`](file_chooser::completion_subscription) from `App::subscription` and call [`pump`](file_chooser::pump) when it wakes, or the chooser is presented and its outcome never arrives. That subscription is **event-driven and pending-gated**: it wakes once per portal event, so do not drive `pump` from `window::frames()` or an `iced::time::every` tick - a frame-sourced portal tick asks for work on every compositor frame while a dialog is open, which is the settled-redraw defect class.
//!
//! ### Feedback and progress
//!
//! Telling the user what happened or what is happening. The tone-carrying
//! members ([`banner`](mod@banner), [`toast`](mod@toast),
//! [`status_dot`](mod@status_dot)) publish accessibility live-region roles
//! keyed off tone, so an assistive technology announces a warning with the
//! right urgency without any extra wiring.
//!
//! - [`banner`](mod@banner) - inline status banner (title + body + tone + optional action + optional dismiss). The default leading glyph is the filled tone chip - the look every existing caller already ships. `EnzBanner::badge_style(BannerBadgeStyle::Bare)` opts into a tone-inked glyph on the banner's own surface, for a banner whose surroundings already carry the tone (for example, mounted through `EnzAppShell::notifications`). Durable actionable status belongs in `EnzAppShell::notifications` rather than hand-mounted in an app's content column.
//! - [`toast`](mod@toast) - transient toast surface (a single toast's chrome; the dismissal story lives in its host, [`toast_host`](mod@toast_host)).
//! - [`toast_host`](mod@toast_host) - the app-facing controller a toast is queued through: auto-dismiss timers, stacking, coalescing, the overflow/waiting budget, and the one live region that announces the message it is showing. You drive `ToastHost` from `update` the way you drive `file_chooser`; its element-producing entry point is [`enz_toast_layer`], which places the mounted host bottom-centre over the presentation without re-parenting it with an optional bottom inset for an app whose own chrome occupies that edge.
//! - [`notice_stack`](mod@notice_stack) - notices about the app's own condition that stay while their cause holds, as a column of toasts that speaks once.
//! - [`notification_inbox`](mod@notification_inbox) - the in-app list of past notifications, unread marked, dismissible, with Clear All.
//! - [`activity_surface`](mod@activity_surface) - the persistent operation-activity status: what the app is doing right now, contributed by every provider into one [`enzui_core::operation_activity::ActivityCenter`] and placed by the shell (`EnzAppShell::activity`) at the sidebar's bottom, as one compact status row by default, or, with the sidebar closed, bottom-left over the content as the detailed panel. Dimensionless fractional completion is shown and announced as an operation percentage; finite input clamps to 0–100%, non-finite input becomes indeterminate, homogeneous fractional operations aggregate with equal operation weight, and mixed units never fabricate a total. Its keyboard/AT-reachable `+N more` action receives an [`ActivityOverflowTarget`](enzui_core::operation_activity::ActivityOverflowTarget) derived from the actual hidden rows, so mixed providers cannot silently route to one provider's panel. A toast leaves; this stays while the work does.
//! - [`status_dot`](mod@status_dot) - compact semantic status indicator.
//! - [`status_page`](mod@status_page) - centred empty-state hero view.
//! - [`spinner`](mod@spinner) - rotating 8-dot indeterminate loader.
//! - [`skeleton`](mod@skeleton) - shimmering loading placeholder.
//! - [`progress_text`](mod@progress_text) - moving ink over one shaped, accessible status label.
//! - [`frame_clock`](mod@frame_clock) - wrap any element so it self-animates from frame timestamps, with no app-side tick and no per-frame `view()` rebuild.
//! - [`progress_bar`](mod@progress_bar) - a styled progress indicator.
//! - [`gauge`](mod@gauge) - a circular, token-driven gauge showing a value within a range.
//! - [`rating`](mod@rating) - a fractional star-rating **display** primitive (aggregate review score). For a histogram of rating counts use a `column!` of [`progress_bar`](mod@progress_bar); the row above records why.
//! - [`rating_input`](mod@rating_input) - a star rating the user sets; pressing the current star clears it.
//! - [`badge`](mod@badge) - a styled badge/tag/pill component.
//! - [`key_badge`](mod@key_badge) - a monospace keyboard-shortcut chip.
//!
//! ### Drag and drop
//!
//! Two hosts you wrap *around* a drag surface. Cross-list dragging and
//! edge-auto-scrolling are the parts that cannot live inside a single list
//! widget, which is why they are separate hosts rather than options on
//! [`reorder_list`](mod@reorder_list).
//!
//! - [`drag_drop_host`](mod@drag_drop_host) - the host that owns one drag session across collections, paints source and target feedback confined to the registering surface in an explicit overlay layer above child/sidebar content, and publishes the grab/grabbing pointer shape a deferred collection cannot.
//! - [`drag_item_surface`](mod@drag_item_surface) - bounded keyed virtual row/grid registration for onto-item and current-location drops.
//! - [`drag_autoscroll_host`](mod@drag_autoscroll_host) - wires auto-scrolling into a real drag path.
//! - [`file_drag`](mod@file_drag) - reusable native file and MIME-data sources for rows, thumbnails and attachments; stable ownership keys, bounded selections, pointer/touch thresholds, optional previews and destination completion callbacks.
//! - [`file_drop`](mod@file_drop) - MIME-aware drop regions with hover feedback and deferred completion; `enz_data_drop_target` receives caller-selected MIME data. Files dragged in from a file manager or another app: wrap a region in `enz_file_drop_target(content, on_drop)` and the dropped paths arrive as one message, through the native drag service `app_setup::run` installs. Text fields keep taking dropped text.
//! - [`zoom_wheel`](mod@zoom_wheel) - Ctrl+wheel zooms the wrapped view instead of scrolling it: one step per notch, touchpad deltas accumulated, at most one step per event.
//!
//! ### Decoration and effects
//!
//! Purely visual wrappers. [`elevation`](mod@elevation) is the one to prefer:
//! it takes a semantic 0–5 level and resolves the shadow from the theme, so
//! two widgets at the same level match. Hand-written `iced::Shadow` values do
//! not.
//!
//! - [`elevation`](mod@elevation) - semantic elevation wrapper with 0-5 shadow levels.
//! - [`glow`](mod@glow) - coloured glow (halo) around content.
//! - [`gradient_background`](mod@gradient_background) - configurable linear-gradient surface.
//!
//! # Export / alias / deprecation boundary
//!
//! This crate re-exports ~365 items (`pub mod` + `pub use`). Not all of them
//! are equally "supported API". This section defines the boundary so a consumer
//! can tell what is safe to depend on, what exists only for internal
//! composition, and what the removal path is.
//!
//! ## Three tiers
//!
//! 1. **Supported public API** - the `enz_*` constructor functions and their
//!    associated `Enz*` builder types, option enums (e.g. [`ButtonKind`],
//!    [`BadgeTone`]), and token structs (e.g. [`BannerTokens`]). These are the
//!    intended surface. Depend on the `enz_*` builder plus the types named in
//!    its `pub use` line.
//!
//!    Tier 1 is also enumerated by task in the § Widget catalog above.
//!
//!    **Import path.** Every tier-1 `Enz*` builder type is re-exported at the
//!    crate root alongside its `enz_*` constructor, so
//!    `enzui_widgets::EnzButton` and `enzui_widgets::button::EnzButton`
//!    both resolve and a caller never has to know which form a given widget
//!    supports.
//!
//! 2. **Internal-composition modules** - these `pub mod`s exist so widgets in
//!    this crate (and the probe app) can compose shared machinery, not because
//!    An application should call them directly. This list is the authoritative
//!    partition boundary: a module named here is internal-composition, a module
//!    named in tier 3 is audit/probe, and **every other `pub mod` is, by
//!    exclusion, tier-1 supported API**. Add a new internal-composition
//!    module here.
//!
//!    [`a11y_actions`](mod@a11y_actions),
//!    [`action_emphasis`](mod@action_emphasis),
//!    [`action_id`](mod@action_id),
//!    [`activation`](mod@activation),
//!    [`ambient_ink`](mod@ambient_ink),
//!    [`anchored_overlay`](mod@anchored_overlay),
//!    [`app_setup`](mod@app_setup),
//!    [`center_clamp_row`](mod@center_clamp_row),
//!    [`chart_geometry`](mod@chart_geometry),
//!    [`chart_palette`](mod@chart_palette),
//!    [`collapsed_region_gate`](mod@collapsed_region_gate),
//!    [`color`](mod@color),
//!    [`context_menu_action`](mod@context_menu_action),
//!    [`cross_fluid_guard`](mod@cross_fluid_guard),
//!    [`date_field`](mod@date_field),
//!    [`decorative`](mod@decorative),
//!    [`diagnostics_overlay`](mod@diagnostics_overlay),
//!    [`exclusive_focus`](mod@exclusive_focus),
//!    [`export_pool`](mod@export_pool),
//!    [`field_chrome`](mod@field_chrome),
//!    [`focus_button`](mod@focus_button),
//!    [`focus_retention`](mod@focus_retention),
//!    [`focus_return`](mod@focus_return),
//!    [`frame_routing`](mod@frame_routing),
//!    [`frame_routing_inventory`](mod@frame_routing_inventory),
//!    [`hairline`](mod@hairline),
//!    [`hover_visibility_gate`](mod@hover_visibility_gate),
//!    [`hug_width`](mod@hug_width),
//!    [`inert`](mod@inert),
//!    [`inert_clip`](mod@inert_clip),
//!    [`inherited_ink`](mod@inherited_ink),
//!    [`keyboard_capture`](mod@keyboard_capture),
//!    [`keyed_diff`](mod@keyed_diff),
//!    [`keyed_row`](mod@keyed_row),
//!    [`listbox_shape`](mod@listbox_shape),
//!    [`max_height`](mod@max_height),
//!    [`max_width`](mod@max_width),
//!    [`min_height`](mod@min_height),
//!    [`min_width`](mod@min_width),
//!    [`monospace_fallback`](mod@monospace_fallback),
//!    [`motion`](mod@motion),
//!    [`nav_split`](mod@nav_split),
//!    [`overlay_adapter`](mod@overlay_adapter),
//!    [`portal_parent`](mod@portal_parent),
//!    [`reflow_pair`](mod@reflow_pair),
//!    [`responsive_switch`](mod@responsive_switch),
//!    [`rich_text`](mod@rich_text),
//!    [`rounded_clip`](mod@rounded_clip),
//!    [`sans_serif_fallback`](mod@sans_serif_fallback),
//!    [`scrim_click`](mod@scrim_click),
//!    [`scroll_arrows`](mod@scroll_arrows),
//!    [`scroll_fade_edge`](mod@scroll_fade_edge),
//!    [`semantics_container`](mod@semantics_container),
//!    [`surface_interaction`](mod@surface_interaction),
//!    [`swallow_press`](mod@swallow_press),
//!    [`swipe`](mod@swipe),
//!    [`switch`](mod@switch),
//!    [`syntax`](mod@syntax),
//!    [`text_baseline`](mod@text_baseline),
//!    [`text_field_semantics`](mod@text_field_semantics),
//!    [`theme_lookup`](mod@theme_lookup),
//!    [`title_bar_gesture`](mod@title_bar_gesture),
//!    [`titlebar_drag_region`](mod@titlebar_drag_region),
//!    [`window_resize_frame`](mod@window_resize_frame),
//!    [`viewport_gate`](mod@viewport_gate),
//!    [`tokens`](mod@tokens),
//!    [`value_action`](mod@value_action),
//!    [`virtual_scroll`](mod@virtual_scroll).
//!
//!    They are `pub` because
//!    the probe binary and sibling widget modules need them across module
//!    boundaries, not as a compatibility promise. Treat their surface as
//!    unstable.
//!
//! 3. **Audit / probe modules** - this list is exhaustive.
//!
//!    [`accessibility_metrics`](mod@accessibility_metrics),
//!    [`accessibility_scan_widgets`](mod@accessibility_scan_widgets),
//!    [`bounds_probe`](mod@bounds_probe),
//!    [`layout_spill_probe`](mod@layout_spill_probe),
//!    [`color_presence`](mod@color_presence),
//!    [`icon_size_token_grid`](mod@icon_size_token_grid),
//!    [`radius_token_grid`](mod@radius_token_grid).
//!
//!    These exist to power the test/probe harness
//!    and A/B tooling. They are public for the probe app's benefit and are not
//!    part of the application-facing API.
//!
//! ## Aliases (two public paths to the same type)
//!
//! An alias is a real risk: a consumer may name a type by the "wrong" path and
//! a future rename then breaks them silently. The crate currently has **one**
//! true type alias:
//!
//! - [`ComboBoxSize`] is a `pub type` alias for [`TextFieldSize`]
//!   (`combo_box.rs`). Both names are re-exported, so `ComboBoxSize` and
//!   `TextFieldSize` are the *same* type reachable two ways. This is
//!   deliberate - combo fields and text fields pair one-to-one in size - but it
//!   means a rename of `TextFieldSize` must keep the `ComboBoxSize` alias.
//!   **Prefer [`TextFieldSize`]** as the canonical name; `ComboBoxSize` is the
//!   convenience path.
//!
//! Two other exports *look* like aliases but are the opposite - they are
//! distinct types that share a leaf name and are disambiguated by rename:
//! `ScrollAxis` (from `scroll_view::Axis`) and `SpacerAxis` (from
//! `spacer::Axis`) are **different** enums. Likewise `WINDOW_CONTROLS_DEFAULT_ORDER`
//! and the secure-field `SECURE_FIELD_PASTE_ID` / `SECURE_FIELD_SELECT_ALL_ID`
//! are prefixed re-exports of module-local constants, not shared identities.
//! These are safe: renaming one leaf cannot affect the other.
//!
//! ## Deprecation path
//!
//! When an item is superseded but must stay exported for source compatibility:
//!
//! - Mark it `#[deprecated(since = "<version>", note = "use <replacement>")]`
//!   **only when the replacement can be named.** A `#[deprecated]` with no
//!   replacement is noise a consumer cannot act on, so do not add one
//!   speculatively.
//! - The replacement must already exist and be public at the time of
//!   deprecation.
//! - Deprecated items are removed no sooner than one minor release after the
//!   `since` version.
//!
//! Currently deprecated in this crate (each names a replacement that exists
//! and is public):
//!
//! - [`RestoreTriggerFocus`](menu_button::RestoreTriggerFocus) - use
//!   [`enzui_core::focus::RestoreFocus`].
//! - [`disabled_enz_button`](button::disabled_enz_button) and
//!   [`inert_enz_button`](button::inert_enz_button) - prefix-first
//!   naming; use `enz_disabled_button` / `enz_inert_button`.
//! - [`revision_text`](revision_text::revision_text) - prefix-first
//!   naming; use `enz_revision_text`.
//! - [`EnzInputOTP`] - RFC 430 casing; use
//!   [`EnzInputOtp`].
//! - [`RICH_SUBJECT_MAX_SHARE`](suggestion_list::RICH_SUBJECT_MAX_SHARE) - the
//!   fixed-width subject/preview split was replaced by one wrapping paragraph;
//!   use [`SuggestionOption::rich_result`](suggestion_list::SuggestionOption::rich_result)
//!   or [`AutocompleteOption::rich_result`](autocomplete_field::AutocompleteOption::rich_result),
//!   which own that layout.
//!
//! The machine-readable list below names every deprecated item; the catalog
//! generator reads it to keep deprecated items off the page.
//!
//! <!-- deprecated-inventory:
//! menu_button::RestoreTriggerFocus
//! input_otp::EnzInputOTP
//! button::disabled_enz_button
//! button::inert_enz_button
//! revision_text::revision_text
//! suggestion_list::RICH_SUBJECT_MAX_SHARE
//! -->
//!
//! ## `docs/widgets.md` discoverability inventory
//!
//! Every `pub mod` appears in `docs/widgets.md` as a catalog entry, as a
//! `mod@` link in group prose or the keyword table, or in the exclusion
//! inventory below with a reason specific to that module
//! (`widgets_md_covers_every_pub_mod` fails otherwise). A module whose reason
//! turns out wrong moves to the keyword table or a catalog group.
//!
//! <!-- widgets-md-exclusion-inventory:
//! action_emphasis - shared `ActionEmphasis` vocabulary for a widget's optional CTA; consumed by `banner` and `status_page`, callers pass the variant to those widgets' emphasis setters.
//! action_id - shared "Action id → widget Id" derivation rule; a framework wiring detail, callers name real widget ids.
//! activation - shared activation-by-Id plumbing; widgets use it internally, apps go through the widget's own on_press/on_change hooks.
//! ambient_ink - shared ambient-ink precedence ladder for the markdown-class family; consumed by rich renderers, not authored against.
//! app_setup - the application and daemon entry point (`run`, `run_with_traversal`), not a widget: it wraps the caller's `iced::application(..)` value with the accessibility bootstrap, the widget-runtime scope and default Tab traversal, so nothing about it is placed in a view. The searchable owner is docs/core-capabilities.md, whose `application setup`, `automatic tab traversal` and `application bootstrap` rows name `enzui_widgets::app_setup::run` and route on to `enzui_core::focus_traversal` and `enzui_core::app_runtime`; a second entry on the widget catalog would answer that search with a widget the app can never place.
//! accessibility_metrics - test/probe harness surface; unstable, not app-facing.
//! accessibility_scan_widgets - test/probe harness surface; unstable, not app-facing.
//! layout_spill_probe - A/B layout-spill scan probe surface; unstable, not app-facing.
//! bounds_probe - A/B bounds probe surface; unstable, not app-facing.
//! chart_geometry - shared Cartesian plot geometry; chart widgets consume it, chart users reach for line_chart / bar_chart / etc.
//! chart_palette - shared categorical series palette for the chart family; consumed by chart widgets, apps get colours through the chart builder.
//! collapsed_region_gate - framework-level gate that walks a REGISTRY of shell shapes that collapse a structural region OUT of the accessibility tree (the sidebar drawer at the Narrow rung) and asserts each still publishes a named, id-carrying `aria-expanded=false` disclosure, plus the reverse for the revealed state. Callers never touch it; a new collapsing shell shape should add an entry so the disclosure state cannot be dropped silently for one shape.
//! color - low-level Color helpers (mix/darken/etc.); internal to token derivation.
//! cross_fluid_guard - flex cross-axis collapse guard; widgets that compose caller elements into a Row/Column consume it, apps never call it.
//! decorative - draw-child-without-a11y-publication wrapper; an ARIA-decorative primitive consumed by other widgets.
//! exclusive_focus - at-most-one-focus-across-stack-layers primitive; modal / overlay widgets consume it.
//! export_pool - bounded worker pool for portal parent-window exports; consumed by portal_parent, not a widget.
//! field_chrome - the bordered field surface wrapper; every field widget consumes it, apps configure the fields directly.
//! focus_button - shared focusable/keyboard-activatable button primitive; every clickable widget composes it, apps use button / clickable / pressable.
//! focus_retention - keep-a-text-field-focused-across-a-rebuild seam; `search_field` wraps itself in it, apps set `.retain_focus(..)` on the field instead of composing this.
//! focus_return - hand-focus-back-to-the-trigger seam for a transient surface that closes; `focus_button` and `menu` arm and reclaim it internally, apps name the trigger through `EnzMenu::focus_return_to` on the widget instead of composing this.
//! frame_routing - internal helpers for cached, targeted redraw delivery; custom widgets call these helpers instead of app code naming them.
//! frame_routing_inventory - test-facing ownership registry for custom widget frame routing; app code never consumes it.
//! hairline - shared thickness validation and snap policy for divider rules; apps configure the divider widgets.
//! hover_visibility_gate - manual RGB token-distance cases for calendar and key-capture-chip hover fills; rendered pixels and runtime backgrounds require separate checks.
//! hug_width - hug-to-preferred-width layout primitive; consumed by responsive containers.
//! icon_size_token_grid - icon-size token probe; A/B harness surface only.
//! inert - withdraw-from-input/focus/a11y wrapper; consumed inside widget internals.
//! inert_clip - internal clipped-input helper used by animated and overlay widgets; apps configure clipping through those widgets.
//! keyboard_capture - shared key-decode wrapper; text_area / text_field / form widgets consume it.
//! keyed_diff - shared key-indexed child reconciliation; consumed by keyed_column / keyed_row / carousel.
//! keyed_row - key-reconciled horizontal collection primitive; view_switcher / document_tabs consume it.
//! listbox_shape - shared a11y-shape builders for the three listbox-shaped widgets: `suggestion_list`, `autocomplete_field` and `command_palette` all produce their `ListBox` / `ListBoxOption` / empty-state `Status` nodes from this one module, so a fix to the shape lands in all three at once. Callers never touch it.
//! max_height - max-height cap wrapper; layout primitive.
//! max_width - max-width cap wrapper; layout primitive.
//! min_height - min-height floor wrapper; layout primitive.
//! min_width - min-width floor wrapper; layout primitive.
//! monospace_fallback - upright/monospace-face fallback for Font::MONOSPACE on Linux; process-wide setup, not a widget.
//! portal_parent - winit → portal parent-handle export; portal wiring, not a widget.
//! radius_token_grid - radius token probe; A/B harness surface only.
//! reflow_pair - two-child row/column-by-breakpoint primitive; responsive containers consume it.
//! responsive_switch - wide/compact arrangement selector; consumed inside widget internals.
//! rounded_clip - shared rounded-rect masking wrapper; consumed by container widgets.
//! sans_serif_fallback - installed-face fallback for Family::SansSerif at every framework weight; process-wide setup, not a widget.
//! scroll_arrows - scroll-arrow affordance primitive; tab_list / overflow_row consume it via composition.
//! scroll_fade_edge - edge fade indicator primitive; scroll containers consume it.
//! semantics_container - publish-grouping-a11y-node wrapper; consumed inside widget internals.
//! surface_interaction - shared interaction/hover/press helpers for pressable surfaces.
//! swallow_press - stop unhandled pointer press from falling through; overlay widgets consume it.
//! swipe - shared swipe-gesture policy; carousel / image_viewer consume it.
//! syntax - pure source tokenizer for code_block; consumed by code_block, apps set language on that widget.
//! text_field_semantics - wrapper that publishes text-field a11y at the input's own bounds/Id; consumed by every field widget.
//! theme_lookup - runtime theme → component-tokens lookup; consumed inside every widget's builder.
//! title_bar_gesture - WindowCommand vocabulary + per-window runtime binding; title_bar consumes it, apps configure through title_bar.
//! titlebar_drag_region - CSD window-move surface + press/double-click resolver; title_bar consumes it.
//! window_resize_frame - CSD window-resize surface: invisible edge/corner regions route a press to window::drag_resize; app_shell wraps its window boundary in it, apps never call it directly.
//! value_action - shared "drive numeric widget by Id" plumbing; stepper / slider / progress consume it.
//! viewport_gate - self-animating-widget viewport visibility gate; shimmer / spinner consume it.
//! -->
//!
#![cfg_attr(docsrs, feature(doc_cfg))]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::redundant_explicit_links)]
#![deny(rustdoc::private_intra_doc_links)]
#![deny(missing_docs)]

pub mod a11y_actions;
pub mod about_dialog;
pub mod accessibility_metrics;
pub mod accessibility_scan_widgets;
pub mod accordion;
/// `EnzActionBar` - the runtime consumer of
/// [`enzui_core::action_overflow`]. Folds whole commands into an
/// overflow menu instead of clipping them.
pub use enzui_core::localization;
pub use enzui_core::localized;
pub mod action_bar;
pub mod action_emphasis;
pub mod action_id;
pub mod activation;
pub mod activity_pulse;
pub mod activity_surface;
pub mod adaptive_glass;
pub mod adaptive_row;
pub mod ambient_ink;
pub mod anchored_overlay;
pub mod animated_image;
pub mod animated_svg;
/// `EnzAnnotationOverlay` - the reusable Canvas overlay host over a
/// picture: draws committed annotations, captures preview/commit/cancel
/// gestures, and hit-tests selection.
pub mod annotation_overlay;
pub mod app_setup;
/// `EnzAppShell` - the layered application shell: a background layer
/// owning the window boundary, with titlebar/sidebar/content riding on it as
/// separate rounded panes.
pub mod app_shell;
pub mod area_chart;
pub mod aspect_ratio;
pub mod async_image;
pub mod autocomplete_field;
pub mod avatar;
pub mod badge;
pub mod band_stack;
pub mod banner;
pub mod bar_chart;
pub mod block_list;
pub mod bounds_probe;
pub mod breadcrumb;
pub mod button;
pub mod button_group;
pub mod cached_subtree;
pub mod calendar;
pub mod canvas_view;
pub mod capped_dialog_body;
pub mod card;
pub mod carousel;
pub mod cartesian_chart;
pub mod center;
pub mod center_clamp_row;
pub mod chart_geometry;
pub mod chart_legend;
mod chart_motion;
pub mod chart_palette;
pub mod chart_tooltip;
pub mod chat_bubble;
pub mod chat_list;
pub mod chat_transcript;
mod check_mark;
pub mod checkbox;
pub mod chip_input;
pub mod clamp;
pub mod clickable;
pub mod code_block;
pub mod collapsed_region_gate;
pub mod color;
pub mod color_picker;
pub mod color_presence;
pub mod color_swatch;
pub mod color_swatch_grid;
pub mod combo_box;
pub mod command_palette;
pub mod composer;
pub mod confirm_dialog;
pub mod content_pane;
pub mod context_menu_action;
pub mod context_menu_host;
pub mod context_target;
pub mod crash_notice;
pub mod cross_fluid_guard;
pub mod date_field;
pub mod date_picker;
pub mod date_range_picker;
pub mod decorative;
pub mod desktop_glass;
pub mod diagnostics_overlay;
pub mod disclosure;
pub mod divider;
pub mod document_tab;
pub mod document_tabs;
pub mod download_list;
pub mod drag_autoscroll_host;
pub mod drag_drop_host;
pub mod drag_item_surface;
pub mod editable_shortcuts;
pub mod editor_scrollbar;
pub mod elevation;
pub mod exclusive_focus;
pub mod expander;
/// Bounded parent-export worker pool. Not a widget; owns the threads that
/// perform Wayland xdg-foreign export round trips for portal parenting.
#[cfg(target_os = "linux")]
pub mod export_pool;
pub mod field_chrome;
pub(crate) mod focus_claim;
mod live_area_chart;
// Internal, non-`pub` composition helper: the field family's stable-identity
// vertical stack (label / stateful input / captions reconciled by logical key,
// not by position). Visible crate-wide via `crate::field_column`; not public
// API, so it carries no tier/catalog entry. See the module docs.
/// Measured dismissal support for transient in-view editors (shared
/// focus/dismiss wrapper). Kept internal: the supported surface is the
/// breadcrumb's own [`breadcrumb::EditBounds`] / [`breadcrumb::track_edit_bounds`]
/// / [`breadcrumb::editable_focus_scope`] re-exports, so callers compose the
/// dismissal through the editable-breadcrumb interaction rather than a raw
/// primitive.
mod caps_lock_hint;
mod edit_menu;
#[cfg(feature = "emoji-picker")]
pub mod emoji_picker;
mod field_column;
/// XDG Desktop Portal **file chooser** surface. Gated on the
/// `portal-dbus` feature; re-exports the `enzui_core::portal` types an app
/// needs and adds a `pump` helper turning a `PendingRequest` into an
/// `iced::Task`. Not a widget - see the module docs for why the portal is not
/// one, and for the `App`-owned polling shape an app has to use.
pub mod file_browser;
pub mod file_chooser;
pub mod file_drag;
pub mod file_drop;
pub mod file_icon;
pub mod find_replace_bar;
pub mod find_surface;
pub mod flow;
pub mod flow_item;
pub mod focus_button;
mod focus_dismiss;
/// Keep a text field focused (and its caret exact) across a rebuild
/// that discards its subtree, without needing a surviving ancestor.
pub mod focus_retention;
pub mod focus_return;
pub mod font_chooser;
pub mod form;
pub mod form_dialog;
pub mod form_layout;
/// `EnzFrameClock` - wrap any element so it self-animates from frame
/// timestamps, with no app-side tick and no per-frame `view()` rebuild.
pub mod frame_clock;
pub mod frame_routing;
#[doc(hidden)]
pub mod frame_routing_inventory;
pub mod full_screen_overlay;
pub mod gauge;
pub mod glass;
pub mod glow;
pub mod glyph_selection;
pub mod gradient_background;
pub mod grain;
pub mod grid;
pub mod group_box;
pub mod hairline;
pub mod header_bar;
pub mod horizontal_scroll_view;
pub(crate) mod hover_control;
pub mod hover_group;
pub mod hover_visibility_gate;
pub mod hug_width;
pub mod icon;
/// The shared round icon-only action leaf. Crate-internal engine: the
/// public surface is [`toolbar_icon`], [`sidebar_toggle_button`] and
/// [`window_control_button`], which are contextual presets over it.
pub(crate) mod icon_button;
pub mod icon_label;
pub mod icon_size_token_grid;
pub mod icon_slot;
pub mod image;
pub mod image_cropper;
pub mod image_viewer;
pub mod inert;
#[doc(hidden)]
pub mod inert_clip;
pub mod inherited_ink;
pub mod inline_view_switcher;
pub mod input_otp;
mod item_strip;
pub mod key_badge;
pub mod key_capture_chip;
pub mod keyboard_capture;
pub mod keyed_column;
pub mod keyed_diff;
pub mod keyed_row;
pub mod label;
pub mod labeled_content;
mod lane_loop;
mod layout_memo;
pub mod layout_spill_probe;
pub mod line_chart;
pub mod line_number_gutter;
pub mod link;
pub mod list_row_surface;
pub mod listbox_shape;
pub mod markdown;
pub mod marquee_area;
pub mod match_ink;
pub mod progress_text;
pub mod text_drag;
// The material paint boundary EnzSurface::material composes. Internal: a
// caller reaches the capability through the surface, not through a second
// wrapper widget.
mod material_boundary;
pub mod max_height;
pub mod max_width;
pub mod media_controls;
pub mod menu;
pub mod menu_bar;
pub mod menu_button;
pub mod menu_row;
pub mod min_height;
pub mod min_width;
pub mod modal_host;
pub mod modal_surface;
pub mod monospace_fallback;
pub mod motion;
pub mod nav_split;
pub mod navigation_outlet;
pub mod navigation_split_view;
pub mod notice_stack;
pub mod notification_inbox;
mod opacity_pulse;
pub(crate) mod optical_centre;
pub mod overflow_row;
pub mod overlay_adapter;
pub mod page_header;
pub mod pagination;
pub mod pane;
pub mod pane_grid;
pub mod pane_header;
pub mod pie_chart;
pub mod popover;
/// Winit → XDG Desktop Portal parent-handle export. Not a widget;
/// the platform-crate half of portal window parenting - see the module docs.
pub mod portal_parent;
pub mod positioned_overlay;
pub mod qr;
pub mod responsive;
pub(crate) mod surface_edge;
pub mod table;
pub mod text;

/// The `portal-dbus` feature must be **off by default**.
///
/// `DbusBackend::new` spawns a `tokio::runtime::Runtime`, so a default-on
/// portal dependency would put a background runtime into every app that just
/// links this crate - and would make `cargo test` require a session bus. This
/// property rots silently (an unrelated `[features]` edit could flip it), so
/// it is pinned here as a compile-time assertion that fires under the
/// **default** feature set.
///
/// Sabotage: set `default = ["portal-dbus"]` in `Cargo.toml` and the
/// `cfg(feature = "portal-dbus")` arm below turns on, so the `_: () = ()` in
/// the *other* arm never compiles - which is the point: a default-on
/// `portal-dbus` should not be a normal build state.
#[cfg(all(test, not(feature = "portal-dbus")))]
const _PORTAL_DBUS_IS_OFF_BY_DEFAULT: () = ();
pub mod offset_scrollbar;
pub mod paged_text_viewer;
pub mod preferences_group;
pub mod preferences_row;
pub mod pressable;
pub mod preview_row;
pub mod print_pdf;
pub mod progress_bar;
pub mod radio_button;
pub mod radio_group;
pub mod radius_token_grid;
pub mod range_slider;
pub mod rating;
pub mod rating_input;
pub mod reading_pane;
pub mod reflow_pair;
pub mod reorder_list;
pub mod responsive_grid;
pub mod responsive_grid_item;
pub mod responsive_switch;
mod reveal_width;
pub mod revision_text;
pub mod rich_edit;
pub mod rich_text;
pub mod rounded_clip;
pub mod row_measure;
pub(crate) mod row_motion;
pub mod sans_serif_fallback;
pub mod save_changes_dialog;
pub mod scatter_chart;
pub mod schedule;
pub mod scrim_click;
pub mod scroll_arrows;
pub mod scroll_card;
pub mod scroll_column;
pub mod scroll_fade_edge;
pub mod scroll_view;
// Population gate over every raw `scrollable(..)` construction in
// this crate. Test-only: the module exists to run the gate; it exposes no
// runtime surface.
#[cfg(test)]
mod scrollable_population;
mod scrollbar_drag;
pub mod search_field;
pub mod section_header;
pub mod secure_field;
pub mod selection_scope;
pub mod semantics_container;
pub mod settings_dialog;
pub mod sheet;
pub mod sparkline;
mod wheel_capture;

pub mod mnemonic;
pub mod shortcut_list;
pub mod shortcuts_dialog;
pub mod sidebar_content;
pub mod sidebar_row;
pub mod sidebar_toggle_button;
#[cfg(feature = "application-dbus")]
#[cfg_attr(docsrs, doc(cfg(feature = "application-dbus")))]
pub mod single_instance;
pub mod skeleton;
pub mod slider;
pub mod spacer;
mod spell_underline;
pub mod spinner;
pub mod stack;
pub mod status_bar;
pub mod status_dot;
pub mod status_page;
pub mod stepper;
pub mod streaming_text;
pub mod suggestion_list;
pub mod surface;
pub mod surface_hints;
pub mod surface_interaction;
pub mod surface_stack;
pub mod svg_image;
pub mod swallow_press;
pub mod swipe;
pub mod switch;
pub mod syntax;
pub mod system_bell;
pub mod tab_overview;
pub mod tab_pill;
pub mod table_header_cell;
pub mod table_text_cell;
pub mod tail_follow;
pub mod text_area;
pub mod text_baseline;
pub mod text_field;
pub mod text_field_semantics;
pub mod text_float;
pub mod text_grid;
pub mod text_measure;
pub mod text_pair;
pub mod theme_lookup;
pub mod time_field;
pub mod time_picker;
/// The CSD titlebar composition - ordered slots, drag regions,
/// per-window gestures and a labelled semantic owner.
pub mod title_bar;
/// The window-chrome intent vocabulary and its mapping onto the
/// vendored per-window runtime tasks.
pub mod title_bar_gesture;
/// The titlebar preset of the shared measured action-overflow
/// engine: progressive folding, a standard toolbar-icon kebab, and trailing
/// collector placement.
pub mod titlebar_actions;
pub mod titlebar_drag_region;
pub mod titlebar_group;
pub mod titlebar_icon_group;
/// Variable-width titlebar document rows with measured legibility folding and
/// a complete-document overflow menu, plus the single-cell height/centreline
/// composition shared with neighbouring [`window_controls`].
pub mod titlebar_tab;
pub mod toast;
pub mod toast_host;
pub mod toggle;
pub mod tokens;
pub mod toolbar_divider;
pub mod toolbar_icon;
pub mod toolbar_row;
pub mod toolbar_spacer;
pub mod toolbar_view;
pub mod tooltip;
pub mod tree_disclosure;
pub mod tree_view;
pub mod value_action;
pub mod video;
pub mod view_switcher;
pub mod view_that_fits;
pub mod viewport_gate;
pub mod virtual_grid;
pub mod virtual_list;
/// The keyed realisation core shared by `virtual_list` and `chat_list`.
/// Internal - it builds no surface a caller could construct.
pub(crate) mod virtual_rows;
pub mod virtual_scroll;
pub mod virtual_text_surface;
pub mod window_control_button;
pub mod window_controls;
pub(crate) mod window_frame_clip;
/// The `iced::Task` binding for the multi-window exit policy - the
/// counterpart to `enzui_core::window_lifecycle`'s decision half. Its
/// `WindowLedger` decides when a daemon must exit; `lifecycle_task` issues it.
pub mod window_lifecycle;
/// The CSD window-resize surface - invisible edge/corner hit regions the
/// shell wraps its window boundary in, routing presses to `window::drag_resize`.
pub mod window_resize_frame;
pub mod window_title;
pub mod wizard;
pub mod workspace_accent_chooser;
pub mod wrap;
pub mod zoom_wheel;

// Compatibility module-path aliases. The canonical component module is
// the unprefixed concept name (`text`, `responsive`, `table`, `qr`, `image`);
// these brand-prefixed / `_widget`-suffixed spellings are the historical paths
// and keep resolving for existing callers (`enz_text::EnzText`,
// `image_widget::enz_image`, ...). They are module re-exports, not `pub mod`
// declarations, so they are NOT tier-1 catalog modules and never acquire a
// second catalog entry or a duplicate type identity. Rust does not carry a
// `#[deprecated]` warning across a bare module re-export, so the warning lives
// on the individual constructor/type spellings where the language honours it;
// the compile-compatibility fixture exercises every path here.
pub use crate::{
    image as image_widget, qr as enz_qr, responsive as enz_responsive, table as enz_table,
    text as enz_text,
};

pub use accessibility_metrics::{
    ContrastMode, ContrastProbe, DEFAULT_MIN_CONTRAST_RATIO, DEFAULT_MIN_CONTRAST_RATIO_LARGE,
    DEFAULT_MIN_HIT_TARGET, HIT_TARGET_EPSILON, HitTargetProbe, SRGB_LINEAR_BREAKPOINT,
    channel_linear, composite_over, contrast_probe, contrast_probe_with, contrast_ratio,
    contrast_ratio_over, hit_target_epsilon_for_scale, hit_target_probe, luminance,
};
pub use accessibility_scan_widgets::{
    ContrastResolver, ContrastScanProbe, ElisionLedger, ElisionScanProbe, HitTargetScanProbe,
    contrast_deficit, contrast_scan_probe, elision_clamp, elision_scan_probe, hit_target_deficit,
    hit_target_scan_probe,
};
pub use action_bar::{
    Collector, CollectorPlacement, EnzActionBar, FoldPolicy, enz_action_bar,
    overflow_focus_restore_operation,
};
pub use action_emphasis::ActionEmphasis;
pub use activity_pulse::{EnzActivityPulse, enz_activity_pulse};
pub use activity_surface::{ActivityPresentation, EnzActivitySurface, enz_activity_surface};
pub use adaptive_row::{
    DEFAULT_ADAPTIVE_ROW_SPACING, DEFAULT_ADAPTIVE_ROW_SPACING_TOKEN, EnzAdaptiveRow,
    EnzAdaptiveRowWidget, enz_adaptive_row,
};
pub use animated_image::{
    AnimatedFrames, AnimatedImageError, EnzAnimatedImage, enz_animated_image,
};
pub use animated_svg::{AnimatedSvg, AnimatedSvgError, EnzAnimatedSvg, enz_animated_svg};
pub use annotation_overlay::{EnzAnnotationOverlay, enz_annotation_overlay};
pub use app_shell::{
    DEFAULT_PAGE_INSET, EnzAppShell, NarrowSplit, PAGE_INSET_TOKEN, PagePresentation, PaneStyle,
    ShellGeometry, ShellLayout, ShellTokens, ShellWindowEvent, ShellWindowIdentity,
    ShellWindowState, SidebarHeader, SplitAxis, enz_app_shell, enz_app_shell_sized,
    enz_app_shell_with_geometry,
};
pub use area_chart::{EnzAreaChart, enz_area_chart};
pub use aspect_ratio::{EnzAspectRatio, enz_aspect_ratio};
pub use async_image::{EnzAsyncImageView, enz_async_image_view};
pub use autocomplete_field::{EnzAutocompleteField, enz_autocomplete_field};
pub use avatar::{AvatarShape, EnzAvatar, Presence, enz_avatar};
pub use badge::{BadgeSize, BadgeSizeTokens, BadgeTone, EnzBadge, enz_badge};
pub use band_stack::{EnzBandStack, enz_band_stack};
pub use banner::{
    BannerActionEmphasis, BannerBadgeStyle, BannerTokens, BannerTone, EnzBanner, enz_banner,
};
pub use bar_chart::{EnzBarChart, enz_bar_chart};
pub use block_list::{EnzBlock, EnzBlockFlow, EnzBlockKind, EnzBlockList, enz_block_list};
pub use bounds_probe::{BoundsProbe, SPILL_EPSILON, bounds_probe, spill_epsilon_for_scale};
pub use breadcrumb::{
    BreadcrumbEditEvent, EnzBreadcrumb, edit_focus_operation as breadcrumb_edit_focus_operation,
    enz_breadcrumb,
};
pub use button::{
    ButtonKind, ButtonSize, EnzButton, enz_button, enz_disabled_button, enz_filter_chip,
    enz_inert_button,
};
pub use button_group::{ButtonGroupStyle, EnzButtonGroup, EnzButtonGroupItem, enz_button_group};
pub use cached_subtree::{
    EnzCachedSubtree, SubtreeCacheStats, SubtreeHandle, SubtreeId, enz_cached_subtree,
};
pub use calendar::{EnzCalendar, enz_calendar};
pub use capped_dialog_body::enz_capped_dialog_body;
pub use card::{CardRhythm, CardTone, EnzCard, enz_card};
pub use carousel::{EnzCarousel, body_slot_height, enz_carousel};
pub use cartesian_chart::{EnzCartesianChart, enz_cartesian_chart};
pub use center::{enz_center, enz_hcenter, enz_vcenter};
pub use chart_legend::{ChartLegendEntry, EnzChartLegend, enz_chart_legend};
pub use layout_spill_probe::{LayoutSpillProbe, layout_spill_probe, spill_amount};
// finding 10 - the typed-row API is the **primary** facade export.
// `enz_chart_tooltip_rows` pairs each entry with its value in a
// [`chart_tooltip::ChartTooltipRow`], which prevents the missing/discarded-value
// class of drift the parallel-vector constructor allows. The typed row struct
// itself is re-exported so callers can construct rows at the facade without
// reaching into the module. `enz_chart_tooltip` (the parallel-vector
// constructor) stays in the facade for source compatibility with existing
// callers, but the docs on the module direct new code to the typed
// constructor.
pub use chart_tooltip::{
    ChartTooltipRow, EnzChartTooltip, enz_chart_tooltip, enz_chart_tooltip_rows,
};
pub use chat_bubble::{ChatRole, EnzChatBubble, enz_chat_bubble};
pub use chat_list::{
    ChatAnchor, EnzChatList, RevealAlignment, chat_list_id, enz_chat_list, reveal_offset,
};
pub use chat_transcript::{
    DEFAULT_BUBBLE_SPACING, DEFAULT_MAX_CONTENT_WIDTH, DEFAULT_TRANSCRIPT_PADDING,
    EnzChatTranscript, enz_chat_transcript,
};
pub use checkbox::{CheckboxSize, EnzCheckbox, enz_checkbox};
pub use chip_input::{ChipInputEvent, EnzChipInput, apply_chip_event, enz_chip_input};
pub use clamp::{ClampAlign, ClampSize, EnzClamp, enz_clamp};
pub use clickable::{
    ClickableStatus, ClickableTokens, EnzClickable, enz_clickable, enz_clickable_maybe,
    enz_clickable_with_status,
};
pub use code_block::{CodeBlockScroll, EnzCodeBlock, enz_code_block};
pub use color_picker::{EnzColorPicker, Hsva, enz_color_picker};
pub use color_swatch::{
    ColorSwatchTokens, EnzColorSwatch, enz_color_swatch, enz_color_swatch_none,
};
pub use color_swatch_grid::{
    EnzColorSwatchGrid, EnzColorSwatchView, SwatchFill, enz_color_swatch_grid,
};
pub use combo_box::{ComboBoxSize, EnzComboBox, enz_combo_box};
pub use command_palette::{EnzCommandPalette, enz_command_palette};
pub use composer::{EnzComposer, composer_height_for_rows, composer_row_height, enz_composer};
pub use confirm_dialog::{EnzConfirmDialog, confirm_dialog_host, enz_confirm_dialog};
pub use content_pane::{
    ContentPaneTokens, DEFAULT_CONTENT_PADDING, DEFAULT_CONTENT_SPACING, EnzContentPane,
    enz_content_pane,
};
pub use context_menu_host::{
    ContextMenu, ContextMenuEvent, ContextMenuItem, ContextMenuState, EnzContextMenuHost,
    enz_context_menu_host, enz_optional_point_context_menu_host, enz_point_context_menu_host,
};
pub use context_target::{EnzContextTarget, enz_context_target};
pub use date_picker::{EnzDatePicker, enz_date_picker};
pub use date_range_picker::{EnzDateRangePicker, enz_date_range_picker};
pub use disclosure::{EnzDisclosure, enz_disclosure};
pub use divider::{DividerOrientation, EnzDivider, enz_divider};
pub use document_tab::{
    DEFAULT_CONTENT_PADDING_X as DOCUMENT_TAB_DEFAULT_CONTENT_PADDING_X,
    DEFAULT_TITLE_SIZE as DOCUMENT_TAB_DEFAULT_TITLE_SIZE, EnzDocumentTab, enz_document_tab,
};
pub use document_tabs::{EnzDocumentTabs, enz_document_tabs};
pub use download_list::{
    DownloadRow, DownloadRowProvider, EnzDownloadList, enz_download_list,
    enz_download_list_provider,
};
pub use drag_autoscroll_host::{EnzDragAutoScrollHost, enz_drag_autoscroll_host};
pub use drag_drop_host::{EnzDragDropHost, enz_drag_drop_host};
pub use drag_item_surface::{DragSurfaceItem, EnzDragItemSurface, enz_drag_item_surface};
pub use editor_scrollbar::{EnzEditorScrollbar, enz_editor_scrollbar};
pub use elevation::{
    ElevationShadow, EnzElevation, elevation_shadow_color, enz_elevation, iced_shadow_for_level,
    iced_shadow_for_theme, iced_shadow_from, iced_shadow_transition,
    iced_shadow_transition_for_theme, iced_shadow_transition_from, shadow_shape, shadow_shape_from,
};
pub use expander::{EnzExpander, ExpanderTokens, ExpanderVariant, enz_expander};
pub use file_browser::{
    EnzFileBrowser, FileBrowserView, FileColumn, FileMetadataColumn, enz_file_browser, format_bytes,
};
pub use file_icon::{EnzFileIcon, FileIconQuery, enz_file_icon};
pub use find_replace_bar::{
    EnzFindReplaceBar, FindReplaceEvent, ReplacementFieldChrome, enz_find_replace_bar,
};
pub use find_surface::{EnzFindSurface, FindSurfaceEvent, FindSurfaceState, enz_find_surface};
pub use flow::{EnzFlow, FlowDirection, enz_flow};
pub use flow_item::{EnzFlowItem, enz_flow_item};
pub use focus_dismiss::{DismissBounds as LayoutBounds, track_bounds};
pub use form::{EnzForm, FormRow, FormWrap, enz_form, form_field, form_multi_field, form_row};
pub use form_dialog::{
    EnzFormDialog, FormDialogEvent, MultilineBridge, enz_form_dialog, field_group_semantics,
    field_semantics, form_dialog_host, form_semantics,
};
pub use form_layout::{DEFAULT_FORM_PADDING, DEFAULT_ROW_SPACING, EnzFormLayout, enz_form_layout};
pub use frame_clock::{EnzFrameClock, FrameClockPhase, enz_frame_clock, frame_clock_phase};
pub use full_screen_overlay::{EnzFullScreenOverlay, enz_full_screen_overlay};
pub use progress_text::{EnzProgressText, enz_progress_text};
pub use qr::{EnzQr, EnzQrData, EnzQrError, enz_qr};
pub use responsive::{EnzResponsive, enz_responsive};
pub use table::{
    CellCoord, ColumnFold, EnzTable, FoldCandidate, FoldPlan, FoldedField, InlineEdit,
    RealizationInfo, SortCommand, TableViewport, TableViewportState, enz_table, plan_columns,
    priority_order,
};
pub use text::{EnzText, TextAlign, TextShaping, TextSize, TextTone, enz_text};
// `Sampled` is the framework's answer to "no measurement yet", re-exported
// at the crate root because app code reaches for it far from `gauge`.
pub use gauge::{
    EnzGauge, GaugeStyle, GaugeTokens, GaugeTone, Sampled, enz_gauge, enz_gauge_sampled,
};
pub use glow::{DEFAULT_GLOW_INTENSITY, DEFAULT_GLOW_SPREAD, EnzGlow, GlowTokens, enz_glow};
pub use gradient_background::{
    DEFAULT_GRADIENT_ANGLE_DEGREES, END_DARKEN_FACTOR, EnzGradientBackground, MID_DARKEN_FACTOR,
    enz_gradient_background,
};
pub use grid::{EnzGrid, enz_grid};
pub use group_box::{EnzGroupBox, enz_group_box};
pub use header_bar::{EnzHeaderBar, HeaderAction, HeaderBarStyle, HeaderBarTokens, enz_header_bar};
pub use horizontal_scroll_view::{
    DEFAULT_STRIP_HEIGHT, EnzHorizontalScrollView, enz_horizontal_scroll_view, strip_overflow,
};
pub use hover_group::{EnzHoverGroup, HoverGroupStyle, enz_hover_group, enz_hover_group_with};
pub use icon::{
    ButtonFgRole, DEFAULT_ICON_SIZE, EnzIcon, ICON_LG, ICON_MD, ICON_SM, ICON_XL, ICON_XXL,
    IconSource, IconTone, enz_icon, enz_icon_from_handle, enz_icon_from_image,
};
pub use icon_label::{
    EnzIconLabel, IconLabelA11y, IconLabelDisplay, IconLabelTone, enz_icon_label,
};
pub use icon_size_token_grid::{
    EnzIconSizeTokenGrid, EnzIconSizeTokenView, enz_icon_size_token_grid,
};
pub use icon_slot::{EnzIconSlot, enz_icon_slot};
pub use image::{EnzImage, EnzImageFit, enz_image, enz_image_handle};
pub use image_cropper::{
    CropEvent, CropPreview, CropperSourceError, EnzImageCropper, ImageCropperTokens,
    enz_image_cropper,
};
pub use image_viewer::{EnzImageViewer, enz_image_viewer};
pub use inline_view_switcher::{
    EnzInlineViewSwitcher, InlineSwitcherPage, enz_inline_view_switcher,
    enz_inline_view_switcher_unwired,
};
pub use input_otp::{EnzInputOtp, InputOtpTokens, enz_input_otp, otp_cells};
// `EnzInputOTP` is renamed to `EnzInputOtp` for RFC 430 casing. The
// old spelling stays reachable at the crate root as a `#[deprecated] pub use`
// so source that imported `enzui_widgets::EnzInputOTP` keeps compiling.
pub use editable_shortcuts::{
    EditableShortcutsEvent, EnzEditableShortcuts, editable_shortcuts_reset_all_action,
    enz_editable_shortcuts, shortcuts_settings_page,
};
pub use glyph_selection::{EnzGlyphSelection, enz_glyph_selection};
#[allow(deprecated, reason = "re-exports a deprecated alias for compatibility")]
pub use input_otp::EnzInputOTP;
pub use key_badge::{EnzKeyBadge, enz_key_badge};
pub use key_capture_chip::{EnzKeyCaptureChip, KeyCapture, enz_key_capture_chip};
pub use label::{
    EnzLabel, IconFrame, LabelA11y, LabelDisplay, LabelPolicy, LabelStyle, LabelTone, enz_label,
};
pub use labeled_content::{
    EnzLabeledContent, LabeledContentLayout, enz_labeled_content, enz_labeled_content_compact,
};
pub use line_chart::{EnzLineChart, enz_line_chart};
pub use line_number_gutter::{
    GutterConfig, GutterRow, enz_line_number_gutter, gutter_rows, gutter_width,
};
pub use link::{
    DEFAULT_EXTERNAL_ANNOTATION, EnzLink, LinkDecoration, LinkSize, LinkTone, LinkWeight, enz_link,
};
pub use list_row_surface::{EnzListRowSurface, ListRowSurfaceTokens, enz_list_row_surface};
pub use markdown::{EnzMarkdown, enz_markdown};
pub use marquee_area::{EnzMarqueeArea, enz_marquee_area};
pub use media_controls::{EnzMediaControls, TimeDisplay, enz_media_controls};
pub use menu::{EnzMenu, EnzMenuItem, MenuTokens, enz_menu};
pub use menu_button::{
    EnzMenuButton, KEBAB_TRIGGER_LABEL, MenuButtonEvent, MenuButtonKind, MenuButtonTokens,
    VisualLiveness, enz_kebab_menu_button, enz_menu_button, enz_split_menu_button,
};
pub use menu_row::{
    EnzMenuRow, MenuRowIconPosition, MenuRowTokens, MenuRowTone, enz_menu_row, enz_menu_separator,
};
pub use modal_host::{
    DialogEvent, DialogStack, EnzModalHost, ModalFocusLifecycle, ModalFocusOutcome,
    ModalPresentation, ModalPresentationKey, enz_modal_host,
};
pub use modal_surface::{
    CLOSE_CONTROL_SIZE, CLOSE_ICON_SIZE, CLOSE_INSET, EnzModalSurface, ModalCloseTokens,
    ModalSurfaceTokens, close_button_geometry, close_button_tokens, concentric_radius,
    enz_modal_surface,
};
pub use motion::{PresentationKey, RetainedPresentation};
pub use navigation_outlet::{EnzNavigationOutlet, enz_navigation_outlet};
pub use navigation_split_view::{EnzNavigationSplitView, enz_navigation_split_view};
pub use notice_stack::{EnzNotice, EnzNoticeStack, enz_notice_stack};
pub use offset_scrollbar::{
    EnzOffsetScrollbar, LANE_WIDTH_PX as OFFSET_SCROLLBAR_LANE_WIDTH_PX,
    MIN_THUMB_PX as OFFSET_SCROLLBAR_MIN_THUMB_PX,
    THUMB_WIDTH_PX as OFFSET_SCROLLBAR_THUMB_WIDTH_PX,
    ThumbGeometry as OffsetScrollbarThumbGeometry, enz_offset_scrollbar,
    thumb_geometry as offset_scrollbar_thumb_geometry,
    thumb_geometry_with_tokens as offset_scrollbar_thumb_geometry_with_tokens,
};
pub use overflow_row::{EnzOverflowRow, enz_overflow_row, enz_overflow_row_content_only};
pub use page_header::{EnzPageHeader, enz_page_header};
pub use paged_text_viewer::{
    EnzPagedTextViewer, PagedTextViewModel, PagedTextViewportMetrics, PagedTextViewportState,
    enz_paged_byte_viewer, enz_paged_text_viewer,
};
pub use pagination::{EnzPagination, enz_pagination};
pub use pane::{EnzPane, enz_pane};
pub use pane_grid::{
    EnzPaneContent, EnzPaneGrid, EnzPaneGridEvent, EnzPaneGridState,
    Orientation as PaneSplitOrientation, PaneDragEvent, PaneDropTarget, PaneEdge, PaneKeyError,
    PaneRegion, PaneTree, enz_pane_grid, enz_pane_grid_with_content,
};
pub use pane_header::{EnzPaneHeader, enz_pane_header};
pub use pie_chart::{EnzPieChart, enz_pie_chart};
pub use popover::{EnzPopover, enz_popover};
pub use positioned_overlay::{
    EnzPositionedOverlay, enz_positioned_overlay, enz_positioned_overlay_with_cause,
};
pub use preferences_group::{EnzPreferencesGroup, enz_preferences_group};
pub use preferences_row::{EnzPreferencesRow, PreferencesRowTokens, enz_preferences_row};
pub use pressable::{
    EnzPressable, PressableStyle, PressableTokens, SurfaceColors, enz_pressable,
    enz_pressable_maybe,
};
pub use preview_row::{EnzPreviewRow, enz_preview_row};
pub use progress_bar::{
    EnzProgressBar, ProgressBarTokens, ProgressTone, enz_progress_bar, enz_progress_bar_sampled,
};
pub use radio_button::{EnzRadioButton, enz_radio_button};
pub use radio_group::{
    EnzRadioGroup, Orientation, RadioChoice, RadioGroupSize, enz_radio_group, enz_radio_group_keyed,
};
pub use radius_token_grid::{EnzRadiusTokenGrid, EnzRadiusTokenView, enz_radius_token_grid};
pub use range_slider::{EnzRangeSlider, enz_range_slider};
pub use rating::{EnzRating, RATING_UNRATED_ANNOUNCEMENT, enz_rating};
pub use reading_pane::{
    EnzReadingKind, EnzReadingPane, EnzReadingParagraph, EnzReadingTheme,
    EnzRevisionReadingParagraph, enz_reading_pane, enz_revisioned_reading_pane,
};
pub use reorder_list::{EnzReorderList, enz_reorder_list, reorder_handle};
pub use responsive_grid::{
    ColumnRamp, EnzResponsiveGrid, EnzSizeClass, class_for_width, enz_responsive_grid,
};
pub use responsive_grid_item::{
    DEFAULT_ITEM_HEIGHT, EnzResponsiveGridItem, enz_responsive_grid_item,
};
pub use revision_text::{
    RevisionText, RevisionTextFormat, TextIdentity, TextSnapshot, enz_revision_text,
};
pub use rich_edit::{
    EnzRichEdit, RichEditAction, RichEditController, RichEditor, enz_rich_edit,
    enz_rich_edit_builder,
};
// The bare `revision_text` spelling stays root-exported for source
// compatibility but is deprecated in favour of `enz_revision_text`.
pub use desktop_glass::{EnzDesktopGlass, enz_desktop_glass};
pub use glass::{EnzGlass, enz_glass};
#[allow(deprecated, reason = "re-exports a deprecated alias for compatibility")]
pub use revision_text::revision_text;
pub use row_measure::{DEFAULT_EPSILON, EnzRowMeasure, RowMeasurement, enz_row_measure};
pub use save_changes_dialog::{
    EnzSaveChangesDialog, SaveChangesAction, SaveChangesEvent, enz_save_changes_dialog,
    save_changes_dialog_host,
};
pub use scatter_chart::{EnzScatterChart, enz_scatter_chart};
pub use schedule::{
    EnzSchedule, ScheduleBlock, ScheduleColumn, ScheduleConfig, ScheduleEvent, ScheduleFrame,
    ScheduleHit, ScheduleTokens, enz_schedule,
};
pub use scroll_card::{EnzScrollCard, ScrollCardRadius, ScrollCardTone, enz_scroll_card};
pub use scroll_column::{EnzScrollColumn, enz_scroll_column};
pub use scroll_view::{
    Axis as ScrollAxis, EnzScrollView, QueryScrollGeometry, ScrollGeometry, ScrollGeometryCell,
    contain_wheel, enz_scroll_view, enz_scrollable, overlay_scrollbar,
    overlay_scrollbar_with_tokens,
};
pub use search_field::{
    EnzSearchField, SearchFieldChrome, SearchPresentation, enz_search_field, enz_titlebar_search,
};
pub use section_header::{EnzSectionHeader, SectionHeaderTokens, enz_section_header};
pub use secure_field::{
    EnzSecureField, PASTE_ID as SECURE_FIELD_PASTE_ID, SELECT_ALL_ID as SECURE_FIELD_SELECT_ALL_ID,
    SecureFieldTokens, enz_secure_field, restricted_context_menu,
};
pub use selection_scope::{EnzSelectionScope, SelectionContextInvocation, enz_selection_scope};
pub use settings_dialog::{
    DialogSize, EnzSettingsDialog, EnzSettingsPresentation, SettingsNav, SettingsPage,
    enz_settings_dialog, enz_settings_presentation_for, enz_settings_sheet_height,
    settings_nav_for,
};
pub use sheet::{
    EnzSheet, SheetDetentChange, SheetPresenter, SheetPresenterEvent, SheetPresenterOutcome,
    SheetResizeDirection, SheetScrollToExpand, enz_resizable_sheet, enz_sheet,
};
pub use shortcut_list::{
    EnzShortcutList, ShortcutCatalogRow, ShortcutEntry, enz_shortcut_list,
    shortcut_catalog_entries, shortcut_entries, shortcut_entries_with, shortcut_entry,
    shortcut_entry_with, shortcut_help_badge,
};
pub use shortcuts_dialog::{EnzShortcutsDialog, enz_shortcuts_dialog};
pub use sidebar_content::{EnzSidebarContent, enz_sidebar_content};
pub use sidebar_row::{EnzSidebarRow, SidebarRowTokens, enz_sidebar_row, enz_sidebar_row_lazy};
pub use sidebar_toggle_button::{EnzSidebarToggleButton, enz_sidebar_toggle_button};
pub use skeleton::{EnzSkeleton, SkeletonKind, enz_skeleton};
pub use slider::{EnzSlider, enz_slider};
pub use spacer::{Axis as SpacerAxis, EnzSpacerGap, enz_spacer_gap};
pub use sparkline::{EnzSparkline, enz_sparkline};
pub use spinner::{EnzSpinner, SpinnerTone, enz_spinner};
pub use stack::{CrossAlign, EnzHStack, EnzVStack, enz_hstack, enz_vstack};
pub use status_bar::{
    Distribution, EnzStatusBar, EnzStatusBarStrip, StatusBarItem, StatusBarTone, enz_status_bar,
    enz_status_bar_strip,
};
pub use status_dot::{EnzStatusDot, StatusDotMotion, StatusKind, enz_status_dot};
pub use status_page::{
    EnzStatusPage, StatusPageTokens, StatusPageTone, StatusPageVariant, enz_status_page,
};
pub use stepper::{
    EnzStepper, StepperSize, StepperTokens, StepperValue, enz_stepper, enz_stepper_u32,
    stepper_adjust,
};
pub use streaming_text::{
    CARET_BLINK, CARET_HALF_CYCLE, EnzStreamingText, StreamingTextTheme, StreamingTextTokens,
    enz_streaming_text,
};
pub use suggestion_list::{
    EnzSuggestionList, SuggestionOption, SuggestionSize, enz_suggestion_list,
};
pub use surface::{BorderStyle, EnzSurface, LineSize, RadiusSize, SurfaceTone, enz_surface};
pub use surface_stack::{
    EnzSurfaceStack, SpaceToken, StackAlign, StackCross, apply_main_axis, apply_main_axis_row,
    enz_surface_stack,
};
pub use tab_pill::{
    EnzTabList, EnzTabPill, TabHoverShape, TabLabelWeight, TabListWidth, TabPillGeometry,
    TabPillItem, TabPillTokens, TabSelectionStyle, enz_tab_list, enz_tab_pill,
};
pub use table_header_cell::{EnzTableHeaderCell, EnzTableHeaderCellAlign, enz_table_header_cell};
pub use table_text_cell::{EnzTableTextAlign, EnzTableTextCell, enz_table_text_cell};
pub use tail_follow::{RELEASE_DISTANCE, enz_tail_follow};
pub use text_area::{
    AreaHeight, EnzTextArea, TextAreaTokens, TextAreaVariant, buffer_char_scans, counter_text,
    enz_text_area, is_over_limit, reset_buffer_char_scans, reset_semantics_value_materializations,
    row_count_at, semantics_value_materializations,
};
pub use text_field::{EnzTextField, TextFieldSize, enz_text_field, enz_text_field_owned};
pub use text_grid::{
    EnzTextGrid, TextGridCell, TextGridCellDecoration, TextGridCellTone, TextGridEdge,
    TextGridPointer, TextGridTheme, TextGridTokens, enz_text_grid, enz_text_grid_shared,
};
pub use text_measure::{
    ESTIMATE_LINE_HEIGHT_RATIO, MeasureTypography, TextMeasureKind, TextMeasurement,
    enz_text_break_offset_with, enz_text_measure, enz_text_measure_width, enz_text_measure_with,
    estimate_advance_ratio, estimate_text_height, paginate_by_height, paginate_by_height_with,
};
pub use text_pair::{EnzTextPair, EnzTextPairElement, enz_text_pair, enz_text_pair_element};
pub use time_field::{
    DEFAULT_STEP_MINUTES, EnzTimeField, TimeFieldEvent, enz_time_field, time_of_day_step,
};
pub use time_picker::{
    DateTimePickerEvent, EnzTimePicker, TimePickerEvent, enz_date_time_picker, enz_time_picker,
};
pub use title_bar::{
    CSD_LIMITATIONS, DEFAULT_TITLEBAR_HEIGHT, DEFAULT_TITLEBAR_PADDING_X, DEFAULT_TITLEBAR_SPACING,
    EnzTitleBar, enz_title_bar,
};
pub use title_bar_gesture::{ResizeEdge, WindowCommand};
pub use titlebar_actions::{
    ActionItem as TitlebarActionItem, DEFAULT_MENU_WIDTH as TITLEBAR_ACTIONS_DEFAULT_MENU_WIDTH,
    DEFAULT_POPUP_GAP as TITLEBAR_ACTIONS_DEFAULT_POPUP_GAP,
    DEFAULT_TRIGGER_ICON as TITLEBAR_ACTIONS_DEFAULT_TRIGGER_ICON, EnzTitlebarActions,
    TRIGGER_LABEL as TITLEBAR_ACTIONS_TRIGGER_LABEL, enz_titlebar_actions,
};
pub use titlebar_drag_region::{DRAG_DEADBAND, TitlebarDragRegion, titlebar_drag_region};
pub use titlebar_group::{
    DEFAULT_GROUP_HEIGHT, DEFAULT_GROUP_SPACING, EnzTitlebarGroup, enz_titlebar_group,
};
pub use titlebar_icon_group::{
    EnzTitlebarIconGroup, TitlebarIconGroupVariant, enz_titlebar_icon_group,
};
pub use titlebar_tab::{
    TitlebarTabItem, TitlebarTabPlan, TitlebarTabRow, enz_titlebar_document_tab,
    enz_titlebar_document_tab_fill, enz_titlebar_document_tab_with, enz_titlebar_tab_row,
};
pub use toast::{EnzToast, ToastSize, ToastTokens, ToastTone, enz_toast};
pub use toast_host::{
    ToastAnnouncements, ToastEvent, ToastExpirySchedule, ToastHost, ToastId, ToastOverlayPlacement,
    ToastSpec, enz_toast_layer, enz_toast_overlay,
};
pub use toggle::{EnzToggle, ToggleSize, enz_toggle};
pub use tokens::{ButtonTokens, FieldVariant};
pub use toolbar_divider::{EnzToolbarDivider, ToolbarDividerTokens, enz_toolbar_divider};
pub use toolbar_icon::{EnzToolbarIcon, ToolbarIconTokens, ToolbarIconVariant, enz_toolbar_icon};
pub use toolbar_row::{EnzToolbarRow, ThreeZoneRow, enz_toolbar_row};
pub use toolbar_spacer::{EnzToolbarSpacer, ToolbarSpacerKind, enz_toolbar_spacer};
pub use toolbar_view::{EnzToolbarView, enz_toolbar_view};
pub use tooltip::{
    FadeTooltip, TOOLTIP_DELAY, TOOLTIP_FADE, TooltipFade, default_tooltip_delay, enz_fade_tooltip,
    enz_tooltip, enz_tooltip_bubble_opacity, enz_tooltip_with_delay, enz_tooltip_with_tokens,
    set_default_tooltip_delay,
};
pub use tree_disclosure::{EnzTreeDisclosure, enz_tree_disclosure};
pub use tree_view::{EnzTreeItem, EnzTreeView, TreeViewRowFills, enz_tree_view, selection_key_for};
pub use video::{EnzVideo, enz_video};
pub use view_switcher::{
    EnzViewSwitcher, SwitcherPage, SwitcherPolicy, SwitcherPresentation, SwitcherStyle,
    SwitcherTokens, enz_view_switcher, enz_view_switcher_unwired,
};
pub use view_that_fits::{EnzFitProbe, EnzFitReport, enz_view_that_fits};
pub use virtual_grid::{EnzVirtualGrid, enz_virtual_grid};
pub use virtual_list::{
    EnzVirtualList, GeometryHandle, enz_virtual_list, enz_virtual_list_variable,
    enz_virtual_list_with_geometry,
};
pub use virtual_text_surface::{VirtualTextSurface, enz_virtual_text_surface};
pub use window_control_button::{EnzWindowControlButton, enz_window_control_button};
pub use window_controls::{
    DEFAULT_ORDER as WINDOW_CONTROLS_DEFAULT_ORDER, EnzWindowControls, WindowControlKind,
    enz_window_controls,
};
pub use window_resize_frame::{
    DEFAULT_RESIZE_CORNER_EXTENT, DEFAULT_RESIZE_EDGE_THICKNESS, DEFAULT_RESIZE_SLIVER_THICKNESS,
    WindowResizeFrame, window_resize_frame,
};
pub use window_title::{EnzWindowTitle, enz_window_title};
pub use workspace_accent_chooser::{EnzWorkspaceAccentChooser, enz_workspace_accent_chooser};
pub use wrap::{EnzBadgeView, EnzWrap, WrapJustify, enz_wrap};
pub use zoom_wheel::{EnzZoomWheel, enz_zoom_wheel};
