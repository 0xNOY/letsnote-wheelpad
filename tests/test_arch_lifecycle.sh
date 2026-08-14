#!/bin/sh
set -eu

package_path=$1
repository_root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
marker_dir=/run/letsnote-wheelpad
marker=$marker_dir/migration-staging

grep -q 'Direct RPM or Arch downgrade without this preparation is unsupported' \
    "$repository_root/README.md"

pacman -U --noconfirm "$package_path"
pacman -Q letsnote-wheelpad-bin >/dev/null
getent passwd letsnote-wheelpad >/dev/null
getent group letsnote-wheelpad >/dev/null
[ ! -e /etc/letsnote-wheelpad/system-service-enabled ]
[ ! -e "$marker" ]
[ ! -e /run/letsnote-wheelpad/migration-block-user-service ]
[ ! -e /etc/systemd/user/graphical-session.target.wants/letsnote-wheelpad.service ]

# A reinstall exercises post_upgrade without changing service or marker state.
pacman -U --noconfirm "$package_path"
[ ! -e /etc/systemd/user/graphical-session.target.wants/letsnote-wheelpad.service ]
[ ! -e /etc/letsnote-wheelpad/system-service-enabled ]

install -d -m 0755 "$marker_dir"
: >"$marker"
if pacman -R --noconfirm letsnote-wheelpad-bin; then
    echo 'Arch removal unexpectedly passed the migration guard' >&2
    exit 1
fi
pacman -Q letsnote-wheelpad-bin >/dev/null
for retained in \
    /usr/bin/letsnote-wheelpad \
    /usr/libexec/letsnote-wheelpad-migrate \
    /usr/lib/systemd/user/letsnote-wheelpad.service \
    /usr/lib/udev/rules.d/70-letsnote-wheelpad.rules \
    /usr/lib/udev/rules.d/72-letsnote-wheelpad-system.rules; do
    [ -e "$retained" ]
done

rm "$marker"
pacman -R --noconfirm letsnote-wheelpad-bin
if pacman -Q letsnote-wheelpad-bin >/dev/null 2>&1; then
    echo 'Arch package database still contains the removed package' >&2
    exit 1
fi
[ ! -e /usr/bin/letsnote-wheelpad ]
[ ! -e /usr/libexec/letsnote-wheelpad-migrate ]
getent passwd letsnote-wheelpad >/dev/null
getent group letsnote-wheelpad >/dev/null

echo 'verified Arch install, upgrade, guarded removal, and final removal lifecycle'
