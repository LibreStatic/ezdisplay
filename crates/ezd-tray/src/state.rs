//! Start/stop state machine of the service, free of any I/O so it can be tested on its own.

/// Where the service is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lifecycle {
    /// No service exists.
    #[default]
    Stopped,
    /// A service exists (it listens, possibly streaming).
    Running,
    /// The service is being stopped on the runtime. The old one still holds the listen port, so a
    /// Start requested meanwhile is queued (`restart`) and runs once the old one has finished.
    Stopping { restart: bool },
}

/// What the caller has to do after a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    None,
    /// Create and start a service.
    StartService,
    /// Stop the current service (asynchronously) and report back with [`Lifecycle::stopped`].
    StopService,
}

impl Lifecycle {
    /// The user asked to start.
    pub fn request_start(&mut self) -> Step {
        match *self {
            Lifecycle::Stopped => {
                *self = Lifecycle::Running;
                Step::StartService
            }
            Lifecycle::Running => Step::None,
            Lifecycle::Stopping { .. } => {
                *self = Lifecycle::Stopping { restart: true };
                Step::None
            }
        }
    }

    /// The user asked to stop. A queued start is cancelled.
    pub fn request_stop(&mut self) -> Step {
        match *self {
            Lifecycle::Stopped => Step::None,
            Lifecycle::Running => {
                *self = Lifecycle::Stopping { restart: false };
                Step::StopService
            }
            Lifecycle::Stopping { .. } => {
                *self = Lifecycle::Stopping { restart: false };
                Step::None
            }
        }
    }

    /// The old service finished stopping.
    pub fn stopped(&mut self) -> Step {
        match *self {
            Lifecycle::Stopping { restart: true } => {
                *self = Lifecycle::Running;
                Step::StartService
            }
            _ => {
                *self = Lifecycle::Stopped;
                Step::None
            }
        }
    }

    /// Starting failed (for example the port is taken): there is no service.
    pub fn start_failed(&mut self) {
        *self = Lifecycle::Stopped;
    }

    /// A service is running or will run once the old one is gone: the toggle offers "Stop".
    pub fn wants_running(&self) -> bool {
        matches!(
            self,
            Lifecycle::Running | Lifecycle::Stopping { restart: true }
        )
    }

    /// A service exists and has not been asked to stop.
    #[cfg(test)]
    pub fn is_running(&self) -> bool {
        *self == Lifecycle::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_then_stop() {
        let mut l = Lifecycle::Stopped;
        assert_eq!(l.request_start(), Step::StartService);
        assert!(l.is_running() && l.wants_running());
        assert_eq!(l.request_start(), Step::None);
        assert_eq!(l.request_stop(), Step::StopService);
        assert_eq!(l, Lifecycle::Stopping { restart: false });
        assert!(!l.wants_running());
        assert_eq!(l.stopped(), Step::None);
        assert_eq!(l, Lifecycle::Stopped);
    }

    #[test]
    fn start_while_stopping_is_queued_until_the_old_service_finished() {
        let mut l = Lifecycle::Running;
        assert_eq!(l.request_stop(), Step::StopService);
        // The old service still holds the port: nothing starts yet.
        assert_eq!(l.request_start(), Step::None);
        assert!(l.wants_running() && !l.is_running());
        // Asking twice changes nothing.
        assert_eq!(l.request_start(), Step::None);
        assert_eq!(l.stopped(), Step::StartService);
        assert_eq!(l, Lifecycle::Running);
    }

    #[test]
    fn stop_cancels_a_queued_start() {
        let mut l = Lifecycle::Running;
        l.request_stop();
        l.request_start();
        assert_eq!(l.request_stop(), Step::None);
        assert_eq!(l.stopped(), Step::None);
        assert_eq!(l, Lifecycle::Stopped);
    }

    #[test]
    fn stop_when_stopped_does_nothing() {
        let mut l = Lifecycle::Stopped;
        assert_eq!(l.request_stop(), Step::None);
        assert_eq!(l, Lifecycle::Stopped);
    }

    #[test]
    fn failed_start_leaves_no_service() {
        let mut l = Lifecycle::Stopped;
        l.request_start();
        l.start_failed();
        assert_eq!(l, Lifecycle::Stopped);
        assert_eq!(l.request_start(), Step::StartService);
    }
}
