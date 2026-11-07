    use super::*;

    #[derive(Debug)]
    pub(super) enum LeaderProbe {
        Running,
        Reaped(ExitStatus),
    }

    #[derive(Debug)]
    pub(super) struct OwnedGroup;

    pub(super) fn configure(_command: &mut Command) {}

    impl OwnedGroup {
        pub(super) fn capture(_child: &Child, _faults: &Faults) -> Self {
            Self
        }

        pub(super) fn setup_error(&self) -> Option<String> {
            None
        }

        pub(super) fn release_identity(&mut self) {}

        pub(super) fn probe_leader(&self, child: &mut Child) -> Result<LeaderProbe, String> {
            child
                .try_wait()
                .map(|status| status.map_or(LeaderProbe::Running, LeaderProbe::Reaped))
                .map_err(|error| bounded_diagnostic(format!("query child: {error}")))
        }

        pub(super) fn terminate(
            &mut self,
            child: &mut Child,
            _grace: Duration,
        ) -> Result<(), TeardownError> {
            child
                .kill()
                .or_else(|error| {
                    if error.kind() == io::ErrorKind::InvalidInput {
                        Ok(())
                    } else {
                        Err(error)
                    }
                })
                .map_err(|error| {
                    TeardownError::wait(bounded_diagnostic(format!("kill child: {error}")))
                })
        }
    }
