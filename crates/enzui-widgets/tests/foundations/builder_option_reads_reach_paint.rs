//! Paint-affecting builder options must be read where they render.
//!
//! # Why this exists
//!
//! The dominant test shape in this crate is:
//!
//! ```ignore
//! #[test]
//! fn foo_setter_stores_the_value() {
//!     let b = enz_thing("x").foo(SOME_COLOR);
//!     assert_eq!(b.foo, Some(SOME_COLOR));
//! }
//! ```
//!
//! That asserts a struct assignment, which Rust's type system already
//! guarantees. The defect it is nominally guarding — the option never being
//! *consulted* at the point it paints — is invisible to it, because the field
//! genuinely is stored. Forcing a `framed(false)` border back on at render
//! time leaves every such test green.
//!
//! Each option gated below is one whose read can be severed — replacing
//! `pin.unwrap_or(theme_default)` with `theme_default`, or `if flag` with
//! `if true`, while leaving the setter correct — without any field test
//! noticing.
//!
//! # How these gates are built so they can fail
//!
//! Each gate pins a loud sentinel colour no theme palette produces, renders
//! the widget, and asserts that colour is present in the frame. A severed read
//! substitutes the theme default, the sentinel never reaches the buffer, and
//! the assertion fails.
//!
//! Every gate is paired with a same-widget control that renders without the
//! pin and asserts the sentinel is *absent*. Without it an "is the colour
//! there" probe could pass by measuring a frame that is loud for some unrelated
//! reason, and an absence-assertion with no positive control cannot tell
//! "clean" from "measured nothing".
//!
//! Nothing here pins a theme hex: the expectation is always the caller's own
//! sentinel, so retuning the palette cannot move it.

// magenta
// pure green

// ---------------------------------------------------------------------------
// icon_size_token_grid: slot_border
// Severed read: `color: slot_border_pin.unwrap_or(p.border)` -> `color: p.border`
// Stayed green: 4449/4449 integration, 18/18 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// radius_token_grid: preview_border
// Severed read: `let border_pin = self.preview_border;` -> `= None;`
// Stayed green: 4449/4449 integration, 11/11 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// color_swatch_grid: swatch_border
// Severed read: `let border_pin = self.swatch_border;` -> `= None;`
// Stayed green: 4449/4449 integration, 16/16 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// group_box: title_color
// Severed read: `if let Some(color) = self.title_color` -> `= None::<Color>`
// Stayed green: 4449/4449 integration, 12/12 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// full_screen_overlay: close_color
// Severed read: `let close_color = self.close_color;` -> `= Color::WHITE;`
// Stayed green: 4449/4449 integration, 17/17 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// table_header_cell: accent_color
// Severed read: `let pinned_accent = self.accent_color;` -> `= None;`
// Stayed green: 4449/4449 integration, 27/27 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// titlebar_icon_group: border_color
// Severed read: `let border_pin = self.border_color;` -> `= None;`
// Stayed green: 4449/4449 integration, 31/31 module unit tests.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// gradient_background: show_mid
// Severed read: `if show_mid {` -> `if true {`
// Stayed green: 4449/4449 integration, 18/18 module unit tests.
//
// This one is a *visibility* flag rather than a colour pin, so it is gated by
// the presence of the mid stop's own colour: with `show_mid(false)` the ramp
// runs start -> end and the mid sentinel must not appear anywhere.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// label: muted_color
// Severed read: `let pinned_muted = self.muted_color;` -> `= None;`
// Stayed green: 4449/4449 integration, 317/317 `label`-filtered unit tests.
//
// `muted_color` inks the subtitle only, so the specimen must carry one:
// without a subtitle the widget never reaches the resolver and the probe would
// measure a label that simply has no muted text to colour.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// marquee_area: highlight_covered
// Severed read: `if self.highlight_covered {` -> `if true {`
// Stayed green: 4449/4449 integration, 51/51 module unit tests.
//
// A visibility flag, so it is gated like `show_mid`: the per-item highlight is
// pinned to a sentinel via `tokens`, and `highlight_covered(false)` must leave
// none of it on screen.
// ---------------------------------------------------------------------------
