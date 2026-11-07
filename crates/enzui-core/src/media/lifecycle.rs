//! Worker lifecycle markers for recording shutdown and join completion before replacement.

/// Where a worker is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    /// Shutdown has not been requested through this lifecycle.
    Running,
    /// Shutdown was requested, but the worker thread has not yet been joined.
    Draining,
    /// The owner has recorded completion of the worker join; terminal.
    Joined,
}

impl Phase {
    /// Returns whether this is the terminal [`Phase::Joined`] state.
    #[must_use]
    pub const fn is_joined(self) -> bool {
        matches!(self, Phase::Joined)
    }
}

/// Tracks the `Running` to `Draining` to `Joined` worker lifecycle.
#[derive(Debug, Clone)]
pub struct WorkerLifecycle {
    phase: Phase,
}

impl WorkerLifecycle {
    /// A newly-started worker in [`Phase::Running`].
    #[must_use]
    pub fn start() -> Self {
        WorkerLifecycle {
            phase: Phase::Running,
        }
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Returns whether the lifecycle has reached [`Phase::Joined`].
    #[must_use]
    pub fn is_joined(&self) -> bool {
        self.phase.is_joined()
    }

    /// Records a transition from [`Phase::Running`] to [`Phase::Draining`] and reports whether it changed.
    pub fn begin_shutdown(&mut self) -> bool {
        match self.phase {
            Phase::Running => {
                self.phase = Phase::Draining;
                true
            }
            Phase::Draining | Phase::Joined => false,
        }
    }

    /// Marks a draining worker joined and returns `true` exactly once.
    ///
    /// Returns `false` without change for running or already joined workers.
    pub fn complete_join(&mut self) -> bool {
        match self.phase {
            Phase::Draining => {
                self.phase = Phase::Joined;
                true
            }
            Phase::Running | Phase::Joined => false,
        }
    }

    /// Advances the shutdown and join markers and reports whether this call entered [`Phase::Joined`].
    pub fn shutdown_and_join(&mut self) -> bool {
        self.begin_shutdown();
        self.complete_join()
    }

    /// Returns a fresh running lifecycle only after this one is joined.
    ///
    /// Returns the blocking phase when replacement is premature.
    ///
    /// # Errors
    ///
    /// The blocking [`Phase`] when this lifecycle has not been joined.
    pub fn replace(&self) -> Result<WorkerLifecycle, Phase> {
        if self.phase.is_joined() {
            Ok(WorkerLifecycle::start())
        } else {
            Err(self.phase)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_without_shutdown_is_refused() {
        let mut w = WorkerLifecycle::start();
        assert!(
            !w.complete_join(),
            "joining a Running worker must be refused"
        );
        assert_eq!(w.phase(), Phase::Running, "a refused join changes nothing");
    }
}
