use std::fs;
use std::path::PathBuf;

fn repository_file(path: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path);
    fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read repository asset {}: {error}",
            path.display()
        )
    })
}

fn udev_rules(contents: &str) -> Vec<String> {
    let mut rules = Vec::new();
    let mut current = String::new();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let continued = line.ends_with('\\');
        let fragment = line.trim_end_matches('\\').trim();
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(fragment);

        if !continued {
            rules.push(std::mem::take(&mut current));
        }
    }

    assert!(current.is_empty(), "udev asset has an unfinished rule");
    rules
}

#[test]
fn sysusers_defines_a_dedicated_unprivileged_identity() {
    let contents = repository_file("packaging/sysusers/letsnote-wheelpad.conf");
    let entries: Vec<_> = contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    assert_eq!(entries.len(), 1, "expected one sysusers entry");
    let leading_fields: Vec<_> = entries[0].split_whitespace().take(3).collect();
    assert_eq!(leading_fields, ["u", "letsnote-wheelpad", "-"]);
    let trailing_fields: Vec<_> = entries[0].split_whitespace().rev().take(2).collect();
    assert_eq!(
        trailing_fields,
        ["-", "/"],
        "sysusers must use / and the default shell"
    );
    assert!(!contents.contains("input"));
    assert!(
        !leading_fields[2]
            .chars()
            .any(|character| character.is_ascii_digit()),
        "UID/GID must not be fixed numerically"
    );
}

