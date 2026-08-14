#!/usr/bin/env python3
"""Verify the Arch binary package without installing it."""

import argparse
import hashlib
import io
import struct
import subprocess
import tarfile
from pathlib import Path


REQUIRED_FILES = {
    "etc/letsnote-wheelpad/config.toml",
    "usr/bin/letsnote-wheelpad",
    "usr/lib/modules-load.d/letsnote-wheelpad.conf",
    "usr/lib/systemd/system/letsnote-wheelpad@.service",
    "usr/lib/systemd/user/letsnote-wheelpad.service",
    "usr/lib/sysusers.d/letsnote-wheelpad.conf",
    "usr/lib/udev/rules.d/70-letsnote-wheelpad.rules",
    "usr/lib/udev/rules.d/72-letsnote-wheelpad-system.rules",
    "usr/libexec/letsnote-wheelpad-bin-guard",
    "usr/libexec/letsnote-wheelpad-migrate",
    "usr/share/libalpm/hooks/30-letsnote-wheelpad-bin-remove.hook",
    "usr/share/licenses/letsnote-wheelpad-bin/LICENSE",
}


def read_ar(path: Path) -> dict[str, bytes]:
    data = path.read_bytes()
    assert data.startswith(b"!<arch>\n"), "not an ar archive"
    members: dict[str, bytes] = {}
    offset = 8
    while offset < len(data):
        header = data[offset : offset + 60]
        assert len(header) == 60 and header[58:60] == b"`\n", "invalid ar member"
        name = header[:16].decode("ascii").strip().removesuffix("/")
        size = int(header[48:58].decode("ascii").strip())
        offset += 60
        members[name] = data[offset : offset + size]
        offset += size + size % 2
    return members


def archive_bytes(path: Path, member: str) -> bytes:
    return subprocess.run(
        ["bsdtar", "-xOf", str(path), member],
        check=True,
        stdout=subprocess.PIPE,
    ).stdout


def archive_names(path: Path) -> set[str]:
    output = subprocess.run(
        ["bsdtar", "-tf", str(path)],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout
    return {line.removeprefix("./").rstrip("/") for line in output.splitlines()}


def deb_binary(path: Path) -> bytes:
    archive = read_ar(path)
    data_names = [name for name in archive if name.startswith("data.tar.")]
    assert len(data_names) == 1, f"expected one Debian data payload: {data_names}"
    with tarfile.open(fileobj=io.BytesIO(archive[data_names[0]]), mode="r:*") as data:
        member = data.getmember("./usr/bin/letsnote-wheelpad")
        extracted = data.extractfile(member)
        assert extracted is not None
        return extracted.read()


def metadata_values(metadata: str, key: str) -> list[str]:
    prefix = f"{key} = "
    return [line[len(prefix) :] for line in metadata.splitlines() if line.startswith(prefix)]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--arch", required=True, type=Path)
    parser.add_argument("--deb", required=True, type=Path)
    args = parser.parse_args()

    names = archive_names(args.arch)
    missing = REQUIRED_FILES - names
    assert not missing, f"missing Arch package files: {sorted(missing)}"
    for forbidden in ("postinst", "prerm", "postrm", "control"):
        assert forbidden not in names, f"Debian control member packaged: {forbidden}"

    metadata = archive_bytes(args.arch, ".PKGINFO").decode()
    assert metadata_values(metadata, "pkgname") == ["letsnote-wheelpad-bin"]
    assert metadata_values(metadata, "pkgver") == ["0.2.0-1"]
    assert metadata_values(metadata, "arch") == ["x86_64"]
    assert "letsnote-wheelpad=0.2.0" in metadata_values(metadata, "provides")
    conflicts = set(metadata_values(metadata, "conflict"))
    assert {"letsnote-wheelpad", "letsnote-wheelpad-git"} <= conflicts
    assert not metadata_values(metadata, "replaces")
    assert "etc/letsnote-wheelpad/config.toml" in metadata_values(metadata, "backup")
    dependencies = set(metadata_values(metadata, "depend"))
    assert {"glibc", "libgcc", "bash", "systemd", "acl", "util-linux"} <= dependencies
    assert "gcc-libs" not in dependencies
    assert not {"rust", "cargo", "git"} & dependencies

    root = Path(__file__).resolve().parent.parent
    packaged_install = archive_bytes(args.arch, ".INSTALL").decode().strip()
    repository_install = (
        root / "packaging/aur/letsnote-wheelpad-bin.install"
    ).read_text().strip()
    assert packaged_install == repository_install, "Arch .INSTALL differs from source"

    hook = archive_bytes(
        args.arch, "usr/share/libalpm/hooks/30-letsnote-wheelpad-bin-remove.hook"
    ).decode()
    assert "AbortOnFail" in hook
    assert "Target = letsnote-wheelpad-bin" in hook
    guard = archive_bytes(args.arch, "usr/libexec/letsnote-wheelpad-bin-guard").decode()
    assert '"$daemon"|"$daemon (deleted)"' in guard

    helper_listing = subprocess.run(
        ["bsdtar", "-tvf", str(args.arch), "usr/libexec/letsnote-wheelpad-migrate"],
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    ).stdout
    assert helper_listing.startswith("-rwxr-xr-x"), helper_listing

    packaged_binary = archive_bytes(args.arch, "usr/bin/letsnote-wheelpad")
    source_binary = deb_binary(args.deb)
    assert packaged_binary == source_binary, "Arch and Debian binaries differ"

    lifecycle = packaged_install + "\n" + guard
    for forbidden in (
        "systemctl --global enable",
        "systemctl --global reenable",
        "systemctl start",
        "systemctl enable",
        "setfacl",
        "userdel",
        "groupdel",
    ):
        assert forbidden not in lifecycle, f"automatic lifecycle mutation: {forbidden}"

    print(
        f"verified Arch artifact: {args.arch} "
        f"sha256={hashlib.sha256(args.arch.read_bytes()).hexdigest()}"
    )


if __name__ == "__main__":
    main()
