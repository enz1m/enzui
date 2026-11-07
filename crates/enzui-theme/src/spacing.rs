//! Theme-scaled spacing tokens.
//!
//! Numeric rungs cover `space0` through `space15`; named aliases cover `xxs`
//! through `3xl`. Token values come from the supplied [`SpacingScale`]; pixel
//! values mentioned below are defaults.
//! [`SpaceToken::Custom`] bypasses the scale and maps negative or non-finite
//! values to zero.
//!
//! # Which token for which situation
//!
//! | Token  | px | Reach for it when… |
//! |--------|----|--------------------|
//! | [`SpaceToken::None`] | 0  | Two things must touch: a seamless run of list rows, a segmented control's abutting cells, a toolbar of icon buttons with no seam. Deliberate 0, not "unset". |
//! | [`SpaceToken::Xxs`]  | 2  | A hairline nudge that must not read as a gap: the first row's clearance under a sidebar header. |
//! | [`SpaceToken::Xs`]   | 4  | Between parts of one compound control — icon and its label, a chip's text and its dismiss glyph, wrapped chips in an overflow row. |
//! | [`SpaceToken::Sm`]   | 8  | Between sibling controls doing the same job: buttons in a row, a form's label/field rows, cells in a dense grid. |
//! | [`SpaceToken::Md`]   | 12 | The default. Between stacked rows inside one grouped block, a form's own inset, a dialog's button-bar gap. Start here when unsure. |
//! | [`SpaceToken::Lg`]   | 16 | Inner padding of a card, panel, pane or page body — the breathing room between a container's edge and its content. Also the gap between two distinct groups inside one pane. |
//! | [`SpaceToken::Xl`]   | 24 | Between major regions that are not the same thing: a page's header block and its content, a settings group and the next group's heading. |
//! | [`SpaceToken::Xxl`]  | 32 | A sparse hero / empty-state layout, or an outer page margin on a wide window. |
//! | [`SpaceToken::Xxxl`] | 48 | Deliberately generous separation — full-bleed marketing-style sections, a centred empty state's inset. |
//! | [`SpaceToken::Custom`] | any | Escape hatch for a value no rung names (a true optical correction, a sub-rung hairline). If the number about to be passed is one of the sixteen ladder rungs, use its token instead — see [the numeric tier](#the-numeric-tier). |
//!
//! Gap grows with semantic distance: parts of one control get `Xs`, siblings
//! `Sm`, rows inside a block `Md`, a container's own inset `Lg`, unrelated
//! regions `Xl` or wider.
//!
//! # The numeric tier
//!
//! Rungs no semantic token describes have numeric tokens named after the
//! ladder slot they read:
//!
//! | Token | px | ladder slot | sits between |
//! |-------|----|-------------|--------------|
//! | [`SpaceToken::Space3`]  | 6  | `space3`  | `Xs` (4) and `Sm` (8) |
//! | [`SpaceToken::Space5`]  | 10 | `space5`  | `Sm` (8) and `Md` (12) |
//! | [`SpaceToken::Space7`]  | 14 | `space7`  | `Md` (12) and `Lg` (16) |
//! | [`SpaceToken::Space9`]  | 20 | `space9`  | `Lg` (16) and `Xl` (24) |
//! | [`SpaceToken::Space12`] | 40 | `space12` | `Xxl` (32) and `Xxxl` (48) |
//! | [`SpaceToken::Space14`] | 56 | `space14` | `Xxxl` (48) and `space15` (64) |
//! | [`SpaceToken::Space15`] | 64 | `space15` | the top of the ladder |

/// Spacing values for numeric rungs and semantic aliases.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpacingScale {
    /// Lowest rung; 0 logical pixels by default.
    pub space0: f32,
    /// Ladder slot 1; 2 logical pixels by default (`Xxs`).
    pub space1: f32,
    /// Ladder slot 2; 4 logical pixels by default (`Xs`).
    pub space2: f32,
    /// Ladder slot 3; 6 logical pixels by default.
    pub space3: f32,
    /// Ladder slot 4; 8 logical pixels by default (`Sm`).
    pub space4: f32,
    /// Ladder slot 5; 10 logical pixels by default.
    pub space5: f32,
    /// Ladder slot 6; 12 logical pixels by default (`Md`).
    pub space6: f32,
    /// Ladder slot 7; 14 logical pixels by default.
    pub space7: f32,
    /// Ladder slot 8; 16 logical pixels by default (`Lg`).
    pub space8: f32,
    /// Ladder slot 9; 20 logical pixels by default.
    pub space9: f32,
    /// Ladder slot 10; 24 logical pixels by default (`Xl`).
    pub space10: f32,
    /// Ladder slot 11; 32 logical pixels by default (`Xxl`).
    pub space11: f32,
    /// Ladder slot 12; 40 logical pixels by default.
    pub space12: f32,
    /// Ladder slot 13; 48 logical pixels by default (`Xxxl`).
    pub space13: f32,
    /// Ladder slot 14; 56 logical pixels by default.
    pub space14: f32,
    /// Ladder slot 15; 64 logical pixels by default.
    pub space15: f32,
}

