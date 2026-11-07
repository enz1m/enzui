//! Validation for focus rings that must wrap on application-defined keys.
//!
//! The driver retains one widget-tree cache while crossing both boundaries and reports the focused identity after each press.

use super::{KeyboardTraversalHost, PressFailure, describe_slot, focusable_count, press_event};
use crate::focus::Traversal;
use iced_core::keyboard::{self, Modifiers, key};
use iced_core::{Event, Size};
use iced_test::runtime::user_interface::Cache;
use std::fmt::Write as _;

/// Builds a named-key press event with an unidentified physical key.
#[must_use]
pub fn key_press(named: key::Named, modifiers: Modifiers) -> Event {
    let key = keyboard::Key::Named(named);
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
        location: keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })
}

fn key_label(event: &Event) -> String {
    let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
        return format!("{event:?}");
    };
    let mut label = String::new();
    for (held, name) in [
        (modifiers.control(), "Ctrl"),
        (modifiers.alt(), "Alt"),
        (modifiers.logo(), "Logo"),
        (modifiers.shift(), "Shift"),
    ] {
        if held {
            label.push_str(name);
            label.push('+');
        }
    }
    match key {
        keyboard::Key::Named(named) => {
            let _ = write!(label, "{named:?}");
        }
        keyboard::Key::Character(c) => label.push_str(c),
        keyboard::Key::Unidentified => label.push_str("<unidentified>"),
    }
    label
}

/// The forward and backward key events that traverse an application's focus ring.
#[derive(Debug, Clone)]
pub struct TraversalKeys {
    forward: Event,
    backward: Event,
    expect_message: Option<String>,
}

impl TraversalKeys {
    /// The forward and backward events this app binds.
    #[must_use]
    pub fn new(forward: Event, backward: Event) -> Self {
        Self {
            forward,
            backward,
            expect_message: None,
        }
    }

    /// Bare Tab / Shift+Tab — the default shell binding.
    #[must_use]
    pub fn tab() -> Self {
        Self::new(
            key_press(key::Named::Tab, Modifiers::default()),
            key_press(key::Named::Tab, Modifiers::SHIFT),
        )
    }

    /// Requires each traversal press to produce a message whose debug form contains `needle`.
    #[must_use]
    pub fn expecting_message(mut self, needle: impl Into<String>) -> Self {
        self.expect_message = Some(needle.into());
        self
    }

    /// The forward event.
    #[must_use]
    pub fn forward(&self) -> &Event {
        &self.forward
    }

    /// The backward event.
    #[must_use]
    pub fn backward(&self) -> &Event {
        &self.backward
    }
}

/// Describes why wrapping traversal could not be validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WrapTraversalError {
    /// The view contains too few focusable controls to determine whether traversal wraps.
    NotEnoughFocusables {
        /// How many focusable registrations the frame published.
        found: usize,
    },
    /// The widget tree captured the key before the host subscription received it.
    TreeCapturedKey {
        /// The key that was captured.
        key: String,
        /// Which press (1-based) captured.
        press: usize,
    },
    /// No recipe of the host's own subscription decoded the key.
    NoMessageForKey {
        /// The key that went undecoded.
        key: String,
        /// Which press (1-based) went undecoded.
        press: usize,
        /// How many recipes were driven, so zero is distinguishable from
        /// "several recipes, none of which decode this key".
        recipes: usize,
    },
    /// The decoded message produced no widget operation.
    NoWidgetOperation {
        /// The key that was decoded.
        key: String,
        /// Which press (1-based).
        press: usize,
        /// The message that went unanswered.
        message: String,
    },
    /// The decoded message produced a widget operation aimed at another window.
    OperationInWrongWindow {
        /// The key that was decoded.
        key: String,
        /// Which press (1-based).
        press: usize,
        /// The window that dispatched the press.
        window: String,
        /// The windows the produced operations named instead.
        targets: Vec<String>,
    },
    /// The produced message did not match the required message substring.
    UnexpectedMessage {
        /// The key pressed.
        key: String,
        /// Which press (1-based).
        press: usize,
        /// The substring the caller required.
        expected: String,
        /// The messages actually produced.
        got: String,
    },
    /// The pipeline completed but left no control focused away from a wrap boundary.
    NothingFocused {
        /// The key pressed.
        key: String,
        /// Which press (1-based).
        press: usize,
    },
    /// A focus identity repeated before completing one traversal lap.
    UnstableFocusIdentity {
        /// Which press (1-based) repeated an identity.
        press: usize,
        /// The repeated identity.
        identity: String,
        /// The earlier press (1-based) that already reported it.
        first_seen_press: usize,
    },
    /// The boundary press did not wrap.
    DidNotWrap {
        /// Which edge was crossed.
        direction: Traversal,
        /// How many focusables the frame registered.
        focusables: usize,
        /// Which press (1-based) should have wrapped.
        press: usize,
        /// The identity the wrap had to reach.
        expected: String,
        /// Where it landed instead.
        got: String,
        /// The whole trail so far.
        trail: Vec<String>,
    },
    /// The backward traversal did not retrace the forward trail.
    DidNotReverse {
        /// Which backward press (1-based).
        press: usize,
        /// The identity the reverse walk had to reach.
        expected: String,
        /// Where it landed instead.
        got: String,
        /// The backward trail so far.
        trail: Vec<String>,
    },
}

