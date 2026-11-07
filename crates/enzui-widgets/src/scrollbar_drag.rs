//! Shared mouse-drag lifetime rule for framework-owned custom scrollbars.

use iced::{Event, mouse, window};

pub(crate) fn cancels_mouse_drag(event: &Event) -> bool {
    // Out-of-bounds motion retains capture; compositor loss can omit release.
    matches!(
        event,
        Event::Mouse(mouse::Event::CursorLeft) | Event::Window(window::Event::Unfocused)
    )
}

/// How far the ends of a vertical rail in `bounds` must move in for a thumb
/// spanning `thumb_x` to clear the rounded corners around it (a rounded
/// window or pane). Measured while handling events, where those clips are
/// known (`iced_core::clip_shape`); a scrollbar keeps it for drawing.
pub(crate) fn vertical_clearance(bounds: iced::Rectangle, thumb_x: (f32, f32)) -> [f32; 2] {
    let (leading, trailing) =
        iced_core::clip_shape::rail_ends(true, thumb_x, (bounds.y, bounds.y + bounds.height));
    [leading, trailing]
}

/// [`vertical_clearance`] for a horizontal rail whose thumb spans `thumb_y`.
pub(crate) fn horizontal_clearance(bounds: iced::Rectangle, thumb_y: (f32, f32)) -> [f32; 2] {
    let (leading, trailing) =
        iced_core::clip_shape::rail_ends(false, thumb_y, (bounds.x, bounds.x + bounds.width));
    [leading, trailing]
}

/// `bounds` with its top and bottom moved in by `clearance`.
pub(crate) fn cleared_vertical(bounds: iced::Rectangle, clearance: [f32; 2]) -> iced::Rectangle {
    let top = clearance[0].min(bounds.height);
    let bottom = clearance[1].min(bounds.height - top);
    iced::Rectangle {
        y: bounds.y + top,
        height: bounds.height - top - bottom,
        ..bounds
    }
}
