use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use nix::fcntl::{flock, FlockArg};

use crate::error::{Error, Result};

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

    #[test]
    fn advisory_lock_rejects_a_live_owner_but_allows_stale_files() {
        let runtime_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts/refactor-input-proxy/lock-test");
        fs::create_dir_all(&runtime_dir).unwrap();
        let device = Path::new("/dev/input/event-test");

        let first = InstanceLock::acquire_at(&runtime_dir, device, "test").unwrap();
        assert!(matches!(
            InstanceLock::acquire_at(&runtime_dir, device, "test"),
            Err(Error::InstanceAlreadyRunning { .. })
        ));
        drop(first);

        let second = InstanceLock::acquire_at(&runtime_dir, device, "test").unwrap();
        drop(second);
        fs::remove_file(runtime_dir.join("letsnote-wheelpad-test.lock")).unwrap();
        fs::remove_dir(&runtime_dir).unwrap();
        let parent = runtime_dir.parent().unwrap();
        if parent.read_dir().unwrap().next().is_none() {
            fs::remove_dir(parent).unwrap();
        }
    }
}