impl std::fmt::Display for WrapTraversalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotEnoughFocusables { found } => write!(
                f,
                "the validated frame registers {found} focusable control(s); at \
                 least 2 are needed for \"did the ring wrap?\" to be a \
                 question this frame can answer. Seed the route you mean \
                 before validating."
            ),
            Self::TreeCapturedKey { key, press } => write!(
                f,
                "press {press} ({key}) was CAPTURED by the widget tree, so the \
                 app's subscription never saw it. That is correct for a \
                 presented modal (it contains focus on purpose) and a defect \
                 for a bare window — validate the bare window, or name the key \
                 the surface leaves free."
            ),
            Self::NoMessageForKey {
                key,
                press,
                recipes,
            } => write!(
                f,
                "NO TRAVERSAL FOR {key}: at press {press} the app's own \
                 subscription() ({recipes} recipe(s)) produced no message, so \
                 nothing can advance the focus ring with this key. Either the \
                 app binds a different key (pass it via TraversalKeys — bare Tab is \
                 not assumed) or the binding is missing entirely."
            ),
            Self::NoWidgetOperation {
                key,
                press,
                message,
            } => write!(
                f,
                "press {press} ({key}) decoded into {message}, but the update \
                 arm answering it ran NO widget operation: the ring is \
                 advertised and never advanced. Answer it with \
                 `iced::advanced::widget::operate(\n\
                 \x20   enzui_core::focus::traversal_operation(direction))`."
            ),
            Self::OperationInWrongWindow {
                key,
                press,
                window,
                targets,
            } => write!(
                f,
                "press {press} ({key}) in window {window} produced a widget \
                 operation aimed at {targets:?}. A traversal must run in the \
                 window that produced the input, or one window's Tab advances \
                 another window's ring."
            ),
            Self::UnexpectedMessage {
                key,
                press,
                expected,
                got,
            } => write!(
                f,
                "press {press} ({key}) was answered by {got}, which does not \
                 contain the required {expected:?}. The ring moved through an \
                 arm other than the one this validation was told to pin, so the \
                 pass would not be about the app's traversal binding."
            ),
            Self::NothingFocused { key, press } => write!(
                f,
                "after press {press} ({key}) NOTHING is focused. A traversal \
                 that drops focus mid-walk strands a keyboard-only user with \
                 no ring on screen and no way back into the window."
            ),
            Self::UnstableFocusIdentity {
                press,
                identity,
                first_seen_press,
            } => write!(
                f,
                "press {press} landed on {identity}, which press \
                 {first_seen_press} already reported. Within one lap every \
                 stop must be distinct: either focus latched onto one control \
                 or the identity is not stable across rebuilds, and in either \
                 case a wrap cannot be told from a repeat."
            ),
            Self::DidNotWrap {
                direction,
                focusables,
                press,
                expected,
                got,
                trail,
            } => {
                let edge = if direction.is_forward() {
                    "forward past the LAST control must return to the FIRST"
                } else {
                    "backward past the FIRST control must return to the LAST"
                };
                write!(
                    f,
                    "THE RING DOES NOT WRAP: {edge}, but press {press} of \
                     {focusables} focusable(s) landed on {got} instead of \
                     {expected}. This is the exact signature of iced's \
                     non-wrapping focus_next()/focus_previous(): every earlier \
                     press moves correctly and only the boundary press stalls. \
                     Answer the traversal with \
                     `enzui_core::focus::traversal_operation(direction)`. \
                     Trail: {trail:?}"
                )
            }
            Self::DidNotReverse {
                press,
                expected,
                got,
                trail,
            } => write!(
                f,
                "the backward key does not reverse: backward press {press} \
                 landed on {got} instead of {expected}, so the backward walk \
                 does not retrace the forward one. `Task`'s Debug renders \
                 focus_next() and focus_previous() identically, so a reducer \
                 test structurally cannot see this. Backward trail: {trail:?}"
            ),
        }
    }
}

