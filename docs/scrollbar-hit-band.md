# Scrollbar hit band

`ScrollbarTokens::DEFAULT` (`crates/enzui-theme/src/scrollbar.rs`) sets the
default geometry used by enzUI overlay scrollbars. Dimensions are logical pixels:

| Field | Value | Meaning |
|---|---|---|
| `width` | 5 px | painted thumb |
| `hit_width` | 15 px | invisible grab/reveal band |
| `content_gutter` | 10 px | trailing clearance, `(hit_width + width) / 2` |
| `border_radius` | 2.5 px | `width / 2` |
| `min_thumb_length` | 24 px | shortest thumb |
| `alpha` / `alpha_hover` | 0.40 / 0.60 | revealed thumb opacity and dragged thumb opacity |

iced anchors the band at the region's trailing edge
(`total_scrollbar_bounds.x = bounds.x + bounds.width - total_width` in
[`iced/widget/src/scrollable.rs`](https://github.com/enz1m/enzim-iced/blob/67f991b7442cbd79821141069c3b864cc12413f2/widget/src/scrollable.rs)), so the band grows towards the content.
The painted thumb's inner edge sits `content_gutter` in from that edge.

Scrollbars overlay their content. `content_gutter` is optional clearance for the
painted thumb, not a reservation of the whole grab band. When placing controls
near the trailing edge, allow for the full hit width as well as the thumb.

## Choosing a wider band

A wider band steals presses from content flush with the trailing edge. The
`virtual_grid_bounded::pressing_a_tile_selects_it_and_deselects_its_siblings`
test (`crates/enzui-widgets/tests/scrolling/virtual_grid_bounded.rs`)
presses the centre of a 32 px tile with an 8 px gap in a 152 px narrow grid; the
rightmost tile's centre is at `x = 136`. `Rectangle::contains` is half-open on
the right, so the band captures a point when `W - hit_width <= p.x < W`.
Keeping the tile clickable needs `152 - hit_width > 136`, that is
`hit_width < 16` for that layout. The default 15 px band preserves that target.

`ScrollbarTokens::DESKTOP_POINTER` provides a 28 px grab band with the same
5 px thumb, a 24 px minimum thumb length and a 16.5 px optional gutter. Pass it
through a scroll surface's `scrollbar_tokens(...)` override, leaving room for
the wider band. Token normalization keeps the hit width at least as wide as
the thumb and the gutter at least `(hit_width + width) / 2`.

## Derived constants

These read `content_gutter` and follow any retune automatically:

* `enzui_widgets::scroll_column::DEFAULT_SCROLLBAR_GUTTER`
* `enzui_widgets::table::SCROLLBAR_GUTTER`

Apps that reserve the gutter or the full hit band should read these constants
or `ScrollbarTokens::DEFAULT`, never a literal.

The table's internal `scrollbar::GUTTER_PX` is private; use the public module
constant above. These constants describe the built-in geometry, not a custom
scrollbar token snapshot.
