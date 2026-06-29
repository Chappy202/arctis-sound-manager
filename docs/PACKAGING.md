# PACKAGING — Arctis Sound Manager release / OTA guide

This document covers building the release artifacts, signing keys, hosting the
update manifest, the udev rule installer, the systemd user unit, and runtime
dependencies.  Steps marked **OWNER-ONLY** require secrets or root access that
live outside the repository.

---

## Artifacts produced by `pnpm tauri build`

Running `pnpm tauri build` (from the `frontend/` directory) produces:

| Artifact | Path under `src-tauri/target/release/bundle/` |
|---|---|
| AppImage | `appimage/arctis-sound-manager_<ver>_amd64.AppImage` |
| AppImage updater sig | `appimage/arctis-sound-manager_<ver>_amd64.AppImage.tar.gz.sig` |
| .deb | `deb/arctis-sound-manager_<ver>_amd64.deb` |
| .rpm | `rpm/arctis-sound-manager-<ver>-1.x86_64.rpm` |
| `latest.json` | Generated alongside each artifact (update manifest) |

`bundle.createUpdaterArtifacts = true` (set in `tauri.conf.json`) causes the
AppImage + `.tar.gz.sig` bundle and the `latest.json` manifest to be emitted
automatically.  NOT Flatpak — the sandbox blocks `hidraw` access and PipeWire
routing rules.

---

## Signing keypair — OWNER-ONLY

The private signing key is generated once and stored securely by the owner.
It is NEVER committed to the repository.

### Keypair status

Generated 2026-06-29 with an **empty passphrase**.

- `~/.signing/arctis-sound-manager.key` — **PRIVATE key (mode 600, outside repo, keep secret)**
- Public key committed to `src-tauri/tauri.conf.json` as `plugins.updater.pubkey` (commit `4aa5f8c`).

### CI secret — OWNER-ONLY

Set the GitHub Actions secret once:

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.signing/arctis-sound-manager.key
```

`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is **not set** — the key has an empty passphrase.

Tauri's build system reads `TAURI_SIGNING_PRIVATE_KEY` automatically during `pnpm tauri build` to
sign the updater artifact.

---

## Building a release — OWNER-ONLY

```sh
cd frontend/
pnpm install
TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.signing/arctis-sound-manager.key)" \
TAURI_SIGNING_PRIVATE_KEY_PASSWORD="" \
pnpm tauri build
```

This requires webkit2gtk (for the Tauri WebView), libudev (for hidraw
enumeration), and a C compiler.  On Nobara/Fedora:

```sh
sudo dnf install webkit2gtk4.1-devel libudev-devel gcc
```

---

## Releasing

1. Bump `version` in `src-tauri/tauri.conf.json`.
2. Commit, then tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. `.github/workflows/release.yml` builds, signs, and publishes to GitHub Releases.
   (Requires the `TAURI_SIGNING_PRIVATE_KEY` secret to be set once.)
4. Users on an older AppImage see the in-app update banner on next launch.

The workflow runs `scripts/check-version.sh` (asserts the tag matches the version in
`tauri.conf.json`) and `scripts/stage-sidecar.sh` (builds `asm-cli` and stages it as a
sidecar binary) before invoking `tauri-action`, which builds, signs, and uploads the
AppImage, `.AppImage.tar.gz`, `.sig`, `latest.json`, `.deb`, and `.rpm` to GitHub Releases.

---

## Update server — OWNER-ONLY

### What must be hosted

1. **`latest.json`** — the update manifest generated alongside the AppImage.
   Content example:
   ```json
   {
     "version": "0.2.0",
     "notes": "Release notes",
     "pub_date": "2026-06-21T00:00:00Z",
     "platforms": {
       "linux-x86_64": {
         "signature": "<minisign sig>",
         "url": "https://your-host/arctis-sound-manager_0.2.0_amd64.AppImage.tar.gz"
       }
     }
   }
   ```
2. **The signed `.AppImage.tar.gz`** — the binary artifact the updater downloads.

### Endpoint URL

The committed endpoint in `src-tauri/tauri.conf.json` under `plugins.updater.endpoints`:

```
https://github.com/Chappy202/arctis-sound-manager/releases/latest/download/latest.json
```

The GitHub Releases pipeline (`.github/workflows/release.yml`) uploads `latest.json` automatically
on each tagged release — no separate hosting required.

The endpoint must return:
- **200** with a `latest.json` body when an update is available.
- **204** (no content) when the client is already up to date.
- **Any non-2xx** causes Tauri to try the next endpoint in the list.

TLS is enforced in production mode — HTTP is rejected.

---

## How the auto-updater works

1. On app startup, the frontend calls `checkForUpdate()` (from `src/lib/updater.ts`).
2. The Tauri updater plugin queries the configured endpoint, passing
   `{{target}}`, `{{arch}}`, and `{{current_version}}` as URL path segments.
