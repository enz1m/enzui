# The shared enzUI theme file

One file, edited by hand or with a theme editor, that every enzUI application
reads for its app chrome. No application should invent a second chrome-theme
source.

> **Default integration.** `theme_file::integration::ThemeIntegration` is the
> framework-owned application seam. `ThemeIntegration::boot()` adopts this
> file by default; only the explicitly named
> `ThemeIntegration::unmanaged_compiled_theme(..)` bypasses it.
> New applications use the same wiring shown below without implementing
> loaders, watcher ordering, or OS-mode races themselves.

Implemented by `enzui_core::theme_file` (schema, loader, runtime) and
`enzui_core::theme_file::registry` (the token table every part reads).

## Where it lives

Highest precedence first:

1. `$XDG_CONFIG_HOME/enzui/theme.toml` - the user's theme (what an editor
   writes).
2. `$XDG_CONFIG_HOME` unset, empty or relative →
   `$HOME/.config/enzui/theme.toml`, when `$HOME` is absolute.
3. Each entry of `$XDG_CONFIG_DIRS` in order, `<dir>/enzui/theme.toml` -
   a system/site theme. `$XDG_CONFIG_DIRS` unset or empty means `/etc/xdg`.
4. No file anywhere → the compiled default: the one shipped family **enzUI
   Nebula**, in whichever mode is active (`enzUI Nebula Dark` / `enzUI Nebula
   Light`). Light and dark are *modes* of that one family, not two theme
   products - `enzui_theme::FAMILY` is the family name and
   `EnzThemeMode::theme_name()` the per-mode one, and no app ships a second
   product palette.

The first regular-file candidate wins; files are not merged across roots
(merging two half-themes from different vendors produces a palette neither
one designed). A root that is not an absolute path is **ignored**, as the XDG
Base Directory specification requires, so `XDG_CONFIG_HOME=.` can never make
an app read a theme out of its working directory.

## Per-application themes

An application can be themed apart from the desktop. Beside `theme.toml`, in
the same `enzui` folder under any of the roots above, `<app-id>.toml` is that
application's own theme, named by the id the application gives its windows
and desktop entry (`io.github.enzui.Terminal.toml`). It has the same shape as
`theme.toml` and layers over it, token by token:

* every token the application file sets replaces the shared one;
* every token it omits is the shared theme's (and, below that, the built-in
  default);
* `mode` is the application file's only when it names one, so an application
  can be light on a dark desktop, or follow the desktop and change only its
  accent.

```toml
# ~/.config/enzui/io.github.enzui.Terminal.toml
version = 1
mode = "light"

[light.palette]
accent = "#e66100"
```

The first regular file wins among the application's candidates, as for the
shared theme. A file that cannot be used (too large, not UTF-8, not TOML)
leaves the shared theme in effect and adds an `AppThemeUnusable` diagnostic
naming it; it never blanks the application. Both files are watched, so an edit
to either applies live.

Every application does this by default. `ThemeIntegration::boot()` follows the
application id the process declares (`enzui_core::app_identity`), and
`DesktopApplicationIdentity` declares itself when the application builds its
window settings, so an application that names its windows needs no theme
wiring of its own; its file applies from the moment the id is declared. An
application that must look like the desktop whatever its users set opts out:

```rust
let (themes, boot) =
    ThemeIntegration::boot_with(ThemeIntegrationConfig::from_env().shared_theme_only());
```

`ThemeIntegration::boot_for(app_id)` and `ThemeIntegrationConfig::app` name the
id explicitly, before any declaration. A plain `ThemePaths` reads the shared
theme alone, as an editor of `theme.toml` must; `ThemePaths::with_app` and
`with_declared_app` add an application's file, and
`ThemePaths::app_user_path` is where an editor writes it.

Resolution is injectable: `ThemePaths::from_env()` captures the environment at
boot, while `ThemePaths::new(config_home, config_dirs, home)` accepts explicit
roots. Non-regular candidates are skipped; open failures and malformed files
are reported rather than silently replaced by a lower-priority theme.

## Shape

```toml
# enzUI shared theme. Every enzUI application reads this file.
# Omitted values inherit from the built-in enzUI Nebula default.
version = 1
name = "My theme"
mode  = "system"        # "system" (default) | "light" | "dark"

# --- common tokens: one value shared by both colour modes -----------------
[spacing]
space6 = 14             # the default 12px "medium" gap

[typography]
body_size = 15
weight_semibold = 650   # CSS-numeric, integer only
text_scale = 1.25       # desktop text size: scales every rung - see "Text size"
# letter_spacing_* is READ-ONLY in this build - see "Typography weights" below

[motion]
normal_ms = 180         # also paces widgets' own small motions (360 = half speed)
easing = "ease_out"     # linear | ease_in | ease_out | ease_in_out
reduced_motion = false

[fonts]
ui = "Adwaita Sans"     # or a generic: sans-serif | serif | monospace | cursive | fantasy
monospace = "Iosevka"

[elevation.level2]
blur = 12
y_offset = 3

# --- colours: one branch per mode, both in the same file ------------------
[light.palette]
accent = "#3584e4"
background = "#ffffff"

[dark.palette]
accent = "#78aeed"
background = "#1b1e24"
```

