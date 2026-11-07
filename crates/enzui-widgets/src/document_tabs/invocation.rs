//! Document-tab keyboard and pointer event decoding.

use super::*;

/// Decode a key press using manual tab activation without modifying the model.
///
/// Along-axis arrows, Home and End request focus movement, skipping disabled
/// tabs and clamping at the ends. Horizontal arrows honor `rtl`; cross-axis
/// arrows are declined. Enter or Space activates the focused tab, Delete requests
/// its close, and Menu or Shift+F10 requests its context menu at `cursor_bounds`.
///
/// Unowned keys and Ctrl/Alt/Super combinations return `None` so callers can let
/// them bubble. Shift+Enter, Shift+Space and Shift+Delete are also declined.
/// Cursored-tab actions return `None` without a focused key. The application
/// applies emitted events to its [`DocumentTabs`] model.
#[must_use]
pub fn event_for_key(
    model: &DocumentTabs,
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    rtl: bool,
    cursor_bounds: enzui_core::context_menu::Rect,
) -> Option<TabEvent> {
    event_for_key_with(
        model,
        key,
        modifiers,
        rtl,
        cursor_bounds,
        ActivationMode::Manual,
    )
}

/// Decode a key press with an explicit keyboard activation mode.
///
/// Like [`event_for_key`], but movement emits [`TabEvent::Activated`] in automatic
/// mode and [`TabEvent::FocusMoved`] in manual mode. Other actions are unchanged.
#[must_use]
pub fn event_for_key_with(
    model: &DocumentTabs,
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    rtl: bool,
    cursor_bounds: enzui_core::context_menu::Rect,
    activation: ActivationMode,
) -> Option<TabEvent> {
    if let Some(movement) = FocusMove::for_key(key, modifiers, model.orientation(), rtl) {
        // Resolve the destination without mutating application-owned state.
        let mut probe = model.clone();
        let dest = probe.move_focus(movement).key().cloned();
        return dest.map(|k| match activation {
            ActivationMode::Automatic => TabEvent::Activated(k),
            ActivationMode::Manual => TabEvent::FocusMoved(k),
        });
    }

    let keyboard::Key::Named(named) = key else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }

    let cursor = model.focused()?.clone();

    Some(match named {
        keyboard::key::Named::Enter | keyboard::key::Named::Space => {
            if modifiers.shift() {
                return None;
            }
            TabEvent::Activated(cursor)
        }
        keyboard::key::Named::Delete => {
            if modifiers.shift() {
                return None;
            }
            TabEvent::CloseRequested(cursor, CloseSource::Keyboard)
        }
        // Bare F10 remains available to application menu bars.
        keyboard::key::Named::F10 if modifiers.shift() => {
            TabEvent::ContextRequested(cursor, InvocationKind::Keyboard(cursor_bounds))
        }
        keyboard::key::Named::ContextMenu => {
            TabEvent::ContextRequested(cursor, InvocationKind::Keyboard(cursor_bounds))
        }
        _ => return None,
    })
}

pub(crate) fn cursored_bounds(
    metrics: &StripMetrics,
    model: &DocumentTabs,
    scroll_offset: f32,
    extent: f32,
    orientation: Orientation,
) -> enzui_core::context_menu::Rect {
    use enzui_core::context_menu::Rect;
    // Model bounds omit the cross axis; context-menu hosts use real layout bounds.
    let zero = Rect::new(0.0, 0.0, 0.0, 0.0);
    let Some(cursor) = model.focused() else {
        return zero;
    };

    if let Some(pos) = metrics.pinned_keys().iter().position(|k| k == cursor) {
        let edge = metrics.pinned_offset_of(pos);
        let (along, _cross) = metrics.pinned_cell_box(pos);
        return match orientation {
            Orientation::Horizontal => Rect::new(edge, 0.0, along, 0.0),
            Orientation::Vertical => Rect::new(0.0, edge, 0.0, along),
        };
    }

    let Some(idx) = metrics.session_keys().iter().position(|k| k == cursor) else {
        return zero;
    };
    let strip_edge = metrics.session_offset_of(idx) - scroll_offset + metrics.pinned_extent();
    let w = metrics.session_width_of(idx);
    if !strip_edge.is_finite() || strip_edge + w < 0.0 || strip_edge > extent {
        return zero;
    }
    match orientation {
        Orientation::Horizontal => Rect::new(strip_edge, 0.0, w, 0.0),
        Orientation::Vertical => Rect::new(0.0, strip_edge, 0.0, w),
    }
}

/// Convert a tab-local context gesture to viewport coordinates.
///
/// `tab_origin` must be the tab’s laid-out origin in the same viewport coordinate
/// space used by the menu host; `local` is the position within that tab.
#[must_use]
pub fn pointer_invocation(tab_origin: (f32, f32), local: (f32, f32)) -> InvocationKind {
    InvocationKind::Pointer(MenuPoint::new(
        tab_origin.0 + local.0,
        tab_origin.1 + local.1,
    ))
}
