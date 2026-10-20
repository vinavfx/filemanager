#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="${HOME}/.local/bin"
FILEMANAGER="${PROJECT_DIR}/still-files"
PLAYER="${PROJECT_DIR}/player"

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
mv -fT -- "${STAGING_DIR}/still-files" "${BIN_DIR}/still-files"
mv -fT -- "${STAGING_DIR}/player" "${BIN_DIR}/player"

PATH_LINE='export PATH="$HOME/.local/bin:$PATH"'
for profile in "${HOME}/.profile" "${HOME}/.bashrc"; do
    if ! grep -Fqx -- "$PATH_LINE" "$profile" 2>/dev/null; then
        printf '\n%s\n' "$PATH_LINE" >>"$profile"
    fi
done

printf 'Installed Still Files and Player in %s\n' "$BIN_DIR"
printf '%s\n' 'Open a new terminal to load PATH, then run: still-files /path/to/folder'
printf '%s\n' 'Restart Still Files and Player to use the new binaries.'
