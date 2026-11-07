//! The state of a step-by-step assistant (a wizard): which step is shown,
//! which were reached, and where Back, Next and the step list may go.
//!
//! The pages and their validation stay with the app; [`Wizard`] only moves
//! between them. `enzui_widgets::wizard` draws it.

/// A wizard over `steps` pages. See the [module](self) documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wizard {
    steps: usize,
    current: usize,
    /// The furthest step reached; steps up to it can be revisited.
    reached: usize,
}

impl Wizard {
    /// A wizard at the first of `steps` pages (at least one).
    #[must_use]
    pub fn new(steps: usize) -> Self {
        Self {
            steps: steps.max(1),
            current: 0,
            reached: 0,
        }
    }

    /// The step shown, from zero.
    #[must_use]
    pub fn current(&self) -> usize {
        self.current
    }

    /// How many steps there are.
    #[must_use]
    pub fn steps(&self) -> usize {
        self.steps
    }

    /// Whether the step shown is the last: Next becomes Finish.
    #[must_use]
    pub fn is_last(&self) -> bool {
        self.current + 1 == self.steps
    }

    /// Whether Back goes anywhere.
    #[must_use]
    pub fn can_go_back(&self) -> bool {
        self.current > 0
    }

    /// Whether `step` has been reached, so the step list may jump to it.
    #[must_use]
    pub fn is_reached(&self, step: usize) -> bool {
        step <= self.reached
    }

    /// Moves to the next step; `false` on the last.
    pub fn advance(&mut self) -> bool {
        if self.is_last() {
            return false;
        }
        self.current += 1;
        self.reached = self.reached.max(self.current);
        true
    }

    /// Moves to the previous step; `false` on the first.
    pub fn back(&mut self) -> bool {
        if self.current == 0 {
            return false;
        }
        self.current -= 1;
        true
    }

    /// Jumps to a reached step; `false` for one not yet reached.
    pub fn go_to(&mut self, step: usize) -> bool {
        if !self.is_reached(step) || step >= self.steps {
            return false;
        }
        self.current = step;
        true
    }

    /// Forgets the steps after the current one, after an answer changed what
    /// they will ask.
    pub fn invalidate_later(&mut self) {
        self.reached = self.current;
    }
}
