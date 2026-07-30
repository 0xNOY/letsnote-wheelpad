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
fn new_system_assets_are_not_yet_packaged() {
    let cargo_toml = repository_file("Cargo.toml");

    assert!(!cargo_toml.contains("packaging/sysusers/letsnote-wheelpad.conf"));
    assert!(!cargo_toml.contains("packaging/udev/72-letsnote-wheelpad-system.rules"));
    assert!(!cargo_toml.contains("packaging/systemd/letsnote-wheelpad@.service"));

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
}
