//! EnzExpander: framed / plain disclosure section.
//!
//! A disclosure section with an optional leading icon, a semibold
//! title + optional muted subtitle, a chevron affordance, and a
//! caller-supplied content slot that shows / hides when the header
//! is toggled.
//!
//! # Scope
//!
//! - Variants: `card` (default — framed with `surface-elevated`
//!   fill, `border`, `card-radius` rounded corners) and `plain`
//!   (flush — no frame).
//! - Slots: leading icon (Lucide), title, optional
//!   subtitle, chevron (right when collapsed, turning down as it expands).
//! - Content: any `Element<'a, Message>` supplied via
//!   `.content(...)`. Rendered inside the framed body when expanded.
//!
//! # Assistive-technology identity
//!
//! The disclosure header is routable by default: its widget id is derived from
//! the title under [`DEFAULT_ID_PREFIX`]. An accessible label alone lets a
//! screen reader announce the section, but it does not give `NodeIdMap` the
//! widget identity required to route Expand/Collapse. Use
//! [`EnzExpander::id`] when the title is dynamic or duplicated on one screen,
//! or [`EnzExpander::id_prefix`] to give a repeated title a distinct namespace.
//!
//! # Enable gate (AdwExpanderRow-style)
//!
//! `.show_enable_switch(true)` adds a header switch (before the chevron) that
//! gates the whole section, mirroring the `show-enable-switch` /
//! `enable-value`. Enable state and expanded state are two independent
//! bindings the caller owns: `.enable_value(bool)` + `.on_enable_toggled(cb)`.
//! While the gate is off the section is `section_active == false` →
//! `effective_expanded` is forced collapsed, the header tap is ignored (only
//! the switch re-enables it), the title dims to muted, and the chevron is
//! hidden.
//!
//! # Body animation
//!
//! The body height animates between 0 (collapsed) and the content's preferred
//! height (expanded) over the theme's `MotionToken::Normal` duration (180 ms
//! default, ease-out). The implementation uses `iced::animation::Animation<bool>`
//! — the same pattern as `code_block.rs`'s copied-pulse. Under reduced motion
//! the transition is instant (`Duration::ZERO`). Once the animation settles,
//! no further redraws are requested.

use crate::focus_button::FocusStatus;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Renderer as _, Shell, overlay, renderer};
use iced::animation::Animation;
use iced::mouse;
use iced::time::Instant;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{Space, container, row};
use iced::window;
use iced::{
    Alignment, Background, Border, Color, Element, Event, Length, Padding, Rectangle, Size,
};
use std::cell::Cell;
use std::rc::Rc;

mod anim;
mod body_gate;
mod builder;
mod tokens;

/// Default namespace for a title-derived expander-header identity.
pub const DEFAULT_ID_PREFIX: &str = "enz-expander";

pub(crate) use anim::{ExpanderAnim, FrameResolver};
pub(crate) use body_gate::{BodyGate, BodyGateShared};
pub use builder::{EnzExpander, enz_expander};
pub use tokens::{ExpanderTokens, ExpanderVariant, HoverScope};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Raw duration transport, driven through a real draw/update lifecycle
// ---------------------------------------------------------------------------

#[cfg(test)]
mod body_gate_bounds;

// ---------------------------------------------------------------------------
// Title colour, focus ring independence, and gate label localization
// ---------------------------------------------------------------------------

#[cfg(test)]
mod title_color_and_gate_label;
