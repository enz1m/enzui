# enzUI - component token API index

See [configuring framework appearance](styling.md) for global scales, snapshot precedence and choosing token roles.

**This file is generated** from public source in `crates/enzui-widgets/src`. See [`token_index.rs`](../crates/enzui-widgets/tests/contracts/token_index.rs). Do not edit by hand; regenerate with

```sh
ENZUI_WRITE_TOKENS_DOC=1 cargo wt --test contracts token_index
```

## Token vocabulary and anatomy

Field names encode the component's anatomy after the type name (the component itself) has been taken out. The rule is:

```
component -> variant/part -> property -> interaction state -> exceptional unit
```

- **parts** - `container`/`background`, `content`/`foreground`, `text`, `icon`, `border`, `track`, `thumb`, `focus_ring`, …
- **states** - rest (implicit), `hover`, `pressed`, `selected`, `disabled`, with combined-state ordering fixed.
- **geometry** - `padding` for inside an edge, `gap` between siblings, `inset` inside an edge, `outset` beyond an edge, `width`/`height`/`radius` for dimensions.
- **tones** - `error`, `danger`, `destructive` are *not* interchangeable; each names a different semantic (validation, intrinsic risk, action).
- **units** - px is implicit for component `f32` dimensions; suffixes are for non-px or dimensionless values (`_ms`, `_fraction`, `_alpha`).

Legacy spellings (`bg`/`fg`/`radius_lg`) stay compatible; see the per-token tables below for the canonical semantic path.

## Token setters and resolution modes

Component paint snapshots use `.tokens(...)`; geometry helpers use dedicated setters such as `.size_tokens(...)`. Passing a value usually pins a per-instance snapshot. Omitting it follows the runtime theme only for channels documented as live. See [styling](styling.md) for scale forwarding and precedence.

### When to override tokens

| You want to… | Technique | Theme-switch behaviour |
|---|---|---|
| Use the framework default | Omit `.tokens()` | Uses component defaults; documented live channels follow the theme |
| Use a named preset (`light`/`dark`) | `.tokens(T::light())` | Snapshot - call again on theme change |
| Derive from the current palette | `.tokens(T::from_palette(p))` | Snapshot - stale after theme change unless re-derived |
| Set one field, keep defaults | `T { field: val, ..T::from_palette(p) }` | Snapshot - same caveat as `from_palette` |
| Hard-code a literal | `T { field_a: x, field_b: y, … }` | Fully pinned - never changes |

**Rule of thumb:** prefer omitting `.tokens()` (the framework default) whenever possible. Call `from_palette` only when you need to change one or two fields and accept the snapshot trade-off; never pin an entire struct literal unless the component genuinely needs a fixed appearance.

### Resolution modes

The generated table below records each setter's *resolution mode* as parsed from its rustdoc:

- **pin** - rustdoc explicitly documents the value pins/bypasses runtime-theme derivation (a fixed snapshot).
- **override** - rustdoc says the value overrides / applies / sets / configures; silent on runtime resolution.
- **runtime** - rustdoc explicitly documents a live runtime-theme resolver (no setter currently classified here; all existing setters produce snapshots).

The `from_palette` constructor is the canonical way to seed a palette-backed snapshot; `light()`/`dark()` are convenience presets, not the runtime-theme path.

Source-derived counts: **86 structs, 859 public fields**.

## Token index

