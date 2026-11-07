//! A popover: a floating panel attached to a control (GTK `Popover`,
//! Qt's popup widgets, SwiftUI `.popover`).
//!
//! The application owns whether it is open: pass its content while it is,
//! `None` while it is not, and close it when the dismiss message arrives. A
//! press outside the popover and its trigger, or <kbd>Escape</kbd>, dismisses
//! it; a press on the trigger reaches the trigger, so a toggle button keeps
//! working. The panel sits below the trigger by default, flips and shifts to
//! stay in the window, and adds no space to the layout.
//!
//! Focus: give it a [`scope`](EnzPopover::scope) and run
//! [`focus_operation`] when it opens to move focus to its first control; on
//! dismissal, focus the trigger again. Assistive technology sees a non-modal
//! dialog named by [`label`](EnzPopover::label).
//!
//! For a list of commands use a menu ([`menu_button`](crate::menu_button));
//! a popover holds arbitrary content: a form, a colour grid, a calendar.

use crate::menu::{MENU_ELEVATION, MenuTokens};
use enzui_core::placement::{Alignment, Direction, Placement, Side, resolve_direction};
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::widget::{Id, Operation};
use iced::widget::container;
use iced::{Background, Border, Element, Length, Padding, Rectangle};
use std::rc::Rc;

/// The space between the trigger and the panel.
pub const POPOVER_GAP: f32 = 6.0;

/// The panel's corner radius in the built-in theme; the theme's `popover`
/// radius is what the panel draws with.
pub const POPOVER_RADIUS: f32 = 12.0;

/// The panel's inner padding.
pub const POPOVER_PADDING: f32 = 12.0;

/// Builder for [`enz_popover`].
#[must_use]
pub struct EnzPopover<'a, Message> {
    trigger: Element<'a, Message>,
    content: Option<Element<'a, Message>>,
    on_dismiss: Rc<dyn Fn() -> Message + 'a>,
    side: Side,
    alignment: Alignment,
    direction: Option<Direction>,
    anchor: Option<Rectangle>,
    width: Option<f32>,
    padding: f32,
    label: Option<String>,
    scope: Option<Id>,
    material: Option<enzui_theme::MaterialStyle>,
    grain: enzui_theme::GrainSpec,
}

/// A popover attached to `trigger`, showing `content` while it is `Some`.
/// `on_dismiss` is published for an outside press or Escape; the application
/// closes the popover in response.
pub fn enz_popover<'a, Message: Clone + 'a>(
    trigger: impl Into<Element<'a, Message>>,
    content: Option<Element<'a, Message>>,
    on_dismiss: impl Fn() -> Message + 'a,
) -> EnzPopover<'a, Message> {
    EnzPopover {
        trigger: trigger.into(),
        content,
        on_dismiss: Rc::new(on_dismiss),
        side: Side::Bottom,
        // Leading edges aligned, as menus open: a panel without a pointer
        // arrow centred on a narrow trigger reads as misplaced and spills
        // past the content it belongs to.
        alignment: Alignment::Start,
        direction: None,
        anchor: None,
        width: None,
        padding: POPOVER_PADDING,
        label: None,
        scope: None,
        material: None,
        grain: enzui_theme::GrainSpec::None,
    }
}

