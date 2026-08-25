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
fn system_udev_assignments_are_behind_the_marker_gate() {
    let contents = repository_file("packaging/udev/72-letsnote-wheelpad-system.rules");
    let rules = udev_rules(&contents);
    let runtime_gate = r#"TEST=="/run/letsnote-wheelpad/migration-staging", GOTO="letsnote_wheelpad_system_authorized""#;
    let persistent_gate = r#"TEST=="/etc/letsnote-wheelpad/system-service-enabled", GOTO="letsnote_wheelpad_system_authorized""#;
    let reject = r#"GOTO="letsnote_wheelpad_system_end""#;
    let authorized = r#"LABEL="letsnote_wheelpad_system_authorized""#;
    let end = r#"LABEL="letsnote_wheelpad_system_end""#;

    assert_eq!(&rules[..3], [runtime_gate, persistent_gate, reject]);
    let authorized_index = rules.iter().position(|rule| rule == authorized).unwrap();
    let end_index = rules.iter().position(|rule| rule == end).unwrap();
    assert!(authorized_index < end_index);

    let mutating_tokens = [
        "MODE=",
        "OWNER=",
        "GROUP=",
        "TAG+=",
        "TAG-=",
        "SYSTEMD_WANTS",
        "OPTIONS+=",
        "RUN+=",
        "setfacl",
    ];
    for (index, rule) in rules.iter().enumerate() {
        if mutating_tokens.iter().any(|token| rule.contains(token)) {
            assert!(
                index > authorized_index && index < end_index,
                "mutating rule is outside the authorized gate: {rule}"
            );
        }
    }
    assert_eq!(
        rules
            .iter()
            .filter(|rule| mutating_tokens.iter().any(|token| rule.contains(token)))
            .count(),
        5
    );
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

    assert!(contents.contains("ConditionPathExists=|/etc/letsnote-wheelpad/system-service-enabled"));
    assert!(contents.contains("ConditionPathExists=|/run/letsnote-wheelpad/migration-staging"));
    assert!(contents.contains("BindsTo=dev-input-%i.device"));
    assert!(contents.contains("RuntimeDirectory=letsnote-wheelpad/%I"));
    assert!(contents.contains("--config /etc/letsnote-wheelpad/config.toml"));
}