| Type | Module | Fields | `from_palette` | Default | Setter mode |
|---|---|---:|:---:|:---:|---|
| [`AnnotationOverlayTokens`](#annotationoverlaytokens) | `annotation_overlay` | 4 | yes | yes | pin |
| [`AreaChartTokens`](#areacharttokens) | `area_chart` | 8 | yes | no | pin |
| [`AvatarTokens`](#avatartokens) | `avatar` | 9 | yes | no | pin |
| [`BadgeSizeTokens`](#badgesizetokens) | `badge` | 7 | no | yes | pin |
| [`BadgeTokens`](#badgetokens) | `badge` | 12 | yes | no | pin |
| [`BannerTokens`](#bannertokens) | `banner` | 9 | no | yes | pin |
| [`BarChartTokens`](#barcharttokens) | `bar_chart` | 6 | yes | no | pin |
| [`BreadcrumbTokens`](#breadcrumbtokens) | `breadcrumb` | 4 | yes | no | pin |
| [`ButtonSizeTokens`](#buttonsizetokens) | `tokens` | 6 | no | no | pin |
| [`ButtonTokens`](#buttontokens) | `tokens` | 40 | yes | no | pin |
| [`CalendarTokens`](#calendartokens) | `calendar` | 17 | yes | no | pin |
| [`CardTokens`](#cardtokens) | `card` | 16 | yes | no | override |
| [`CarouselTokens`](#carouseltokens) | `carousel` | 10 | yes | no | pin |
| [`CartesianTokens`](#cartesiantokens) | `cartesian_chart` | 8 | yes | no | (no builder setter) |
| [`ChartLegendTokens`](#chartlegendtokens) | `chart_legend` | 6 | yes | no | pin |
| [`ChartTooltipTokens`](#charttooltiptokens) | `chart_tooltip` | 15 | yes | no | pin |
| [`ChatBubbleTokens`](#chatbubbletokens) | `chat_bubble` | 21 | yes | no | pin |
| [`CheckboxTokens`](#checkboxtokens) | `checkbox` | 12 | yes | no | pin |
| [`CheckerboardTokens`](#checkerboardtokens) | `slider` | 3 | no | yes | pin |
| [`ClickableTokens`](#clickabletokens) | `clickable` | 8 | yes | no | pin |
| [`ColorPickerSizeTokens`](#colorpickersizetokens) | `color_picker` | 6 | no | yes | pin |
| [`ColorSwatchTokens`](#colorswatchtokens) | `color_swatch` | 9 | yes | yes | pin |
| [`ContentPaneTokens`](#contentpanetokens) | `content_pane` | 1 | yes | no | override |
| [`DateFieldTokens`](#datefieldtokens) | `date_field` | 10 | yes | no | (no builder setter) |
| [`DisclosureTokens`](#disclosuretokens) | `disclosure` | 6 | yes | no | pin |
| [`DividerTokens`](#dividertokens) | `divider` | 6 | yes | no | pin |
| [`DragFeedbackTokens`](#dragfeedbacktokens) | `drag_drop_host` | 7 | no | yes | pin |
| [`ExpanderTokens`](#expandertokens) | `expander` | 15 | yes | no | pin |
| [`FieldSizeTokens`](#fieldsizetokens) | `tokens` | 4 | no | no | (no builder setter) |
| [`GaugeTokens`](#gaugetokens) | `gauge` | 9 | yes | no | pin |
| [`GlowTokens`](#glowtokens) | `glow` | 3 | yes | no | override |
| [`HeaderBarTokens`](#headerbartokens) | `header_bar` | 5 | no | yes | override |
| [`ImageCropperTokens`](#imagecroppertokens) | `image_cropper` | 4 | yes | no | pin |
| [`ImageViewerTokens`](#imageviewertokens) | `image_viewer` | 4 | yes | no | override |
| [`InputOtpTokens`](#inputotptokens) | `input_otp` | 13 | yes | no | pin |
| [`KeyBadgeTokens`](#keybadgetokens) | `key_badge` | 3 | yes | no | pin |
| [`KeyCaptureChipTokens`](#keycapturechiptokens) | `key_capture_chip` | 20 | yes | no | override |
| [`LabeledContentTokens`](#labeledcontenttokens) | `labeled_content` | 2 | yes | no | pin |
| [`LineChartTokens`](#linecharttokens) | `line_chart` | 8 | yes | no | pin |
| [`LinkPresentationTokens`](#linkpresentationtokens) | `rich_text::presentation` | 4 | no | yes | (no builder setter) |
| [`LinkTokens`](#linktokens) | `link` | 6 | yes | no | pin |
| [`ListRowSurfaceTokens`](#listrowsurfacetokens) | `list_row_surface` | 16 | yes | no | pin |
| [`MarqueeTokens`](#marqueetokens) | `marquee_area` | 6 | yes | yes | pin |
| [`MediaControlsTokens`](#mediacontrolstokens) | `media_controls` | 9 | yes | no | pin |
| [`MenuButtonTokens`](#menubuttontokens) | `menu_button` | 8 | yes | no | pin |
| [`MenuRowTokens`](#menurowtokens) | `menu_row` | 20 | yes | no | pin |
| [`MenuTokens`](#menutokens) | `menu` | 3 | yes | no | pin |
| [`ModalCloseTokens`](#modalclosetokens) | `modal_surface` | 5 | yes | no | (no builder setter) |
| [`ModalSurfaceTokens`](#modalsurfacetokens) | `modal_surface` | 2 | yes | no | override |
| [`NavigationOutletTokens`](#navigationoutlettokens) | `navigation_outlet` | 3 | no | yes | override |
| [`PageHeaderTokens`](#pageheadertokens) | `page_header` | 5 | no | yes | pin |
| [`PaneGridTokens`](#panegridtokens) | `pane_grid` | 11 | yes | yes | (no builder setter) |
| [`PieChartTokens`](#piecharttokens) | `pie_chart` | 5 | yes | no | pin |
| [`PreferencesRowTokens`](#preferencesrowtokens) | `preferences_row` | 18 | yes | yes | override |
| [`PressableTokens`](#pressabletokens) | `pressable` | 12 | yes | no | pin |
| [`ProgressBarTokens`](#progressbartokens) | `progress_bar` | 9 | yes | no | pin |
| [`RadioTokens`](#radiotokens) | `radio_group` | 12 | yes | no | pin |
| [`ScatterChartTokens`](#scattercharttokens) | `scatter_chart` | 7 | yes | no | pin |
| [`ScheduleTokens`](#scheduletokens) | `schedule` | 29 | yes | yes | pin |
| [`SectionHeaderTokens`](#sectionheadertokens) | `section_header` | 1 | yes | no | pin |
| [`SecureFieldTokens`](#securefieldtokens) | `secure_field` | 2 | no | yes | override |
| [`SheetTokens`](#sheettokens) | `sheet` | 3 | yes | no | (no builder setter) |
| [`ShellTokens`](#shelltokens) | `app_shell` | 39 | yes | yes | override |
| [`SidebarRowTokens`](#sidebarrowtokens) | `sidebar_row` | 18 | yes | no | override |
| [`SkeletonTokens`](#skeletontokens) | `skeleton` | 9 | yes | no | pin |
| [`SliderTokens`](#slidertokens) | `slider` | 22 | yes | no | pin |
| [`SpinnerTokens`](#spinnertokens) | `spinner` | 5 | yes | no | pin |
| [`StatusDotTokens`](#statusdottokens) | `status_dot` | 5 | yes | no | override |
| [`StatusPageTokens`](#statuspagetokens) | `status_page` | 15 | yes | yes | pin |
| [`StepperTokens`](#steppertokens) | `stepper` | 24 | yes | no | pin |
| [`StreamingTextTokens`](#streamingtexttokens) | `streaming_text` | 5 | no | yes | override |
| [`SurfaceTokens`](#surfacetokens) | `surface` | 12 | yes | no | pin |
| [`SwitcherTokens`](#switchertokens) | `view_switcher` | 11 | no | yes | override |
| [`TabPillTokens`](#tabpilltokens) | `tab_pill` | 26 | yes | no | pin |
| [`TextAreaTokens`](#textareatokens) | `text_area` | 13 | yes | no | pin |
| [`TextGridTokens`](#textgridtokens) | `text_grid` | 5 | no | yes | pin |
| [`TextPairTokens`](#textpairtokens) | `text_pair` | 2 | yes | no | pin |
| [`TextTokens`](#texttokens) | `text` | 8 | yes | no | override |
| [`TitlebarAffordanceTokens`](#titlebaraffordancetokens) | `toolbar_icon` | 3 | no | yes | (no builder setter) |
| [`ToastTokens`](#toasttokens) | `toast` | 35 | yes | no | pin |
| [`ToggleTokens`](#toggletokens) | `toggle` | 14 | yes | no | pin |
| [`ToolbarDividerTokens`](#toolbardividertokens) | `toolbar_divider` | 3 | yes | no | pin |
| [`ToolbarIconTokens`](#toolbaricontokens) | `toolbar_icon` | 14 | yes | no | override |
| [`ToolbarSpacerTokens`](#toolbarspacertokens) | `toolbar_spacer` | 3 | yes | no | pin |
| [`TooltipTokens`](#tooltiptokens) | `tooltip` | 11 | yes | no | pin |
| [`WindowTitleTokens`](#windowtitletokens) | `window_title` | 5 | no | yes | pin |

## Canonical aliases

Three component-owned token types have names shorter than their `<ModuleStem>Tokens` rule would predict. The short spelling is preserved for source compatibility; a canonical `pub type` alias with the derivable name is added in the same module.

| Canonical name | Compatibility name | Owning module |
|---|---|---|
| `AppShellTokens` | `ShellTokens` | `app_shell` |
| `ViewSwitcherTokens` | `SwitcherTokens` | `view_switcher` |
| `MarqueeAreaTokens` | `MarqueeTokens` | `marquee_area` |

## Per-token details

### `AnnotationOverlayTokens`

- **File:** `crates/enzui-widgets/src/annotation_overlay.rs`
- **Module:** `annotation_overlay`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (4):**
  - `selection`: `Color`
  - `selection_dash`: `f32`
  - `selection_width`: `f32`
  - `selection_inset`: `f32`

### `AreaChartTokens`

- **File:** `crates/enzui-widgets/src/area_chart.rs`
- **Module:** `area_chart`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (8):**
  - `grid`: `Color`
  - `axis`: `Color`
  - `tick_label`: `Color`
  - `edge`: `Color`
  - `fill`: `Color`
  - `plot_background`: `Color`
  - `focus_ring`: `Color`
  - `series_palette`: `crate::chart_palette::ChartPalette`

### `AvatarTokens`

- **File:** `crates/enzui-widgets/src/avatar.rs`
- **Module:** `avatar`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (9):**
  - `background`: `Color`
  - `foreground`: `Color`
  - `presence_online`: `Color`
  - `presence_away`: `Color`
  - `presence_busy`: `Color`
  - `presence_offline`: `Color`
  - `presence_ring`: `Color`
  - `shimmer_base`: `Color`
  - `shimmer_sheen`: `Color`

### `BadgeSizeTokens`

- **File:** `crates/enzui-widgets/src/badge.rs`
- **Module:** `badge`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (7):**
  - `height`: `f32`
  - `text_size`: `f32`
  - `radius`: `f32`
  - `padding_x`: `f32`
  - `min_width`: `f32`
  - `icon_size`: `f32`
  - `icon_gap`: `f32`

### `BadgeTokens`

- **File:** `crates/enzui-widgets/src/badge.rs`
- **Module:** `badge`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (12):**
  - `default_bg`: `Color`
  - `default_fg`: `Color`
  - `accent_bg`: `Color`
  - `accent_fg`: `Color`
  - `accent_strong_bg`: `Color`
  - `accent_strong_fg`: `Color`
  - `success_bg`: `Color`
  - `success_fg`: `Color`
  - `warning_bg`: `Color`
  - `warning_fg`: `Color`
  - `error_bg`: `Color`
  - `error_fg`: `Color`

### `BannerTokens`

- **File:** `crates/enzui-widgets/src/banner.rs`
- **Module:** `banner`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (9):**
  - `radius`: `f32`
  - `padding_x`: `f32`
  - `padding_y`: `f32`
  - `gap`: `f32`
  - `title_size`: `f32`
  - `body_size`: `f32`
  - `icon_badge`: `f32`
  - `elevation`: `u8`
  - `edge_depth`: `f32`

### `BarChartTokens`

- **File:** `crates/enzui-widgets/src/bar_chart.rs`
- **Module:** `bar_chart`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (6):**
  - `grid`: `Color`
  - `axis`: `Color`
  - `tick_label`: `Color`
  - `bar`: `Color`
  - `plot_background`: `Color`
  - `focus_ring`: `Color`

### `BreadcrumbTokens`

- **File:** `crates/enzui-widgets/src/breadcrumb.rs`
- **Module:** `breadcrumb`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (4):**
  - `text_color`: `Color`
  - `link_color`: `Color`
  - `hover_color`: `Color`
  - `separator_color`: `Color`

### `ButtonSizeTokens`

- **File:** `crates/enzui-widgets/src/tokens.rs`
- **Module:** `tokens`
- **Palette constructor:** no
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (6):**
  - `height`: `f32`
  - `padding_x`: `f32`
  - `text_size`: `f32`
  - `icon_size`: `f32`
  - `radius`: `f32`
  - `gap`: `f32`

### `ButtonTokens`

- **File:** `crates/enzui-widgets/src/tokens.rs`
- **Module:** `tokens`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (40):**
  - `primary_bg`: `Color`
  - `primary_hover_bg`: `Color`
  - `primary_pressed_bg`: `Color`
  - `primary_fg`: `Color`
  - `primary_border`: `Color`
  - `primary_border_width`: `f32`
  - `secondary_bg`: `Color`
  - `secondary_hover_bg`: `Color`
  - `secondary_pressed_bg`: `Color`
  - `secondary_fg`: `Color`
  - `secondary_border`: `Color`
  - `outlined_border`: `Color`
  - `ghost_bg`: `Color`
  - `ghost_hover_bg`: `Color`
  - `ghost_pressed_bg`: `Color`
  - `ghost_fg`: `Color`
  - `ghost_hover_fg`: `Color`
  - `ghost_pressed_fg`: `Color`
  - `ghost_border`: `Color`
  - `ghost_border_width`: `f32`
  - `accent_bg`: `Color`
  - `accent_hover_bg`: `Color`
  - `accent_pressed_bg`: `Color`
  - `accent_fg`: `Color`
  - `accent_hover_fg`: `Color`
  - `accent_pressed_fg`: `Color`
  - `destructive_bg`: `Color`
  - `destructive_hover_bg`: `Color`
  - `destructive_pressed_bg`: `Color`
  - `destructive_fg`: `Color`
  - `quiet_destructive_fg`: `Color`
  - `success_bg`: `Color`
  - `success_hover_bg`: `Color`
  - `success_pressed_bg`: `Color`
  - `success_fg`: `Color`
  - `disabled_bg`: `Color`
  - `disabled_fg`: `Color`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`
  - `border_width`: `f32`

### `CalendarTokens`

- **File:** `crates/enzui-widgets/src/calendar/tokens.rs`
- **Module:** `calendar`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (17):**
  - `text`: `Color`
  - `adjacent`: `Color`
  - `disabled`: `Color`
  - `header`: `Color`
  - `title`: `Color`
  - `selected_background`: `Color`
  - `selected_foreground`: `Color`
  - `range_background`: `Color`
  - `hover_background`: `Color`
  - `today_surface`: `Color`
  - `today_foreground`: `Color`
  - `combined_background`: `Color`
  - `combined_foreground`: `Color`
  - `focus_ring`: `Color`
  - `surface`: `Color`
  - `border`: `Color`
  - `marker_dot`: `Color`

### `CardTokens`

- **File:** `crates/enzui-widgets/src/card.rs`
- **Module:** `card`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (16):**
  - `surface`: `Color`
  - `surface_sunken`: `Color`
  - `background`: `Color`
  - `border`: `Color`
  - `control_border`: `Color`
  - `title_color`: `Color`
  - `description_color`: `Color`
  - `radius`: `f32`
  - `border_width`: `f32`
  - `padding_x`: `f32`
  - `padding_y`: `f32`
  - `gap`: `f32`
  - `title_size`: `f32`
  - `description_size`: `f32`
  - `footer_size`: `f32`
  - `header_gap`: `f32`

### `CarouselTokens`

- **File:** `crates/enzui-widgets/src/carousel/tokens.rs`
- **Module:** `carousel`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (10):**
  - `background`: `iced::Color`
  - `border`: `iced::Color`
  - `indicator`: `iced::Color`
  - `indicator_hover`: `iced::Color`
  - `indicator_selected`: `iced::Color`
  - `chrome_foreground`: `iced::Color`
  - `chrome_background`: `iced::Color`
  - `radius`: `f32`
  - `swipe_threshold`: `f32`
  - `indicator_fade_ms`: `u16`

### `CartesianTokens`

- **File:** `crates/enzui-widgets/src/cartesian_chart.rs`
- **Module:** `cartesian_chart`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (8):**
  - `grid`: `Color`
  - `axis`: `Color`
  - `tick_label`: `Color`
  - `mark`: `Color`
  - `plot_background`: `Color`
  - `focus_ring`: `Color`
  - `crosshair`: `Color`
  - `series_palette`: `ChartPalette`

### `ChartLegendTokens`

- **File:** `crates/enzui-widgets/src/chart_legend.rs`
- **Module:** `chart_legend`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (6):**
  - `swatch_size`: `f32`
  - `swatch_radius`: `f32`
  - `item_gap`: `f32`
  - `label_gap`: `f32`
  - `label_color`: `Color`
  - `accent`: `Color`

### `ChartTooltipTokens`

- **File:** `crates/enzui-widgets/src/chart_tooltip.rs`
- **Module:** `chart_tooltip`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (15):**
  - `background`: `Color`
  - `border`: `Color`
  - `border_width`: `f32`
  - `radius`: `f32`
  - `padding`: `f32`
  - `row_spacing`: `f32`
  - `swatch_size`: `f32`
  - `swatch_radius`: `f32`
  - `swatch_gap`: `f32`
  - `min_spacer`: `f32`
  - `shadow_blur`: `f32`
  - `shadow_color`: `Color`
  - `text_color`: `Color`
  - `muted_color`: `Color`
  - `accent`: `Color`

### `ChatBubbleTokens`

- **File:** `crates/enzui-widgets/src/chat_bubble/tokens.rs`
- **Module:** `chat_bubble`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (21):**
  - `user_background`: `Color`
  - `user_foreground`: `Color`
  - `assistant_background`: `Color`
  - `assistant_foreground`: `Color`
  - `system_background`: `Color`
  - `system_foreground`: `Color`
  - `author_color`: `Color`
  - `footnote_color`: `Color`
  - `shadow_color`: `Color`
  - `bubble_radius`: `f32`
  - `bubble_padding_x`: `f32`
  - `bubble_padding_y`: `f32`
  - `bubble_max_width`: `f32`
  - `bubble_min_height`: `f32`
  - `author_size`: `f32`
  - `body_size`: `f32`
  - `author_gap`: `f32`
  - `footnote_size`: `f32`
  - `footnote_gap`: `f32`
  - `shadow_blur`: `f32`
  - `shadow_offset_y`: `f32`

### `CheckboxTokens`

- **File:** `crates/enzui-widgets/src/checkbox.rs`
- **Module:** `checkbox`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (12):**
  - `radius`: `Option<f32>`
  - `border_width`: `Option<f32>`
  - `focus_ring_width`: `f32`
  - `disabled_alpha`: `f32`
  - `border`: `Color`
  - `background`: `Color`
  - `hover_border`: `Color`
  - `accent`: `Color`
  - `check_color`: `Color`
  - `text_color`: `Color`
  - `description_color`: `Color`
  - `focus_ring`: `Color`

### `CheckerboardTokens`

- **File:** `crates/enzui-widgets/src/slider.rs`
- **Module:** `slider`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (3):**
  - `square_size`: `f32`
  - `light`: `Color`
  - `dark`: `Color`

### `ClickableTokens`

- **File:** `crates/enzui-widgets/src/clickable.rs`
- **Module:** `clickable`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (8):**
  - `background`: `Color`
  - `hover_overlay`: `Color`
  - `pressed_overlay`: `Color`
  - `border`: `Color`
  - `text`: `Color`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`
  - `disabled_alpha`: `f32`

### `ColorPickerSizeTokens`

- **File:** `crates/enzui-widgets/src/color_picker.rs`
- **Module:** `color_picker`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (6):**
  - `handle_radius`: `f32`
  - `handle_focus_width`: `f32`
  - `handle_focus_outset`: `f32`
  - `track_height`: `f32`
  - `swatch_width`: `f32`
  - `swatch_height`: `f32`

### `ColorSwatchTokens`

- **File:** `crates/enzui-widgets/src/color_swatch.rs`
- **Module:** `color_swatch`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (9):**
  - `normal_border`: `Color`
  - `selected_border`: `Color`
  - `hover_border`: `Color`
  - `focus_ring`: `Color`
  - `swatch_size`: `f32`
  - `swatch_radius`: `f32`
  - `normal_border_width`: `f32`
  - `selected_border_width`: `f32`
  - `disabled_alpha`: `f32`

### `ContentPaneTokens`

- **File:** `crates/enzui-widgets/src/content_pane.rs`
- **Module:** `content_pane`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (1):**
  - `pane_background`: `Color`

### `DateFieldTokens`

- **File:** `crates/enzui-widgets/src/date_field.rs`
- **Module:** `date_field`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (10):**
  - `background`: `Color`
  - `border`: `Color`
  - `border_hover`: `Color`
  - `focus_ring`: `Color`
  - `text`: `Color`
  - `placeholder`: `Color`
  - `label`: `Color`
  - `icon`: `Color`
  - `popup_background`: `Color`
  - `popup_border`: `Color`

### `DisclosureTokens`

- **File:** `crates/enzui-widgets/src/disclosure.rs`
- **Module:** `disclosure`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (6):**
  - `text_color`: `Color`
  - `icon_color`: `Color`
  - `hover_background`: `Color`
  - `pressed_background`: `Color`
  - `focus_ring_color`: `Color`
  - `focus_ring_width`: `f32`

### `DividerTokens`

- **File:** `crates/enzui-widgets/src/divider.rs`
- **Module:** `divider`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (6):**
  - `line_color`: `Color`
  - `label_color`: `Color`
  - `label_size`: `f32`
  - `label_gap`: `f32`
  - `thickness`: `f32`
  - `fade_ends`: `bool`

### `DragFeedbackTokens`

- **File:** `crates/enzui-widgets/src/drag_drop_host.rs`
- **Module:** `drag_drop_host`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (7):**
  - `item_radius`: `f32`
  - `area_radius`: `f32`
  - `highlight_border_width`: `f32`
  - `drag_fill_alpha`: `f32`
  - `highlight_fill_alpha`: `f32`
  - `area_fill_alpha`: `f32`
  - `dragging_elevation`: `u8`

### `ExpanderTokens`

- **File:** `crates/enzui-widgets/src/expander/tokens.rs`
- **Module:** `expander`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (15):**
  - `background`: `Color`
  - `border`: `Color`
  - `hover_background`: `Color`
  - `title_color`: `Color`
  - `subtitle_color`: `Color`
  - `icon_color`: `Color`
  - `divider_color`: `Color`
  - `card_radius`: `f32`
  - `header_height`: `f32`
  - `content_inset`: `f32`
  - `content_gap`: `f32`
  - `icon_size`: `f32`
  - `chevron_size`: `f32`
  - `title_size`: `f32`
  - `subtitle_size`: `f32`

### `FieldSizeTokens`

- **File:** `crates/enzui-widgets/src/tokens.rs`
- **Module:** `tokens`
- **Palette constructor:** no
- **Default / preset:** no
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (4):**
  - `height`: `f32`
  - `padding_x`: `f32`
  - `text_size`: `f32`
  - `radius`: `f32`

### `GaugeTokens`

- **File:** `crates/enzui-widgets/src/gauge.rs`
- **Module:** `gauge`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (9):**
  - `accent_color`: `Color`
  - `success_color`: `Color`
  - `warning_color`: `Color`
  - `danger_color`: `Color`
  - `track_color`: `Color`
  - `track_border_color`: `Color`
  - `text_color`: `Color`
  - `muted_color`: `Color`
  - `unsampled_color`: `Color`

### `GlowTokens`

- **File:** `crates/enzui-widgets/src/glow.rs`
- **Module:** `glow`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (3):**
  - `surface_elevated`: `Color`
  - `default_glow_color`: `Color`
  - `content_radius`: `f32`

### `HeaderBarTokens`

- **File:** `crates/enzui-widgets/src/header_bar.rs`
- **Module:** `header_bar`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (5):**
  - `bar_height`: `f32`
  - `padding_x`: `f32`
  - `gap`: `f32`
  - `radius_lg`: `f32`
  - `action_min_width`: `f32`

### `ImageCropperTokens`

- **File:** `crates/enzui-widgets/src/image_cropper.rs`
- **Module:** `image_cropper`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (4):**
  - `scrim`: `Color`
  - `outline`: `Color`
  - `handle`: `Color`
  - `caption`: `Color`

### `ImageViewerTokens`

- **File:** `crates/enzui-widgets/src/image_viewer.rs`
- **Module:** `image_viewer`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (4):**
  - `canvas`: `Color`
  - `border`: `Color`
  - `focus_ring`: `Color`
  - `radius`: `f32`

### `InputOtpTokens`

- **File:** `crates/enzui-widgets/src/input_otp.rs`
- **Module:** `input_otp`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (13):**
  - `background`: `Color`
  - `border`: `Color`
  - `error_border`: `Color`
  - `focus_border`: `Color`
  - `text`: `Color`
  - `caret`: `Color`
  - `cell_size`: `f32`
  - `cell_radius`: `f32`
  - `border_width`: `f32`
  - `focus_border_width`: `f32`
  - `cell_gap`: `f32`
  - `group_gap`: `f32`
  - `char_size`: `f32`

### `KeyBadgeTokens`

- **File:** `crates/enzui-widgets/src/key_badge.rs`
- **Module:** `key_badge`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (3):**
  - `background`: `Color`
  - `border`: `Color`
  - `text_color`: `Color`

### `KeyCaptureChipTokens`

- **File:** `crates/enzui-widgets/src/key_capture_chip.rs`
- **Module:** `key_capture_chip`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (20):**
  - `background`: `Color`
  - `hover_background`: `Color`
  - `capture_background`: `Color`
  - `disabled_background`: `Color`
  - `border`: `Color`
  - `hover_border`: `Color`
  - `focus_border`: `Color`
  - `capture_border`: `Color`
  - `invalid_border`: `Color`
  - `text_color`: `Color`
  - `placeholder_color`: `Color`
  - `capture_text_color`: `Color`
  - `disabled_text_color`: `Color`
  - `height`: `f32`
  - `min_width`: `f32`
  - `padding_x`: `f32`
  - `radius`: `f32`
  - `text_size`: `f32`
  - `border_width`: `f32`
  - `capture_border_width`: `f32`

### `LabeledContentTokens`

- **File:** `crates/enzui-widgets/src/labeled_content.rs`
- **Module:** `labeled_content`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (2):**
  - `label_color`: `Color`
  - `value_color`: `Color`

### `LineChartTokens`

- **File:** `crates/enzui-widgets/src/line_chart.rs`
- **Module:** `line_chart`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (8):**
  - `grid`: `Color`
  - `axis`: `Color`
  - `tick_label`: `Color`
  - `mark`: `Color`
  - `plot_background`: `Color`
  - `focus_ring`: `Color`
  - `crosshair`: `Color`
  - `series_palette`: `crate::chart_palette::ChartPalette`

### `LinkPresentationTokens`

- **File:** `crates/enzui-widgets/src/rich_text/presentation.rs`
- **Module:** `rich_text::presentation`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (4):**
  - `rest`: `TextTone`
  - `hover`: `TextTone`
  - `focus`: `TextTone`
  - `underline`: `LinkUnderline`

### `LinkTokens`

- **File:** `crates/enzui-widgets/src/link.rs`
- **Module:** `link`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (6):**
  - `accent`: `Color`
  - `accent_hover`: `Color`
  - `text_muted`: `Color`
  - `danger`: `Color`
  - `success`: `Color`
  - `focus_ring`: `Color`

### `ListRowSurfaceTokens`

- **File:** `crates/enzui-widgets/src/list_row_surface.rs`
- **Module:** `list_row_surface`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (16):**
  - `surface_background`: `Color`
  - `hover_overlay`: `Color`
  - `pressed_overlay`: `Color`
  - `selected_background`: `Color`
  - `border_color`: `Color`
  - `selected_border`: `Color`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`
  - `text_color`: `Color`
  - `row_height`: `f32`
  - `row_radius`: `f32`
  - `border_width`: `f32`
  - `outer_gutter`: `f32`
  - `content_padding`: `f32`
  - `content_align_y`: `iced::Alignment`
  - `disabled_alpha`: `f32`

### `MarqueeTokens`

- **File:** `crates/enzui-widgets/src/marquee_area.rs`
- **Module:** `marquee_area`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (6):**
  - `band_fill`: `Color`
  - `band_border`: `Color`
  - `band_border_width`: `f32`
  - `band_radius`: `f32`
  - `highlight`: `Color`
  - `drag_threshold`: `f32`

### `MediaControlsTokens`

- **File:** `crates/enzui-widgets/src/media_controls.rs`
- **Module:** `media_controls`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (9):**
  - `background`: `Color`
  - `text_color`: `Color`
  - `text_muted`: `Color`
  - `accent`: `Color`
  - `track_color`: `Color`
  - `fill_color`: `Color`
  - `error_color`: `Color`
  - `error_bg`: `Color`
  - `border_color`: `Color`

### `MenuButtonTokens`

- **File:** `crates/enzui-widgets/src/menu_button.rs`
- **Module:** `menu_button`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (8):**
  - `background`: `Color`
  - `hover_background`: `Color`
  - `pressed_background`: `Color`
  - `disabled_background`: `Color`
  - `foreground`: `Color`
  - `disabled_foreground`: `Color`
  - `divider`: `Color`
  - `focus_ring`: `Color`

### `MenuRowTokens`

- **File:** `crates/enzui-widgets/src/menu_row.rs`
- **Module:** `menu_row`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (20):**
  - `text`: `Color`
  - `description`: `Color`
  - `trailing`: `Color`
  - `destructive`: `Color`
  - `disabled`: `Color`
  - `hover_background`: `Color`
  - `separator`: `Color`
  - `check_accent`: `Color`
  - `focus_ring`: `Color`
  - `row_height`: `f32`
  - `row_height_two_line`: `f32`
  - `row_radius`: `f32`
  - `background_inset_y`: `f32`
  - `padding_x`: `f32`
  - `body_size`: `f32`
  - `description_size`: `f32`
  - `icon_size`: `f32`
  - `row_gap`: `f32`
  - `description_gap`: `f32`
  - `separator_inset`: `f32`

### `MenuTokens`

- **File:** `crates/enzui-widgets/src/menu.rs`
- **Module:** `menu`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (3):**
  - `background`: `Color`
  - `border`: `Color`
  - `border_width`: `f32`

### `ModalCloseTokens`

- **File:** `crates/enzui-widgets/src/modal_surface.rs`
- **Module:** `modal_surface`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (5):**
  - `fg`: `Color`
  - `bg`: `Color`
  - `hover_bg`: `Color`
  - `pressed_bg`: `Color`
  - `focus_ring`: `Color`

### `ModalSurfaceTokens`

- **File:** `crates/enzui-widgets/src/modal_surface.rs`
- **Module:** `modal_surface`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (2):**
  - `background`: `Color`
  - `border`: `Color`

### `NavigationOutletTokens`

- **File:** `crates/enzui-widgets/src/navigation_outlet/tokens.rs`
- **Module:** `navigation_outlet`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (3):**
  - `transition_distance`: `f32`
  - `back_swipe_edge_width`: `f32`
  - `back_swipe_threshold`: `f32`

### `PageHeaderTokens`

- **File:** `crates/enzui-widgets/src/page_header.rs`
- **Module:** `page_header`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (5):**
  - `title_size`: `f32`
  - `primary_min_width`: `f32`
  - `action_spacing`: `f32`
  - `compact_spacing`: `f32`
  - `compact_breakpoint`: `f32`

### `PaneGridTokens`

- **File:** `crates/enzui-widgets/src/pane_grid.rs`
- **Module:** `pane_grid`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (11):**
  - `seam_thickness`: `f32`
  - `seam`: `Color`
  - `seam_active`: `Color`
  - `handle_ink`: `Color`
  - `handle_slop`: `f32`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`
  - `focus_ring_radius`: `f32`
  - `min_pane_px`: `f32`
  - `min_ratio`: `f32`
  - `keyboard_step`: `f32`

### `PieChartTokens`

- **File:** `crates/enzui-widgets/src/pie_chart.rs`
- **Module:** `pie_chart`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (5):**
  - `separator_color`: `Color`
  - `separator_width`: `f32`
  - `empty_color`: `Color`
  - `palette`: `ChartPalette`
  - `highlight_color`: `Color`

### `PreferencesRowTokens`

- **File:** `crates/enzui-widgets/src/preferences_row/mod.rs`
- **Module:** `preferences_row`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (18):**
  - `row_height`: `f32`
  - `row_height_with_subtitle`: `f32`
  - `row_radius`: `f32`
  - `padding_x`: `f32`
  - `gap`: `f32`
  - `wrap_vertical_padding`: `f32`
  - `icon_size`: `f32`
  - `icon_slot`: `f32`
  - `chevron_size`: `f32`
  - `chevron_slot`: `f32`
  - `title_size`: `f32`
  - `subtitle_size`: `f32`
  - `value_size`: `f32`
  - `inline_field_width`: `f32`
  - `inline_field_max_width`: `f32`
  - `switch_width`: `f32`
  - `switch_height`: `f32`
  - `switch_thumb`: `f32`

### `PressableTokens`

- **File:** `crates/enzui-widgets/src/pressable.rs`
- **Module:** `pressable`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (12):**
  - `card_bg`: `Color`
  - `chip_bg`: `Color`
  - `card_border`: `Color`
  - `card_selected_border`: `Color`
  - `card_selected_bg`: `Color`
  - `row_chip_selected_bg`: `Color`
  - `hover_overlay`: `Color`
  - `pressed_overlay`: `Color`
  - `text`: `Color`
  - `disabled_alpha`: `f32`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`

### `ProgressBarTokens`

- **File:** `crates/enzui-widgets/src/progress_bar.rs`
- **Module:** `progress_bar`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (9):**
  - `track`: `Color`
  - `track_border`: `Color`
  - `accent`: `Color`
  - `success`: `Color`
  - `warning`: `Color`
  - `danger`: `Color`
  - `text_color`: `Color`
  - `value_text_color`: `Color`
  - `unsampled`: `Color`

### `RadioTokens`

- **File:** `crates/enzui-widgets/src/radio_group.rs`
- **Module:** `radio_group`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (12):**
  - `border_width`: `Option<f32>`
  - `focus_ring_width`: `f32`
  - `focus_ring_radius`: `f32`
  - `disabled_alpha`: `f32`
  - `border`: `Color`
  - `background`: `Color`
  - `hover_border`: `Color`
  - `accent`: `Color`
  - `dot_color`: `Color`
  - `text_color`: `Color`
  - `group_label_color`: `Color`
  - `focus_ring`: `Color`

### `ScatterChartTokens`

- **File:** `crates/enzui-widgets/src/scatter_chart.rs`
- **Module:** `scatter_chart`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (7):**
  - `grid`: `Color`
  - `axis`: `Color`
  - `tick_label`: `Color`
  - `mark`: `Color`
  - `plot_background`: `Color`
  - `focus_ring`: `Color`
  - `series_palette`: `ChartPalette`

### `ScheduleTokens`

- **File:** `crates/enzui-widgets/src/schedule/tokens.rs`
- **Module:** `schedule`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (29):**
  - `background`: `Color`
  - `header_background`: `Color`
  - `all_day_background`: `Color`
  - `rule`: `Color`
  - `minor_rule`: `Color`
  - `off_hours_rule`: `Color`
  - `off_hours_minor_rule`: `Color`
  - `column_rule`: `Color`
  - `off_hours`: `Color`
  - `today_column`: `Color`
  - `selected_column`: `Color`
  - `selected_header`: `Color`
  - `range_preview`: `Color`
  - `range_preview_border`: `Color`
  - `range_preview_ink`: `Color`
  - `now_line`: `Color`
  - `text`: `Color`
  - `text_muted`: `Color`
  - `today_disc`: `Color`
  - `today_disc_ink`: `Color`
  - `selection_ring`: `Color`
  - `selection_ring_width`: `f32`
  - `block_radius`: `f32`
  - `header_text_size`: `f32`
  - `day_number_size`: `f32`
  - `hour_label_size`: `f32`
  - `block_title_size`: `f32`
  - `block_subtitle_size`: `f32`
  - `now_line_width`: `f32`

### `SectionHeaderTokens`

- **File:** `crates/enzui-widgets/src/section_header.rs`
- **Module:** `section_header`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (1):**
  - `text_color`: `Color`

### `SecureFieldTokens`

- **File:** `crates/enzui-widgets/src/secure_field.rs`
- **Module:** `secure_field`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (2):**
  - `eye_icon_size`: `f32`
  - `eye_inset`: `f32`

### `SheetTokens`

- **File:** `crates/enzui-widgets/src/sheet/tokens.rs`
- **Module:** `sheet`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (3):**
  - `surface`: `Color`
  - `border`: `Color`
  - `handle`: `Color`

### `ShellTokens`

- **File:** `crates/enzui-widgets/src/app_shell.rs`
- **Module:** `app_shell`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (39):**
  - `gap`: `f32`
  - `edge_inset`: `Option<f32>`
  - `pane_gutter`: `Option<f32>`
  - `page_presentation`: `PagePresentation`
  - `page_inset`: `f32`
  - `notifications_inset`: `Option<f32>`
  - `window_radius`: `f32`
  - `pane_radius`: `f32`
  - `titlebar_height`: `f32`
  - `subtitlebar_height`: `f32`
  - `sidebar_width`: `f32`
  - `sidebar_width_adaptive`: `bool`
  - `content_minimum`: `f32`
  - `secondary_minimum`: `Option<f32>`
  - `shell_tone`: `SurfaceTone`
  - `frame_material`: `Option<enzui_theme::MaterialStyle>`
  - `content`: `PaneStyle`
  - `secondary_content`: `PaneStyle`
  - `secondary_width`: `Option<f32>`
  - `split_axis`: `SplitAxis`
  - `narrow_split`: `NarrowSplit`
  - `secondary_leading`: `bool`
  - `sidebar`: `PaneStyle`
  - `titlebar`: `PaneStyle`
  - `subtitlebar`: `PaneStyle`
  - `footer`: `PaneStyle`
  - `sidebar_header`: `SidebarHeader`
  - `sidebar_collapsible`: `bool`
  - `show_close`: `bool`
  - `show_minimize`: `bool`
  - `show_maximize`: `bool`
  - `full_width_drawer`: `bool`
  - `sidebar_first`: `bool`
  - `seam`: `Option<SeamStyle>`
  - `seam_thickness`: `Option<f32>`
  - `seam_color`: `Option<iced::Color>`
  - `toggle_anchor`: `Option<ToggleAnchor>`
  - `toggle_icon_tokens`: `Option<crate::toolbar_icon::ToolbarIconTokens>`
  - `direction`: `Option<Direction>`

### `SidebarRowTokens`

- **File:** `crates/enzui-widgets/src/sidebar_row.rs`
- **Module:** `sidebar_row`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (18):**
  - `base_background`: `Color`
  - `hover_background`: `Color`
  - `selected_background`: `Color`
  - `text`: `Color`
  - `selected_text`: `Color`
  - `muted`: `Color`
  - `icon`: `Color`
  - `disabled`: `Color`
  - `focus_ring`: `Color`
  - `row_height`: `f32`
  - `row_radius`: `f32`
  - `row_padding_x`: `f32`
  - `outer_gutter`: `f32`
  - `fill_inset_y`: `f32`
  - `icon_size`: `f32`
  - `icon_gap`: `f32`
  - `label_size`: `f32`
  - `count_min_width`: `f32`

### `SkeletonTokens`

- **File:** `crates/enzui-widgets/src/skeleton.rs`
- **Module:** `skeleton`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (9):**
  - `base_color`: `Color`
  - `line_height`: `f32`
  - `line_spacing`: `f32`
  - `circle_size`: `f32`
  - `card_height`: `f32`
  - `card_radius`: `f32`
  - `text_min_width`: `f32`
  - `loading_opacity`: `f32`
  - `idle_opacity`: `f32`

### `SliderTokens`

- **File:** `crates/enzui-widgets/src/slider.rs`
- **Module:** `slider`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (22):**
  - `track`: `Color`
  - `track_border`: `Color`
  - `fill`: `Color`
  - `fill_hover`: `Color`
  - `thumb`: `Color`
  - `thumb_border`: `Color`
  - `thumb_shadow`: `Color`
  - `thumb_dragging`: `Color`
  - `text_color`: `Color`
  - `value_text_color`: `Color`
  - `focus_ring`: `Color`
  - `track_border_width`: `Option<f32>`
  - `thumb_border_width`: `Option<f32>`
  - `focus_ring_width`: `f32`
  - `focus_ring_outset`: `f32`
  - `thumb_shadow_offset`: `iced::Vector`
  - `thumb_shadow_blur`: `f32`
  - `disabled_alpha`: `f32`
  - `step_mark_size`: `f32`
  - `step_mark`: `Color`
  - `step_mark_on_fill`: `Color`
  - `step_mark_min_spacing`: `f32`

### `SpinnerTokens`

- **File:** `crates/enzui-widgets/src/spinner.rs`
- **Module:** `spinner`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (5):**
  - `accent_color`: `Color`
  - `success_color`: `Color`
  - `warning_color`: `Color`
  - `danger_color`: `Color`
  - `track_color`: `Color`

### `StatusDotTokens`

- **File:** `crates/enzui-widgets/src/status_dot.rs`
- **Module:** `status_dot`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (5):**
  - `neutral_color`: `Color`
  - `info_color`: `Color`
  - `success_color`: `Color`
  - `warning_color`: `Color`
  - `danger_color`: `Color`

### `StatusPageTokens`

- **File:** `crates/enzui-widgets/src/status_page.rs`
- **Module:** `status_page`
- **Palette constructor:** yes
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (15):**
  - `icon_size_page`: `f32`
  - `icon_size_page_compact`: `f32`
  - `icon_size_inline`: `f32`
  - `title_size_page`: `f32`
  - `title_size_page_compact`: `f32`
  - `description_size`: `f32`
  - `status_size`: `f32`
  - `action_min_width`: `f32`
  - `outer_spacing`: `f32`
  - `outer_spacing_compact`: `f32`
  - `action_gap`: `f32`
  - `min_height_page`: `f32`
  - `min_height_page_compact`: `f32`
  - `min_height_has_image`: `f32`
  - `min_height_has_image_compact`: `f32`

### `StepperTokens`

- **File:** `crates/enzui-widgets/src/stepper.rs`
- **Module:** `stepper`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (24):**
  - `background`: `Color`
  - `border`: `Color`
  - `divider`: `Color`
  - `segment_hover`: `Color`
  - `segment_pressed`: `Color`
  - `text`: `Color`
  - `muted`: `Color`
  - `disabled`: `Color`
  - `disabled_text`: `Color`
  - `focus_ring`: `Color`
  - `error`: `Color`
  - `selection`: `Color`
  - `caret`: `Color`
  - `height`: `f32`
  - `button_width`: `f32`
  - `value_min_width`: `f32`
  - `value_inset`: `f32`
  - `radius`: `f32`
  - `border_width`: `f32`
  - `focus_ring_width`: `f32`
  - `divider_width`: `f32`
  - `body_size`: `f32`
  - `label_size`: `f32`
  - `glyph_size`: `f32`

### `StreamingTextTokens`

- **File:** `crates/enzui-widgets/src/streaming_text.rs`
- **Module:** `streaming_text`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (5):**
  - `font_size`: `f32`
  - `caret_width`: `f32`
  - `caret_height`: `f32`
  - `caret_gap`: `f32`
  - `font`: `Option<iced::Font>`

### `SurfaceTokens`

- **File:** `crates/enzui-widgets/src/surface.rs`
- **Module:** `surface`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (12):**
  - `surface`: `Color`
  - `surface_alt`: `Color`
  - `surface_elevated`: `Color`
  - `surface_selected`: `Color`
  - `surface_hover`: `Color`
  - `background`: `Color`
  - `accent_soft`: `Color`
  - `image_canvas`: `Color`
  - `video_canvas`: `Color`
  - `border`: `Color`
  - `border_strong`: `Color`
  - `accent`: `Color`

### `SwitcherTokens`

- **File:** `crates/enzui-widgets/src/view_switcher.rs`
- **Module:** `view_switcher`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** override
- **Fields (11):**
  - `strip_height`: `f32`
  - `tab_height`: `f32`
  - `strip_radius`: `f32`
  - `tab_radius`: `f32`
  - `padding`: `f32`
  - `tab_padding_x`: `f32`
  - `label_size`: `f32`
  - `tab_min_width`: `f32`
  - `auto_floor`: `f32`
  - `focus_ring_width`: `Option<f32>`
  - `focus_ring_outset`: `f32`

### `TabPillTokens`

- **File:** `crates/enzui-widgets/src/tab_pill/tokens.rs`
- **Module:** `tab_pill`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (26):**
  - `selected_bg`: `Color`
  - `selected_edge`: `Color`
  - `selected_fg`: `Color`
  - `unselected_fg`: `Color`
  - `disabled_fg`: `Color`
  - `disabled_bg`: `Color`
  - `segment_disabled_alpha`: `f32`
  - `segment_bg`: `Color`
  - `segment_selected_bg`: `Color`
  - `segment_selected_hover_bg`: `Color`
  - `segment_selected_pressed_bg`: `Color`
  - `segment_hover_bg`: `Color`
  - `segment_pressed_bg`: `Color`
  - `segment_selected_fg`: `Color`
  - `seam`: `Color`
  - `hover_overlay`: `Color`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`
  - `focus_ring_outset`: `f32`
  - `pill_height`: `f32`
  - `pill_radius`: `f32`
  - `pill_padding_x`: `f32`
  - `label_size`: `f32`
  - `icon_size`: `f32`
  - `icon_gap`: `f32`
  - `strip_gap`: `f32`

### `TextAreaTokens`

- **File:** `crates/enzui-widgets/src/text_area.rs`
- **Module:** `text_area`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (13):**
  - `background`: `Color`
  - `border`: `Color`
  - `hover_border`: `Color`
  - `error_border`: `Color`
  - `focus_border`: `Color`
  - `disabled_background`: `Color`
  - `text`: `Color`
  - `muted`: `Color`
  - `disabled_text`: `Color`
  - `selection`: `Color`
  - `area_height`: `f32`
  - `padding`: `f32`
  - `label_size`: `f32`

### `TextGridTokens`

- **File:** `crates/enzui-widgets/src/text_grid.rs`
- **Module:** `text_grid`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (5):**
  - `cell_width`: `f32`
  - `cell_height`: `f32`
  - `font_size`: `f32`
  - `font`: `Option<iced::Font>`
  - `glyph_bleed_x`: `f32`

### `TextPairTokens`

- **File:** `crates/enzui-widgets/src/text_pair.rs`
- **Module:** `text_pair`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (2):**
  - `leading_color`: `Color`
  - `trailing_color`: `Color`

### `TextTokens`

- **File:** `crates/enzui-widgets/src/text.rs`
- **Module:** `text`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (8):**
  - `default_color`: `Color`
  - `muted_color`: `Color`
  - `subtle_color`: `Color`
  - `accent_color`: `Color`
  - `success_color`: `Color`
  - `warning_color`: `Color`
  - `danger_color`: `Color`
  - `disabled_color`: `Color`

### `TitlebarAffordanceTokens`

- **File:** `crates/enzui-widgets/src/toolbar_icon.rs`
- **Module:** `toolbar_icon`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** (no builder setter for this type)
- **Fields (3):**
  - `control_size`: `f32`
  - `glyph_size`: `f32`
  - `slot_height`: `f32`

### `ToastTokens`

- **File:** `crates/enzui-widgets/src/toast.rs`
- **Module:** `toast`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (35):**
  - `text`: `Color`
  - `info_fg`: `Color`
  - `info_bg`: `Color`
  - `info_border`: `Color`
  - `info_secondary_fg`: `Color`
  - `success_fg`: `Color`
  - `success_bg`: `Color`
  - `success_border`: `Color`
  - `success_secondary_fg`: `Color`
  - `warning_fg`: `Color`
  - `warning_bg`: `Color`
  - `warning_border`: `Color`
  - `warning_secondary_fg`: `Color`
  - `error_fg`: `Color`
  - `error_bg`: `Color`
  - `error_border`: `Color`
  - `error_secondary_fg`: `Color`
  - `neutral_fg`: `Color`
  - `neutral_bg`: `Color`
  - `neutral_border`: `Color`
  - `neutral_secondary_fg`: `Color`
  - `max_width`: `f32`
  - `radius`: `f32`
  - `padding_x`: `f32`
  - `padding_y`: `f32`
  - `gap`: `f32`
  - `border_width`: `f32`
  - `title_size`: `f32`
  - `description_size`: `f32`
  - `icon_size`: `f32`
  - `dismiss_glyph_size`: `f32`
  - `elevation`: `u8`
  - `action_radius`: `f32`
  - `control_focus_width`: `f32`
  - `control_hover_alpha`: `f32`

### `ToggleTokens`

- **File:** `crates/enzui-widgets/src/toggle.rs`
- **Module:** `toggle`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (14):**
  - `track_off`: `Color`
  - `track_off_hover`: `Color`
  - `track_on`: `Color`
  - `track_on_hover`: `Color`
  - `thumb`: `Color`
  - `thumb_hover`: `Color`
  - `text_color`: `Color`
  - `description_color`: `Color`
  - `disabled_alpha`: `f32`
  - `focus_ring_width`: `f32`
  - `focus_ring`: `Color`
  - `thumb_shadow`: `Color`
  - `thumb_shadow_blur`: `f32`
  - `thumb_shadow_offset_y`: `f32`

### `ToolbarDividerTokens`

- **File:** `crates/enzui-widgets/src/toolbar_divider.rs`
- **Module:** `toolbar_divider`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (3):**
  - `rule`: `Color`
  - `inset`: `f32`
  - `thickness`: `f32`

### `ToolbarIconTokens`

- **File:** `crates/enzui-widgets/src/toolbar_icon.rs`
- **Module:** `toolbar_icon`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** override
- **Fields (14):**
  - `icon_color`: `Color`
  - `hover_color`: `Color`
  - `pressed_color`: `Color`
  - `active_color`: `Color`
  - `disabled_color`: `Color`
  - `active_background`: `Color`
  - `hover_background`: `Color`
  - `hover_fill_inset`: `f32`
  - `pressed_background`: `Color`
  - `button_size`: `f32`
  - `slot_height`: `f32`
  - `icon_size`: `f32`
  - `focus_ring`: `Color`
  - `focus_ring_width`: `f32`

### `ToolbarSpacerTokens`

- **File:** `crates/enzui-widgets/src/toolbar_spacer.rs`
- **Module:** `toolbar_spacer`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (3):**
  - `divider_color`: `Color`
  - `divider_width`: `f32`
  - `divider_height_fraction`: `f32`

### `TooltipTokens`

- **File:** `crates/enzui-widgets/src/tooltip/tokens.rs`
- **Module:** `tooltip`
- **Palette constructor:** yes
- **Default / preset:** no
- **Setter resolution mode:** pin
- **Fields (11):**
  - `background`: `Color`
  - `text_color`: `Color`
  - `radius`: `f32`
  - `padding_x`: `f32`
  - `padding_y`: `f32`
  - `text_size`: `f32`
  - `gap`: `f32`
  - `max_width`: `f32`
  - `shadow_blur`: `f32`
  - `shadow_offset_y`: `f32`
  - `shadow_color`: `Color`

### `WindowTitleTokens`

- **File:** `crates/enzui-widgets/src/window_title.rs`
- **Module:** `window_title`
- **Palette constructor:** no
- **Default / preset:** yes
- **Setter resolution mode:** pin
- **Fields (5):**
  - `title_size`: `f32`
  - `subtitle_size`: `f32`
  - `height_single`: `f32`
  - `height_double`: `f32`
  - `spacing`: `f32`

