#!/usr/bin/env bash
set -euo pipefail

# Regional runner mirrors have exhausted the desktop job during downloads.
# Keep the runner's signed Ubuntu sources and prefer the official HTTPS archives.
mirrors=/etc/apt/apt-mirrors.txt
if [[ -f "$mirrors" ]]; then
    printf 'https://archive.ubuntu.com/ubuntu/\tpriority:1\nhttps://security.ubuntu.com/ubuntu/\tpriority:2\n' |
        sudo tee "$mirrors" >/dev/null
fi

network=(-o Acquire::Retries=2 -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30)
sudo apt-get "${network[@]}" update
sudo apt-get "${network[@]}" install --yes --no-install-recommends "$@"
