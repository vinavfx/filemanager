#!/usr/bin/env bash
set -euo pipefail

PROJECT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v cargo >/dev/null 2>&1; then
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        export PATH="${HOME}/.cargo/bin:${PATH}"
    else
        printf '%s\n' 'Rust is required. Install Rust before starting Still Files.' >&2
        exit 1
    fi
fi

exec cargo run --locked \
    --manifest-path "${PROJECT_DIR}/app/Cargo.toml" -- "$@"
