use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use nix::fcntl::{flock, FlockArg};
use nix::sys::signal::{SigSet, Signal};
use nix::sys::signalfd::{SfdFlags, SignalFd};

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct ShutdownSignal {
    fd: SignalFd,
    previous_mask: SigSet,
}

impl ShutdownSignal {
    pub fn new() -> Result<Self> {
        let previous_mask = SigSet::thread_get_mask().map_err(|source| Error::Signal { source })?;
        let mut mask = SigSet::empty();
        mask.add(Signal::SIGTERM);
        mask.add(Signal::SIGINT);
        mask.thread_block()
            .map_err(|source| Error::Signal { source })?;
        match SignalFd::with_flags(&mask, SfdFlags::SFD_NONBLOCK | SfdFlags::SFD_CLOEXEC) {
            Ok(fd) => Ok(Self { fd, previous_mask }),
            Err(source) => {
                let _ = previous_mask.thread_set_mask();
                Err(Error::Signal { source })
            }
        }
    }

    pub fn fd(&self) -> &SignalFd {
        &self.fd
    }

    pub fn read_requested(&mut self) -> std::result::Result<bool, nix::errno::Errno> {
        Ok(self.fd.read_signal()?.is_some_and(|signal| {
            signal.ssi_signo == Signal::SIGTERM as u32 || signal.ssi_signo == Signal::SIGINT as u32
        }))
    }
}

impl Drop for ShutdownSignal {
    fn drop(&mut self) {
        let _ = self.previous_mask.thread_set_mask();
    }
}

pub struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    pub fn acquire(device: &Path) -> Result<Self> {
        let runtime_dir = runtime_directory()?;
        let metadata = fs::metadata(device).map_err(|source| Error::DeviceIdentity {
            path: device.to_path_buf(),
            source,
        })?;
        let key = format!("device-{:x}", metadata.rdev());
        Self::acquire_at(&runtime_dir, device, &key)
    }

    fn acquire_at(runtime_dir: &Path, device: &Path, key: &str) -> Result<Self> {
        let path = runtime_dir.join(format!("letsnote-wheelpad-{key}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&path)
            .map_err(|source| Error::InstanceLockOpen {
                path: path.clone(),
                source,
            })?;

        match flock(file.as_raw_fd(), FlockArg::LockExclusiveNonblock) {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(nix::errno::Errno::EWOULDBLOCK) => Err(Error::InstanceAlreadyRunning {
                device: device.to_path_buf(),
                lock_path: path,
            }),
            Err(source) => Err(Error::InstanceLock { path, source }),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn runtime_directory() -> Result<PathBuf> {
    let value =
        std::env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| Error::RuntimeDirUnavailable {
            reason: "XDG_RUNTIME_DIR is not set".to_string(),
        })?;
    let configured = PathBuf::from(value);
    if !configured.is_absolute() {
        return Err(Error::RuntimeDirUnavailable {
            reason: format!("{} is not an absolute path", configured.display()),
        });
    }
    let path = fs::canonicalize(&configured).map_err(|source| Error::RuntimeDirUnavailable {
        reason: format!("{}: {source}", configured.display()),
    })?;
    let metadata = fs::metadata(&path).map_err(|source| Error::RuntimeDirUnavailable {
        reason: format!("{}: {source}", path.display()),
    })?;
    if !metadata.is_dir() {
        return Err(Error::RuntimeDirUnavailable {
            reason: format!("{} is not a directory", path.display()),
        });
    }

    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    let effective_uid = unsafe { libc::geteuid() };
    if metadata.uid() != effective_uid {
        return Err(Error::RuntimeDirUnavailable {
            reason: format!("{} is not owned by the current user", path.display()),
        });
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(Error::RuntimeDirUnavailable {
            reason: format!("{} is writable by another user or group", path.display()),
        });
    }
    Ok(path)
}

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

    #[error("shutdown signal read failed: {source}")]
    SignalReadFailed {
        #[source]
        source: nix::errno::Errno,
    },
}

impl LoopExit {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::RequestedShutdown)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use nix::poll::{poll, PollFd, PollFlags};

    use super::*;

    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TestTempDir {
        path: PathBuf,
    }

    impl TestTempDir {
        fn new() -> Self {
            loop {
                let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "letsnote-wheelpad-test-{}-{sequence}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Self { path },
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("failed to create test temp directory: {error}"),
                }
            }
        }
    }

    impl Drop for TestTempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

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

    #[test]
    fn advisory_lock_rejects_a_live_owner_but_allows_stale_files() {
        let runtime_dir = TestTempDir::new();
        let device = Path::new("/dev/input/event-test");

        let first = InstanceLock::acquire_at(&runtime_dir.path, device, "test").unwrap();
        assert!(matches!(
            InstanceLock::acquire_at(&runtime_dir.path, device, "test"),
            Err(Error::InstanceAlreadyRunning { .. })
        ));
        drop(first);

        let second = InstanceLock::acquire_at(&runtime_dir.path, device, "test").unwrap();
        drop(second);
    }

    #[test]
    fn shutdown_signal_restores_the_calling_threads_mask() {
        let before = SigSet::thread_get_mask().unwrap();
        {
            let signal = ShutdownSignal::new().unwrap();
            let blocked = SigSet::thread_get_mask().unwrap();
            assert!(blocked.contains(Signal::SIGTERM));
            assert!(blocked.contains(Signal::SIGINT));
            assert!(signal.fd().as_raw_fd() >= 0);
        }
        let after = SigSet::thread_get_mask().unwrap();
        assert_eq!(
            before.contains(Signal::SIGTERM),
            after.contains(Signal::SIGTERM)
        );
        assert_eq!(
            before.contains(Signal::SIGINT),
            after.contains(Signal::SIGINT)
        );
    }

    #[test]
    fn shutdown_signal_wakes_poll_without_input_activity() {
        let mut signal = ShutdownSignal::new().unwrap();

        // SAFETY: pthread_self returns the current valid pthread identifier,
        // and pthread_kill only queues SIGTERM to that thread. ShutdownSignal
        // has blocked SIGTERM first, so it is consumed through signalfd rather
        // than invoking the process-default termination action.
        let result = unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGTERM) };
        assert_eq!(result, 0);

        let mut fds = [PollFd::new(signal.fd(), PollFlags::POLLIN)];
        assert_eq!(poll(&mut fds, 1_000).unwrap(), 1);
        assert!(signal.read_requested().unwrap());
    }
}
