#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
BINARY="${PROJECT_DIR}/app/target/release/still-files"
if [[ "${PROJECT_DIR}/app/target/debug/still-files" -nt "$BINARY" ]]; then
    BINARY="${PROJECT_DIR}/app/target/debug/still-files"
fi

if [[ -x "$BINARY" ]] && [[ -z "$(
    find "${PROJECT_DIR}/app/src" "${PROJECT_DIR}/app/Cargo.toml" \
        "${PROJECT_DIR}/app/Cargo.lock" -newer "$BINARY" -print -quit
)" ]]; then
    exec "$BINARY" "$@"
fi

if ! command -v cargo >/dev/null 2>&1; then
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        export PATH="${HOME}/.cargo/bin:${PATH}"
    else
        printf '%s\n' 'Rust is required. Install Rust before starting Still Files.' >&2
        exit 1
    fi
fi

exec cargo run --release --locked \
    --manifest-path "${PROJECT_DIR}/app/Cargo.toml" -- "$@"