#[test]
fn udev_rules_preserve_device_isolation() {
    let contents = repository_file("packaging/udev/72-letsnote-wheelpad-system.rules");
    let rules = udev_rules(&contents);

    assert!(contents.contains(r#"GROUP="letsnote-wheelpad""#));
    assert!(contents.contains(r#"TAG-="uaccess""#));
    assert!(!contents.contains(r#"TAG+="uaccess""#));
    assert!(!contents.contains(r#"GROUP="input""#));
    assert!(!contents.contains("LIBINPUT_IGNORE_DEVICE"));
    assert!(!contents.contains("RUN+="));
    assert!(!contents.contains("setfacl"));
    assert!(!contents.contains(r#"MODE="0666""#));

    let uinput = rules
        .iter()
        .find(|rule| rule.contains(r#"SUBSYSTEM=="misc""#) && rule.contains(r#"KERNEL=="uinput""#))
        .expect("missing uinput rule");
    assert!(uinput.contains(r#"ACTION=="add|change""#));
    assert!(uinput.contains(r#"MODE="0660""#));
    assert!(uinput.contains(r#"GROUP="letsnote-wheelpad""#));
    assert!(uinput.contains(r#"OPTIONS+="static_node=uinput""#));

    for product in ["7470", "7770"] {
        let virtual_rule = rules
            .iter()
            .find(|rule| {
                rule.contains(r#"ATTRS{id/bustype}=="0006""#)
                    && rule.contains(r#"ATTRS{id/vendor}=="6c6e""#)
                    && rule.contains(&format!(r#"ATTRS{{id/product}}=="{product}""#))
            })
            .unwrap_or_else(|| panic!("missing virtual device rule for product {product}"));
        assert!(virtual_rule.contains(r#"TAG-="uaccess""#));
        assert!(!virtual_rule.contains("SYSTEMD_WANTS"));
        assert!(!virtual_rule.contains(r#"TAG+="systemd""#));
        assert!(!virtual_rule.contains("MODE="));
        assert!(!virtual_rule.contains("GROUP="));
    }

    let physical_rules: Vec<_> = rules
        .iter()
        .filter(|rule| rule.contains("SYSTEMD_WANTS"))
        .collect();
    assert_eq!(physical_rules.len(), 2);
    for rule in physical_rules {
        assert!(rule.contains(r#"SUBSYSTEM=="input""#));
        assert!(rule.contains(r#"KERNEL=="event*""#));
        assert!(rule.contains(r#"ENV{ID_INPUT_TOUCHPAD}=="1""#));
        assert!(rule.contains(r#"MODE="0660""#));
        assert!(rule.contains(r#"GROUP="letsnote-wheelpad""#));
        assert!(rule.contains(r#"TAG-="uaccess""#));
        assert!(rule.contains(r#"TAG+="systemd""#));
        assert!(rule.contains(r#"ENV{SYSTEMD_WANTS}+="letsnote-wheelpad@%k.service""#));
        assert!(
            rule.contains(r#"ATTRS{name}=="*TM3562*""#)
                || (rule.contains(r#"ATTRS{id/bustype}=="0011""#)
                    && rule.contains(r#"ATTRS{id/vendor}=="0002""#)
                    && rule.contains(r#"ATTRS{id/product}=="0007""#)
                    && rule.contains(r#"ATTRS{name}=="SynPS/2 Synaptics TouchPad""#)),
            "physical rule must use a supported narrow identity"
        );
    }

    for rule in &rules {
        if rule.contains(r#"ENV{ID_INPUT_TOUCHPAD}=="1""#) {
            assert!(
                rule.contains("TM3562")
                    || (rule.contains(r#"ATTRS{id/bustype}=="0011""#)
                        && rule.contains(r#"ATTRS{id/vendor}=="0002""#)
                        && rule.contains(r#"ATTRS{id/product}=="0007""#)),
                "touchpad rule is broader than a supported physical identity"
            );
        }
    }
}

#[test]
fn system_service_allows_only_the_instance_and_uinput() {
    let contents = repository_file("packaging/systemd/letsnote-wheelpad@.service");
    let timeout_start_directives: Vec<_> = contents
        .lines()
        .filter(|line| line.starts_with("TimeoutStartSec="))
        .collect();

    assert!(contents
        .lines()
        .any(|line| line == "User=letsnote-wheelpad"));
    assert!(contents
        .lines()
        .any(|line| line == "Group=letsnote-wheelpad"));
    assert!(contents.lines().any(|line| line == "DevicePolicy=closed"));
    assert!(contents
        .lines()
        .any(|line| line == "DeviceAllow=/dev/input/%I rw"));
    assert!(contents
        .lines()
        .any(|line| line == "DeviceAllow=/dev/uinput rw"));
    assert_eq!(timeout_start_directives, ["TimeoutStartSec=infinity"]);
    assert!(!contents.lines().any(|line| line == "TimeoutStartSec=10s"));
    assert!(contents.lines().any(|line| line == "Type=notify"));
    assert!(contents.lines().any(|line| line == "Restart=on-failure"));
    assert!(!contents.contains("DeviceAllow=/dev/input/*"));
    assert!(!contents.lines().any(|line| line.trim() == "[Install]"));
    assert!(!contents.contains("DynamicUser"));
    assert!(!contents.contains("SupplementaryGroups=input"));
    assert!(!contents.contains("LIBINPUT_IGNORE_DEVICE"));

    assert!(contents.contains("ConditionPathExists=/etc/letsnote-wheelpad/system-service-enabled"));
    assert!(contents.contains("BindsTo=dev-input-%i.device"));
    assert!(contents.contains("RuntimeDirectory=letsnote-wheelpad/%I"));
    assert!(contents.contains("--config /etc/letsnote-wheelpad/config.toml"));
}

#[test]
fn package_config_matches_compiled_defaults() {
    use letsnote_wheelpad::config::{Config, ConfigRequest, ConfigSource};

    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packaging/config/letsnote-wheelpad.toml");
    let loaded = Config::load(ConfigRequest::Explicit(path.clone()))
        .expect("package config must parse and validate");
    assert_eq!(loaded.source, ConfigSource::File(path));

    let actual = loaded.config;
    let expected = Config::default();
    assert_eq!(actual.device, expected.device);
    assert_eq!(actual.device_name_regex, expected.device_name_regex);
    assert_eq!(actual.scroll.enable, expected.scroll.enable);
    assert_eq!(
        actual.scroll.reverse_vertical,
        expected.scroll.reverse_vertical
    );
    assert_eq!(
        actual.scroll.horizontal_enable,
        expected.scroll.horizontal_enable
    );
    assert_eq!(
        actual.scroll.reverse_horizontal,
        expected.scroll.reverse_horizontal
    );
    assert_eq!(actual.scroll.sensitivity, expected.scroll.sensitivity);
    assert_eq!(
        actual.scroll.detect_area_width,
        expected.scroll.detect_area_width
    );
    assert_eq!(
        actual.scroll.detect_area_radius,
        expected.scroll.detect_area_radius
    );
    assert_eq!(
        actual.scroll.coordinate_y_scale,
        expected.scroll.coordinate_y_scale
    );
    assert_eq!(
        actual.scroll.minimum_rotation_radius,
        expected.scroll.minimum_rotation_radius
    );
    assert_eq!(
        actual.scroll.horizontal_start,
        expected.scroll.horizontal_start
    );
    assert_eq!(actual.scroll.horizontal_end, expected.scroll.horizontal_end);
    assert_eq!(actual.log.level, expected.log.level);
}

#[test]
fn inert_system_assets_are_packaged_for_debian_and_rpm() {
    let cargo_toml = repository_file("Cargo.toml");

    assert_eq!(
        cargo_toml
            .matches("packaging/sysusers/letsnote-wheelpad.conf")
            .count(),
        2
    );
    assert_eq!(
        cargo_toml
            .matches("packaging/systemd/letsnote-wheelpad@.service")
            .count(),
        2
    );
    assert_eq!(
        cargo_toml
            .matches("packaging/config/letsnote-wheelpad.toml")
            .count(),
        2
    );
    assert!(!cargo_toml.contains("packaging/udev/72-letsnote-wheelpad-system.rules"));

    assert_eq!(
        cargo_toml
            .matches("packaging/udev/70-letsnote-wheelpad.rules")
            .count(),
        2,
        "Debian and RPM must still package the current udev rule"
    );
    assert_eq!(
        cargo_toml
            .matches("packaging/systemd/letsnote-wheelpad.service")
            .count(),
        2,
        "Debian and RPM must still package the current user service"
    );

    for forbidden in [
        "system-service-enabled",
        "migration-staging",
        "migrate-system-service",
        "letsnote-wheelpad-migrate",
        "setfacl",
    ] {
        assert!(
            !cargo_toml.contains(forbidden),
            "package metadata introduced migration or activation token: {forbidden}"
        );
    }

    assert_eq!(cargo_toml.matches("udevadm trigger || true").count(), 1);
    assert_eq!(
        cargo_toml
            .matches("systemctl daemon-reload || true")
            .count(),
        1
    );
}
