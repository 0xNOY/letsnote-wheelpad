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

    #[error("failed to open evdev device {path:?}: {source}")]
    EvdevOpen { path: PathBuf, source: io::Error },

    #[error("evdev device {path:?} is missing required capability: {capability}")]
    EvdevMissingCap {
        path: PathBuf,
        capability: &'static str,
    },

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

    #[error("invalid device-name regex `{pattern}`: {source}")]
    RegexInvalid {
        pattern: String,
        source: regex::Error,
    },

    #[error("signal handling setup failed: {source}")]
    Signal { source: nix::errno::Errno },
}

pub type Result<T> = std::result::Result<T, Error>;
