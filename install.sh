#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
APPLICATIONS_DIR="${XDG_DATA_HOME:-${HOME}/.local/share}/applications"
FILEMANAGER="${PROJECT_DIR}/still-files"
PLAYER="${PROJECT_DIR}/player"

stop_applications() {
    local user_id signal attempt
    user_id="$(id -u)"
    for signal in TERM KILL; do
        pkill "-$signal" -u "$user_id" -x 'still-files|still_files|player' \
            || [[ $? -eq 1 ]]
        for attempt in {1..30}; do
            if ! pgrep -u "$user_id" -x 'still-files|still_files|player' \
                >/dev/null; then
                return 0
            fi
            sleep 0.1
        done
    done
    printf '%s\n' 'Could not stop Still Files and Player.' >&2
    return 1
}

install_binaries() {
    if [[ ! -f "$FILEMANAGER" ]]; then
        FILEMANAGER="${PROJECT_DIR}/app/target/release/still-files"
        PLAYER="${PROJECT_DIR}/player/target/release/player"
    fi

    if [[ ! -f "$FILEMANAGER" || ! -f "$PLAYER" ]]; then
        printf '%s\n' 'Both release binaries are required.' \
            'Build or download them first.' >&2
        exit 1
    fi

    mkdir -p -- "$BIN_DIR"
    STAGING_DIR="$(mktemp -d "${BIN_DIR}/.still-files-install.XXXXXX")"
    trap 'rm -rf -- "$STAGING_DIR"' EXIT
    install -m 755 -- "$FILEMANAGER" "${STAGING_DIR}/still-files"
    install -m 755 -- "$PLAYER" "${STAGING_DIR}/player"
    stop_applications
    mv -fT -- "${STAGING_DIR}/still-files" "${BIN_DIR}/still-files"
    mv -fT -- "${STAGING_DIR}/player" "${BIN_DIR}/player"
}

configure_path() {
    local path_line='export PATH="$HOME/.local/bin:$PATH"'
    local profile
    for profile in "${HOME}/.profile" "${HOME}/.bashrc"; do
        if ! grep -Fqx -- "$path_line" "$profile" 2>/dev/null; then
            printf '\n%s\n' "$path_line" >>"$profile"
        fi
    done
}

install_desktop_entry() {
    local executable="${BIN_DIR}/still-files"
    local character
    for character in '\' '"' '`' '$'; do
        executable=${executable//"$character"/\\"$character"}
    done
    executable=${executable//%/%%}
    executable=${executable//\\/\\\\}

    mkdir -p -- "$APPLICATIONS_DIR"
    printf '%s\n' '[Desktop Entry]' 'Type=Application' 'Name=Still Files' \
        'Comment=Browse files and folders' "Exec=\"${executable}\" %f" \
        'Icon=system-file-manager' 'Terminal=false' \
        'Categories=System;FileTools;FileManager;' 'MimeType=inode/directory;' \
        'StartupNotify=false' >"${APPLICATIONS_DIR}/still-files.desktop"
}

main() {
    install_binaries
    configure_path
    install_desktop_entry
    printf 'Installed Still Files and Player in %s\n' "$BIN_DIR"
    printf 'Created desktop entry in %s\n' "$APPLICATIONS_DIR"
    printf '%s\n' 'Open Still Files from your application menu.'
    printf '%s\n' 'Still Files and Player were stopped. Reopen them to continue.'
}

main "$@"