impl<'a, Message: Clone + 'a> EnzPopover<'a, Message> {
    /// Which side of the trigger the panel opens on; default below.
    pub fn side(mut self, side: Side) -> Self {
        self.side = side;
        self
    }

    /// How the panel lines up along that side; default its leading edge on
    /// the trigger's.
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// The writing direction for start and end alignment; by default the
    /// locale's.
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Attach to a window-space rectangle instead of the trigger's bounds:
    /// a text selection, a chart point, a canvas item.
    pub fn anchor(mut self, bounds: Rectangle) -> Self {
        self.anchor = Some(bounds);
        self
    }

    /// A fixed panel width; by default it hugs its content.
    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    /// The space inside the panel's edge; default [`POPOVER_PADDING`].
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = padding.max(0.0);
        self
    }

    /// The name assistive technology announces for the panel.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The panel's focus scope, for [`focus_operation`] and scoped Tab.
    pub fn scope(mut self, scope: Id) -> Self {
        self.scope = Some(scope);
        self
    }

    /// Lay grain on the panel; see [`crate::grain`].
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Paint the panel in a glass material instead of the solid surface.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Build the trigger with the panel attached.
    pub fn into_element(self) -> Element<'a, Message> {
        let direction = resolve_direction(self.direction);
        let placement = Placement::new(self.side)
            .gap(POPOVER_GAP)
            .alignment(self.alignment)
            .direction(direction);
        let material = self.material;
        let padding = self.padding;
        let width = self.width;
        let label = self.label;
        let scope = self.scope;
        let grain = self.grain;
        let radius = enzui_theme::text_defaults::shape_radius(|s| s.popover);
        let panel = self.content.map(|content| {
            let content: Element<'a, Message> = if grain == enzui_theme::GrainSpec::None {
                content
            } else {
                crate::grain::GrainLayer::new(content, grain)
                    .outset(Padding::from(padding))
                    .radius(radius.into())
                    .into()
            };
            let mut body = container(content).padding(Padding::from(padding));
            if let Some(scope) = scope {
                body = body.id(scope);
            }
            if let Some(width) = width {
                body = body.width(Length::Fixed(width));
            }
            let body = body.style(move |theme: &iced::Theme| {
                if material.is_some() {
                    return container::Style::default();
                }
                let tokens = crate::theme_lookup::resolve(theme, MenuTokens::from_palette);
                container::Style {
                    background: Some(Background::Color(tokens.background)),
                    border: Border {
                        color: tokens.border,
                        width: tokens.border_width,
                        radius: radius.into(),
                    },
                    shadow: crate::elevation::iced_shadow_for_theme(theme, MENU_ELEVATION),
                    ..Default::default()
                }
            });
            let mut panel: Element<'a, Message> = body.into();
            if let Some(style) = material {
                panel = crate::glass::enz_glass(panel)
                    .style(style)
                    .radius(radius)
                    .elevation(MENU_ELEVATION)
                    .into();
            }
            let mut semantics = Semantics::new(Role::Dialog).modal(false);
            if let Some(label) = label {
                semantics = semantics.label(label);
            }
            Element::from(crate::semantics_container::SemanticsContainer::new(
                panel, semantics,
            ))
        });
        let on_dismiss = self.on_dismiss;
        let dismiss = move || on_dismiss();
        match self.anchor {
            Some(anchor) => crate::anchored_overlay::dismissible_anchored_menu_at(
                self.trigger,
                panel,
                anchor,
                placement,
                dismiss,
            ),
            None => crate::anchored_overlay::dismissible_anchored_menu(
                self.trigger,
                panel,
                placement,
                dismiss,
            ),
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzPopover<'a, Message>> for Element<'a, Message> {
    fn from(popover: EnzPopover<'a, Message>) -> Self {
        popover.into_element()
    }
}

/// Move focus to the first control in the popover with `scope`, taking it
/// off the trigger and everything else. Run it with
/// `iced::advanced::widget::operate` when the popover opens.
#[must_use]
pub fn focus_operation<T>(scope: Id) -> Box<dyn Operation<T>>
where
    T: Send + 'static,
{
    Box::new(UnfocusThen {
        next: std::sync::Mutex::new(Some(crate::modal_host::initial_focus_operation(scope))),
    })
}

/// Unfocuses every focusable widget, then runs `next`.
struct UnfocusThen<T> {
    /// Taken once, when this pass finishes.
    next: std::sync::Mutex<Option<Box<dyn Operation<T>>>>,
}

impl<T: Send + 'static> Operation<T> for UnfocusThen<T> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        state.unfocus();
    }

    fn finish(&self) -> iced::advanced::widget::operation::Outcome<T> {
        use iced::advanced::widget::operation::Outcome;
        let next = self
            .next
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        next.map_or(Outcome::None, Outcome::Chain)
    }
}
