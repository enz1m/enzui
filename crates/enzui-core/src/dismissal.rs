//! Renderer-neutral dismissal policy for transient surfaces.
//!
//! The surface, opening trigger, and additional registered regions count as inside. Pointer
//! dismissal is decided on press, modal surfaces consume outside input, and Escape is
//! configured independently.

use iced_core::{Point, Rectangle};

/// Controls whether input outside a surface is consumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Modality {
    /// Lets outside input reach the underlying target.
    #[default]
    NonModal,
    /// Consumes outside input so a dismissing press cannot activate an underlying target.
    Modal,
}

impl Modality {
    /// Whether an outside press is consumed rather than passed through.
    #[must_use]
    pub const fn consumes_outside_input(self) -> bool {
        matches!(self, Self::Modal)
    }
}

/// Controls whether an outside pointer press dismisses the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutsidePolicy {
    /// Allows dismissal by an outside pointer press.
    #[default]
    Dismiss,
    /// Prevents spatial dismissal; Escape and explicit actions remain independent.
    Ignore,
}

/// Controls whether Escape dismisses the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscapePolicy {
    /// Escape dismisses.
    #[default]
    Dismiss,
    /// Escape does not dismiss.
    Ignore,
}

/// The full dismissal policy of one surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DismissPolicy {
    /// The outside-input modality.
    pub modality: Modality,
    /// The outside-pointer dismissal policy.
    pub outside: OutsidePolicy,
    /// The Escape dismissal policy.
    pub escape: EscapePolicy,
}

impl DismissPolicy {
    /// A dropdown / popover / menu: light dismiss, pass-through, Escape closes.
    #[must_use]
    pub const fn popover() -> Self {
        Self {
            modality: Modality::NonModal,
            outside: OutsidePolicy::Dismiss,
            escape: EscapePolicy::Dismiss,
        }
    }

    /// A modal surface that consumes and dismisses on outside presses.
    #[must_use]
    pub const fn modal() -> Self {
        Self {
            modality: Modality::Modal,
            outside: OutsidePolicy::Dismiss,
            escape: EscapePolicy::Dismiss,
        }
    }

    /// A modal surface dismissed only by explicit actions.
    #[must_use]
    pub const fn insistent() -> Self {
        Self {
            modality: Modality::Modal,
            outside: OutsidePolicy::Ignore,
            escape: EscapePolicy::Ignore,
        }
    }

    /// A non-modal surface that ignores pointer presses, passes input through, and dismisses on
    /// Escape.
    #[must_use]
    pub const fn transient() -> Self {
        Self {
            modality: Modality::NonModal,
            outside: OutsidePolicy::Ignore,
            escape: EscapePolicy::Dismiss,
        }
    }

    /// Set the modality.
    #[must_use]
    pub const fn modality(mut self, modality: Modality) -> Self {
        self.modality = modality;
        self
    }

    /// Set the outside-press policy.
    #[must_use]
    pub const fn outside(mut self, outside: OutsidePolicy) -> Self {
        self.outside = outside;
        self
    }

    /// Set the Escape policy.
    #[must_use]
    pub const fn escape(mut self, escape: EscapePolicy) -> Self {
        self.escape = escape;
        self
    }
}

/// Regions treated as inside, with the opening trigger tracked separately from the surface.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DismissRegions {
    surface: Option<Rectangle>,
    trigger: Option<Rectangle>,
    extra: Vec<Rectangle>,
}

impl DismissRegions {
    /// Regions covering just the surface.
    #[must_use]
    pub fn new(surface: Rectangle) -> Self {
        Self {
            surface: Some(surface),
            trigger: None,
            extra: Vec::new(),
        }
    }

    /// Regions with no surface yet — every press reads as outside.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add the trigger that opened this surface.
    #[must_use]
    pub fn trigger(mut self, trigger: Rectangle) -> Self {
        self.trigger = Some(trigger);
        self
    }

    /// Adds another rectangle that counts as inside.
    #[must_use]
    pub fn with(mut self, rect: Rectangle) -> Self {
        self.extra.push(rect);
        self
    }

    /// Returns whether a point lies in any inside region; non-finite coordinates are outside.
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() {
            return false;
        }
        self.surface
            .iter()
            .chain(self.trigger.iter())
            .chain(self.extra.iter())
            .any(|rect| contains_point(rect, point))
    }

    /// Returns whether the press landed on the opening trigger, which the host handles as a
    /// toggle rather than a dismissal.
    #[must_use]
    pub fn hits_trigger(&self, point: Point) -> bool {
        if !point.x.is_finite() || !point.y.is_finite() {
            return false;
        }
        self.trigger
            .as_ref()
            .is_some_and(|rect| contains_point(rect, point))
    }

    /// How many regions count as inside.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.surface.is_some()) + usize::from(self.trigger.is_some()) + self.extra.len()
    }

    /// Whether no region counts as inside.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// Use half-open bounds so a shared right or bottom edge belongs to the adjacent region.
fn contains_point(rect: &Rectangle, point: Point) -> bool {
    point.x >= rect.x
        && point.x < rect.x + rect.width
        && point.y >= rect.y
        && point.y < rect.y + rect.height
}

/// What a host should do with an interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dismissal {
    /// Closes the surface and consumes the interaction.
    Dismiss,
    /// Closes the surface and lets the interaction propagate.
    DismissPassThrough,
    /// Do not close, and consume the interaction anyway (modal shielding).
    Shield,
    /// Do not close; let the interaction through.
    Ignore,
}

impl Dismissal {
    /// Whether the surface should close.
    #[must_use]
    pub const fn closes(self) -> bool {
        matches!(self, Self::Dismiss | Self::DismissPassThrough)
    }

    /// Returns whether the host should consume the interaction.
    #[must_use]
    pub const fn consumes(self) -> bool {
        matches!(self, Self::Dismiss | Self::Shield)
    }
}

/// Decides a pointer-press outcome.
///
/// Trigger presses are ignored so the trigger’s toggle handler owns the transition; release
/// positions are not used for dismissal.
#[must_use]
pub fn on_press(policy: DismissPolicy, regions: &DismissRegions, point: Point) -> Dismissal {
    if regions.contains(point) {
        return Dismissal::Ignore;
    }
    match policy.outside {
        OutsidePolicy::Dismiss => {
            if policy.modality.consumes_outside_input() {
                Dismissal::Dismiss
            } else {
                Dismissal::DismissPassThrough
            }
        }
        OutsidePolicy::Ignore => {
            if policy.modality.consumes_outside_input() {
                Dismissal::Shield
            } else {
                Dismissal::Ignore
            }
        }
    }
}

/// Decides the outcome of pressing Escape.
#[must_use]
pub fn on_escape(policy: DismissPolicy) -> Dismissal {
    match policy.escape {
        EscapePolicy::Dismiss => Dismissal::Dismiss,
        EscapePolicy::Ignore => Dismissal::Ignore,
    }
}

/// Decides propagation for a non-press pointer event using the inside regions and modality.
#[must_use]
pub fn on_other_pointer_event(
    policy: DismissPolicy,
    regions: &DismissRegions,
    point: Point,
) -> Dismissal {
    if regions.contains(point) {
        return Dismissal::Ignore;
    }
    if policy.modality.consumes_outside_input() {
        Dismissal::Shield
    } else {
        Dismissal::Ignore
    }
}
