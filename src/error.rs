use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("config file {path:?}: {source}")]
    ConfigIo { path: PathBuf, source: io::Error },

    #[error("config file {path:?}: parse error: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("config value out of range: {key} = {value}; expected {expected}")]
    ConfigRange {
        key: &'static str,
        value: i64,
        expected: &'static str,
    },

    #[error("config value out of range: {key} = {value}; expected {expected}")]
    ConfigFloatRange {
        key: &'static str,
        value: f64,
        expected: &'static str,
    },

    #[error("could not find a touchpad matching `{regex}`. Set `device = \"/dev/input/eventN\"` in the config to override.")]
    DeviceNotFound { regex: String },

    #[error("multiple physical touchpads match `{regex}`; specify `--device` explicitly. Candidates:\n{candidates}")]
    DeviceAmbiguous { regex: String, candidates: String },

    #[error("refusing virtual input source {path:?} ({name})")]
    VirtualInputSource { path: PathBuf, name: String },

    #[error("failed to open evdev device {path:?}: {source}")]
    EvdevOpen { path: PathBuf, source: io::Error },

    #[error("evdev device {path:?} is missing required capability: {capability}")]
    EvdevMissingCap {
        path: PathBuf,
        capability: &'static str,
    },

    #[error(
        "evdev device {path:?} has unsupported ABS_MT_SLOT range [{minimum}, {maximum}]: {expected}"
    )]
    EvdevSlotRange {
        path: PathBuf,
        minimum: i32,
        maximum: i32,
        expected: &'static str,
    },

    #[error("evdev device {path:?} has invalid Type B state: {reason}")]
    EvdevState { path: PathBuf, reason: String },

    #[error("evdev read error: {source}")]
    EvdevRead { source: io::Error },

    #[error("/dev/uinput is not available; load the kernel module with `sudo modprobe uinput`")]
    UinputMissing,

    #[error("failed to create uinput device: {source}")]
    UinputCreate { source: io::Error },

    #[error("failed to write uinput event: {source}")]
    UinputWrite { source: io::Error },

    #[error("EVIOCGRAB ioctl failed: {source}")]
    Grab { source: io::Error },

    #[error("EVIOCGRAB release failed during startup retry: {source}")]
    Ungrab { source: io::Error },

    #[error("input proxy terminated: {source}")]
    Runtime {
        #[source]
        source: crate::runtime::LoopExit,
    },

    #[error("invalid device-name regex `{pattern}`: {source}")]
    RegexInvalid {
        pattern: String,
        source: regex::Error,
    },

    #[error("signal handling setup failed: {source}")]
    Signal { source: nix::errno::Errno },

    #[error("runtime directory is unavailable for the instance lock: {reason}")]
    RuntimeDirUnavailable { reason: String },

    #[error("failed to identify physical device {path:?} for locking: {source}")]
    DeviceIdentity { path: PathBuf, source: io::Error },

    #[error("failed to open instance lock {path:?}: {source}")]
    InstanceLockOpen { path: PathBuf, source: io::Error },

    #[error("another letsnote-wheelpad instance already owns {device:?} (lock {lock_path:?})")]
    InstanceAlreadyRunning { device: PathBuf, lock_path: PathBuf },

    #[error("failed to lock {path:?}: {source}")]
    InstanceLock {
        path: PathBuf,
        source: nix::errno::Errno,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
