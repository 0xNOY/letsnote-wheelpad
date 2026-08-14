use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigRequest {
    Explicit(PathBuf),
    ImplicitDefault(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    File(PathBuf),
    CompiledDefaults { missing_path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub source: ConfigSource,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub device: Option<PathBuf>,
    pub device_name_regex: String,
    pub scroll: Scroll,
    pub log: Log,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scroll {
    pub enable: bool,
    pub reverse_vertical: bool,
    pub horizontal_enable: bool,
    pub reverse_horizontal: bool,
    pub sensitivity: i32,
    pub detect_area_width: i32,
    pub detect_area_radius: f64,
    pub coordinate_y_scale: f64,
    pub minimum_rotation_radius: f64,
    pub horizontal_start: i32,
    pub horizontal_end: i32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Log {
    pub level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: None,
            device_name_regex: "Synaptics.*TM3562".to_string(),
            scroll: Scroll::default(),
            log: Log::default(),
        }
    }
}

impl Default for Scroll {
    fn default() -> Self {
        // Defaults verbatim from WheelPad.exe — see RE-findings.md §3 and
        // DECISIONS.md D-008..D-010. Sensitivity index 0 selects the
        // middle entry (multiplier 20) of [10, 14, 20, 28, 40].
        Self {
            enable: true,
            reverse_vertical: false,
            horizontal_enable: false,
            reverse_horizontal: false,
            sensitivity: 0,
            detect_area_width: 0,
            detect_area_radius: 200.0,
            coordinate_y_scale: 1.0,
            minimum_rotation_radius: 250.0,
            horizontal_start: 2,
            horizontal_end: 6,
        }
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
        }
    }
}

impl Config {
    pub fn load(request: ConfigRequest) -> Result<LoadedConfig> {
        let path = match &request {
            ConfigRequest::Explicit(path) | ConfigRequest::ImplicitDefault(path) => path,
        };
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => match request {
                ConfigRequest::ImplicitDefault(missing_path) => {
                    return Ok(LoadedConfig {
                        config: Self::default(),
                        source: ConfigSource::CompiledDefaults { missing_path },
                    });
                }
                ConfigRequest::Explicit(path) => {
                    return Err(Error::ConfigIo { path, source });
                }
            },
            Err(source) => {
                let path = match request {
                    ConfigRequest::Explicit(path) | ConfigRequest::ImplicitDefault(path) => path,
                };
                return Err(Error::ConfigIo { path, source });
            }
        };
        let cfg: Config = toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
        cfg.validate()?;
        let path = match request {
            ConfigRequest::Explicit(path) | ConfigRequest::ImplicitDefault(path) => path,
        };
        Ok(LoadedConfig {
            config: cfg,
            source: ConfigSource::File(path),
        })
    }

    pub fn validate(&self) -> Result<()> {
        let s = &self.scroll;
        if !(-2..=2).contains(&s.sensitivity) {
            return Err(Error::ConfigRange {
                key: "scroll.sensitivity",
                value: s.sensitivity as i64,
                expected: "-2..=2",
            });
        }
        if !(0..=10).contains(&s.detect_area_width) {
            return Err(Error::ConfigRange {
                key: "scroll.detect_area_width",
                value: s.detect_area_width as i64,
                expected: "0..=10",
            });
        }
        if !s.detect_area_radius.is_finite() || s.detect_area_radius <= 0.0 {
            return Err(Error::ConfigFloatRange {
                key: "scroll.detect_area_radius",
                value: s.detect_area_radius,
                expected: "a finite value greater than 0",
            });
        }
        if !s.coordinate_y_scale.is_finite() || s.coordinate_y_scale <= 0.0 {
            return Err(Error::ConfigFloatRange {
                key: "scroll.coordinate_y_scale",
                value: s.coordinate_y_scale,
                expected: "a finite value greater than 0",
            });
        }
        if !s.minimum_rotation_radius.is_finite() || s.minimum_rotation_radius < 0.0 {
            return Err(Error::ConfigFloatRange {
                key: "scroll.minimum_rotation_radius",
                value: s.minimum_rotation_radius,
                expected: "a finite value greater than or equal to 0",
            });
        }
        if !(0..=15).contains(&s.horizontal_start) {
            return Err(Error::ConfigRange {
                key: "scroll.horizontal_start",
                value: s.horizontal_start as i64,
                expected: "0..=15",
            });
        }
        if !(0..=15).contains(&s.horizontal_end) {
            return Err(Error::ConfigRange {
                key: "scroll.horizontal_end",
                value: s.horizontal_end as i64,
                expected: "0..=15",
            });
        }
        Ok(())
    }

    /// Default path: `$XDG_CONFIG_HOME/letsnote-wheelpad/config.toml` falling
    /// back to `$HOME/.config/letsnote-wheelpad/config.toml`.
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            if !xdg.is_empty() {
                return PathBuf::from(xdg)
                    .join("letsnote-wheelpad")
                    .join("config.toml");
            }
        }
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("letsnote-wheelpad")
            .join("config.toml")
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicU64, Ordering};

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
                    "letsnote-wheelpad-config-test-{}-{sequence}",
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
    fn defaults_match_windows() {
        let c = Config::default();
        assert!(c.scroll.enable);
        assert!(!c.scroll.reverse_vertical);
        assert!(!c.scroll.horizontal_enable);
        assert_eq!(c.scroll.sensitivity, 0);
        assert_eq!(c.scroll.detect_area_width, 0);
        assert_eq!(c.scroll.detect_area_radius, 200.0);
        assert_eq!(c.scroll.coordinate_y_scale, 1.0);
        assert_eq!(c.scroll.minimum_rotation_radius, 250.0);
        assert_eq!(c.scroll.horizontal_start, 2);
        assert_eq!(c.scroll.horizontal_end, 6);
    }

    #[test]
    fn parses_partial_config() {
        let toml = r#"
            [scroll]
            sensitivity = -1
            horizontal_enable = true
        "#;
        let c: Config = toml::from_str(toml).unwrap();
        assert_eq!(c.scroll.sensitivity, -1);
        assert!(c.scroll.horizontal_enable);
        // unspecified keys keep defaults
        assert_eq!(c.scroll.horizontal_start, 2);
        assert_eq!(c.scroll.detect_area_radius, 200.0);
        assert_eq!(c.scroll.coordinate_y_scale, 1.0);
        assert_eq!(c.scroll.minimum_rotation_radius, 250.0);
    }

    #[test]
    fn validate_rejects_out_of_range_sensitivity() {
        let mut c = Config::default();
        c.scroll.sensitivity = 5;
        assert!(c.validate().is_err());
    }

    #[test]
    fn validate_rejects_invalid_radial_gate_geometry() {
        let mut c = Config::default();
        c.scroll.detect_area_radius = 0.0;
        assert!(c.validate().is_err());

        c.scroll.detect_area_radius = 200.0;
        c.scroll.coordinate_y_scale = f64::NAN;
        assert!(c.validate().is_err());

        c.scroll.coordinate_y_scale = 1.0;
        c.scroll.minimum_rotation_radius = -1.0;
        assert!(c.validate().is_err());
    }

    #[test]
    fn explicit_existing_file_is_loaded() {
        let temp = TestTempDir::new();
        let path = temp.path.join("explicit.toml");
        fs::write(&path, "[scroll]\nsensitivity = -1\n").unwrap();

        let loaded = Config::load(ConfigRequest::Explicit(path.clone())).unwrap();

        assert_eq!(loaded.config.scroll.sensitivity, -1);
        assert_eq!(loaded.source, ConfigSource::File(path));
    }

    #[test]
    fn explicit_missing_file_is_fatal_and_returns_no_defaults() {
        let temp = TestTempDir::new();
        let path = temp.path.join("missing.toml");

        let error = Config::load(ConfigRequest::Explicit(path.clone())).unwrap_err();

        match error {
            Error::ConfigIo {
                path: error_path,
                source,
            } => {
                assert_eq!(error_path, path);
                assert_eq!(source.kind(), io::ErrorKind::NotFound);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn implicit_existing_file_is_loaded() {
        let temp = TestTempDir::new();
        let path = temp.path.join("implicit.toml");
        fs::write(&path, "[scroll]\nhorizontal_enable = true\n").unwrap();

        let loaded = Config::load(ConfigRequest::ImplicitDefault(path.clone())).unwrap();

        assert!(loaded.config.scroll.horizontal_enable);
        assert_eq!(loaded.source, ConfigSource::File(path));
    }

    #[test]
    fn implicit_missing_file_uses_compiled_defaults() {
        let temp = TestTempDir::new();
        let path = temp.path.join("missing.toml");

        let loaded = Config::load(ConfigRequest::ImplicitDefault(path)).unwrap();

        assert_eq!(
            loaded.config.scroll.sensitivity,
            Config::default().scroll.sensitivity
        );
        assert_eq!(
            loaded.config.device_name_regex,
            Config::default().device_name_regex
        );
    }

    #[test]
    fn implicit_missing_file_reports_compiled_default_source() {
        let temp = TestTempDir::new();
        let path = temp.path.join("missing.toml");

        let loaded = Config::load(ConfigRequest::ImplicitDefault(path.clone())).unwrap();

        assert_eq!(
            loaded.source,
            ConfigSource::CompiledDefaults { missing_path: path }
        );
    }

    #[test]
    fn config_parse_error_is_fatal() {
        let temp = TestTempDir::new();
        let path = temp.path.join("invalid.toml");
        fs::write(&path, "[scroll\n").unwrap();

        let error = Config::load(ConfigRequest::ImplicitDefault(path.clone())).unwrap_err();

        assert!(matches!(
            error,
            Error::ConfigParse {
                path: error_path,
                ..
            } if error_path == path
        ));
    }

    #[test]
    fn config_range_error_is_fatal() {
        let temp = TestTempDir::new();
        let path = temp.path.join("out-of-range.toml");
        fs::write(&path, "[scroll]\nsensitivity = 3\n").unwrap();

        let error = Config::load(ConfigRequest::Explicit(path)).unwrap_err();

        assert!(matches!(
            error,
            Error::ConfigRange {
                key: "scroll.sensitivity",
                ..
            }
        ));
    }
}
