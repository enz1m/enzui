//! Realized option windows for virtualized popups.

use super::*;

pub(crate) const POPUP_OVERSCAN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupWindow {
    pub start: usize,
    pub len: usize,
    pub total: usize,
}

impl PopupWindow {
    pub(crate) fn eager(total: usize) -> Self {
        Self {
            start: 0,
            len: total,
            total,
        }
    }

    pub(crate) fn option_at(&self, built_position: usize) -> Option<usize> {
        if built_position >= self.len {
            return None;
        }
        let opt = self.start.checked_add(built_position)?;
        (opt < self.total).then_some(opt)
    }

    pub(crate) fn built_position_of(&self, option_index: usize) -> Option<usize> {
        if option_index < self.start {
            return None;
        }
        let p = option_index - self.start;
        (p < self.len).then_some(p)
    }
}

pub(crate) fn compute_popup_window(
    total: usize,
    row_h: f32,
    scroll_y: Option<f32>,
    active: Option<usize>,
) -> PopupWindow {
    let Some(y) = scroll_y else {
        return PopupWindow::eager(total);
    };
    if total == 0 {
        return PopupWindow::eager(0);
    }
    // Overscan absorbs the translation cell's delay while spacers retain full list height.
    let win_len = VISIBLE_ROWS.saturating_add(POPUP_OVERSCAN * 2).min(total);
    let row_h = row_h.max(1.0);
    let visible_start_f = (y / row_h).max(0.0);
    let mut start = visible_start_f as usize;
    start = start.saturating_sub(POPUP_OVERSCAN);
    if let Some(a) = active
        && a < total
    {
        if a < start {
            start = a.saturating_sub(POPUP_OVERSCAN);
        } else if a >= start + win_len {
            start = a
                .saturating_add(1)
                .saturating_add(POPUP_OVERSCAN)
                .saturating_sub(win_len);
        }
    }
    if start + win_len > total {
        start = total.saturating_sub(win_len);
    }
    PopupWindow {
        start,
        len: win_len,
        total,
    }
}
