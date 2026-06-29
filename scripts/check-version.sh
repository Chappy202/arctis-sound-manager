#!/usr/bin/env bash
# Assert that a release tag (vX.Y.Z) matches tauri.conf.json `version`.
# Usage: scripts/check-version.sh "$GITHUB_REF_NAME"   (or any vX.Y.Z string)
set -euo pipefail

cd "$(dirname "$0")/.."

tag="${1:?usage: check-version.sh <vX.Y.Z>}"
tag_ver="${tag#v}"
conf_ver="$(jq -r '.version' src-tauri/tauri.conf.json)"

if [[ "$tag_ver" != "$conf_ver" ]]; then
  echo "ERROR: tag '${tag}' (=> ${tag_ver}) != tauri.conf.json version '${conf_ver}'" >&2
  echo "Bump 'version' in src-tauri/tauri.conf.json to match the tag, then re-tag." >&2
  exit 1
fi
echo "Version OK: ${conf_ver}"
