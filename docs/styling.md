# Configuring framework appearance

Use an `EnzTheme` for shared palette, spacing, corner, sizing, typography,
stroke, elevation and motion scales. Supply `theme.iced_theme()` through the
application's iced theme callback. Stock iced themes provide their colours and
use the framework's default scales.

The [component token index](tokens.md) lists public overrides. A component's
`.tokens(...)` setter usually pins a snapshot. Rebuild that snapshot when the
application theme changes. A `from_palette` constructor receives colours only;
it cannot inherit spacing, rounding or typography from an `EnzTheme`.

Global scales apply where the component documents a consumer. They do not
rescale every preset automatically. In particular, `sizing.control_height` is
not the button-height ladder. `sizing.row_height` sizes unpinned list, tree and
disclosure rows; explicit row geometry retains precedence. See
[theme-file coverage and precedence](theme-file.md#sizing-and-corner-coverage).

## Choosing a token

| Meaning | Configuration |
|---|---|
| Shared semantic colour | `EnzTheme::palette`; use the role for the intended state and part |
| Shared padding or sibling gap | `SpacingScale` / `SpaceToken`, resolved against the configured scale |
| Shared surface corner | `RadiusScale` and its rungs (`xs`, `sm`, `md`, `lg`), where the widget follows that scale |
| Separator or thin outline | `StrokeScale::hairline` / `StrokeToken::Hairline` |
| Keyboard focus ring width | `StrokeScale::focus_ring` / `StrokeToken::FocusRing` |
| Control or window extent | The appropriate `SizingTokens` role, where documented |
| Text hierarchy | `TypographyScale` / `TextSize`; a monospace metric is not a label size |
| Component-specific paint or geometry | The component's tokens or explicit builder override |
| Full circle, pill or proportional inset | A shape or ratio derived from the configured geometry |
| Measurement, time, protocol or algorithm constant | Keep the value with its owning API; it is not an appearance token |

Reuse a role because its meaning matches. An equal default number does not make
a heading size suitable for a control label, or a surface corner suitable for a
circular indicator. Add a component token for an independently configurable
property with no existing role. Add a global role when multiple components
should deliberately change together.

A literal in a token constructor defines a default. A literal used directly in
paint or layout needs a documented override or a reason it is a geometric or
algorithmic constraint. Calling `Scale::default()` or a token's `.px()` method
also reads a default; use a configured scale's resolver when custom density is
intended.

## Following the desktop

An app that takes its theme from `ThemeIntegration` (the shared theme file)
follows two desktop colour settings without code of its own:

- **Accent.** The desktop's accent colour (the Settings portal's
  `accent-color`) replaces the built-in accent, unless the theme file sets
  `palette.accent`; see [theme-file.md](theme-file.md).
