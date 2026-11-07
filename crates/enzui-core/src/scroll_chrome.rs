//! Scroll-linked chrome state with two-threshold hysteresis and an always-visible top position.

/// Restore and collapse thresholds for scroll-linked chrome.
///
/// Reversed inputs are sorted, and equal values disable the hysteresis gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollChromeThresholds {
    restore: f32,
    collapse: f32,
}

impl ScrollChromeThresholds {
    /// Creates a threshold pair without panicking.
    ///
    /// Negative or non-finite values become zero, and reversed finite values are sorted.
    #[must_use]
    pub fn new(restore: f32, collapse: f32) -> Self {
        let r = if restore.is_finite() && restore >= 0.0 {
            restore
        } else {
            0.0
        };
        let c = if collapse.is_finite() && collapse >= 0.0 {
            collapse
        } else {
            0.0
        };
        Self {
            restore: r.min(c),
            collapse: r.max(c),
        }
    }

    /// The restore threshold.
    #[must_use]
    pub fn restore(&self) -> f32 {
        self.restore
    }

    /// The collapse threshold.
    #[must_use]
    pub fn collapse(&self) -> f32 {
        self.collapse
    }
}

impl Default for ScrollChromeThresholds {
    /// Uses a 48 px collapse threshold and a 16 px restore threshold.
    fn default() -> Self {
        Self {
            restore: 16.0,
            collapse: 48.0,
        }
    }
}

/// Tracks whether scroll-linked chrome is collapsed using hysteresis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollChrome {
    thresholds: ScrollChromeThresholds,
    collapsed: bool,
    dirty: bool,
}

impl ScrollChrome {
    /// Creates chrome state that is initially shown.
    #[must_use]
    pub fn new(thresholds: ScrollChromeThresholds) -> Self {
        Self {
            thresholds,
            collapsed: false,
            dirty: false,
        }
    }

    /// Is the chrome currently collapsed (hidden)?
    #[must_use]
    pub fn collapsed(&self) -> bool {
        self.collapsed
    }

    /// Returns whether the last update changed collapse state, then clears the flag.
    pub fn changed(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    /// The thresholds this chrome was created with.
    #[must_use]
    pub fn thresholds(&self) -> ScrollChromeThresholds {
        self.thresholds
    }

    /// Updates state from a vertical offset in logical pixels.
    ///
    /// Offsets at or below zero show chrome. A shown state collapses strictly above the collapse threshold, and a collapsed state restores strictly below the restore threshold; all other values, including `NaN`, preserve the current state.
    pub fn update(&mut self, y_offset: f32) {
        let prev = self.collapsed;

        if y_offset <= 0.0 {
            self.collapsed = false;
        } else if !self.collapsed && y_offset > self.thresholds.collapse {
            self.collapsed = true;
        } else if self.collapsed && y_offset < self.thresholds.restore {
            self.collapsed = false;
        }

        self.dirty = self.collapsed != prev;
    }
}
