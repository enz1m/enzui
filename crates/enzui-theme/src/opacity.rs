//! Theme-wide opacities for whole-control states.
//!
//! Roles for disabled controls and drag feedback:
//!
//! - [`OpacityScale::disabled`] (default `0.62`): how far a disabled
//!   *control* fades — the text-entry family's text, fill and border, and the
//!   default of checkbox, radio, toggle and slider tokens.
//! - [`OpacityScale::disabled_surface`] (default `0.55`): how far a disabled
//!   *surface* fades as a whole, fill and content together — clickable and
//!   pressable surfaces, list rows, swatches, and a selected segment in a tab
//!   list or button group.
//!
//! - [`OpacityScale::drop_target`] (default `0.08`): the accepting drag target
//!   tint above its content, bounded to `0..=0.2` so labels remain visible.
//!
//! A component token pinned by the caller keeps its own value; unpinned
//! tokens follow these roles.
//!
//! # Example
//!
//! ```
//! use enzui_theme::{EnzTheme, OpacityScale};
//!
//! let theme = EnzTheme::dark().with_opacity(OpacityScale {
//!     disabled: 0.5,
//!     ..OpacityScale::default()
//! });
//! assert_eq!(theme.opacity.disabled(), 0.5);
//! assert_eq!(theme.opacity.disabled_surface(), 0.55);
//! let broken = OpacityScale { disabled: f32::NAN, disabled_surface: 2.0, ..OpacityScale::default() };
//! assert_eq!((broken.disabled(), broken.disabled_surface()), (0.62, 1.0));
//! ```

/// Theme-adjustable opacities for whole-control states.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpacityScale {
    /// Alpha multiplier for a disabled control's colours, `0..=1`; defaults
    /// to `0.62`.
    pub disabled: f32,
    /// Alpha multiplier for a disabled surface faded as a whole, `0..=1`;
    /// defaults to `0.55`.
    pub disabled_surface: f32,
    /// Accepting drag-target tint above content, `0..=0.2`; defaults to `0.08`.
    pub drop_target: f32,
}

/// The default disabled opacity.
pub const DEFAULT_DISABLED_OPACITY: f32 = 0.62;

/// The default disabled-surface opacity.
pub const DEFAULT_DISABLED_SURFACE_OPACITY: f32 = 0.55;

/// The default accepting drag-target overlay opacity.
pub const DEFAULT_DROP_TARGET_OPACITY: f32 = 0.08;

impl Default for OpacityScale {
    fn default() -> Self {
        Self {
            disabled: DEFAULT_DISABLED_OPACITY,
            disabled_surface: DEFAULT_DISABLED_SURFACE_OPACITY,
            drop_target: DEFAULT_DROP_TARGET_OPACITY,
        }
    }
}

impl OpacityScale {
    /// The disabled opacity, clamped to `0..=1`; a non-finite value uses the
    /// default, so a malformed theme cannot make a disabled control vanish
    /// or brighten.
    #[must_use]
    pub fn disabled(&self) -> f32 {
        ratio(self.disabled, DEFAULT_DISABLED_OPACITY)
    }

    /// The disabled-surface opacity, clamped to `0..=1`; a non-finite value
    /// uses the default.
    #[must_use]
    pub fn disabled_surface(&self) -> f32 {
        ratio(self.disabled_surface, DEFAULT_DISABLED_SURFACE_OPACITY)
    }
    /// Accepting drag-target tint, bounded to preserve the underlying content.
    #[must_use]
    pub fn drop_target(&self) -> f32 {
        ratio(self.drop_target, DEFAULT_DROP_TARGET_OPACITY).min(0.2)
    }
}

fn ratio(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default
    }
}
