# Colored controls

Opaque colored controls pair their fill with their foreground through
[`Palette::filled_color`](../crates/enzui-theme/src/palette.rs). The returned
pair contains the background in `color` and the foreground in `text`. The
resolver makes the fill opaque, replacing its alpha with `1.0`.

## Foreground polarity and contrast targets

The resolver uses an APCA-style polarity comparison to choose light or dark ink,
then checks the opaque pair against the contrast targets below. These are the
framework's control-color rules; they do not certify an entire interface or a
custom translucent material.

- **White, on a deepened fill.** When white reads better, the label is white
  and the fill is deepened until white reaches 4.5:1 (4.8:1 on a dark palette,
  headroom against channel rounding). Deepening lowers OkLab lightness and
  keeps the hue, giving up only the chroma the sRGB gamut forces, so a blue
  stays blue instead of greying the way mixing toward black does. The result is resolved for each fill.
- **A tinted dark ink, on the kept fill.** A fill too light for white (yellow,
  amber, a pale custom colour) keeps its colour and takes a near-black in its
  own hue at 4.5:1 or better, with a small hue contribution from the fill.

[`Palette::ink_on`](../crates/enzui-theme/src/palette.rs) applies the same
rule to a fill that cannot change (raw roles, authored colours): white where
it reads best and reaches at least 3:1, else the tinted dark ink. The `on_*`
helpers use it on the raw roles, and `enzui_widgets::tokens::ink_for` uses it
for everything drawn on a filled surface, keeping the palette's own text
colour on neutral surfaces.

The resolver leaves raw accent and status roles unchanged for links, outlines,
indicators and charts. Custom
filled controls should paint both members of the returned pair together.
Changing the fill afterward, including its opacity, requires checking contrast
against the background actually painted.

## Default consumers

These defaults apply to enabled controls using opaque surfaces. Explicit widget
token overrides remain caller-controlled; disabled controls can dim their colors.

| Consumers | Resolution |
|---|---|
| Accent, Success and Destructive buttons, including their icons | Paired fills and foregrounds in resting, hover and pressed states |
| Pill tabs and view switchers with `TabSelectionStyle::FilledAccent` | Paired accent selection; dark pills add an accent edge |
| Inline segmented switchers with filled accent selection | Selection adjusted for contrast against the track, with foreground resolved from that selection |
| Strong accent badges and avatar initials | Paired opaque accent |
| User chat bubbles | Paired opaque accent |
| Checked checkboxes and selected radio buttons | Paired accent and tick/dot |
| Calendar selected dates, including selected today | Paired accent and pressed accent |
| Schedule today marker, timed events and all-day chips | Paired danger/event color; timed-event selection ring derives from the painted fill |
| Active text-search highlights | Resolved accent fill with independently contrast-resolved glyph ink |

Segmented controls use the page background for their dark track and `surface_alt`
for their light track. Their selected accent fill targets 3:1 contrast against
the track, then chooses readable ink for the resolved fill. Dark pill selections
add a thin accent edge for brighter titlebar surfaces. The 4.8:1 white-ink rule
of `filled_color` should not be assumed for these separately adjusted segments.

## Other color treatments

Switches and toggles use a light thumb in both appearances. Their track resolver
adjusts the accent to target 3:1 contrast against that thumb, using the non-text
contrast floor instead of `filled_color`.

Accent buttons on glass use translucent fills. Tab selections using
`TranslucentAccent` or `NeutralMaterial` also follow their own material and
accessibility policies. The opaque fill rule does not describe these treatments.

Soft status badges, neutral controls, translucent annotations and color-picker
samples retain their own semantics. A color sample represents authored content;
it should not be converted into a control fill. Annotation ink must account for
the highlight composited over its paper, and search-match glyph ink is resolved
independently against its match fill.
