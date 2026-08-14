use std::fs;
use std::path::PathBuf;

fn repository_file(path: &str) -> String {
    fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(path)).unwrap()
}

fn lifecycle_scripts() -> Vec<(&'static str, String)> {
    [
        "packaging/deb/postinst",
        "packaging/deb/prerm",
        "packaging/deb/postrm",
        "packaging/rpm/post.sh",
        "packaging/rpm/preun.sh",
        "packaging/rpm/postun.sh",
        "packaging/arch/letsnote-wheelpad.install",
        "packaging/aur/letsnote-wheelpad-bin.install",
        "packaging/aur/letsnote-wheelpad-bin-guard",
    ]
    .into_iter()
    .map(|path| (path, repository_file(path)))
    .collect()
}

#[test]
fn lifecycle_device_refreshes_are_exact_and_supported_only() {
    for (path, script) in lifecycle_scripts() {
        if !script.contains("udevadm trigger") {
            continue;
        }
        assert!(
            script.contains("for event_path in /sys/class/input/event*"),
            "{path}"
        );
        assert!(script.contains("ID_INPUT_TOUCHPAD=1"), "{path}");
        assert!(script.contains("*TM3562*"), "{path}");
        assert!(script.contains("SynPS/2 Synaptics TouchPad"), "{path}");
        assert!(script.contains("0011:0002:0007"), "{path}");
        assert!(script.contains("0006:6c6e:7470|0006:6c6e:7770"), "{path}");
        assert!(
            script.contains("--sysname-match=\"$event_sysname\""),
            "{path}"
        );
        assert!(script.contains("--sysname-match=uinput"), "{path}");

        assert_eq!(
            script.matches("udevadm trigger").count(),
            script.matches("--sysname-match=").count(),
            "non-exact trigger in {path}"
        );

        for line in script
            .lines()
            .filter(|line| line.contains("udevadm trigger"))
        {
            assert!(
                line.ends_with("--subsystem-match=input \\")
                    || line.ends_with("--subsystem-match=misc \\"),
                "unexpected trigger form in {path}: {line}"
            );
        }
        assert!(!script.contains("--sysname-match='event*'"), "{path}");
        assert!(!script.contains("/dev/input/*"), "{path}");
    }
}

#[test]
fn distro_action_matrices_are_narrow() {
    let postinst = repository_file("packaging/deb/postinst");
    let prerm = repository_file("packaging/deb/prerm");
    let postrm = repository_file("packaging/deb/postrm");
    assert!(postinst.contains("case \"${1:-}\" in\n    configure)"));
    assert!(prerm.contains("    remove)"));
    assert!(prerm.contains("    upgrade)"));
    assert!(prerm.contains("dpkg-query -W -f='${Version}'"));
    assert!(prerm.contains("dpkg --compare-versions"));
    assert!(postrm.contains("case \"${1:-}\" in\n    remove)"));

    let rpm_post = repository_file("packaging/rpm/post.sh");
    let rpm_preun = repository_file("packaging/rpm/preun.sh");
    let rpm_postun = repository_file("packaging/rpm/postun.sh");
    assert!(!rpm_post.contains("$1"));
    assert!(rpm_preun.contains("[ \"${1:-1}\" -eq 0 ]"));
    assert!(rpm_postun.contains("[ \"${1:-1}\" -eq 0 ]"));

    let arch = repository_file("packaging/aur/letsnote-wheelpad-bin.install");
    assert!(arch.contains("post_install()"));
    assert!(arch.contains("post_upgrade()"));
    assert!(arch.contains("post_remove()"));
    assert!(!arch.contains("pre_remove()"));
    assert!(!arch.contains("pre_upgrade()"));
}

#[test]
fn lifecycle_never_enables_starts_kills_or_mutates_accounts_or_acls() {
    for (path, script) in lifecycle_scripts() {
        for forbidden in [
            "systemctl --global enable",
            "systemctl --global reenable",
            "systemctl start",
            "systemctl enable",
            "kill ",
            "killall",
            "pkill",
            "setfacl",
            "userdel",
            "groupdel",
            "rm -rf",
        ] {
            assert!(!script.contains(forbidden), "{path} contains {forbidden}");
        }
    }
}

#[test]
fn final_removal_guards_markers_and_exact_executables() {
    for path in [
        "packaging/deb/prerm",
        "packaging/rpm/preun.sh",
        "packaging/aur/letsnote-wheelpad-bin-guard",
    ] {
        let script = repository_file(path);
        for marker in [
            "/etc/letsnote-wheelpad/system-service-enabled",
            "/run/letsnote-wheelpad/migration-staging",
            "/run/letsnote-wheelpad/migration-block-user-service",
        ] {
            assert!(script.contains(marker), "{path} omits {marker}");
        }
        assert!(
            script.contains(r#""$daemon"|"$daemon (deleted)""#),
            "{path}"
        );
        assert!(script.contains("/proc/[0-9]*/exe"), "{path}");
        assert!(!script.contains("cmdline"), "{path}");
    }
}

#[test]
fn upgrades_preserve_user_service_state_and_do_not_create_markers() {
    let debian = repository_file("packaging/deb/prerm");
    let arch = repository_file("packaging/aur/letsnote-wheelpad-bin.install");
    assert!(!debian
        .split_once("upgrade)")
        .unwrap()
        .1
        .contains("--global disable"));
    assert!(!arch.contains(" disable "));

    for (path, script) in lifecycle_scripts() {
        for creation in [
            ": >\"$persistent_marker\"",
            ": >\"$staging_marker\"",
            ": >\"$user_block_marker\"",
            "touch /etc/letsnote-wheelpad/system-service-enabled",
        ] {
            assert!(!script.contains(creation), "{path} creates migration state");
        }
    }
}

#[test]
fn arch_binary_template_has_the_release_boundary() {
    let pkgbuild = repository_file("packaging/aur/PKGBUILD.in");
    assert!(pkgbuild.contains("pkgname=letsnote-wheelpad-bin"));
    assert!(pkgbuild.contains("pkgver=@PKGVER@"));
    assert!(pkgbuild.contains("'@DEB_SHA256@'"));
    assert!(pkgbuild.contains("releases/download/v${pkgver}/${_deb_file}"));
    assert!(pkgbuild.contains("noextract=(\"${_deb_file}\")"));
    assert!(pkgbuild.contains("provides=(\"letsnote-wheelpad=${pkgver}\")"));
    assert!(pkgbuild.contains("conflicts=('letsnote-wheelpad' 'letsnote-wheelpad-git')"));
    assert!(!pkgbuild.contains("replaces="));
    assert!(!pkgbuild.contains("'SKIP'"));
    assert!(!pkgbuild.contains("cargo build"));
    assert!(!pkgbuild.contains("makedepends=('cargo"));

    let hook = repository_file("packaging/aur/30-letsnote-wheelpad-bin-remove.hook");
    assert!(hook.contains("Operation = Remove"));
    assert!(hook.contains("Target = letsnote-wheelpad-bin"));
    assert!(hook.contains("AbortOnFail"));
}

#[test]
fn package_version_and_legacy_assets_remain() {
    let cargo = repository_file("Cargo.toml");
    assert!(cargo.contains("version = \"0.2.0\""));
    for asset in [
        "packaging/udev/70-letsnote-wheelpad.rules",
        "packaging/udev/72-letsnote-wheelpad-system.rules",
        "packaging/systemd/letsnote-wheelpad.service",
    ] {
        assert!(cargo.contains(asset));
        assert!(repository_file("packaging/aur/PKGBUILD.in")
            .contains(asset.rsplit_once('/').unwrap().1));
    }
}