3. If the server returns a newer version + a `latest.json` with a valid
   minisign signature (verified against `plugins.updater.pubkey`), the update
   info is surfaced to the user via a banner in the UI.
4. The user clicks "Install & Relaunch" — the AppImage `.tar.gz` is downloaded,
   signature verified against the committed public key, installed, and the app
   relaunches.

Signature mismatch → update is rejected.  No download of an unsigned artifact.

---

## Daemon bundling

`asm-cli` ships as a Tauri `externalBin` sidecar, staged by `scripts/stage-sidecar.sh` during the
release workflow.

| Install type | `asm-cli` location |
|---|---|
| AppImage | GUI copies it to `~/.local/bin/asm-cli` on first launch |
| `.deb` / `.rpm` | Installed to `/usr/bin/asm-cli` by the package manager |

For AppImage installs the GUI performs the copy on launch so that the systemd `ExecStart` path
(`%h/.local/bin/asm-cli`) remains valid across in-app updates (the AppImage is replaced in place,
but the extracted binary at `~/.local/bin/asm-cli` is refreshed automatically).

---

## AppImage install

```sh
scripts/install-appimage.sh arctis-sound-manager_<ver>_amd64.AppImage
```

This registers the app as a launcher application (creates the `.desktop` entry, marks the
AppImage executable, and places it in `~/.local/bin/`).  Then run the udev installer once:

```sh
asm-cli setup-udev
```

---

## udev rule installer

The udev rule in `packaging/udev/70-arctis-sound-manager.rules` grants the
active user `hidraw` access to SteelSeries devices (`idVendor 0x1038`).

### Automatic (first run)

```sh
asm-cli setup-udev
```

This:
1. Checks whether `/etc/udev/rules.d/70-arctis-sound-manager.rules` is present.
2. If not, constructs and prints a `pkexec sh -c '…'` command.
3. Runs it via `pkexec` (prompts for authentication — never silent sudo).
4. After install, asks the user to replug the headset.

Dry-run (preview only, no execution):

```sh
asm-cli setup-udev --dry-run
```

### Manual fallback

```sh
sudo cp packaging/udev/70-arctis-sound-manager.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
# Then replug the headset.
```

The rule uses priority `70-` (before `73-seat-late.rules`) so the `uaccess`
ACL is applied before seat-late processing — the classic symptom of a
mis-ordered rule is a `root`-only `/dev/hidraw*` node.

---

## systemd user unit

`packaging/systemd/arctis-sound-manager.service` runs the daemon as a user
service (no root).

### Install

```sh
mkdir -p ~/.config/systemd/user/
cp packaging/systemd/arctis-sound-manager.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now arctis-sound-manager.service
```

### Check status / logs

```sh
systemctl --user status arctis-sound-manager.service
journalctl --user -u arctis-sound-manager.service -f
```

### OWNER-FILL: ExecStart path

Adjust `ExecStart` in the `.service` file to match the installed binary path:
- System-wide `.deb`/`.rpm`: `/usr/bin/asm-cli daemon`
- Per-user AppImage: `%h/.local/bin/asm-cli daemon`

---

## .desktop file

`packaging/arctis-sound-manager.desktop` registers the app in the desktop menu.

The `Exec` line uses `env WEBKIT_DISABLE_DMABUF_RENDERER=1` prefix to work
around a WebKitGTK DMA-buf compositor bug with proprietary NVIDIA drivers
(blank window without this flag — see ARCHITECTURE §11).

### Install

```sh
cp packaging/arctis-sound-manager.desktop ~/.local/share/applications/
update-desktop-database ~/.local/share/applications/
```

---

## Runtime dependencies

| Dependency | Required for |
|---|---|
| PipeWire 1.x + WirePlumber 0.5.x | Audio routing + EQ sink lifecycle |
| `pw-record`, `pw-cli`, `pw-metadata` | Called as subprocesses by the daemon |
| `pkexec` (polkit) | First-run udev rule installer |
| webkit2gtk / WebKitGTK | Tauri WebView |
| libudev | hidraw device enumeration (linked into the binary) |
| LADSPA plugins (optional) | Mic DSP chain (deepfilter, rnnoise, sc4m) |

No PulseAudio required.  The system runs PipeWire 1.4.x only (48 kHz).

---

## NOT Flatpak

Flatpak is explicitly excluded (ARCHITECTURE G9).  The Flatpak sandbox blocks:
- Direct `hidraw` access to SteelSeries HID devices.
- PipeWire routing rules (`node.rules` / WirePlumber scripts in
  `~/.local/share/wireplumber/main.lua.d/`).

AppImage is the primary portable artifact.
