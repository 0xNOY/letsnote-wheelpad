#!/usr/bin/env python3
"""Verify the Phase 2A-3b package payload without installing it."""

import argparse
import hashlib
import io
import struct
import tarfile
from pathlib import Path


EXPECTED_FILES = {
    "/etc/letsnote-wheelpad/config.toml",
    "/usr/lib/modules-load.d/letsnote-wheelpad.conf",
    "/usr/lib/systemd/system/letsnote-wheelpad@.service",
    "/usr/lib/systemd/user/letsnote-wheelpad.service",
    "/usr/lib/sysusers.d/letsnote-wheelpad.conf",
    "/usr/lib/udev/rules.d/70-letsnote-wheelpad.rules",
    "/usr/lib/udev/rules.d/72-letsnote-wheelpad-system.rules",
    "/usr/libexec/letsnote-wheelpad-migrate",
    "/usr/bin/letsnote-wheelpad",
}


def normalize_path(name: str) -> str:
    return "/" + name.removeprefix("./").lstrip("/")


def require_payload(files: set[str]) -> None:
    missing = EXPECTED_FILES - files
    assert not missing, f"missing package files: {sorted(missing)}"


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


def read_tar_members(data: bytes) -> dict[str, tuple[bytes, int]]:
    members: dict[str, tuple[bytes, int]] = {}
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
        for member in archive.getmembers():
            if member.isfile():
                extracted = archive.extractfile(member)
                assert extracted is not None
                members[normalize_path(member.name)] = (extracted.read(), member.mode)
    return members


def verify_deb(path: Path) -> None:
    archive = read_ar(path)
    control_name = next(name for name in archive if name.startswith("control.tar"))
    data_name = next(name for name in archive if name.startswith("data.tar"))
    control = read_tar_members(archive[control_name])
    payload = read_tar_members(archive[data_name])

    metadata = control["/control"][0].decode()
    version = next(line for line in metadata.splitlines() if line.startswith("Version: "))
    assert version.split(maxsplit=1)[1].split("-", 1)[0] == "0.2.0", version
    dependencies = next(
        line for line in metadata.splitlines() if line.startswith("Depends: ")
    )
    dependency_names = {item.strip().split()[0] for item in dependencies[9:].split(",")}
    assert "acl" in dependency_names
    require_payload(set(payload))

    conffiles = control["/conffiles"][0].decode().splitlines()
    assert "/etc/letsnote-wheelpad/config.toml" in conffiles, conffiles

    root = Path(__file__).resolve().parent.parent
    for name in ("postinst", "prerm", "postrm"):
        packaged = control[f"/{name}"][0].decode().strip()
        repository = (root / "packaging" / "deb" / name).read_text().strip()
        assert packaged == repository, f"Debian {name} changed in the artifact"

    assert payload["/usr/libexec/letsnote-wheelpad-migrate"][1] == 0o755
    for packaged_path, repository_path in {
        "/usr/lib/udev/rules.d/70-letsnote-wheelpad.rules": "packaging/udev/70-letsnote-wheelpad.rules",
        "/usr/lib/udev/rules.d/72-letsnote-wheelpad-system.rules": "packaging/udev/72-letsnote-wheelpad-system.rules",
        "/usr/lib/systemd/user/letsnote-wheelpad.service": "packaging/systemd/letsnote-wheelpad.service",
        "/usr/lib/systemd/system/letsnote-wheelpad@.service": "packaging/systemd/letsnote-wheelpad@.service",
        "/usr/libexec/letsnote-wheelpad-migrate": "packaging/migrate/letsnote-wheelpad-migrate",
    }.items():
        assert payload[packaged_path][0] == (root / repository_path).read_bytes()

    print(f"verified Debian artifact: {path}")


def read_rpm_header(data: bytes, offset: int) -> tuple[dict[int, object], int]:
    assert data[offset : offset + 4] == b"\x8e\xad\xe8\x01", "invalid RPM header"
    count, store_size = struct.unpack_from(">II", data, offset + 8)
    indexes = offset + 16
    store_offset = indexes + count * 16
    store = data[store_offset : store_offset + store_size]
    values: dict[int, object] = {}

    for index in range(count):
        tag, value_type, value_offset, value_count = struct.unpack_from(
            ">IIII", data, indexes + index * 16
        )
        if value_type == 3:
            values[tag] = list(
                struct.unpack_from(f">{value_count}H", store, value_offset)
            )
        elif value_type == 4:
            values[tag] = list(
                struct.unpack_from(f">{value_count}I", store, value_offset)
            )
        elif value_type == 5:
            values[tag] = list(
                struct.unpack_from(f">{value_count}Q", store, value_offset)
            )
        elif value_type == 6:
            end = store.index(0, value_offset)
            values[tag] = store[value_offset:end].decode()
        elif value_type in (8, 9):
            strings = []
            cursor = value_offset
            for _ in range(value_count):
                end = store.index(0, cursor)
                strings.append(store[cursor:end].decode())
                cursor = end + 1
            values[tag] = strings

    return values, store_offset + store_size


