//! Bubble painting helpers: colour clamping, alpha scaling and the `container::Style` builder.

use super::*;

pub(crate) fn scale_alpha(color: Color, opacity: f32) -> Color {
    Color {
        a: color.a * opacity.clamp(0.0, 1.0),
        ..color
    }
}

pub(crate) fn finite_color(c: Color) -> Color {
    let ch = |v: f32| {
        if v.is_finite() {
            v.clamp(0.0, 1.0)
        } else {
            0.0
        }
    };
    Color {
        r: ch(c.r),
        g: ch(c.g),
        b: ch(c.b),
        a: ch(c.a),
    }
}

// Bound shadow geometry before software rendering can allocate pathological buffers.
pub(crate) const SHADOW_BLUR_MAX: f32 = 256.0;

pub(crate) const SHADOW_OFFSET_MAX: f32 = 256.0;

pub(crate) fn bubble_style_opacity(
    tokens: &TooltipTokens,
    radius: f32,
    opacity: f32,
) -> container::Style {
    let shadow_blur = FiniteNonNeg::new_or(tokens.shadow_blur, 0.0)
        .get()
        .min(SHADOW_BLUR_MAX);
    let shadow_offset_y = FiniteF32::new_or(tokens.shadow_offset_y, 0.0)
        .get()
        .clamp(-SHADOW_OFFSET_MAX, SHADOW_OFFSET_MAX);
    let shadow_color = finite_color(tokens.shadow_color);
    let text_color = finite_color(tokens.text_color);
    let background = finite_color(tokens.background);
    container::Style {
        // EnzText owns its ink resolver; this inherited colour does not sanitize that resolver.
        text_color: Some(scale_alpha(text_color, opacity)),
        background: Some(scale_alpha(background, opacity).into()),
        border: Border {
            radius: radius.into(),
            width: 0.0,
            color: Color::TRANSPARENT,
        },
        shadow: Shadow {
            color: scale_alpha(shadow_color, opacity),
            offset: Vector::new(0.0, shadow_offset_y),
            blur_radius: shadow_blur,
        },
        ..Default::default()
    }
}
