#!/usr/bin/env bash
# Build asm-cli (release) and stage it as the Tauri externalBin sidecar.
# Tauri requires the sidecar at src-tauri/binaries/asm-cli-<target-triple>.
set -euo pipefail

cd "$(dirname "$0")/.."

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
echo "Staging asm-cli sidecar for target: ${TRIPLE}"

# Ship with the `pw-watcher` feature so the resident daemon re-applies remembered
# per-app routes when an application's output stream (re)appears — without it,
# routes set on one stream node are lost when the app spawns a new node (e.g. a
# browser starting a video). Requires clang + libpipewire-0.3-dev at build time.
cargo build --release -p arctis-cli --features pw-watcher

mkdir -p src-tauri/binaries
cp "target/release/asm-cli" "src-tauri/binaries/asm-cli-${TRIPLE}"
chmod +x "src-tauri/binaries/asm-cli-${TRIPLE}"
echo "Staged: src-tauri/binaries/asm-cli-${TRIPLE}"
