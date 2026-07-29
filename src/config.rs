use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

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
    /// Load from a TOML file. Missing file → defaults (warned by caller).
    pub fn load(path: &Path) -> Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(Error::ConfigIo {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let cfg: Config = toml::from_str(&text).map_err(|source| Error::ConfigParse {
            path: path.to_path_buf(),
            source,
        })?;
        cfg.validate()?;
        Ok(cfg)
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
    use super::*;

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
}