- **Contrast.** When the desktop asks for more contrast (GNOME's High
  Contrast, the portal's `contrast` setting), every palette, the theme file's
  included, becomes `Palette::high_contrast()`: surfaces take the page colour
  and are told apart by edges in the text colour, and the accent, state
  colours and focus ring reach 7:1 against the page. Glass turns opaque.

A palette built in code can call `Palette::high_contrast()` itself, for an
in-app contrast setting.

## Liquid glass

Most widgets with a panel take `.material(style)`, which puts them on
sampled-backdrop glass of their own shape:

```rust
enz_banner("Update installed")
    .material(MaterialStyle::Regular)
```

| Style | Use |
|---|---|
| `Regular` | The default: clear glass with a bending rim, for controls and chrome |
| `Subtle` | A light frost, slightly darker, for controls over busy content |
| `Frosted` | Frosted and darker, with maximum luminance roll-off, for panels that carry text |
| `Lens` | Strong refraction and colour fringe on a deep rim, for hero elements |
| `Clear` | Media glass: light ink over glass that dims bright content |
| `Optical` | Diagnostic optical geometry with zero tint and configurable lighting and diffusion |
| `FlatFrosted` | Flat theme-tinted blur with theme ink, without lens distortion or rim lighting |
| `Solid` | The opaque panel, with no glass |

`MaterialStyle::ink` selects the foreground policy:

- Every style except `Clear` keeps the app's **theme ink** and fill, whatever
  passes beneath. Their blur, tint and lighting presets determine the backdrop
  treatment.
- `Clear` is media glass: always light ink, over glass that dims bright
  content just enough. Use it for controls over video and photos.
- **Adaptive ink** is an opt-in (`enz_glass(..).ink(GlassInk::Adaptive)`,
  `EnzSurface::glass_ink`). The renderer measures the average brightness
  behind the glass surface, and its ink is dark over light content, light over
  dark, whatever the app's mode. It does not chase frames: a backdrop between
  the two thresholds keeps the current ink, a change must last about a second
  before the ink switches, and the switch is a 0.3 s crossfade, so a playing
  video does not flicker it. A new surface settles for a moment first, so a
  page still loading its images does not lock in the wrong ink.

Contrast protection adjusts backdrop luminance; it does not guarantee a
foreground contrast ratio on arbitrary content. Check text against the rendered
backdrop. `MaterialInk::Protected` reserves the material's protected inset for
content; the inset is layout metadata, not a shader mask.

Controls inside glass default to glass. Their fills and borders turn
translucent, secondary text becomes the ink at reduced opacity, and ghost
glyphs take the ink instead of the accent. Tokens a caller pinned are kept.

`enz_glass(widget)` wraps anything else; give it the content's radius with
`.radius(px)`, or `.radius_resolved(|theme| ...)` when the content's corners
follow the theme's radius scale. Menu buttons, context menus and action bars
take `.menu_material(style)` for their popups, and app-shell panes take
`PaneStyle::material`. Reduced transparency, increased contrast and an
unsupported renderer each paint the opaque panel instead.

### Glass over the desktop

A pane can show the wallpaper and the windows behind the app instead of the
app's own drawing. The compositor paints it from the same tokens over
`enzui-material-v1`, so `Regular` looks the same in both places:

```rust
// Once the window is open:
desktop_glass::bind(id).map(Message::Glass)

// In the view:
let sidebar = enz_surface(places)
    .material(MaterialStyle::Regular)
    .backdrop(Backdrop::Window);
enz_desktop_glass(&state.glass, row![sidebar, content])
```

App-shell panes take `PaneStyle::material(style).behind_window()`. The window
must be transparent, with nothing painted under the pane (`shell_tone:
SurfaceTone::Transparent` in a shell). A pane over the app's own content keeps
in-app glass: the compositor sees only what lies below the window. The pane
draws nothing until `bind` resolves. A compositor without the protocol gets
in-app glass, and reduced transparency makes the pane solid. Subscribe to
`desktop_glass::luminance_changes` so adaptive ink follows the desktop while
the window is idle.

Keep window-backdrop panes outside scrollables: their regions use window
coordinates without scroll clipping. The compositor receives at most 32 panes.
On a compositor that measures luminance, a pane waiting for its measurement can
put the whole window's glass into measurement-only mode.

## Grain

Grain is a paper-like speckle on a surface. It is not a material: it lies on
any surface's fill (or on glass) and works in every renderer.

```rust
enz_surface(content).grain(Grain::default())        // under the content
enz_surface(content).grain(GrainSpec::Theme)        // the theme's [grain]
PaneStyle::default().grain(Grain::subtle())         // a grainy shell pane
enz_grain(picture).grain(Grain::paper())            // over any element
```

| Preset | Look |
|---|---|
| `Grain::default()` | Fine light and dark specks on nearly every pixel, quiet enough for UI panels |
| `Grain::subtle()` | The same, barely there, for large surfaces |
| `Grain::paper()` | Strong, laid over the content too: a printed page |
| `Grain::film()` | Coarser specks that shimmer at 24 fps |

Every knob is a builder: `intensity`, `density`, `scale`, `tone` (light and
dark specks, or one), `inks`/`tint`, `opacity`, the built-in noise's
`frequency`, `octaves` and `seed`, `tile`, `tiling`, `placement` and
`motion`. `Grain::svg(document)` takes any SVG: its brightness becomes specks,
or `.mapping(GrainMapping::Artwork)` draws it in its own colours. The renderer
does not stitch `feTurbulence` tiles, so an `feTurbulence` SVG gets
`GrainTiling::Blend`, which cross-fades its edges into a shifted copy; art that
does not repeat by itself can use `GrainTiling::Mirror`.

The built-in grain is periodic noise generated in code, so its tile repeats
without a seam. Tiles are made once per screen scale and cached; a grainy
surface costs one image draw per visible tile, still grain asks for no frames,
and film grain redraws its window at its frame rate (reduced motion stills
it). High-contrast palettes draw no grain. The theme file's `[grain]` table
shapes the theme's grain (see the theme file guide).

## Selection controls

Checkboxes follow the live hairline stroke unless `CheckboxTokens::border_width`
is `Some`. Invalid overrides use the live stroke; zero removes the outline.
`CheckboxTokens::radius` selects the box and focus shape; `None` uses the size's
circular shape. This shape is independent of surface rounding.

`CheckboxTokens` and `ToggleTokens` expose `focus_ring_width` in logical pixels
(default: the theme's `stroke.focus_ring`, 2) and `disabled_alpha` (default `0.62`). Disabled opacity multiplies
the configured colour alpha, preserving transparent fills. It does not change
focusability or activation. A non-finite opacity uses the default; finite values
are clamped to `[0, 1]`. Invalid focus widths use the default; zero hides the ring.

For example, derive a checkbox snapshot from the application's current palette:

```rust
use enzui_theme::EnzTheme;
use enzui_widgets::checkbox::{CheckboxTokens, enz_checkbox};

let theme = EnzTheme::dark();
let tokens = CheckboxTokens {
    radius: Some(3.0),
    border_width: None,
    focus_ring_width: 2.5,
    disabled_alpha: 0.5,
    ..CheckboxTokens::from_palette(&theme.palette)
};
let checkbox = enz_checkbox(false, "Remember me", |_| ()).tokens(tokens);
```

Radio buttons and groups share `RadioTokens`. Their unselected outline follows
the live hairline unless `border_width` is pinned; the selected circle uses twice
that extent. Focus width and disabled opacity have the same defaults and invalid
value policy as checkboxes. `focus_ring_radius` controls the activation outline's
corners (default `4.0` logical pixels), independently of the circular indicator.
The group's focus outset follows its focus width. These are per-component roles,
not surface corners selected by matching default numbers.

Switch track and thumb dimensions use `ToggleSize` presets or the toggle's
`.geometry(SwitchGeometry)` override. Shadow geometry and colour are separate
`ToggleTokens` fields. The bare switch accepts a `.colors(...)` resolver for
state-specific paint; its disabled thumb icon follows the resolved thumb alpha.

Segmented tab strips expose `TabPillTokens::segment_disabled_alpha` for selected
cells that are disabled (default `0.55`). It multiplies both the selected fill
and foreground alpha, including the moving indicator and SVG icons. Unselected
disabled cells continue to use `disabled_bg` and `disabled_fg`. Non-finite values
use the default; finite values are clamped to `[0, 1]`. Pill geometry already has
`TabPillTokens` fields and the geometry-only `TabPillGeometry` override.

Tab focus rings use component roles rather than the hairline separator role.
`TabPillTokens`, `TabPillGeometry` and `SwitcherTokens` expose
`focus_ring_width` and `focus_ring_outset`. The width defaults to the theme's
`stroke.focus_ring` (an unset `Option` on the geometry presets); the outset to
`2.0` logical pixels. Invalid dimensions use the defaults. Zero width hides the ring; zero
outset draws within the cell. Single pills draw within their own bounds and do
not consume the composite-strip outset. Larger outsets need parent clearance;
framed view switchers also need sufficient track padding to avoid clipping.
`TabPillGeometry` wins over pinned token geometry, preserving live colours when
no colour snapshot is pinned. `SwitcherTokens` forward these dimensions to both
pill and inline segmented presentations. See the compiled example on
`SwitcherTokens::tab_pill_geometry`.

The full-screen overlay's close control uses shared `ButtonTokens` through
`.close_tokens(...)`, including Ghost rest/hover/pressed fills, ink, border and
focus paint. This snapshot takes precedence over `.close_color(...)` in either
builder order. `.close_icon_size(...)` configures its glyph (default `16.0`
logical pixels), independently of `.close_size(...)` (default `36.0`).
`.close_inset(...)` configures the top/right inset (default `16.0`). Invalid
values use the defaults; glyph and control sizes must be positive, and the
inset may be zero. Its circle is derived from the control diameter. The default
white ink and washes are relative to the dark scrim, rather than the app's
surface colour; changing the scrim also calls for reviewing that paint.

Progress bars and sliders accept `.typography(theme.typography)` and
`.spacing_scale(theme.spacing)` for captions. They read the existing
`label_size` and `space2` roles; they do not borrow an equal-valued caption or
body rung. Both label and value runs use the forwarded typography, and the gap
applies between the runs and between the caption and control. Rebuild with the
current scales when the theme changes: layout happens before the runtime theme
is available. Without forwarding, these roles use the built-in defaults (`11.0`
and `4.0` logical pixels). Invalid label sizes use the default; gaps may be zero
and invalid gaps use the default. See the compiled example on
`EnzProgressBar::typography`.

Slider rail and thumb outlines follow the live `StrokeScale::hairline` unless
`SliderTokens::track_border_width` or `thumb_border_width` is pinned. Invalid
pins use the live stroke; zero hides the outline. Focus width/outset and thumb
shadow offset/blur have their own component roles, defaulting to 2/3 logical
pixels and (0, 2)/4 respectively. Nonnegative dimensions reject negative and
non-finite values in favour of defaults; shadow offsets allow negative values
and replace non-finite axes independently.

`SliderTokens::disabled_alpha` multiplies original alpha across captions,
ordinary rails, gradient stops, the alpha checkerboard, outlines, step marks,
thumbs and shadows. Its default and invalid-value policy match selection
controls. Focus paint retains its own ink; disabled sliders release focus.
The thumb's capsule radius and rail's half-thickness radius remain derived from
configured geometry. See the compiled example on `EnzSlider::tokens`.

Slider stop dots use `SliderTokens::step_mark_size` (default 3 logical pixels)
and `step_mark_min_spacing` (default 8 logical pixels between centres). Invalid
values use those defaults; zero size hides dots without changing snapping, and
zero spacing allows dense dots within the 64-interval paint limit. Dot colours
follow the thumb ink and disabled opacity; their circular radius is half the
configured diameter. Custom gradient rails suppress dots. The interval cap is
an algorithmic work limit, rather than an appearance role.

Colour-picker plane and preview-swatch outlines follow the live
`StrokeScale::hairline`; their ink and corners already use `palette.border` and
`radius.sm`. Zero stroke hides those outlines; invalid strokes resolve to zero
through the shared `StrokeToken::Hairline` policy. HSV white/black gradient
endpoints define the saturation and brightness axes; they stay fixed to
represent the selected colour accurately.

## Colour-picker geometry

`ColorPickerSizeTokens`, passed to `EnzColorPicker::size_tokens(...)`, sizes the
picker's fixed parts in logical pixels:

| field | default | notes |
|---|---|---|
| `handle_radius` | 11 | Plane handle radius. Values below 4 are raised to 4 so both contrast rings fit. |
| `handle_focus_width` | `stroke.focus_ring` (2) | Width of the handle's keyboard focus ring. Zero hides it. |
| `handle_focus_outset` | 3 | Gap between the handle and its focus ring. Zero draws the ring on the handle's edge. |
| `track_height` | 14 | Hue and alpha rail thickness. |
| `swatch_width`, `swatch_height` | 44, 28 | Live preview swatch. |

`NaN`, infinities and negative values use the default, as does zero for a
field that must be positive. The plane's height has its own setter,
`plane_height(...)`. The handle's 1 px dark and 2 px white rings are not
configurable: together they keep the handle visible on both the white and the
black corners of the plane, in any theme. The focus ring uses the palette's
`focus_ring` ink. See the compiled example on `EnzColorPicker::size_tokens`.

## Transparency previews

Slider alpha ramps and colour-picker previews share `CheckerboardTokens`.
Pass a snapshot to `TrackPaint::translucent(...).checker_tokens(...)` or
`EnzColorPicker::checker_tokens(...)`; the picker applies it to both the alpha
rail and the live swatch. The default cells are 5 logical pixels, with neutral
RGB 0.85 and 0.55 colours. These preview colours describe transparency and are
independent of page light/dark surfaces. Passing the same snapshot keeps the
previews consistent across components.

`square_size` rejects nonpositive and non-finite values in favour of the default.
Positive subpixel cells are clamped to 1 logical pixel to bound paint work.
Colours retain their supplied channels and alpha. The alternating colour paints
over the base colour; opacity multiplies both on disabled slider ramps.
The existing `paint_checkerboard(...)` function retains these defaults;
`paint_checkerboard_with_tokens(...)` accepts the same snapshot for custom
renderers. Caller clipping and supplied bounds still apply. See the compiled
example on `EnzColorPicker::checker_tokens`.

## Badge geometry

Badges keep a component-specific small/medium/large ladder. `BadgeSizeTokens`
configures height, label size, radius, horizontal padding, minimum outer width,
glyph size and glyph gap through `EnzBadge::size_tokens(...)`. Seed it with
`BadgeSizeTokens::from_size(...)`; its default is the medium preset. The override
wins over `.size(...)` in either builder order, while colours stay live unless
`.tokens(...)` is also set.

Values are logical pixels. Invalid values use the medium defaults. Height and
label size must be positive; the other fields allow zero. A zero glyph size
omits the icon and its gap. The effective height accommodates the label line
box and visible glyph; vertical centring follows that geometry. Parent bounds
still constrain the badge. These badge corners and text metrics are separate
from button geometry and surface rounding; matching numbers do not establish a
shared role. See the compiled example on `EnzBadge::size_tokens`.

## Text grids on their own canvas

`EnzTextGrid` tones (`Default`, `Muted`, `Subtle`, `Accent`, `Success`,
`Warning`, `Error`) resolve from the application palette, which assumes the
page behind them. A grid painted on a different colour, such as a terminal's
scheme background, declares it with `.canvas(color)`. Each tone is then raised
to the 4.5:1 text floor against the cell's own background when it has one, or
otherwise against the canvas, keeping the tone's hue. Explicit cell colours are
painted as given. Without a canvas the palette inks are unchanged.

## Toast controls

`ToastTokens` also styles a toast's text action button: `action_radius`
(default 6), `control_focus_width` (the focus outline, default 1.5; zero hides
it) and `control_hover_alpha` (the title-ink wash behind a hovered or pressed
button, default 0.10, clamped to 0–1). The dismiss (×) control is the shared
round toolbar icon and follows the toolbar icon's own styling.

## Meter tracks and switch knobs

A slider, progress bar or gauge paints its unfilled track in
`Palette::meter_track`: a soft groove, the text mixed 18% into the page in
light mode and 14% in dark, with no separate outline (`meter_track_border`
returns the track itself). The value is read where the fill ends, so the
fill needs contrast against the track. Custom palettes and materials still
require checking the actual painted colors; these roles do not validate
arbitrary overrides.

The slider thumb's rim is a shade of its own fill (`surface_edge`), so a white
thumb stays crisp on a light page. When the slider is disabled, its fill stops
at the thumb's edge, so the faded thumb never shows the fill through it.

Switch and toggle knobs are light in both modes: `surface_elevated` on a light
page, the `text` colour on a dark one. Each track is raised only as far as 3:1
against that knob. On opaque surfaces the off track first adjusts the meter
groove to stand apart from `surface_elevated`, then resolves contrast against
the knob. Glass keeps its translucent groove before knob-contrast resolution.

Selected tab pills keep their rim in dark mode. For the built-in dark palette,
the selection cue on a `surface_alt` titlebar comes from a lightened shade of
the same accent around the deeper fill. Set `track`,
`thumb_border`, `track_off` and the other component tokens to override any of
these.

## Disabled opacity

`EnzTheme::opacity` (`OpacityScale`, theme-file group `opacity`) holds disabled-state roles and the accepting drop-target tint. Disabled roles
multiply the alpha of the colours they fade, preserving relative transparency.

| role | default | follows it |
|---|---|---|
| `disabled` | 0.62 | disabled **controls**: text field, text area, secure field, search field (text, fill, border and icons), combo box, autocomplete, stepper, date field ink; the default fade of checkbox, radio, toggle, switch and slider |
| `disabled_surface` | 0.55 | disabled **surfaces** faded as a whole: clickable, pressable, list rows (sidebar, tree, preference and file-browser rows), colour swatches, a selected segment in a segmented tab list or button group, calendar value fills, carousel end buttons |
| `drop_target` | 0.08 | accepting native file/MIME target tint, bounded to 0–0.2; independent of disabled-state fading |

Set them with `EnzTheme::with_opacity(OpacityScale { .. })`; invalid values use
the defaults. A component token pinned by the caller (`CheckboxTokens`,
`SliderTokens`, `TabPillTokens`, `ClickableTokens` and so on, or
`EnzButtonGroup::disabled_alpha`) keeps its own value; unpinned tokens take the
theme's role. Disabled placeholders use the contrast-audited
`disabled_text_ink` instead, so they stay legible at any setting.
`date_field::dim` keeps the 0.62 default for callers without a theme;
`date_field::dim_by` takes an explicit opacity.

## Button groups

`EnzButtonGroup::border_width(...)` sets the outer frame and the seams between
cells for the framed kinds (Secondary, Outlined); the default is 1 logical
pixel and zero hides both. The seams are laid out when the view is built, so
this is an explicit width rather than the live hairline stroke.
`disabled_alpha(...)` (default 0.55, the same role as
`TabPillTokens::segment_disabled_alpha`) multiplies the alpha of a disabled
selected cell's fill, label and icon, so translucent overrides keep their
relative transparency. Unselected disabled cells use the button tokens'
disabled colours.

## Drag feedback

`DragFeedbackTokens` sets how strongly a drag is shown. Pass it to
`EnzDragDropHost::feedback_tokens(...)` and to each hosted
`EnzReorderList::feedback_tokens(...)`; giving both the same value keeps a
host and its lists consistent. Lengths are logical pixels.

| field | default | used by |
|---|---|---|
| `item_radius` | 6 | host: corners of a dragged or target item's fill |
| `area_radius` | 8 | host: a whole-collection target; list: an empty target list |
| `highlight_border_width` | 2 | host: target outlines; list: insertion bar and empty-list outline |
| `drag_fill_alpha` | 0.32 | host: fill over dragged items |
| `highlight_fill_alpha` | 0.16 | host: fill over a target item |
| `area_fill_alpha` | 0.08 | host: a whole-collection target; list: an empty target list |
| `dragging_elevation` | 3 | list: shadow level of a lifted row |

Colours follow the live theme: `primary.weak` for dragged items,
`primary.strong` for targets, and `danger` for a refused target. Invalid values
use the defaults, alphas clamp to 0–1 and the elevation to 5. A zero border
width hides target outlines and the insertion bar.

A reorder list's own row shape is separate: `.row_radius(...)` (default 6) sets
the radius its keyboard focus ring and lifted-row plate follow. Pill rows pass
their pill radius.

## Fixed geometry

These literals are deliberate. They are hairlines, layout geometry or fixed
control sizes, not a style a theme is expected to change:

- Code blocks take their corners from the radius scale: the frame and header
  use `radius.lg`, the copy chip `radius.sm`. The frame's 1 px edge is a
  hairline.
- Carousel chrome buttons are a fixed 32 px circle with a 1 px hairline; the
  dot indicator's radius follows its height token.
- Schedule event blocks inset their text by 4 px across, 2 px down and 3 px
  between title and time, and the hour labels sit by `HOUR_LABEL_GAP_PX` and
  `HOUR_LABEL_INSET_PX`. These place text inside a block; they are public
  constants, not tokens.

## Reviewing source candidates

Generate a syntax-derived inventory with:

```sh
cargo build --workspace --all-features --bin enzui-style-inventory --profile test
target/debug/enzui-style-inventory > /tmp/style-inventory.tsv
```

The columns identify file, enclosing scope, expression context, candidate kind
and value. The scan skips test directories, `tests.rs` files, and test-only
modules and functions recognized by its syntax visitor. Comments and string
expressions do not contribute numeric candidates. Macros are listed separately
because they are not expanded. The scan does not evaluate arbitrary build
configurations or resolve the module graph or expression types. The inventory
includes numeric domain constants and token defaults; it is a review aid,
not a compliance result. Check each candidate's public override and actual
layout or paint consumption before classifying it.
