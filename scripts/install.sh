#!/usr/bin/env bash
# Build and install the Aether desktop and SSH terminal host on Linux.
set -euo pipefail

repo="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
app_id="io.github.beamiter.aether"
prefix="${HOME:+$HOME/.local}"
bin_dir=""
data_dir=""
destdir="${DESTDIR:-}"
binary=""
host_binary=""
desktop=1
dry_run=0
local_sources=0

usage() {
    cat <<'EOF'
Usage: ./scripts/install.sh [options]

  --prefix PATH         Installation prefix (default: ~/.local)
  --bin-dir PATH        Binary directory (default: PREFIX/bin)
  --data-dir PATH       Desktop data directory (default: XDG_DATA_HOME or PREFIX/share)
  --binary PATH         Prebuilt desktop executable (tauri-terminal-lab)
  --host-binary PATH    Prebuilt SSH host executable (aether-terminal-host)
  --local-sources       Build with sibling sources through jterm_core's family runner
  --no-desktop          Skip the desktop entry and icon
  --dry-run             Print the build and install commands
  -h, --help            Show this help

With --binary, the SSH host executable defaults to aether-terminal-host
in the same directory.
DESTDIR prepends a packaging root. CARGO_TARGET_DIR defaults to <repo>/target.
EOF
}

die() {
    printf 'aether install: %s\n' "$*" >&2
    exit 1
}