Rules:

* **Colours are mode-scoped.** Every `palette.*` token must appear under
  `[light.…]` or `[dark.…]`. Writing `[palette]` at the top level is rejected
  with a diagnostic naming the right table - one file, two complete colour
  branches, so a mode switch never falls back to a half-defined palette.
* **The desktop accent fills in.** When the file sets no `palette.accent`
  for the active mode, the desktop's accent colour (the Settings portal's
  `org.freedesktop.appearance accent-color`) replaces the built-in accent
  family: accent, hover, pressed, soft and the focus ring. A file accent
  always wins.
* **Everything else is common.** Geometry, typography, motion, fonts,
  breakpoints, elevation and scrollbar tokens are shared; writing them inside
  a mode table is rejected the same way.
* **Shared `[fonts]` is the layer below the desktop.** When the shared host follows a
  desktop font source (`enzui_core::font_preferences`), the desktop's
  UI/document/monospace choice outranks these keys. A per-app theme file
  (`enzui/<application-id>.toml`) overrides the desktop family, and
  an explicit runtime application override outranks both. A key here applies when no desktop family is selected. An
  unsupported source releases that layer; a failed read retains its last
  successful delivery rather than clearing it.
* **Anything omitted is inherited**, field by field, from the built-in
  default for the active mode. A three-line file is a valid theme, and a
  later framework default change still reaches every token the user did not
  pin.
* **Colours** are `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`. No named
  colours, no `rgb()` functions, no expressions, no `@import`, no script:
  the file is data.
* **Numbers** are plain TOML numbers in the token's documented range.
* **A few tokens are read-only.** The registry records whether a token reaches
  a renderer (`TokenSupport`). A read-only token is still listed, still
  readable, and still documented - but writing it is refused with an
  `Unsupported` diagnostic instead of being stored as a value nothing can
  honour. The read-only keys are the three `typography.letter_spacing_*` rungs;
  iced's text descriptor does not expose letter tracking.
* **Nesting is bounded.** A table/key path may contain at most
  `MAX_THEME_NESTING_DEPTH` components. Arrays and inline tables use the same
  inclusive bound for their container nesting. Quoted dots are part of one
  key, and dots in strings or comments are data, so none of those consume the
  nesting budget. Arrays are not theme token values and are still reported as
  `WrongType`; bounding them before document construction keeps malformed
  schema input safe to reject.

The full token list is the registry: `enzui_core::theme_file::registry::tokens()`
returns every path, its scope, its kind and its range. It is the same table
the loader validates with and an editor lists, so there is no second
"supported tokens" document to drift.

See [configuring framework appearance](styling.md) for global scales, component
snapshots and choosing semantic tokens.

## Sizing and corner coverage

`TokenSupport::Rendered` means there is a supported layout/paint consumer, not
that every widget family follows the token or that explicit geometry pins move.
`control_height` feeds composed toolbar/labeled rows when their sizing scale is
forwarded; it is not the independent five-preset button-height ladder.
`min_hit_target` remains supported, including an app's session action target.

`sizing.row_height` feeds unpinned list rows, trees and disclosure rows.
Virtual tables/lists with explicit caller-owned row geometry retain those pins.
Sidebar rows use their separate `sizing.sidebar_row_height` role.
`sizing.titlebar_height` controls both standalone titlebars and the shell's
inherited default band; an explicit height or shell band retains precedence.
The shell's default sidebar width follows `sizing.sidebar_width`.

Rating stars use `palette.rating`; presence dots use the success, warning and
destructive roles. Badges use the semantic soft backgrounds and paired inks,
including custom palettes, without comparing against stock color literals.

Unpinned surfaces/cards, buttons/groups and the field families have live radius
consumers. This is not a universal corner-style or density switch for every
control: other fixed presets and explicit pixel/token pins retain their geometry.
Consult each widget's documented scale/pin policy rather than deriving geometry
from a palette-only token constructor.

## Component shapes

`[radius]` says how round each step is; `[shape]` says which step a component
family's corners take. A value is a radius step name: `none`, `xs`, `sm`,
`md`, `lg`, `xl`, `xxl`, `full` (a pill) or `window` (the window's own
`window.radius`).

