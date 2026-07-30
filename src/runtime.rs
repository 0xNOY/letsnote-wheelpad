use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
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
    let runtime_directory = std::env::var_os("RUNTIME_DIRECTORY");
    let xdg_runtime_dir = std::env::var_os("XDG_RUNTIME_DIR");
    resolve_runtime_directory(
        runtime_directory.as_deref(),
        xdg_runtime_dir.as_deref(),
        effective_uid(),
    )
}

fn resolve_runtime_directory(
    runtime_directory: Option<&OsStr>,
    xdg_runtime_dir: Option<&OsStr>,
    effective_uid: u32,
) -> Result<PathBuf> {
    if let Some(value) = runtime_directory {
        return validate_runtime_directory("RUNTIME_DIRECTORY", value, effective_uid);
    }

    let value = xdg_runtime_dir.ok_or_else(|| Error::RuntimeDirUnavailable {
        reason: "RUNTIME_DIRECTORY and XDG_RUNTIME_DIR are not set".to_string(),
    })?;
    validate_runtime_directory("XDG_RUNTIME_DIR", value, effective_uid)
}

fn validate_runtime_directory(
    source_variable: &'static str,
    value: &OsStr,
    effective_uid: u32,
) -> Result<PathBuf> {
    if value.is_empty() {
        return Err(runtime_directory_error(
            source_variable,
            "value is empty".to_string(),
        ));
    }
    if source_variable == "RUNTIME_DIRECTORY" && value.as_bytes().contains(&b':') {
        return Err(runtime_directory_error(
            source_variable,
            "multiple colon-separated paths are not supported".to_string(),
        ));
    }

    let configured = PathBuf::from(value);
    if !configured.is_absolute() {
        return Err(runtime_directory_error(
            source_variable,
            format!("{} is not an absolute path", configured.display()),
        ));
    }
    let path = fs::canonicalize(&configured).map_err(|source| {
        runtime_directory_error(
            source_variable,
            format!("cannot canonicalize {}: {source}", configured.display()),
        )
    })?;
    let metadata = fs::metadata(&path).map_err(|source| {
        runtime_directory_error(
            source_variable,
            format!("cannot inspect {}: {source}", path.display()),
        )
    })?;
    validate_runtime_directory_metadata(
        source_variable,
        &path,
        metadata.is_dir(),
        metadata.uid(),
        metadata.mode(),
        effective_uid,
    )?;
    Ok(path)
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

fn validate_runtime_directory_metadata(
    source_variable: &'static str,
    path: &Path,
    is_directory: bool,
    owner_uid: u32,
    mode: u32,
    effective_uid: u32,
) -> Result<()> {
    if !is_directory {
        return Err(runtime_directory_error(
            source_variable,
            format!("{} is not a directory", path.display()),
        ));
    }
    if owner_uid != effective_uid {
        return Err(runtime_directory_error(
            source_variable,
            format!(
                "{} is owned by UID {owner_uid}, not effective UID {effective_uid}",
                path.display()
            ),
        ));
    }
    if mode & 0o022 != 0 {
        return Err(runtime_directory_error(
            source_variable,
            format!("{} is group- or world-writable", path.display()),
        ));
    }
    Ok(())
}

fn runtime_directory_error(source_variable: &'static str, reason: String) -> Error {
    Error::RuntimeDirUnavailable {
        reason: format!("{source_variable}: {reason}"),
    }
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
    use std::ffi::OsStr;
    use std::os::unix::fs::{symlink, PermissionsExt};
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

    fn resolve(
        runtime_directory: Option<&OsStr>,
        xdg_runtime_dir: Option<&OsStr>,
    ) -> Result<PathBuf> {
        resolve_runtime_directory(runtime_directory, xdg_runtime_dir, effective_uid())
    }

    fn error_message(error: Error) -> String {
        error.to_string()
    }

    #[test]
    fn valid_runtime_directory_is_used() {
        let temp = TestTempDir::new();

        assert_eq!(
            resolve(Some(temp.path.as_os_str()), None).unwrap(),
            fs::canonicalize(&temp.path).unwrap()
        );
    }

    #[test]
    fn runtime_directory_takes_priority_over_xdg_runtime_dir() {
        let runtime = TestTempDir::new();
        let xdg = TestTempDir::new();

        assert_eq!(
            resolve(Some(runtime.path.as_os_str()), Some(xdg.path.as_os_str())).unwrap(),
            fs::canonicalize(&runtime.path).unwrap()
        );
    }

    #[test]
    fn xdg_runtime_dir_is_used_when_runtime_directory_is_unset() {
        let xdg = TestTempDir::new();

        assert_eq!(
            resolve(None, Some(xdg.path.as_os_str())).unwrap(),
            fs::canonicalize(&xdg.path).unwrap()
        );
    }

    #[test]
    fn runtime_directory_rejects_relative_paths() {
        let message = error_message(resolve(Some(OsStr::new("relative")), None).unwrap_err());

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("not an absolute path"));
    }

    #[test]
    fn runtime_directory_rejects_colon_separated_paths() {
        let message =
            error_message(resolve(Some(OsStr::new("/run/first:/run/second")), None).unwrap_err());

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("colon-separated"));
    }

    #[test]
    fn runtime_directory_rejects_wrong_owner_metadata() {
        let error = validate_runtime_directory_metadata(
            "RUNTIME_DIRECTORY",
            Path::new("/run/example"),
            true,
            1001,
            0o40700,
            1000,
        )
        .unwrap_err();
        let message = error_message(error);

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("UID 1001"));
        assert!(message.contains("effective UID 1000"));
    }

    #[test]
    fn runtime_directory_rejects_group_or_world_writable_directory() {
        let temp = TestTempDir::new();
        fs::set_permissions(&temp.path, fs::Permissions::from_mode(0o722)).unwrap();

        let message = error_message(resolve(Some(temp.path.as_os_str()), None).unwrap_err());

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("group- or world-writable"));
    }

    #[test]
    fn runtime_directory_rejects_missing_directory() {
        let temp = TestTempDir::new();
        let missing = temp.path.join("missing");

        let message = error_message(resolve(Some(missing.as_os_str()), None).unwrap_err());

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("cannot canonicalize"));
    }

    #[test]
    fn runtime_directory_rejects_non_directory() {
        let temp = TestTempDir::new();
        let file = temp.path.join("file");
        File::create(&file).unwrap();

        let message = error_message(resolve(Some(file.as_os_str()), None).unwrap_err());

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("not a directory"));
    }

    #[test]
    fn runtime_directory_rejects_empty_value() {
        let message = error_message(resolve(Some(OsStr::new("")), None).unwrap_err());

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("empty"));
    }

    #[test]
    fn invalid_runtime_directory_does_not_fall_back_to_xdg() {
        let xdg = TestTempDir::new();

        let message = error_message(
            resolve(Some(OsStr::new("invalid")), Some(xdg.path.as_os_str())).unwrap_err(),
        );

        assert!(message.contains("RUNTIME_DIRECTORY"));
        assert!(message.contains("invalid is not an absolute path"));
    }

    #[test]
    fn runtime_directory_uses_canonicalized_symlink_target() {
        let target = TestTempDir::new();
        let links = TestTempDir::new();
        let link = links.path.join("runtime");
        symlink(&target.path, &link).unwrap();

        assert_eq!(
            resolve(Some(link.as_os_str()), None).unwrap(),
            fs::canonicalize(&target.path).unwrap()
        );
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

    #[test]
    fn pending_shutdown_remains_readable_during_nonblocking_startup_work() {
        let mut signal = ShutdownSignal::new().unwrap();

        // SAFETY: as in the poll test above, SIGINT is directed to this
        // thread after ShutdownSignal has blocked it for signalfd.
        let result = unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGINT) };
        assert_eq!(result, 0);

        let finite_startup_work = (0..100).sum::<usize>();
        assert_eq!(finite_startup_work, 4_950);
        assert!(signal.read_requested().unwrap());
    }
}