/// What one press observed, in the terms this driver reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraversalStep {
    /// The focused identity after the press, or `None` when no control is focused.
    pub identity: Option<String>,
    /// The ring slot that holds focus, if any.
    pub slot: Option<usize>,
    /// The messages the host's own subscription produced.
    pub messages: Vec<String>,
    /// How many widget operations the host's `update` returned.
    pub operations: usize,
}

/// What a failure text prints where an identity would go, when nothing is
/// focused at all.
pub const NOTHING_FOCUSED: &str = "<nothing focused>";

/// Retains widget-tree focus state across a sequence of traversal presses.
pub struct TraversalSession {
    cache: Option<Cache>,
    presses: usize,
}

impl Default for TraversalSession {
    fn default() -> Self {
        Self::new()
    }
}

impl TraversalSession {
    /// A session with no retained tree yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            cache: None,
            presses: 0,
        }
    }

    /// How many presses this session has driven.
    #[must_use]
    pub fn presses(&self) -> usize {
        self.presses
    }

    /// Drives one key press through the host and reports its resulting focus state.
    ///
    /// # Errors
    ///
    /// Returns the pipeline failure that prevented a focus reading.
    pub fn press<H: KeyboardTraversalHost>(
        &mut self,
        host: &mut H,
        size: Size,
        event: &Event,
        expect_message: Option<&str>,
    ) -> Result<TraversalStep, WrapTraversalError> {
        self.presses += 1;
        let press = self.presses;
        let key = key_label(event);
        let outcome = press_event(host, size, event.clone(), &mut self.cache).map_err(
            |failure| match failure {
                PressFailure::TreeCaptured => WrapTraversalError::TreeCapturedKey {
                    key: key.clone(),
                    press,
                },
                PressFailure::NoMessage { recipes } => WrapTraversalError::NoMessageForKey {
                    key: key.clone(),
                    press,
                    recipes,
                },
                PressFailure::NoWidgetOperation { message } => {
                    WrapTraversalError::NoWidgetOperation {
                        key: key.clone(),
                        press,
                        message,
                    }
                }
                PressFailure::OperationInWrongWindow { window, targets } => {
                    WrapTraversalError::OperationInWrongWindow {
                        key: key.clone(),
                        press,
                        window,
                        targets,
                    }
                }
            },
        )?;
        if let Some(needle) = expect_message
            && !outcome.messages.iter().any(|m| m.contains(needle))
        {
            return Err(WrapTraversalError::UnexpectedMessage {
                key,
                press,
                expected: needle.to_owned(),
                got: outcome.messages.join(", "),
            });
        }
        Ok(TraversalStep {
            identity: describe_slot(outcome.focused.as_ref()),
            slot: outcome.focused.as_ref().map(|(slot, _)| *slot),
            messages: outcome.messages,
            operations: outcome.operations,
        })
    }
}

/// What a wrapping-traversal validation measured when it succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrapTraversalReport {
    /// How many focusable controls the validated frame registered.
    pub focusables: usize,
    /// The identity after each forward press — `focusables + 1` of them, the
    /// last being the wrap back onto the first.
    pub forward_trail: Vec<String>,
    /// The identity after each backward press — `focusables` of them, the
    /// first being the wrap back onto the last.
    pub backward_trail: Vec<String>,
}

