#!/usr/bin/env bash
# Renders assets/demo.gif: builds the fake world with setup.sh, plays
# demo.tape through VHS, adds the key captions with captions.py, then stops
# the demo server.
#
# usage: scripts/demo/record.sh [flockr-binary]
# Needs vhs and ttyd (Arch: pacman -S vhs ttyd) and JetBrainsMono Nerd Font.
set -euo pipefail

repo_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
flockr_bin="${1:-$repo_dir/target/release/flockr}"
cd "$repo_dir"

# Run outside any flockr session, so nothing reaches a real server.
unset $(env | awk -F= '/^(FLOCKR|HERDR)_/ {print $1}')

scripts/demo/setup.sh "$flockr_bin"
# VHS's shell inherits these, so even a stray flockr call there reaches only
# the demo server.
export FLOCKR_SOCKET_PATH=/tmp/flockr-demo/home/.config/flockr/flockr.sock
export FLOCKR_CLIENT_SOCKET_PATH=/tmp/flockr-demo/home/.config/flockr/flockr-client.sock
trap 'HOME=/tmp/flockr-demo/home XDG_CONFIG_HOME=/tmp/flockr-demo/home/.config "$flockr_bin" server stop >/dev/null 2>&1 || true' EXIT
vhs scripts/demo/demo.tape
python3 -I scripts/demo/captions.py scripts/demo/demo.tape /tmp/flockr-demo/demo.mp4 assets/demo.gif
ls -lh assets/demo.gif
