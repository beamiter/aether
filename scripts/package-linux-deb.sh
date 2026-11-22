#!/usr/bin/env bash
set -euo pipefail

if (( $# < 3 || $# > 4 )); then
    printf 'Usage: %s DESKTOP_BINARY SSH_HOST_BINARY OUTPUT_DIRECTORY [PACKAGE_VERSION]\n' "$0" >&2
    exit 2
fi

desktop=$(realpath -- "$1")
host=$(realpath -- "$2")
for binary in "$desktop" "$host"; do
    if [[ ! -f "$binary" || ! -x "$binary" ]]; then
        printf 'Expected an executable file at %s\n' "$binary" >&2
        exit 2
    fi
done

project=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
mkdir -p -- "$3"
output=$(realpath -- "$3")
stage=$(mktemp -d "$output/.aether-bundle.XXXXXX")
trap 'rm -r -- "$stage"' EXIT
install -D -m 0755 -- "$desktop" "$stage/release/tauri-terminal-lab"
install -m 0755 -- "$host" "$stage/release/aether-terminal-host"

overlay=$(python3 -B - "$stage/release/aether-terminal-host" "${4:-}" <<'PY'
import json
import sys

host, version = sys.argv[1:]
config = {"bundle": {"linux": {"deb": {"files": {"/usr/bin/aether-terminal-host": host}}}}}
if version:
    config["version"] = version
print(json.dumps(config))
PY
)

cd -- "$project"
CARGO_TARGET_DIR="$stage" npm run --silent tauri -- bundle --bundles deb --ci --no-binary-patching \
    --config "$project/packaging/linux-deb.json" --config "$overlay" >&2

shopt -s nullglob
packages=("$stage"/release/bundle/deb/*.deb)
if (( ${#packages[@]} != 1 )); then
    printf 'Expected one Debian package, found %s\n' "${#packages[@]}" >&2
    exit 1
fi
package="$output/${packages[0]##*/}"
ln -T -- "${packages[0]}" "$package"
printf '%s\n' "$package"