#[test]
fn legacy_user_service_is_blocked_by_persistent_or_runtime_migration() {
    let contents = repository_file("packaging/systemd/letsnote-wheelpad.service");
    assert!(contents.contains("ConditionPathExists=!/etc/letsnote-wheelpad/system-service-enabled"));
    assert!(contents
        .contains("ConditionPathExists=!/run/letsnote-wheelpad/migration-block-user-service"));
    assert!(contents.contains("ExecStart=/usr/bin/letsnote-wheelpad"));
    assert!(contents.contains("WantedBy=graphical-session.target"));
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
fn migration_assets_are_packaged_for_debian_and_rpm() {
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
    assert_eq!(
        cargo_toml
            .matches("packaging/udev/72-letsnote-wheelpad-system.rules")
            .count(),
        2
    );
    assert_eq!(
        cargo_toml
            .matches("packaging/migrate/letsnote-wheelpad-migrate")
            .count(),
        2
    );
    assert_eq!(
        cargo_toml
            .matches("usr/libexec/letsnote-wheelpad-migrate")
            .count(),
        1
    );

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

    assert!(cargo_toml.contains("depends = \"$auto, acl, systemd, udev\""));
    assert!(cargo_toml.contains("post_install_script = \"packaging/rpm/post.sh\""));
    assert!(cargo_toml.contains("pre_uninstall_script = \"packaging/rpm/preun.sh\""));
    assert!(cargo_toml.contains("post_uninstall_script = \"packaging/rpm/postun.sh\""));
}

#[test]
fn package_scripts_verify_identity_before_reloading_udev() {
    let debian = repository_file("packaging/deb/postinst");
    let rpm_post = repository_file("packaging/rpm/post.sh");

    for script in [&debian, &rpm_post] {
        let sysusers = script
            .find("systemd-sysusers /usr/lib/sysusers.d/letsnote-wheelpad.conf")
            .unwrap();
        let passwd_verification = script[sysusers..]
            .find("getent passwd letsnote-wheelpad")
            .map(|offset| offset + sysusers)
            .unwrap();
        let group_verification = script[sysusers..]
            .find("getent group letsnote-wheelpad")
            .map(|offset| offset + sysusers)
            .unwrap();
        let refresh = script
            .rfind("targeted_udev_refresh")
            .expect("missing targeted refresh invocation");
        assert!(sysusers < passwd_verification);
        assert!(sysusers < group_verification);
        assert!(passwd_verification < refresh);
        assert!(group_verification < refresh);
        assert!(!script.contains("userdel"));
        assert!(!script.contains("groupdel"));
    }

    for script in [&debian[..], &rpm_post] {
        assert!(!script.contains("system-service-enabled"));
        assert!(!script.contains("migration-staging"));
        assert!(!script.contains("migration-block-user-service"));
        assert!(!script.contains("systemctl start"));
    }
}

#[test]
fn migration_helper_has_narrow_mutation_surface() {
    let helper = repository_file("packaging/migrate/letsnote-wheelpad-migrate");

    assert!(helper.starts_with("#!/bin/sh\nset -eu\n"));
    for forbidden in ["eval ", "kill ", "killall", "pkill", "setfacl", "rm -rf"] {
        assert!(!helper.contains(forbidden), "helper contains {forbidden}");
    }
    assert!(!helper
        .lines()
        .any(|line| line.trim_start().starts_with("systemctl --user")));
    assert_eq!(
        helper
            .matches("--subsystem-match=input --sysname-match=\"$selected_event\"")
            .count(),
        2
    );
    assert_eq!(
        helper
            .matches("--subsystem-match=misc --sysname-match=uinput")
            .count(),
        2
    );
    assert!(!helper.contains("udevadm trigger /dev/input"));
    assert!(!helper.contains("for node in /dev/input/"));
    assert!(!helper.contains("setfacl"));

    let unique_check = helper
        .find(r#"[ "$candidate_count" -eq 1 ]"#)
        .expect("missing unique-candidate check");
    let staging_create = helper
        .find(r#": >"$staging_marker""#)
        .expect("missing staging marker creation");
    let explicit_start = helper
        .find(r#"systemctl start "$selected_unit""#)
        .expect("missing explicit system instance start");
    let ready_pid_check = helper
        .find(r#"main_pid=$(systemctl show --property=MainPID --value "$selected_unit")"#)
        .expect("missing MainPID check");
    let marker_creation = helper
        .find("marker_tmp=$(mktemp /etc/letsnote-wheelpad/.system-service-enabled.XXXXXX)")
        .expect("missing atomic persistent marker creation");
    assert!(unique_check < staging_create);
    assert!(staging_create < explicit_start);
    assert!(explicit_start < ready_pid_check);
    assert!(ready_pid_check < marker_creation);
}

#[test]
fn migration_helper_auto_selects_only_one_supported_device() {
    let helper = repository_file("packaging/migrate/letsnote-wheelpad-migrate");

    assert!(helper.contains("enable [--device /dev/input/eventN]"));
    assert!(helper.contains("disable [--device /dev/input/eventN]"));
    assert!(helper.contains("supplied=${1-}"));
    assert!(helper.contains(
        "if [ -z \"$supplied\" ]; then\n        scan_candidates\n        [ \"$candidate_count\" -ne 0 ]"
    ));
    assert!(helper.contains(
        "[ \"$candidate_count\" -eq 1 ] ||\n            fail \"found $candidate_count supported physical touchpads; exactly one is required\""
    ));
    assert!(helper.contains("printf 'auto-selected device: %s\\n' \"$supplied\""));
    assert!(helper.contains("1) device_argument= ;;"));
    assert!(helper.contains("[ \"$2\" = --device ] || usage"));
    assert!(helper.contains("[ -n \"$3\" ] || usage"));
    assert!(helper.contains("enable_system_service \"$device_argument\""));
    assert!(helper.contains("disable_system_service \"$device_argument\""));
}

#[test]
fn migration_helper_matches_only_package_daemon_executables() {
    let helper = repository_file("packaging/migrate/letsnote-wheelpad-migrate");
    let running_daemons = helper
        .split_once("running_daemons()\n{")
        .expect("missing running_daemons")
        .1
        .split_once("\n}\n\nprint_node_state()")
        .expect("unterminated running_daemons")
        .0;

    assert!(running_daemons.contains(r#"[ -L "$process_exe" ] || continue"#));
    assert!(running_daemons.contains(r#""$daemon"|"$daemon (deleted)")"#));
    assert_eq!(running_daemons.matches(r#""$daemon""#).count(), 1);
    assert_eq!(running_daemons.matches(r#""$daemon (deleted)""#).count(), 1);
    assert!(!running_daemons.contains("${resolved##*/}"));
    assert!(!running_daemons.contains("cmdline"));
    assert!(!running_daemons.contains(r#""*/letsnote-wheelpad"#));
}

#[test]
fn migration_helper_treats_getfacl_failure_as_unknown_or_fatal() {
    let helper = repository_file("packaging/migrate/letsnote-wheelpad-migrate");
    let named_acl_entries = helper
        .split_once("named_acl_entries()\n{")
        .expect("missing named_acl_entries")
        .1
        .split_once("\n}\n\nprint_acl_state()")
        .expect("unterminated named_acl_entries")
        .0;

    let capture = named_acl_entries
        .find(r#"acl_output=$(getfacl -cp "$acl_node" 2>/dev/null) || return 1"#)
        .expect("getfacl failure must be preserved");
    let extraction = named_acl_entries
        .find(r#"printf '%s\n' "$acl_output" |"#)
        .expect("ACL extraction must use captured output");
    assert!(capture < extraction);
    assert!(!named_acl_entries.contains(r#"getfacl -cp "$acl_node" 2>/dev/null |"#));
    assert_eq!(
        helper
            .matches(r#"if ! acl_entries=$(named_acl_entries "$acl_node"); then"#)
            .count(),
        2
    );
    assert!(helper.contains("named user ACLs: unknown (getfacl failed)"));
    assert!(helper.contains("getfacl failed for $acl_node; staging will be rolled back"));
}

#[test]
fn migration_helper_preserves_preexisting_interruption_state() {
    let helper = repository_file("packaging/migrate/letsnote-wheelpad-migrate");
    let cleanup_failure = helper
        .split_once("cleanup_failure()\n{")
        .expect("missing cleanup_failure")
        .1
        .split_once("\n}\n\nenable_system_service()")
        .expect("unterminated cleanup_failure")
        .0;
    let enable = helper
        .split_once("enable_system_service()\n{")
        .expect("missing enable_system_service")
        .1
        .split_once("\n}\n\ndisable_system_service()")
        .expect("unterminated enable_system_service")
        .0;

    let lock = enable.find("    acquire_lock").unwrap();
    let device = enable.find(r#"    select_device "$1""#).unwrap();
    let config = enable.find("    verify_config").unwrap();
    let identity = enable.find("    verify_identity").unwrap();
    let persistent = enable.find("    persistent_present=0").unwrap();
    let staging = enable.find("    staging_present=0").unwrap();
    let user_block = enable.find("    user_block_present=0").unwrap();
    let active = enable.find("    system_active=0").unwrap();
    let inconsistent = enable
        .find("interrupted migration state detected; recover with: letsnote-wheelpad-migrate disable --device $selected_path")
        .unwrap();
    let cleanup = enable.find("    cleanup_mode=enable").unwrap();
    assert!(lock < device);
    assert!(device < config);
    assert!(config < identity);
    assert!(identity < persistent);
    assert!(persistent < staging);
    assert!(staging < user_block);
    assert!(user_block < active);
    assert!(active < inconsistent);
    assert!(inconsistent < cleanup);

    assert!(helper.contains(
        "if [ \"$user_block_created\" -eq 1 ]; then\n            rm -f \"$user_block_marker\""
    ));
    assert!(helper.contains(
        "if [ \"$staging_created\" -eq 1 ]; then\n            rm -f \"$staging_marker\""
    ));
    assert!(!cleanup_failure.contains(r#"rm -f "$staging_marker" "$user_block_marker""#));
    assert!(!cleanup_failure.contains(r#"elif [ -n "$cleanup_mode" ]; then"#));
    assert!(helper.contains(r#"[ -e "$user_block_marker" ] || : >"$user_block_marker""#));
}
