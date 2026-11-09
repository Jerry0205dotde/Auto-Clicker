#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunState {
    #[default]
    Ready,
    Starting,
    Clicking,
    Stopped,
    Error,
    Closing,
}

#[derive(Debug, Default)]
pub struct StateMachine {
    state: RunState,
    ended_runs: u64,
}

impl StateMachine {
    pub const fn state(&self) -> RunState {
        self.state
    }

    /// Counts how often a pending or active run has ended, whatever the cause.
    pub const fn ended_runs(&self) -> u64 {
        self.ended_runs
    }

    const fn is_active(&self) -> bool {
        matches!(self.state, RunState::Starting | RunState::Clicking)
    }

    pub fn request_start(&mut self) -> bool {
        match self.state {
            RunState::Ready | RunState::Stopped | RunState::Error => {
                self.state = RunState::Starting;
                true
            }
            RunState::Starting | RunState::Clicking | RunState::Closing => false,
        }
    }

    pub fn started(&mut self) -> bool {
        if self.state == RunState::Starting {
            self.state = RunState::Clicking;
            true
        } else {
            false
        }
    }

    pub fn stop(&mut self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.state = RunState::Stopped;
        self.ended_runs = self.ended_runs.wrapping_add(1);
        true
    }

    pub fn fail(&mut self) {
        if self.is_active() {
            self.ended_runs = self.ended_runs.wrapping_add(1);
        }
        if self.state != RunState::Closing {
            self.state = RunState::Error;
        }
    }

    pub fn close(&mut self) {
        self.state = RunState::Closing;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prevents_double_start() {
        let mut machine = StateMachine::default();
        assert!(machine.request_start());
        assert!(!machine.request_start());
        assert!(machine.started());
        assert!(!machine.request_start());
    }

    #[test]
    fn stops_while_active() {
        let mut machine = StateMachine::default();
        assert!(machine.request_start());
        assert!(machine.started());
        assert!(machine.stop());
        assert_eq!(machine.state(), RunState::Stopped);
    }

    #[test]
    fn stops_while_starting() {
        let mut machine = StateMachine::default();
        assert!(machine.request_start());
        assert!(machine.stop());
        assert_eq!(machine.state(), RunState::Stopped);
        assert!(!machine.started());
    }

    #[test]
    fn counts_every_end_of_a_pending_or_active_run() {
        let mut machine = StateMachine::default();
        machine.fail();
        assert!(!machine.stop());
        assert_eq!(machine.ended_runs(), 0);
        // Stopped while starting, stopped while clicking, failed while starting.
        assert!(machine.request_start());
        assert!(machine.stop());
        assert!(machine.request_start());
        assert!(machine.started());
        assert!(machine.stop());
        assert!(machine.request_start());
        machine.fail();
        assert_eq!(machine.ended_runs(), 3);
        // A failure after the Stop belongs to the same run.
        assert!(machine.request_start());
        assert!(machine.started());
        assert!(machine.stop());
        machine.fail();
        assert_eq!(machine.ended_runs(), 4);
    }

    #[test]
    fn close_is_terminal() {
        let mut machine = StateMachine::default();
        assert!(machine.request_start());
        machine.close();
        assert_eq!(machine.state(), RunState::Closing);
        assert!(!machine.request_start());
        assert!(!machine.stop());
        // A late failure while closing must not reopen the machine.
        machine.fail();
        assert_eq!(machine.state(), RunState::Closing);
        assert!(!machine.request_start());
    }
}
