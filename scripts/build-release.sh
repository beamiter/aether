#!/usr/bin/env bash
set -euo pipefail

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
local_sources=0
dry_run=0

die() {
    printf 'aether build: %s\n' "$*" >&2
    exit 1
}

run() {
    printf ' '
    printf ' %q' "$@"
    printf '\n'
    if ((dry_run == 0)); then
        "$@"
    fi
}

for option in "$@"; do
    case "$option" in
        --local-sources) local_sources=1 ;;
        --dry-run) dry_run=1 ;;
        -h|--help)
            printf 'Usage: %s [--local-sources] [--dry-run]\n' "$0"
            exit 0 ;;
        *) die "unknown option: $option" ;;
    esac
done

cd -- "$repo"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}"
if [[ "$CARGO_TARGET_DIR" != /* ]]; then
    CARGO_TARGET_DIR="$repo/$CARGO_TARGET_DIR"
fi
printf 'Building Aether release in %s\n' "$CARGO_TARGET_DIR"

if ((dry_run == 0)); then
    if ! command -v cargo >/dev/null 2>&1 && [[ -f "${HOME:-}/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
    fi
    command -v cargo >/dev/null 2>&1 || die "cargo is required"
fi

if ((local_sources == 1)); then
    runner="$repo/../jterm_core/scripts/family-cargo.py"
    if ((dry_run == 0)); then
        [[ -f "$runner" ]] || die "--local-sources requires sibling jterm_core/scripts/family-cargo.py"
        python3 -c 'import tomllib' 2>/dev/null || die "--local-sources requires Python 3.11+"
    fi
    run python3 "$runner" aether build --release --bin tauri-terminal-lab
    run python3 "$runner" aether-bridge build --release --bin aether-terminal-host
else
    run cargo build --release --locked --manifest-path src-tauri/Cargo.toml --bin tauri-terminal-lab
    run cargo build --release --locked --manifest-path bridge/Cargo.toml --bin aether-terminal-host
fi