validate_directory() {
    local label="$1" directory="$2"
    [[ "$directory" == /* && "/${directory#/}/" != */../* && ! "$directory" =~ [[:cntrl:]] ]] \
        || die "$label requires an absolute path without '..' or control characters"
}

validate_staged_directory() {
    local directory="$1"
    [[ -n "$destdir" ]] || return 0
    while [[ -n "$directory" && "$directory" != / ]]; do
        [[ ! -L "$directory" ]] || die "symbolic link in staged directory: $directory"
        directory="${directory%/*}"
    done
}

write_desktop_entry() {
    local destination="$1" program="$bin_dir/aether" exec_value=""
    local index character
    # Desktop entries decode the string before splitting Exec into arguments.
    for ((index=0; index<${#program}; index++)); do
        character="${program:index:1}"
        case "$character" in
            \\) exec_value+="\\\\\\\\" ;;
            '"') exec_value+='\"' ;;
            '`') exec_value+='\`' ;;
            '$') exec_value+='\\$' ;;
            *) exec_value+="$character" ;;
        esac
    done
    cat >"$destination" <<EOF
[Desktop Entry]
Type=Application
Name=Aether Terminal
Comment=Open a local or SSH terminal
Exec="$exec_value"
Icon=$app_id
Terminal=false
Categories=System;TerminalEmulator;
EOF
    chmod 0644 -- "$destination"
}

stages=()
destinations=()
publishing=0

# Roll back files that were renamed before a publish failed.
cleanup() {
    local index stage dest
    for ((index=${#stages[@]}-1; index>=0; index--)); do
        stage="${stages[index]}"
        dest="${destinations[index]}"
        if ((publishing == 1)) && [[ ! -e "$stage/new" ]]; then
            if [[ -e "$stage/previous" || -L "$stage/previous" ]]; then
                if ! mv -fT -- "$stage/previous" "$dest"; then
                    printf 'aether install: could not restore %s from %s/previous\n' "$dest" "$stage" >&2
                    continue
                fi
            else
                rm -f -- "$dest" || true
            fi
        fi
        rm -f -- "$stage/new" "$stage/previous"
        rmdir -- "$stage"
    done
}

stage_artifact() {
    local dest="$1" stage
    shift
    printf ' '
    printf ' %q' "$@" "$dest"
    printf '\n'
    ((dry_run == 0)) || return 0

    mkdir -p -- "${dest%/*}"
    # Keep the new file and its backup on the destination filesystem.
    stage="$(mktemp -d -- "${dest%/*}/.aether-install.XXXXXX")"
    stages+=("$stage")
    destinations+=("$dest")
    if [[ -L "$dest" || -f "$dest" ]]; then
        cp -a --reflink=auto -- "$dest" "$stage/previous"
    elif [[ -e "$dest" ]]; then
        die "cannot replace $dest"
    fi
    "$@" "$stage/new"
}

while (($# > 0)); do
    case "$1" in
        --prefix|--bin-dir|--data-dir|--binary|--host-binary)
            (($# >= 2)) || die "$1 requires a path"
            option="$1"; value="$2"; shift 2 ;;
        --prefix=*|--bin-dir=*|--data-dir=*|--binary=*|--host-binary=*)
            option="${1%%=*}"; value="${1#*=}"; shift ;;
        --no-desktop) desktop=0; shift; continue ;;
        --dry-run) dry_run=1; shift; continue ;;
        --local-sources) local_sources=1; shift; continue ;;
        -h|--help) usage; exit 0 ;;
        --) shift; (($# == 0)) || die "unexpected positional arguments: $*"; break ;;
        *) die "unknown option: $1" ;;
    esac
    [[ -n "$value" ]] || die "$option requires a path"
    case "$option" in
        --prefix) prefix="$value" ;;
        --bin-dir) bin_dir="$value" ;;
        --data-dir) data_dir="$value" ;;
        --binary) binary="$value" ;;
        --host-binary) host_binary="$value" ;;
    esac
done

bin_dir="${bin_dir:-${prefix%/}/bin}"
data_dir="${data_dir:-${XDG_DATA_HOME:-${prefix%/}/share}}"
validate_directory --prefix "$prefix"
validate_directory --bin-dir "$bin_dir"
if [[ -n "$destdir" ]]; then
    validate_directory DESTDIR "$destdir"
fi
staged_bin_dir="${destdir%/}${bin_dir%/}"
staged_bin_dir="${staged_bin_dir:-/}"
validate_staged_directory "$staged_bin_dir"

if ((desktop == 1)); then
    validate_directory --data-dir "$data_dir"
    [[ "$bin_dir" != *[%=]* ]] || die "desktop executable path cannot contain '%' or '='"
    staged_data_dir="${destdir%/}${data_dir%/}"
    applications_dir="$staged_data_dir/applications"
    icon_dir="$staged_data_dir/icons/hicolor/32x32/apps"
    icon="$repo/src-tauri/icons/icon.png"
    validate_staged_directory "$applications_dir"
    validate_staged_directory "$icon_dir"
    [[ -r "$icon" && -f "$icon" ]] || die "missing desktop icon: $icon"
fi

if [[ -n "$binary" ]]; then
    ((local_sources == 0)) || die "--local-sources cannot be combined with prebuilt binaries"
    host_binary="${host_binary:-$(dirname -- "$binary")/aether-terminal-host}"
else
    [[ -z "$host_binary" ]] || die "--host-binary requires --binary"
    build_args=()
    if ((local_sources == 1)); then build_args+=(--local-sources); fi
    if ((dry_run == 1)); then build_args+=(--dry-run); fi
    "$repo/scripts/build-release.sh" "${build_args[@]}"
    target_dir="${CARGO_TARGET_DIR:-$repo/target}"
    if [[ "$target_dir" != /* ]]; then target_dir="$repo/$target_dir"; fi
    binary="$target_dir/release/tauri-terminal-lab"
    host_binary="$target_dir/release/aether-terminal-host"
fi

if ((dry_run == 0)); then
    for source in "$binary" "$host_binary"; do
        [[ -f "$source" && -r "$source" && -s "$source" ]] \
            || die "expected a nonempty readable binary at $source"
    done
    mkdir -p -- "$staged_bin_dir"
    # Serialize installers that publish to the same binary directory.
    exec {install_lock}<"$staged_bin_dir"
    flock -x "$install_lock"
    trap cleanup EXIT
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM
fi

stage_artifact "${staged_bin_dir%/}/aether-terminal-host" install -m 0755 -- "$host_binary"
stage_artifact "${staged_bin_dir%/}/aether" install -m 0755 -- "$binary"
if ((desktop == 1)); then
    stage_artifact "$icon_dir/$app_id.png" install -m 0644 -- "$icon"
    stage_artifact "$applications_dir/$app_id.desktop" write_desktop_entry
fi

if ((dry_run == 0)); then
    publishing=1
    for index in "${!stages[@]}"; do
        mv -fT -- "${stages[index]}/new" "${destinations[index]}"
    done
    publishing=0
    if ((desktop == 1)) && [[ -z "$destdir" ]] && command -v gtk-update-icon-cache >/dev/null 2>&1; then
        gtk-update-icon-cache --force --ignore-theme-index --quiet "$staged_data_dir/icons/hicolor" \
            || printf 'aether install: icon cache refresh failed\n' >&2
    fi
    printf 'Installed aether and aether-terminal-host in %s\n' "$staged_bin_dir"
fi