impl WrapTraversalReport {
    /// Total presses the validation drove.
    #[must_use]
    pub fn presses(&self) -> usize {
        self.forward_trail.len() + self.backward_trail.len()
    }
}

/// Drives both boundaries of the host's focus ring and returns the observed trails.
///
/// # Errors
///
/// Returns the first [`WrapTraversalError`] encountered.
pub fn validate_wrapping_traversal<H: KeyboardTraversalHost>(
    host: &mut H,
    size: Size,
    keys: &TraversalKeys,
) -> Result<WrapTraversalReport, WrapTraversalError> {
    // One runtime scope spans the walk so focus survives rebuilds without state from earlier validations.
    let _runtime_scope = crate::widget_runtime::WidgetRuntimeScope::new();
    let focusables = focusable_count(host, size);
    if focusables < 2 {
        return Err(WrapTraversalError::NotEnoughFocusables { found: focusables });
    }
    let expect = keys.expect_message.as_deref();

    let mut session = TraversalSession::new();
    let mut forward_trail: Vec<String> = Vec::new();
    for _ in 0..focusables {
        let step = session.press(host, size, &keys.forward, expect)?;
        let Some(identity) = step.identity else {
            return Err(WrapTraversalError::NothingFocused {
                key: key_label(&keys.forward),
                press: session.presses(),
            });
        };
        if let Some(first) = forward_trail.iter().position(|seen| *seen == identity) {
            return Err(WrapTraversalError::UnstableFocusIdentity {
                press: session.presses(),
                identity,
                first_seen_press: first + 1,
            });
        }
        forward_trail.push(identity);
    }

    // The extra forward press is the boundary check; absence of focus there is a failure to wrap.
    let step = session.press(host, size, &keys.forward, expect)?;
    let landed = step.identity.unwrap_or_else(|| NOTHING_FOCUSED.to_owned());
    if landed != forward_trail[0] {
        return Err(WrapTraversalError::DidNotWrap {
            direction: Traversal::Next,
            focusables,
            press: session.presses(),
            expected: forward_trail[0].clone(),
            got: landed,
            trail: forward_trail,
        });
    }
    forward_trail.push(landed);

    let mut backward_trail: Vec<String> = Vec::new();
    for back in 1..=focusables {
        let step = session.press(host, size, &keys.backward, expect)?;
        let expected = forward_trail[focusables - back].clone();
        // No focus on the first backward press means no wrap; later absence means focus was dropped mid-lap.
        let landed = match step.identity {
            Some(identity) => identity,
            None if back == 1 => NOTHING_FOCUSED.to_owned(),
            None => {
                return Err(WrapTraversalError::NothingFocused {
                    key: key_label(&keys.backward),
                    press: session.presses(),
                });
            }
        };
        if landed != expected {
            backward_trail.push(landed.clone());
            return Err(if back == 1 {
                WrapTraversalError::DidNotWrap {
                    direction: Traversal::Previous,
                    focusables,
                    press: session.presses(),
                    expected,
                    got: landed,
                    trail: backward_trail,
                }
            } else {
                WrapTraversalError::DidNotReverse {
                    press: back,
                    expected,
                    got: landed,
                    trail: backward_trail,
                }
            });
        }
        backward_trail.push(landed);
    }

    Ok(WrapTraversalReport {
        focusables,
        forward_trail,
        backward_trail,
    })
}

/// Asserts that the host's focus ring wraps in both directions.
///
/// # Panics
///
/// Panics with the [`WrapTraversalError`] when validation fails.
pub fn assert_traversal_wraps<H: KeyboardTraversalHost>(
    host: &mut H,
    size: Size,
    keys: &TraversalKeys,
) -> WrapTraversalReport {
    match validate_wrapping_traversal(host, size, keys) {
        Ok(report) => report,
        #[allow(
            clippy::panic,
            reason = "the asserting form of `validate_wrapping_traversal`, for tests"
        )]
        Err(e) => panic!(
            "wrapping-traversal validation failed at {}x{} on {} / {}:\n\n{e}\n",
            size.width,
            size.height,
            key_label(keys.forward()),
            key_label(keys.backward()),
        ),
    }
}
