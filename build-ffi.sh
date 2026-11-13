#!/usr/bin/env bash
# build-ffi.sh — build the renderer cdylib (libosm_renderer.so) for local dev
# and staging into a Basecamp install dir. The .lgx packaging itself is done by
# the nix flake (app/flake.nix) in CI; this script is for quick local iteration
# against a running Basecamp where you swap the .so in place (dev only).
set -euo pipefail
cd "$(dirname "$0")/renderer"
export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release
echo "built: $(pwd)/target/release/libosm_renderer.so"
