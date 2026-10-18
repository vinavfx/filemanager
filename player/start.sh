#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

binary="$script_dir/target/release/player"
if [[ -x "$binary" ]] && [[ -z "$(
    find "$script_dir/src" "$script_dir/Cargo.toml" \
        "$script_dir/Cargo.lock" -newer "$binary" -print -quit
)" ]]; then
    exec "$binary" "$@"
fi

exec cargo run --release --offline --locked --quiet \
    --manifest-path "$script_dir/Cargo.toml" -- "$@"
