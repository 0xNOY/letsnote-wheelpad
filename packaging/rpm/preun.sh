#!/bin/sh
set -e

persistent_marker=/etc/letsnote-wheelpad/system-service-enabled
staging_marker=/run/letsnote-wheelpad/migration-staging
user_block_marker=/run/letsnote-wheelpad/migration-block-user-service
daemon=/usr/bin/letsnote-wheelpad

running_daemons()
{
    for process_exe in /proc/[0-9]*/exe; do
        [ -L "$process_exe" ] || continue
        resolved=$(readlink "$process_exe" 2>/dev/null) || continue
        case "$resolved" in
            "$daemon"|"$daemon (deleted)")
                printf '%s\n' "${process_exe#/proc/}" | cut -d/ -f1
                ;;
        esac
    done
}

if [ "${1:-1}" -eq 0 ]; then
    if [ -e "$persistent_marker" ] || [ -e "$staging_marker" ] ||
        [ -e "$user_block_marker" ] || [ -n "$(running_daemons)" ]; then
        echo "letsnote-wheelpad: final erase is blocked by migration state or a running daemon" >&2
        echo "Run /usr/libexec/letsnote-wheelpad-migrate status, disable system mode," >&2
        echo "stop the intended legacy user service, then retry the erase." >&2
        exit 1
    fi
fi

exit 0
