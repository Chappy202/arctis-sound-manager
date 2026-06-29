#!/usr/bin/env bash
# Install a downloaded Arctis Sound Manager AppImage as a launcher app.
# Usage: scripts/install-appimage.sh ~/Downloads/arctis-sound-manager_X.Y.Z_amd64.AppImage
set -euo pipefail

src="${1:?usage: install-appimage.sh <path-to .AppImage>}"
[[ -f "$src" ]] || { echo "Not a file: $src" >&2; exit 1; }

app_dir="${HOME}/Applications"
dest="${app_dir}/arctis-sound-manager.AppImage"
desktop_dir="${HOME}/.local/share/applications"
icon_dir="${HOME}/.local/share/icons/hicolor/128x128/apps"

mkdir -p "$app_dir" "$desktop_dir" "$icon_dir"
install -m 0755 "$src" "$dest"

# Extract the bundled icon (best-effort — must never abort the install).
# Note: the AppDir root holds a *.png symlink into usr/share/icons; extracting
# only the root glob leaves it dangling, so target the real hicolor icon and
# accept only regular files (-type f skips broken symlinks).
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
( cd "$tmp" && APPIMAGE_EXTRACT_AND_RUN=1 "$dest" --appimage-extract 'usr/share/icons/hicolor/128x128/apps/*.png' >/dev/null 2>&1 || true )
icon_src="$(find "$tmp" -path '*128x128*' -name '*.png' -type f | head -n1 || true)"
if [[ -z "$icon_src" ]]; then
  # Fallback: extract everything, take any real (non-empty, non-symlink) png.
  ( cd "$tmp" && APPIMAGE_EXTRACT_AND_RUN=1 "$dest" --appimage-extract >/dev/null 2>&1 || true )
  icon_src="$(find "$tmp" -name '*.png' -type f -not -empty | head -n1 || true)"
fi
if [[ -n "$icon_src" && -f "$icon_src" ]]; then
  install -m 0644 "$icon_src" "${icon_dir}/arctis-sound-manager.png" || true
fi

cat > "${desktop_dir}/arctis-sound-manager.desktop" <<EOF
[Desktop Entry]
Version=1.1
Type=Application
Name=Arctis Sound Manager
GenericName=Headset Audio Manager
Comment=Per-app audio routing and EQ for SteelSeries Arctis headsets
Exec=env WEBKIT_DISABLE_DMABUF_RENDERER=1 ${dest} %U
Icon=arctis-sound-manager
Terminal=false
StartupNotify=true
StartupWMClass=arctis-sound-manager
Categories=Audio;AudioVideo;
Keywords=SteelSeries;Arctis;headset;EQ;equalizer;audio;sound;
EOF

update-desktop-database "$desktop_dir" >/dev/null 2>&1 || true
echo "Installed: ${dest}"
echo "Launcher entry: ${desktop_dir}/arctis-sound-manager.desktop"
echo "Next: launch the app, then run 'asm-cli setup-udev' once for headset access."