def rpm_files(header: dict[int, object]) -> tuple[list[str], list[int], list[int]]:
    basenames = header[1117]
    dir_indexes = header[1116]
    dirnames = header[1118]
    flags = header[1037]
    modes = header[1030]
    assert isinstance(basenames, list)
    assert isinstance(dir_indexes, list)
    assert isinstance(dirnames, list)
    assert isinstance(flags, list)
    assert isinstance(modes, list)
    files = [normalize_path(dirnames[index] + name) for name, index in zip(basenames, dir_indexes)]
    return files, flags, modes


def verify_rpm(path: Path) -> None:
    data = path.read_bytes()
    assert data[:4] == b"\xed\xab\xee\xdb", "not an RPM archive"
    _, signature_end = read_rpm_header(data, 96)
    header_offset = signature_end + (-signature_end % 8)
    header, _ = read_rpm_header(data, header_offset)

    assert header[1001] == "0.2.0", f"unexpected RPM version: {header[1001]}"
    files, flags, modes = rpm_files(header)
    require_payload(set(files))

    config_path = "/etc/letsnote-wheelpad/config.toml"
    config_flags = flags[files.index(config_path)]
    assert config_flags & 1, "RPM system config is not marked config"
    assert config_flags & 16, "RPM system config is not marked noreplace"
    helper_path = "/usr/libexec/letsnote-wheelpad-migrate"
    assert modes[files.index(helper_path)] & 0o7777 == 0o755

    root = Path(__file__).resolve().parent.parent
    assert str(header[1024]).strip() == (
        root / "packaging/rpm/post.sh"
    ).read_text().strip(), "RPM %post changed"
    assert str(header[1025]).strip() == (
        root / "packaging/rpm/preun.sh"
    ).read_text().strip(), "RPM %preun changed"
    assert str(header[1026]).strip() == (
        root / "packaging/rpm/postun.sh"
    ).read_text().strip(), "RPM %postun changed"
    assert 1023 not in header, "unexpected RPM %pre"

    requires = header[1049]
    assert isinstance(requires, list)
    assert {"acl", "systemd", "systemd-udev"} <= set(requires)

    for required in (
        "/usr/lib/udev/rules.d/70-letsnote-wheelpad.rules",
        "/usr/lib/udev/rules.d/72-letsnote-wheelpad-system.rules",
        "/usr/lib/systemd/user/letsnote-wheelpad.service",
        "/usr/lib/systemd/system/letsnote-wheelpad@.service",
        helper_path,
    ):
        assert required in files

    digests = header[1035]
    digest_algorithm = header.get(5011, [1])[0]
    algorithms = {1: "md5", 8: "sha256"}
    assert digest_algorithm in algorithms, f"unsupported RPM digest: {digest_algorithm}"
    assert isinstance(digests, list)
    for packaged_path, repository_path in {
        "/usr/lib/udev/rules.d/70-letsnote-wheelpad.rules": "packaging/udev/70-letsnote-wheelpad.rules",
        "/usr/lib/udev/rules.d/72-letsnote-wheelpad-system.rules": "packaging/udev/72-letsnote-wheelpad-system.rules",
        "/usr/lib/systemd/user/letsnote-wheelpad.service": "packaging/systemd/letsnote-wheelpad.service",
        "/usr/lib/systemd/system/letsnote-wheelpad@.service": "packaging/systemd/letsnote-wheelpad@.service",
        helper_path: "packaging/migrate/letsnote-wheelpad-migrate",
    }.items():
        expected = hashlib.new(
            algorithms[digest_algorithm], (root / repository_path).read_bytes()
        ).hexdigest()
        assert digests[files.index(packaged_path)] == expected

    print(f"verified RPM artifact: {path}")


def main() -> None:
    parser = argparse.ArgumentParser()
    formats = parser.add_mutually_exclusive_group(required=True)
    formats.add_argument("--deb", type=Path)
    formats.add_argument("--rpm", type=Path)
    args = parser.parse_args()
    if args.deb:
        verify_deb(args.deb)
    else:
        verify_rpm(args.rpm)


if __name__ == "__main__":
    main()
