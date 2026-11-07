//! Which step of the [`RadiusScale`] each component family's corners use.
//!
//! The radius scale says how round each step is; [`ShapeScale`] says which
//! step a button, a search field or a card takes. Setting `button = "md"` in
//! the theme file's `[shape]` table squares every unpinned button off to the
//! `md` step; `"full"` makes it a pill.
//!
//! | Family | Default step |
//! |---|---|
//! | `button` | `full` |
//! | `button_group` | `full` |
//! | `search_field` | `full` |
//! | `badge` | `full` |
//! | `card` | `lg` |
//! | `checkbox` | `full` |
//! | `field` | `md` (medium fields; small one step less, large one more) |
//! | `dialog` | `window`: modal dialog surfaces and the stock dialogs, rounded like the window |
//! | `popover` | `xl`: popovers |
//! | `popup` | `lg`: menus, dropdowns, suggestion and autocomplete lists, the command palette |
//! | `row` | `md`: rows inside those popups, and reorderable list rows |
//! | `tooltip` | `md`: tooltips and chart tooltips |
//! | `tab` | `lg`: document tabs |
//! | `key` | `sm`: key caps, key-capture chips and shortcut badges |
//! | `media` | `lg`: the image viewer and carousels |

use crate::RadiusScale;

/// One step of the [`RadiusScale`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RadiusStep {
    /// [`RadiusScale::none`].
    None,
    /// [`RadiusScale::xs`].
    Xs,
    /// [`RadiusScale::sm`].
    Sm,
    /// [`RadiusScale::md`].
    Md,
    /// [`RadiusScale::lg`].
    Lg,
    /// [`RadiusScale::xl`].
    Xl,
    /// [`RadiusScale::xxl`].
    Xxl,
    /// [`RadiusScale::full`]: a pill.
    Full,
    /// The window's own corner radius (the theme's `window.radius`), so a
    /// surface shaped like a window, such as a dialog, matches the window
    /// behind it.
    Window,
}

impl RadiusStep {
    /// Every step, in scale order.
    pub const ALL: [Self; 9] = [
        Self::None,
        Self::Xs,
        Self::Sm,
        Self::Md,
        Self::Lg,
        Self::Xl,
        Self::Xxl,
        Self::Full,
        Self::Window,
    ];

    /// The step's name in the theme file, as in `radius.<name>`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Xs => "xs",
            Self::Sm => "sm",
            Self::Md => "md",
            Self::Lg => "lg",
            Self::Xl => "xl",
            Self::Xxl => "xxl",
            Self::Full => "full",
            Self::Window => "window",
        }
    }

    /// The next rounder step; `none` and `full` stay.
    #[must_use]
    pub const fn rounder(self) -> Self {
        match self {
            Self::Xs => Self::Sm,
            Self::Sm => Self::Md,
            Self::Md => Self::Lg,
            Self::Lg => Self::Xl,
            Self::Xl | Self::Xxl => Self::Xxl,
            other => other,
        }
    }

    /// The next squarer step; `none` and `full` stay.
    #[must_use]
    pub const fn squarer(self) -> Self {
        match self {
            Self::Xxl => Self::Xl,
            Self::Xl => Self::Lg,
            Self::Lg => Self::Md,
            Self::Md => Self::Sm,
            Self::Sm | Self::Xs => Self::Xs,
            other => other,
        }
    }

    /// The step named `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|step| step.name() == name)
    }
}

impl RadiusScale {
    /// The radius of `step`.
    #[must_use]
    pub const fn step(&self, step: RadiusStep) -> f32 {
        match step {
            RadiusStep::None => self.none,
            RadiusStep::Xs => self.xs,
            RadiusStep::Sm => self.sm,
            RadiusStep::Md => self.md,
            RadiusStep::Lg => self.lg,
            RadiusStep::Xl => self.xl,
            RadiusStep::Xxl => self.xxl,
            RadiusStep::Full => self.full,
            // The scale holds no window radius: the theme's own resolution
            // (`text_defaults::shape_radius`) reads `window.radius`.
            RadiusStep::Window => crate::DEFAULT_WINDOW_RADIUS,
        }
    }
}

/// The radius step each component family's unpinned corners use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeScale {
    /// Buttons of every size, filter chips, chip-input chips and pagination
    /// cells.
    pub button: RadiusStep,
    /// The outline of a button group.
    pub button_group: RadiusStep,
    /// An expanded search field.
    pub search_field: RadiusStep,
    /// Badges.
    pub badge: RadiusStep,
    /// Cards.
    pub card: RadiusStep,
    /// Checkbox boxes: `full` makes them round.
    pub checkbox: RadiusStep,
    /// Text fields, password fields, dropdowns, autocomplete and text areas
    /// of the medium size; small ones take the step below and large ones the
    /// step above, except `none` and `full`, which every size takes.
    pub field: RadiusStep,
    /// Modal dialog surfaces and the stock dialogs built on them.
    pub dialog: RadiusStep,
    /// Popovers.
    pub popover: RadiusStep,
    /// Menus, dropdown lists, suggestion and autocomplete lists and the
    /// command palette.
    pub popup: RadiusStep,
    /// Rows inside those popups, and reorderable list rows.
    pub row: RadiusStep,
    /// Tooltips, chart tooltips included.
    pub tooltip: RadiusStep,
    /// Document tabs.
    pub tab: RadiusStep,
    /// Key caps, key-capture chips and shortcut badges.
    pub key: RadiusStep,
    /// The image viewer, the image cropper and carousels.
    pub media: RadiusStep,
}

impl Default for ShapeScale {
    fn default() -> Self {
        Self {
            button: RadiusStep::Full,
            button_group: RadiusStep::Full,
            search_field: RadiusStep::Full,
            badge: RadiusStep::Full,
            card: RadiusStep::Lg,
            // A rounded square: a round checkbox reads as a radio button,
            // and the two often share a form.
            checkbox: RadiusStep::Sm,
            field: RadiusStep::Md,
            dialog: RadiusStep::Window,
            popover: RadiusStep::Xl,
            popup: RadiusStep::Lg,
            row: RadiusStep::Md,
            tooltip: RadiusStep::Md,
            tab: RadiusStep::Lg,
            key: RadiusStep::Sm,
            media: RadiusStep::Lg,
        }
    }
}
