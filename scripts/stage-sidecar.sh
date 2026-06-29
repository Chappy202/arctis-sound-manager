#!/usr/bin/env bash
# Build asm-cli (release) and stage it as the Tauri externalBin sidecar.
# Tauri requires the sidecar at src-tauri/binaries/asm-cli-<target-triple>.
set -euo pipefail

cd "$(dirname "$0")/.."

TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
echo "Staging asm-cli sidecar for target: ${TRIPLE}"

cargo build --release -p arctis-cli

mkdir -p src-tauri/binaries
cp "target/release/asm-cli" "src-tauri/binaries/asm-cli-${TRIPLE}"
chmod +x "src-tauri/binaries/asm-cli-${TRIPLE}"
echo "Staged: src-tauri/binaries/asm-cli-${TRIPLE}"
