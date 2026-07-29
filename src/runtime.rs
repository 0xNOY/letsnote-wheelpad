use std::io;

#[derive(Debug, thiserror::Error)]
pub enum LoopExit {
    #[error("shutdown requested")]
    RequestedShutdown,

    #[error("physical input device disconnected: {source}")]
    InputDisconnected {
        #[source]
        source: io::Error,
    },

    #[error("physical input read failed: {source}")]
    InputReadFailed {
        #[source]
        source: io::Error,
    },

    #[error("input poll failed: {source}")]
    PollFailed {
        #[source]
        source: nix::errno::Errno,
    },

    #[error("virtual touchpad output failed: {source}")]
    VirtualTouchpadFailed {
        #[source]
        source: io::Error,
    },

    #[error("virtual wheel output failed: {source}")]
    VirtualWheelFailed {
        #[source]
        source: io::Error,
    },
}

impl LoopExit {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::RequestedShutdown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_requested_shutdown_is_successful() {
        assert!(LoopExit::RequestedShutdown.is_success());
        assert!(!LoopExit::PollFailed {
            source: nix::errno::Errno::EIO
        }
        .is_success());
        assert!(!LoopExit::VirtualTouchpadFailed {
            source: io::Error::other("write failed")
        }
        .is_success());
        assert!(!LoopExit::VirtualWheelFailed {
            source: io::Error::other("write failed")
        }
        .is_success());
    }
}