```toml
[shape]
button = "full"        # every button size, filter and input chips, pagination cells
button_group = "full"  # the outline of a segmented button group
search_field = "full"  # an expanded search field
badge = "full"         # badges
card = "lg"            # cards
checkbox = "full"      # checkbox boxes: round; "xs" gives rounded squares
field = "md"           # text fields, dropdowns, text areas (medium size;
                       # small one step squarer, large one rounder)
dialog = "window"      # modal dialog surfaces and the stock dialogs
popover = "xl"         # popovers
popup = "lg"           # menus, dropdown and suggestion lists, the command palette
row = "md"             # rows inside those popups, reorderable list rows
tooltip = "md"         # tooltips and chart tooltips
tab = "lg"             # document tabs
key = "sm"             # key caps, key-capture chips, shortcut badges
media = "lg"           # the image viewer, the image cropper and carousels
```

These are the defaults. A widget whose radius the app pinned keeps it.

Dialogs default to `window`, so a dialog's corners match the window behind
it. Their Close control then sits concentric with the corner, as the window
controls do: 8 px in from the edge at the default 22 px, and the header's
title shares its centre line.

## Focus ring

`stroke.focus_ring` is the width of the keyboard focus ring, in logical pixels,
on every widget that draws one. The default is 2; an accessibility theme can
raise it to 3 or 4 so focus reads at a glance. Dense cells (calendar days,
dropdown triggers) draw three quarters of it. A widget whose ring width the
app pinned keeps it.

```toml
[stroke]
hairline = 1      # separators and thin outlines
focus_ring = 3    # keyboard focus rings
```

## Drag target tint

`[opacity] drop_target = 0.08` sets the accepting native drag target overlay.
The supported range is 0..0.2 so its accent tint preserves destination labels.
The outline follows `palette.focus_ring`, `stroke.focus_ring` and `shape.row`.

## Button elevation

`elevation.button` is the level, 0 to 5, that filled buttons (Primary,
Secondary, Accent, Destructive, Success) are lifted to. The default is 0:
buttons are flat, set apart from the page by their fill. Above 0 a filled
button casts a faint contact shadow in the palette's `shadow` colour, a little
deeper per level, and none while pressed. Outlined and quiet kinds, glass and
high contrast stay flat.

```toml
[elevation]
button = 1             # a faint contact shadow; the default 0 is flat
```

An app raises or flattens one button with `enz_button(..).elevation(level)`.

## Grain

`[grain]` is the theme's grain: the paper-like speckle a surface draws when it
asks for the theme's (`GrainSpec::Theme`), for example a shell pane styled
with `PaneStyle::default().grain(GrainSpec::Theme)`. Surfaces carry no grain
unless they ask for it; these keys shape it once for every app.

```toml
[grain]
intensity = 0.14       # 0..1   alpha of a full-strength speck
density = 0.95         # 0..1   share of the surface carrying specks
scale = 1              # 0.25..8  speck size (1 = one texel per device pixel)
opacity = 1            # 0..1   the whole layer
tone = "both"          # both | dark | light
placement = "behind"   # behind (under content) | over (print look)
dark_ink = "#000000"   # dark speck colour
light_ink = "#ffffff"  # light speck colour
frequency = 1.3        # 0.05..1.5  built-in noise frequency, higher is finer
octaves = 2            # 1..6   built-in noise detail
seed = 7               # 0..65535  built-in noise seed
tile = 256             # 32..1024 px  the repeating tile
motion_fps = 0         # 0 still; 1..60 film-grain shimmer (reduced motion stills it)
```

The grain is common to both colour modes, like the other non-colour tokens.
High-contrast palettes draw no grain. A custom SVG grain is an application
choice (`Grain::svg`), not a theme-file key.

## Window buttons and corners

`[window]` picks the controls a window that draws its own frame shows. The
theme file comes first; at the default `"desktop"` the desktop's button layout
decides (GNOME's `button-layout`, GTK's `gtk-decoration-layout`), and where the
desktop publishes none, only Close shows. Close always shows, so a self-drawn
window stays closable. The desktop's layout still decides which side the
buttons sit on.

```toml
[window]
buttons = "desktop"    # desktop | close | minimize_close | maximize_close | all
radius = 22            # 0..48 px  corner radius of a self-drawn window
frame = "app"          # app | opaque | frosted | flat_frosted | subtle | glass | clear
```

`frame` decides what the frame around and between a window's panes is made
of, for every application. At `"app"`, the default, each application chooses;
`"opaque"` keeps every frame solid; a glass style shows the desktop and the
windows behind each window through its frame, blurred and tinted by the
compositor, while the panes themselves stay filled. Only a compositor that
paints enzUI glass (`enzui-material-v1`, the enzimOS desktop) shows it; on any
other desktop the frame stays opaque. Under a dark theme the frame's glass is
tinted more strongly with the window's own tone and darkened, so bright windows
and wallpapers behind it read as dim frost rather than a pale band.