impl SpacingScale {
    /// Returns `space2`, whose default is 4 logical pixels.
    pub fn xs(&self) -> f32 {
        self.space2
    }
    /// Returns `space4`, whose default is 8 logical pixels.
    pub fn sm(&self) -> f32 {
        self.space4
    }
    /// Returns `space6`, whose default is 12 logical pixels.
    pub fn md(&self) -> f32 {
        self.space6
    }
    /// Returns `space8`, whose default is 16 logical pixels.
    pub fn lg(&self) -> f32 {
        self.space8
    }
    /// Returns `space10`, whose default is 24 logical pixels.
    pub fn xl(&self) -> f32 {
        self.space10
    }
    /// Returns `space11`, whose default is 32 logical pixels.
    pub fn xxl(&self) -> f32 {
        self.space11
    }
    /// Returns `space13`, whose default is 48 logical pixels.
    pub fn xxxl(&self) -> f32 {
        self.space13
    }
}

/// Semantic space token.
///
/// Semantic tier — a token names a *relationship*:
///
/// | Token        | px  | ladder slot |
/// |--------------|-----|-------------|
/// | `None`       | 0   | space0      |
/// | `Xxs`        | 2   | space1      |
/// | `Xs`         | 4   | space2      |
/// | `Sm`         | 8   | space4      |
/// | `Md` (deflt) | 12  | space6      |
/// | `Lg`         | 16  | space8      |
/// | `Xl`         | 24  | space10     |
/// | `Xxl` (2xl)  | 32  | space11     |
/// | `Xxxl` (3xl) | 48  | space13     |
///
/// Numeric tier — a token names a *rung* the semantic set does not describe
/// (see [the numeric tier](crate::spacing#the-numeric-tier) for why these are
/// deliberately not given semantic names):
///
/// | Token        | px  | ladder slot |
/// |--------------|-----|-------------|
/// | `Space3`     | 6   | space3      |
/// | `Space5`     | 10  | space5      |
/// | `Space7`     | 14  | space7      |
/// | `Space9`     | 20  | space9      |
/// | `Space12`    | 40  | space12     |
/// | `Space14`    | 56  | space14     |
/// | `Space15`    | 64  | space15     |
///
/// Escape hatch — the only variant that does not consult [`SpacingScale`]:
///
/// | Token        | px  | ladder slot |
/// |--------------|-----|-------------|
/// | `Custom(px)` | px  | —           |
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SpaceToken {
    /// Resolves to `space0`, whose default is 0 logical pixels.
    None,
    /// Resolves to `space1`, whose default is 2 logical pixels.
    Xxs,
    /// Resolves to `space2`, whose default is 4 logical pixels.
    Xs,
    /// Resolves to `space4`, whose default is 8 logical pixels.
    Sm,
    /// Resolves to `space6`, whose default is 12 logical pixels; this is the
    /// default token.
    #[default]
    Md,
    /// Resolves to `space8`, whose default is 16 logical pixels.
    Lg,
    /// Resolves to `space10`, whose default is 24 logical pixels.
    Xl,
    /// Resolves to `space11`, whose default is 32 logical pixels.
    Xxl,
    /// Resolves to `space13`, whose default is 48 logical pixels.
    Xxxl,

    /// Resolves to `space3`, whose default is 6 logical pixels.
    Space3,
    /// Resolves to `space5`, whose default is 10 logical pixels.
    Space5,
    /// Resolves to `space7`, whose default is 14 logical pixels.
    Space7,
    /// Resolves to `space9`, whose default is 20 logical pixels.
    Space9,
    /// Resolves to `space12`, whose default is 40 logical pixels.
    Space12,
    /// Resolves to `space14`, whose default is 56 logical pixels.
    Space14,
    /// Resolves to `space15`, whose default is 64 logical pixels.
    Space15,

    /// Unscaled logical-pixel value; negative or non-finite values resolve to
    /// zero.
    Custom(f32),
}

impl SpaceToken {
    /// Resolve against a concrete [`SpacingScale`] (the theme-driven path).
    ///
    /// Prefer this over [`SpaceToken::px`] when a theme is in scope, so a theme
    /// that rescales its ladder flows through.
    pub fn resolve(self, scale: &SpacingScale) -> f32 {
        match self {
            SpaceToken::None => scale.space0,
            SpaceToken::Xxs => scale.space1,
            SpaceToken::Xs => scale.space2,
            SpaceToken::Sm => scale.space4,
            SpaceToken::Md => scale.space6,
            SpaceToken::Lg => scale.space8,
            SpaceToken::Xl => scale.space10,
            SpaceToken::Xxl => scale.space11,
            SpaceToken::Xxxl => scale.space13,
            SpaceToken::Space3 => scale.space3,
            SpaceToken::Space5 => scale.space5,
            SpaceToken::Space7 => scale.space7,
            SpaceToken::Space9 => scale.space9,
            SpaceToken::Space12 => scale.space12,
            SpaceToken::Space14 => scale.space14,
            SpaceToken::Space15 => scale.space15,
            SpaceToken::Custom(v) => {
                if v.is_finite() && v >= 0.0 {
                    v
                } else {
                    0.0
                }
            }
        }
    }

    /// Resolve against the default [`SpacingScale`].
    ///
    /// Convenience for the common case where the default ladder is in force.
    /// Prefer [`SpaceToken::resolve`] whenever a theme is in scope, so a custom
    /// density is honoured rather than silently dropped.
    pub fn px(self) -> f32 {
        self.resolve(&SpacingScale::default())
    }
}

impl Default for SpacingScale {
    fn default() -> Self {
        Self {
            space0: 0.0,
            space1: 2.0,
            space2: 4.0,
            space3: 6.0,
            space4: 8.0,
            space5: 10.0,
            space6: 12.0,
            space7: 14.0,
            space8: 16.0,
            space9: 20.0,
            space10: 24.0,
            space11: 32.0,
            space12: 40.0,
            space13: 48.0,
            space14: 56.0,
            space15: 64.0,
        }
    }
}
