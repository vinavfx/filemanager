#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

binary="$script_dir/target/release/player"
if [[ "$script_dir/target/debug/player" -nt "$binary" ]]; then
    binary="$script_dir/target/debug/player"
fi

if [[ -x "$binary" ]] && [[ -z "$(
    find "$script_dir/src" "$script_dir/Cargo.toml" \
        "$script_dir/Cargo.lock" -newer "$binary" -print -quit
)" ]]; then
    exec "$binary" "$@"
fi

exec cargo run --offline --locked --quiet \
    --manifest-path "$script_dir/Cargo.toml" -- "$@"