`radius` rounds the window's corners; its panes take the same radius, 2 px
tighter. The default 22 is concentric with the close control's circle. A window
that is maximized, tiled, fullscreen or framed by the compositor keeps square
corners, and an application that sets its own radius keeps it. The enzimOS
desktop rounds every window it frames with this radius too, unless a radius is
chosen in Settings, Windows.

A desktop that draws the frame itself (server-side decorations) draws its own
controls, and this key does not apply there.

## Window shadow and edge lines

`[window_shadow]` sets the drop shadow around a window that draws its own frame,
and the hairlines that keep two windows of the same colour apart where they
overlap. The defaults are subtle: enough to lift a window off one behind it
without darkening what is around it.

```toml
[window_shadow]
size = 20               # 0..64 px  how far the soft shadow reaches; 0 draws none
offset = 2              # -32..32 px  how far it falls below the window
opacity = 0.22          # 0..1  under the active window
inactive_opacity = 0.11 # 0..1  under every other window
outline = 0.07          # 0..1  dark hairline just outside the edge
highlight = 0.06        # 0..1  light hairline just inside the edge, dark mode only
```

The shadow is a wide soft layer, a tight one close to the edge and the outline
hairline; an inactive window's is lighter and narrower. With `size = 0` only the
outline remains; with `outline = 0` as well, nothing is drawn outside the window.

The platform paints the shadow outside the window's bounds as the compositor
knows them (the Wayland window geometry). A compositor's own active-window
border or highlight, snapping and tiling therefore wrap the window and not its
shadow. The 8 px of shadow next to each edge resizes the window, and the rest
lets clicks through to what is behind. Edges the window is tiled against, and a
maximized or fullscreen window, cast none.

The shadow is Wayland-only and applies while the application draws its own
frame. A compositor that decorates the window itself (server-side decorations,
including Hyprland, which answers every window with them) owns the shadow
there, through its own settings, and enzUI draws none, so a window never gets
two. X11 draws no shadow. The light inside hairline is drawn by the window in
dark mode on any compositor.


## Typography weights

The five `typography.weight_*` tokens are **rendered**. Each is a CSS numeric
weight (1…1000) for one *semantic rung*. Unpinned framework text resolves
the rung its role sits on:

| Token | Rung | Default | Who reads it |
|---|---|---|---|
| `weight_regular` | Regular | 400 | body/caption/footnote text - `TextSize::{Caption2, Caption, Footnote, Subheadline, Callout, Body, Xs, Sm, Md}` |
| `weight_medium` | Medium | 500 | `enz_label`'s default title weight |
| `weight_semibold` | Semibold | 600 | `TextSize::{Headline, Title3, Lg}` |
| `weight_bold` | Bold | 700 | `TextSize::{Title2, Title1, Xl}` |
| `weight_extrabold` | ExtraBold | 800 | `TextSize::LargeTitle` |

