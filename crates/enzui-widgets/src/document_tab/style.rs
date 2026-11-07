//! Internal colour and geometry resolution for document tabs.

use super::*;
pub(crate) const TAB_ROW_SPACING: f32 = super::DEFAULT_ROW_SPACING;
pub(crate) const MIN_TITLE_CAP: f32 = 24.0;
pub(crate) fn title_max_width(
    tab_max_width: f32,
    content_padding_leading: f32,
    content_padding_trailing: f32,
    close_visible: bool,
    close_size: f32,
    has_favicon: bool,
    favicon_size: f32,
) -> f32 {
    let mut reserved = content_padding_leading + content_padding_trailing;
    if close_visible {
        reserved += close_size + TAB_ROW_SPACING;
    }
    if has_favicon {
        reserved += favicon_size + TAB_ROW_SPACING;
    }
    (tab_max_width - reserved).max(MIN_TITLE_CAP)
}
pub(crate) fn title_font_for(selected: bool) -> iced::Font {
    enzui_theme::text_defaults::font(
        enzui_theme::FontRole::Ui,
        if selected {
            enzui_theme::WeightRung::Semibold
        } else {
            enzui_theme::WeightRung::Regular
        },
    )
}
pub(crate) fn document_tab_composited_fill(p: &enzui_theme::Palette, state_fill: Color) -> Color {
    if state_fill.a < 1.0 {
        enzui_theme::tab_wash_over(p.surface, state_fill, state_fill.a)
    } else {
        state_fill
    }
}

pub(crate) fn document_tab_state_bg(
    p: &enzui_theme::Palette,
    status: FocusStatus,
    selected: bool,
    base_bg: Color,
    selected_background: Option<Color>,
    hover_background: Option<Color>,
    workspace_accent: Option<Color>,
) -> Color {
    use crate::surface_interaction::composite;
    let state_bg = if selected {
        selected_background.unwrap_or(p.surface_selected)
    } else {
        match status {
            FocusStatus::Pressed => composite(base_bg, p.overlay_pressed()),
            FocusStatus::Hovered => {
                hover_background.unwrap_or_else(|| composite(base_bg, p.overlay_hover()))
            }
            _ => base_bg,
        }
    };
    match workspace_accent {
        None => state_bg,
        Some(accent) => {
            // Resolve translucent state ink over the shell before adding the accent wash.
            let painted = document_tab_composited_fill(p, state_bg);
            let alpha = if selected {
                enzui_theme::DEFAULT_WASH_ALPHA_SELECTED
            } else {
                enzui_theme::DEFAULT_WASH_ALPHA_REST
            };
            enzui_theme::tab_wash_over(painted, accent, alpha)
        }
    }
}
pub(crate) fn document_tab_muted_icon_ink(p: &enzui_theme::Palette, painted_fill: Color) -> Color {
    p.text_muted_on(painted_fill)
}
pub(crate) fn document_tab_close_icon_ink(
    p: &enzui_theme::Palette,
    painted_fill: Color,
    status: FocusStatus,
    enabled: bool,
) -> Color {
    if !enabled {
        return p.disabled;
    }
    match status {
        FocusStatus::Hovered | FocusStatus::Pressed | FocusStatus::Focused => p.text,
        _ => document_tab_muted_icon_ink(p, painted_fill),
    }
}
pub(crate) fn close_button_style(
    theme: &iced::Theme,
    status: FocusStatus,
    radius: f32,
) -> ButtonStyle {
    let p = theme_lookup::palette(theme);
    // Press fill stays translucent over selected and caller-pinned tab backgrounds.
    let bg = match status {
        FocusStatus::Pressed => p.overlay_pressed(),
        _ => Color::TRANSPARENT,
    };
    let border = if matches!(status, FocusStatus::Focused) {
        crate::focus_button::icon_action_focus_ring(theme, bg, radius)
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        }
    };
    let ink = match status {
        FocusStatus::Hovered | FocusStatus::Pressed | FocusStatus::Focused => p.text,
        _ => p.text_muted,
    };
    ButtonStyle {
        background: Some(Background::Color(bg)),
        text_color: ink,
        border,
        ..ButtonStyle::default()
    }
}
pub(crate) fn tab_surface_border(
    p: &enzui_theme::Palette,
    status: FocusStatus,
    model_focused: bool,
    selected: bool,
    radius: f32,
) -> Border {
    if model_focused && !selected {
        Border {
            color: p.focus_ring,
            width: enzui_theme::text_defaults::focus_ring_width(),
            radius: radius.into(),
        }
    } else if matches!(status, FocusStatus::Focused) {
        Border {
            color: p.focus_ring,
            width: 1.5,
            radius: radius.into(),
        }
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        }
    }
}
