//! Border-radius tokens.
//!
//! [`RadiusScale`] holds the configurable ladder. In the default scale, the
//! `lg` container radius (10) is one step above the `md` control radius (6).
//!
//! | Token | Default px |
//! |---|---|
//! | `none` | 0 |
//! | `xs` | 3 |
//! | `sm` | 5 |
//! | `md` | 6 |
//! | `lg` | 10 |
//! | `xl` | 12 |
//! | `xxl` | 20 |
//! | `full` | 9999 |

/// Border radius scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadiusScale {
    /// No-rounding rung; zero logical pixels by default.
    pub none: f32,
    /// Extra-small rung; 3 logical pixels by default.
    pub xs: f32,
    /// Small rung; 5 logical pixels by default.
    pub sm: f32,
    /// Medium control/field rung; 6 logical pixels by default.
    pub md: f32,
    /// Container rung for cards, panels, panes, and popovers; 10 logical pixels
    /// by default, one step above `md`.
    pub lg: f32,
    /// Dialog rung; 12 logical pixels by default.
    pub xl: f32,
    /// Hero-element rung; 20 logical pixels by default.
    pub xxl: f32,
    /// Full-pill rung; 9999 logical pixels by default.
    pub full: f32,
}

impl Default for RadiusScale {
    fn default() -> Self {
        Self {
            none: 0.0,
            xs: 3.0,
            sm: 5.0,
            md: 6.0,
            lg: 10.0,
            xl: 12.0,
            xxl: 20.0,
            full: 9999.0,
        }
    }
}