The number is mapped onto the nine discrete faces the renderer can select
(`iced::font::Weight`, and through it cosmic-text's 100…900 constants) by
rounding **to the nearest hundred, ties up**: 449 is the 400 face, 450 is the
500 face, anything from 850 up is the 900 face. The mapping lives once, in
`enzui_theme::typography::css_weight_to_iced`; the rung table lives once,
in `TextSize::weight_rung`. Both painting (`EnzText`) and measurement
(`MeasureTypography`) resolve through them, so a themed weight cannot paint one
face and wrap at another.

A widget that pins a face explicitly (`enz_text(..).weight(Weight::Bold)`) keeps
it under every theme. A forwarded `.typography(scale)` wins over the published
application defaults; otherwise the current semantic rung applies. The compiled
defaults retain the usual faces when the file does not mention weights.

### Application-wide family and weight defaults

`ThemeIntegration` publishes the resolved `fonts.ui` and all five weight rungs
through `DesktopFontFeed`. `enz_text(..)` and framework control labels read them
without per-widget `.font_from_theme(..)` or `.typography(..)` calls. Button,
menu, preference-row, tab, sidebar, field, card and prose-block text retain their semantic
rungs. Measurement resolves the same family and weight as painting. A
`.size_px(..)` pin still follows the family and weight while preserving its
pixel size; an explicit `.weight(..)` or `.font_from_theme(..)` pin wins.
`font_role(..)` selects an unpinned catalog role.

The default desktop font feed uses the freedesktop Settings portal with
GTK/KDE settings files as fallback. GNOME publishes font choices in the
`org.gnome.desktop.interface` extension namespace; no GNOME library is needed.
There is no universal freedesktop font key. Initial preferences and live
`SettingChanged` deliveries use the same path. A desktop UI description also
sets the base text size (points use 96 logical DPI) and shifts the semantic
weight ramp. Explicit theme body size and weight tokens retain precedence,
and accessibility text scaling applies once. The text engine tries the requested
family's nearest weight before using another family, so a static font without
a Medium or Semibold face still follows the chosen family. Missing preferences
restore the built-in ramp.

Stock iced text with its default font follows the UI family, size and regular weight
through the renderer default task. Explicitly chosen code, terminal, document
and custom-canvas fonts retain their own policies.

A host constructing `EnzTheme` directly must publish its resolved theme at boot
and after font or typography changes, rebuild its views, and invalidate its
windows. Using `DesktopFontFeed::renderer_sync` provides both publication and
the renderer/invalidation task; return that task from the application update:

```rust
let task: iced::Task<Message> = font_feed.renderer_sync(&theme);
```

A host without a feed can publish and synchronize manually:

```rust
use enzui_theme::{text_defaults, FontRole, WeightRung};
text_defaults::publish(&theme);
let task: iced::Task<Message> = iced::font::set_defaults(
    text_defaults::font(FontRole::Ui, WeightRung::Regular),
    iced::Pixels(16.0),
);
```

`text_defaults::TextDefaultsScope` provides a nested thread-local override for
isolated view construction and measurement; it does not replace publication in
an application with several windows or threads.

### Letter spacing is read-only

`typography.letter_spacing_tight` / `_normal` / `_wide` exist, are documented,
and are **not rendered**. iced's text descriptor carries no tracking field, so
nothing downstream of the theme can shape with the value; accepting the edit
would save a number that changes no glyph advance, no wrap point and no measured
width. The file layer therefore refuses the write and says why, so an editor can
render the three controls read-only with the same sentence attached.

The rungs stay in `TypographyScale` (at their compiled defaults) for the day the
descriptor grows the field. Two things are recorded here so that bridge is not
built on a guess: these values are **logical pixels**, while cosmic-text's
`Attrs::letter_spacing` is **em-relative** - wiring them requires dividing by the
resolved font size, not passing the number through.

## Modes

`mode` states what the *user* wants; the OS states what the desktop is doing;
`ENZUI_PROBE_LIGHT=1` / `ENZUI_PROBE_DARK=1` state what a deterministic
capture needs. Precedence, highest first:

1. `ENZUI_PROBE_LIGHT` / `ENZUI_PROBE_DARK` (light wins if both are set),
2. a process-local `ThemeIntegration::set_mode_override(Some(mode))`,
3. `mode = "light"` / `mode = "dark"` in the file,
4. the OS colour scheme, when `mode = "system"` (the default).

`set_mode_override(None)` resumes the file/desktop preference without writing
the file. Probe overrides keep priority.

The probe variables select a **mode**, never a second theme: the tokens still
come from this file, so a capture proves the real user theme in a known mode.

## Text size (Large Text)

`typography.text_scale` is one bounded multiplier (0.5–3.0) on **every** size
rung. It is the desktop “Large Text” control for enzUI applications: the twelve
semantic rungs and the line boxes derived from them move together, so a 125%
setting resizes captions, body text, headings, editors and dialog copy by the
same ratio instead of ten applications each inventing a ladder.

```toml
[typography]
body_size  = 15     # the authored base ramp
text_scale = 1.25   # what the user asked every enzUI app to apply to it
```

A pinned rung is a **base** value: the scale multiplies it, whatever order the
two keys appear in, so the example above resolves `body_size = 18.75`.
`EnzTheme::text_scale()` reports the factor a resolved theme carries, and
`EnzTheme::with_text_scale` replaces it rather than compounding with it.

### Precedence

`enzui_theme::text_scale::TextScalePolicy` resolves four layers, highest
first; `enzui_core::theme_file::ThemeRuntime` (and the `ThemeIntegration`
seam above it) publishes the winner:

1. **User** - an explicit in-application choice
   (`ThemeIntegration::set_user_text_scale`), the analogue of a preferences
   control. `reset_text_scale` withdraws it again.
2. **Theme** - `typography.text_scale` in this file. An explicit `1.0` is a
   request for 100% and outranks a session that asks for more, exactly as
   `mode = "dark"` outranks the OS.
3. **Platform** - session delivery: the `ENZUI_TEXT_SCALE` variable
   (`ENZUI_TEXT_SCALE=1.5`) when set, otherwise the desktop's text scaling
   factor (GNOME's `org.gnome.desktop.interface text-scaling-factor`, read
   through the Settings portal and followed live). A desktop adapter can feed
   the same layer through `ThemeIntegration::accept_text_scale_delivery`.
4. **Application** - the compiled default
   (`ThemeIntegrationConfig::application_text_scale(...)`), for a host that must ship
   a size where no desktop delivers one.

With no layer set the factor is 100% and the ramp is bit-identical to the
compiled one. A desktop that publishes no text scaling factor is the ordinary
case rather than an error.

### Bounds, faults and reset

| Delivered value | Result |
|---|---|
| 0.5–3.0 | applied |
| a finite number outside that range | clamped to the nearest bound (`TextScaleAdmission::Clamped`) |
| `NaN`, ±inf, zero or negative | 100% (`TextScaleAdmission::Rejected`) |
| unset, empty, or not a number | no platform layer at all - the layers below stay in force |
| a bad `typography.text_scale` in this file | the usual per-key diagnostic (`OutOfRange` / `NotFinite` / `WrongType`); the ramp keeps its pins |

### What the scale deliberately does not touch

* **Monitor scale.** A window's device-pixel ratio is applied by the windowing
  layer to the whole frame. The text scale multiplies *logical* sizes exactly
  once, so 150% text on a 2× display paints 3× the baseline glyph height, not
  4.5×. Spacing, radii and sizing tokens are layout policy and do not move.
* **Explicit pixel pins.** `EnzText::size_px` and `EnzTextArea::text_size_px`
  are absolute logical sizes and stay where the caller put them, which is what
  keeps a document-zoom control independent of the accessibility preference.
  Compose the two deliberately with `TextScale::apply_px`.
* **Weights and tracking.** A text scale is a size policy; `typography.weight_*`
  and the read-only `letter_spacing_*` rungs are unchanged by it.

Text that is never handed a `TypographyScale` still follows the preference:
`ThemeIntegration` publishes the effective factor to
`enzui_theme::text_scale::effective`, which the semantic ramp reads when no
scale was forwarded. The two carriers are published together and a resolver
applies the factor once. An unforwarded size uses its built-in ramp at that
factor; it does not inherit custom base sizes from the theme file. Forward the
live `TypographyScale` to consumers that must use those authored sizes.
Forwarded and ambient text can also have different line-box rules, so matching
scale factors alone do not guarantee identical measurements.
`TextScaleScope::enter` provides a thread-local factor override for isolated
measurement.

## Reduced motion

`ThemeIntegration` also owns the reduced-motion preference that is carried by
`EnzTheme.motion.reduced_motion`. Applications using the normal integration do
not need a separate startup task, subscription, or field: the existing
iced/mundy host adapter supplies the startup value and live changes, and the
integration republishes the served theme when the effective policy changes.

Precedence is explicit, highest first:

1. an in-application override set with
   `ThemeIntegration::set_reduced_motion_override`;
2. an explicit `motion.reduced_motion` value in this file, including `false`;
3. a platform request to reduce motion; and
4. the animation-enabled framework default.

For host adapters reading `org.freedesktop.appearance` / `reduced-motion`,
`ReducedMotionPreference::from_portal_value` decodes `0` as no preference, `1`
as a request to reduce motion, and unknown values as no preference.
The default Linux route remains iced's mundy integration, so a
working GNOME, KDE, GTK, portal, or other host implementation is accepted
without enzUI requiring its toolkit or schemas. Iced's existing boolean surface
folds an unsupported or failed query into no preference; a host adapter that
can distinguish those outcomes uses the typed variants below.

`ThemeEvent::ReducedMotionBootstrap` and `ThemeEvent::ReducedMotionChanged`
keep startup and live delivery distinct. Once a live value arrives, a delayed
startup snapshot is stale and cannot overwrite it. A custom shell can inject
`ReducedMotionRead::Delivered`, `Unsupported`, or `Failed` through the same
events. `Unsupported` and delivered no-preference withdraw the platform layer;
a failure is not a preference and retains the last successful delivery.
Identical effective values do not advance `theme_generation`, and rebuilding
the theme after a file or colour-scheme event reapplies the policy instead of
resetting it to the compiled default. The integration only consumes host
preferences; it publishes no D-Bus service and provides no settings UI.

## What happens when the file is wrong

| Fault | Behaviour |
|---|---|
| TOML syntax error | The whole file is rejected. A running app keeps its **last known-good** theme and reports the parser's line/column; a launching app uses the compiled default. |
| Duplicate key | TOML forbids it, so this is a syntax error - same handling as above. |
| Unknown key | Ignored, with an `UnknownKey` diagnostic naming it. Forward-compatible: a file written by a newer enzUI still loads. |
| Wrong type (`space6 = "twelve"`) | That key is dropped with a `WrongType` diagnostic; every other key still applies. |
| NaN / ±inf | Dropped, `NotFinite`. |
| Out of range (negative radius, alpha `2.0`) | Dropped, `OutOfRange`, message quotes the accepted range. |
| Malformed colour | Dropped, `BadColor`. |
| Unknown enum word | Dropped, `BadEnum`. |
| Font description naming no family | Dropped, `BadFontFamily`. |
| Mode-scoped key at the top level (or vice versa) | Dropped, `WrongScope`, message names the correct table. |
| A read-only token given a non-default value (`letter_spacing_wide = 2`) | Dropped, `Unsupported`, message names the limitation. |
| `typography.text_scale` outside 0.5–3.0 | Dropped, `OutOfRange`; the remaining user/platform/application precedence determines the scale. Positive finite values delivered by a session or host are clamped instead. |
| A read-only token given its compiled default | Silently ignored: the file asks for nothing this build cannot deliver, so a theme written before the token became read-only still loads clean. |
| Non-monotonic breakpoints | **Normalised**, not dropped: `BreakpointScale::new` raises each edge to at least its predecessor, so a band can never become unreachable. |
| `version` missing | Assumed current, `MissingVersion` diagnostic. |
| `version` from the future | Known keys still apply, `UnknownVersion` diagnostic. |
| File > 64 KiB (`MAX_THEME_FILE_BYTES`) | Refused. The ceiling is enforced at **read time** (see below), so the refusal holds even if the size changes after the check. |
| Table/key path or array/inline-table nesting > `MAX_THEME_NESTING_DEPTH` | Refused before a TOML document is constructed, as typed `ThemeParseError::NestingTooDeep { depth, limit }`. |
| Not valid UTF-8 | Refused, `NotUtf8`. |
| Symlinked `theme.toml` | Followed - a dotfile manager may symlink the file into `~/.config`. The read-time ceiling applies to the target. |
| A directory / FIFO / device at a candidate path | It **opens** (on Unix with `O_NONBLOCK`, so a writer-less FIFO cannot block), then the descriptor's `fstat` says "not a regular file": skipped like an absent candidate, so precedence falls through to the next root. |
| A candidate that cannot be opened at all | An `Io` fault **naming that path** - no fall-through. This covers an unreadable file (no permission) and a Unix socket, whose `open` fails with `ENXIO`. Substituting a system theme for an unreadable user file would hide the fault instead of reporting it. |
| File absent | The ordinary case: compiled default. A dangling symlink is `NotFound` and behaves the same way. |

A per-key value fault leaves valid neighboring keys available. A whole-file
failure preserves the running application's last known-good theme.

### Read-time size and symlink safety

The loader opens each candidate **once**, `fstat`s the open descriptor (not
the path), and reads through a cap of `MAX_THEME_FILE_BYTES + 1` bytes. The
bound is therefore a property of the bytes actually pulled, not of a `stat`
that a later write could invalidate. This closes three real holes a
stat-then-read design leaves open:

* a **symlink swapped** between the check and the read (e.g. re-pointed at
  `/dev/zero`);
* a file **growing** under a live editor between the check and the read;
* a **length-lying source** - a `/proc`-style file is a *regular* file whose
  `stat` size is `0` and whose stream is megabytes long, so a stat-only gate
  would incorrectly accept it.

On Unix the open carries **`O_NONBLOCK`**. A read-only `open(2)` of a FIFO
with no writer blocks *inside the open*, before any descriptor check can run,
so without the flag a named pipe planted at a candidate path would hang the
loading thread forever rather than being skipped.

Symlinks are deliberately followed (dotfile managers are a first-class use
case), and a symlink to a directory, FIFO or device resolves through the
descriptor's `fstat` to "not a regular file" and is skipped. No `O_NOFOLLOW`
is used: the theme file is non-secret user config, and refusing symlinks would
break the common managed-dotfiles workflow for no security gain (the file
grants no capability); a swap after the open cannot redirect anything, because
every later step uses the descriptor, never the path. What is **not** skipped
is a candidate that fails to open - that is an `Io` fault for that path, so an
unreadable highest-precedence file is reported instead of being silently
replaced by a system theme. Permissions on save are `0644` - user config,
never group/world writable.

## How an application adopts it

The integration API is identical for `iced::application` and `iced::daemon`;
its timer does not depend on an open window.

```rust,ignore
use enzui_core::theme_file::integration::{ThemeEvent, ThemeIntegration};

enum Message { Theme(Box<ThemeEvent>), /* app messages */ }
struct App { themes: ThemeIntegration, /* app state */ }

// boot: shared XDG is the default
let (themes, boot) = ThemeIntegration::boot();
let app = App { themes };
(app, boot.map(|event| Message::Theme(Box::new(event))))

// update: the integration owns load/watch/OS ordering
Message::Theme(event) => {
    let before = app.themes.theme_generation();
    let task = app.themes.update(*event);
    if app.themes.theme_generation() != before {
        app.invalidate_theme_dependent_caches();
    }
    task.map(|event| Message::Theme(Box::new(event)))
}

// subscription and iced theme
app.themes.subscription().map(|event| Message::Theme(Box::new(event)))
app.themes.theme().iced_theme()
```

The only opt-out is explicit and named:

```rust,ignore
let (themes, no_boot_work) =
    ThemeIntegration::unmanaged_compiled_theme(EnzTheme::dark());
```

* The parse happens **once per change**, off the UI thread. Nothing stats,
  reads or parses in `view`, `layout` or `draw`; rendering reads the immutable
  resolved theme.
* The integration watches every bounded XDG candidate, so creation of a new
  higher-precedence user file, atomic replacement, in-place edits and deletion
  all re-resolve precedence. It debounces transitions and rejects stale load
  generations.
* `status()` reports `Default`, `Loaded`, typed `LastKnownGood { error }`, or
  `UnmanagedCompiled`; `diagnostics()` exposes per-key parse diagnostics and
  `watch_diagnostic()` exposes a transient probe failure. A malformed save
  never replaces the last-known-good theme.
* `theme_generation()` advances when the document or the effective mode
  changes, and when a resolved preference policy changes: text scale, desktop
  font families, or reduced motion. It is not a diff of the rendered tokens, so
  a replacement document that resolves identically still advances it, while
  identical platform deliveries, baseline reconciliation, malformed loads, and
  stale loads do not invalidate caches. Treat it as the invalidation key for
  app-owned build-time layout inputs.

### Cache boundaries

The adopted shape is: store `themes: ThemeIntegration`, box `ThemeEvent` in
the app's message, batch `boot()`'s task with existing boot work, delegate the
event in `update`, batch `subscription()` with existing subscriptions, and read
`themes.theme()` in the iced theme callback and views. Do not keep an older OS
snapshot or subscription beside it.

When `theme_generation()` changes, invalidate any cache that includes font
metrics or theme-derived geometry (pagination, measured row heights, editor
viewport measurements) and rebuild shell breakpoints from the live theme.
Content-only caches (thumbnails, artwork, search projections) stay valid.

An application that keeps its own bootstrap instead of `ThemeIntegration` must
still read the document once at boot and **overlay it** (`ThemeDocument::apply`)
on every theme payload its system-theme handlers receive. The payload supplies
the base tokens it legitimately carries (a runtime-adjusted motion policy, for
instance); every token the user pinned wins over it, and a payload whose mode
lost the precedence contest is dropped for the compiled default of the winning
mode. Assigning the payload directly loses the user's tokens on the first
colour-scheme change.

`ContrastFloor` and the theme's contrast resolvers describe foreground/background
requirements. Loading a user palette does not validate every painted pair;
an editor should check contrast for the actual foreground and background.

## Writing the file

`save_theme_atomic` writes a temp file in the destination directory,
`fsync`s it, and `rename`s it into place with mode `0644`. A concurrently
launching app therefore sees either the whole old file or the whole new one,
never a half-written mixture.

`ThemeDocument::to_toml` on a parsed document writes an edit of the file it
came from: the keys an editor set or cleared change, and everything else stays
as it was, including comments, key order, keys and values this build does not
understand, and a newer `version`. An editor built on an older enzUI therefore
cannot delete what a newer release added. A document started from
`ThemeDocument::empty` is written canonically, overrides only. An editor should
still never save over bytes it could not read (above `MAX_THEME_FILE_BYTES`,
or an IO failure).

## Compatibility between versions

The file is shared by applications built on different enzUI releases, so each
release reads what it knows and leaves the rest:

* A key a build does not know, or a value it cannot read (a new enum word, a
  value past the range it accepts), is skipped with a diagnostic; that token
  keeps the build's default and every other key still applies.
* Saving keeps those keys and values (see above), so the newer release still
  finds them.
* Keys are added, never renamed or given a new meaning, and enum words are
  only added. A key whose meaning would have to change gets a new name instead.
* `version` is raised only for a change in the file's structure. A build
  reading a newer `version` reports it and still applies the keys it knows.


## Known gaps (do not assume these are covered)

* **Workspace accents** (`WorkspaceAccentPalette`) are derived from the
  palette's mode rather than stored on `EnzTheme`, so they are not registry
  tokens yet. Making them editable means adding a field to `EnzTheme` and its
  `iced::Theme` transport first.
