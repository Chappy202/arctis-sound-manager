# Distribution & Auto-Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the app installable as a real desktop app and updateable with one in-app click, via a tagged-release GitHub Actions pipeline that builds, signs, and publishes an AppImage (+ deb/rpm) carrying the `asm-cli` daemon.

**Architecture:** Tauri v2 emits a signed AppImage whose in-app updater (already wired in the frontend) pulls `latest.json` from GitHub Releases. The `asm-cli` daemon ships *inside* every artifact as a Tauri `externalBin` sidecar; on AppImage launch the GUI copies it to a stable `~/.local/bin/asm-cli` so the systemd user unit's `ExecStart` survives updates. A release workflow on `v*` tags builds the daemon, stages the sidecar, signs, and publishes.

**Tech Stack:** Rust (Cargo workspace), Tauri v2, `tauri-plugin-updater`, minisign signing, pnpm/Svelte frontend, GitHub Actions, AppImage/deb/rpm bundling.

## Global Constraints

- **Linux x86_64 only.** Target triple: `x86_64-unknown-linux-gnu`. No multi-arch, no Win/macOS.
- **48 kHz audio, PipeWire only** — irrelevant to packaging but do not add resampling or PulseAudio deps.
- **Never write the headset OLED; never replay unverified firmware opcodes** (ARCHITECTURE G2). This plan touches no device-write paths.
- **No Flatpak/Snap** (ARCHITECTURE G9) — AppImage is the OTA artifact; deb/rpm are bonus, non-updating.
- **The minisign private key never enters the repo.** It lives at `~/.signing/arctis-sound-manager.key` (mode 600) and is injected into CI as `TAURI_SIGNING_PRIVATE_KEY`. Empty passphrase → no password secret.
- **Committed pubkey is authoritative:** `tauri.conf.json` `plugins.updater.pubkey` = `dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY2QjBGRUZEMTVDRUFBQkQKUldTOXFzNFYvZjZ3OWllNW9XVURUc0dlcGVHZ1ErM2VNMkRjOTFrUFRNcU5vNWZzNVR4L3ljNkkK` (already committed in `4aa5f8c`).
- **App version single source of truth:** `src-tauri/tauri.conf.json` `version`. The release tag `vX.Y.Z` must equal it.
- **Run the Tauri CLI from the repo root** (where `src-tauri/` is a subfolder), never from `frontend/`.
- **Typed errors, no `unwrap` on runtime paths** (ARCHITECTURE G7). Follow the existing `DaemonEnv` seam pattern in `daemon_control.rs` for testability.

---

### Task 1: `asm-cli --version` flag

Gives the daemon a real version string (for diagnostics and parity with the app version). Pure clap change.

**Files:**
- Modify: `crates/cli/src/main.rs:17-22` (the `#[derive(Parser)]` struct)

**Interfaces:**
- Produces: `asm-cli --version` prints `asm-cli <CARGO_PKG_VERSION>` and exits 0.

- [ ] **Step 1: Add a failing integration test**

Create `crates/cli/tests/version.rs`:

```rust
use std::process::Command;

#[test]
fn version_flag_prints_crate_version() {
    let bin = env!("CARGO_BIN_EXE_asm-cli");
    let out = Command::new(bin).arg("--version").output().expect("run asm-cli");
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "expected version in output, got: {stdout:?}"
    );
}
```

- [ ] **Step 2: Run it, verify it fails**

Run: `cargo test -p arctis-cli --test version`
Expected: FAIL — clap currently has no `version`, so `--version` is an "unexpected argument" error and exit is non-zero.

- [ ] **Step 3: Add `version` to the command attribute**

In `crates/cli/src/main.rs`, change:

```rust
#[command(name = "asm-cli", about = "Arctis Sound Manager CLI (read-only probe)")]
```
to:
```rust
#[command(name = "asm-cli", version, about = "Arctis Sound Manager CLI (read-only probe)")]
```

- [ ] **Step 4: Run it, verify it passes**

Run: `cargo test -p arctis-cli --test version`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/src/main.rs crates/cli/tests/version.rs
git commit -m "feat(cli): add asm-cli --version flag"
```

---

### Task 2: Bundle `asm-cli` as a Tauri `externalBin` sidecar

Make every artifact carry the daemon. Tauri expects the sidecar at a target-triple-suffixed path *relative to `tauri.conf.json`* (i.e. under `src-tauri/`). We stage it via a script reused by CI and local builds.

**Files:**
- Modify: `src-tauri/tauri.conf.json` (add `bundle.externalBin`)
- Create: `scripts/stage-sidecar.sh`
- Modify: `src-tauri/.gitignore` (ignore the staged binary)

**Interfaces:**
- Produces: after `scripts/stage-sidecar.sh` runs, `src-tauri/binaries/asm-cli-x86_64-unknown-linux-gnu` exists and is executable. After `tauri build`, `asm-cli` sits next to the GUI binary inside every bundle.

- [ ] **Step 1: Add the staging script**

Create `scripts/stage-sidecar.sh`:

```bash
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
```

- [ ] **Step 2: Make it executable and run it**

Run:
```bash
chmod +x scripts/stage-sidecar.sh && ./scripts/stage-sidecar.sh
```
Expected: ends with `Staged: src-tauri/binaries/asm-cli-x86_64-unknown-linux-gnu`. Verify:
```bash
test -x src-tauri/binaries/asm-cli-x86_64-unknown-linux-gnu && echo OK
```
Expected: `OK`.

- [ ] **Step 3: Wire `externalBin` into the bundle config**

In `src-tauri/tauri.conf.json`, inside the `"bundle"` object (alongside `"targets"`/`"icon"`), add:

```json
    "externalBin": ["binaries/asm-cli"],
```

(Tauri appends `-<target-triple>` automatically when resolving the file.)

- [ ] **Step 4: Ignore the staged artifact**

Append to `src-tauri/.gitignore`:

```
# Staged externalBin sidecar (built by scripts/stage-sidecar.sh)
/binaries/
```

- [ ] **Step 5: Verify config parses and the sidecar is git-ignored**

Run:
```bash
jq -e '.bundle.externalBin == ["binaries/asm-cli"]' src-tauri/tauri.conf.json
git status --porcelain src-tauri/binaries
```
Expected: `jq` prints `true`; `git status` prints nothing (binary ignored).

- [ ] **Step 6: Commit**

```bash
git add scripts/stage-sidecar.sh src-tauri/tauri.conf.json src-tauri/.gitignore
git commit -m "feat(bundle): ship asm-cli daemon as a Tauri externalBin sidecar"
```

---

### Task 3: Sync the bundled daemon to `~/.local/bin` on AppImage launch

The systemd unit can't point inside the ephemeral AppImage mount, so on AppImage launch we copy the sidecar to a stable path. Pure decision logic goes through the existing `DaemonEnv` seam and is unit-tested; the env/path glue is a thin wrapper.

**Files:**
- Modify: `src-tauri/src/daemon_control.rs` (add `copy_file` to the seam; add `sync_daemon_binary` + `maybe_sync_bundled_daemon`; extend `MockEnv`)
- Modify: `src-tauri/src/lib.rs:75` area (call the sync in `.setup`)

**Interfaces:**
- Consumes: `DaemonEnv`, `home_dir()` (existing in `daemon_control.rs`).
- Produces:
  - `DaemonEnv::copy_file(&self, src: &Path, dst: &Path) -> std::io::Result<()>`
  - `pub fn sync_daemon_binary(env: &impl DaemonEnv, bundled: &Path, dest: &Path, marker: &Path, app_version: &str) -> std::io::Result<bool>` — returns `Ok(true)` if it copied, `Ok(false)` if already current.
  - `pub fn maybe_sync_bundled_daemon(env: &impl DaemonEnv, app_version: &str)` — no-op unless `$APPIMAGE` is set.

- [ ] **Step 1: Write failing unit tests**

In `src-tauri/src/daemon_control.rs`, add to the `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn sync_copies_when_dest_missing() {
        let env = MockEnv::default();
        let copied = sync_daemon_binary(
            &env,
            Path::new("/mnt/app/asm-cli"),
            Path::new("/home/x/.local/bin/asm-cli"),
            Path::new("/home/x/.local/share/arctis-sound-manager/daemon-version"),
            "0.2.0",
        ).unwrap();
        assert!(copied);
        // marker now records the version
        assert_eq!(
            env.files.borrow().get(Path::new("/home/x/.local/share/arctis-sound-manager/daemon-version")).map(|s| s.as_str()),
            Some("0.2.0")
        );
    }

    #[test]
    fn sync_skips_when_marker_matches_and_dest_exists() {
        let env = MockEnv::default();
        let dest = PathBuf::from("/home/x/.local/bin/asm-cli");
        let marker = PathBuf::from("/home/x/.local/share/arctis-sound-manager/daemon-version");
        env.existing.borrow_mut().insert(dest.clone());
        env.files.borrow_mut().insert(marker.clone(), "0.2.0".to_string());
        let copied = sync_daemon_binary(&env, Path::new("/mnt/app/asm-cli"), &dest, &marker, "0.2.0").unwrap();
        assert!(!copied);
    }

    #[test]
    fn sync_recopies_when_version_changed() {
        let env = MockEnv::default();
        let dest = PathBuf::from("/home/x/.local/bin/asm-cli");
        let marker = PathBuf::from("/home/x/.local/share/arctis-sound-manager/daemon-version");
        env.existing.borrow_mut().insert(dest.clone());
        env.files.borrow_mut().insert(marker.clone(), "0.1.0".to_string());
        let copied = sync_daemon_binary(&env, Path::new("/mnt/app/asm-cli"), &dest, &marker, "0.2.0").unwrap();
        assert!(copied);
    }
```

- [ ] **Step 2: Run them, verify they fail**

Run: `cargo test -p arctis-sound-manager-ui sync_`
Expected: FAIL — `sync_daemon_binary` and `copy_file` don't exist (compile error).

- [ ] **Step 3: Add `copy_file` to the `DaemonEnv` trait**

In the `pub trait DaemonEnv` block, add:

```rust
    /// Copy `src` to `dst` (creating parent dirs), preserving the executable bit.
    fn copy_file(&self, src: &Path, dst: &Path) -> std::io::Result<()>;
```

- [ ] **Step 4: Implement `copy_file` for `RealEnv`**

In `impl DaemonEnv for RealEnv`, add:

```rust
    fn copy_file(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst)?;
        let mut perms = std::fs::metadata(dst)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(dst, perms)
    }
```

- [ ] **Step 5: Implement `copy_file` for `MockEnv`**

In `impl DaemonEnv for MockEnv`, add (records the copy and marks dst present):

```rust
    fn copy_file(&self, _src: &Path, dst: &Path) -> std::io::Result<()> {
        self.existing.borrow_mut().insert(dst.to_path_buf());
        self.files.borrow_mut().insert(dst.to_path_buf(), "<binary>".to_string());
        Ok(())
    }
```

- [ ] **Step 6: Add the pure sync function + the AppImage wrapper**

Place near `candidate_binaries()` in `daemon_control.rs`:

```rust
/// Copy the bundled daemon to a stable path when it is missing or stale.
///
/// `app_version` is the running GUI's version. A marker file records the
/// version last synced; we copy when `dest` is absent OR the marker differs.
/// Returns `Ok(true)` if a copy happened.
pub fn sync_daemon_binary(
    env: &impl DaemonEnv,
    bundled: &Path,
    dest: &Path,
    marker: &Path,
    app_version: &str,
) -> std::io::Result<bool> {
    let current = env.path_exists(dest)
        && env.read_file(marker).map(|v| v.trim() == app_version).unwrap_or(false);
    if current {
        return Ok(false);
    }
    env.copy_file(bundled, dest)?;
    env.write_file_atomic(marker, app_version)?;
    Ok(true)
}

/// When running as an AppImage, copy the bundled `asm-cli` sidecar to the stable
/// `~/.local/bin/asm-cli` path so the systemd unit's `ExecStart` survives updates.
/// No-op for deb/rpm/dev where `asm-cli` is already at a stable path.
pub fn maybe_sync_bundled_daemon(env: &impl DaemonEnv, app_version: &str) {
    if std::env::var_os("APPIMAGE").is_none() {
        return;
    }
    let Some(bundled) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join("asm-cli")))
    else {
        return;
    };
    let home = home_dir();
    let dest = home.join(".local/bin/asm-cli");
    let marker = home.join(".local/share/arctis-sound-manager/daemon-version");
    let _ = sync_daemon_binary(env, &bundled, &dest, &marker, app_version);
}
```

- [ ] **Step 7: Run the tests, verify they pass**

Run: `cargo test -p arctis-sound-manager-ui sync_`
Expected: PASS (3 tests).

- [ ] **Step 8: Call the sync at GUI startup**

In `src-tauri/src/lib.rs`, as the first statement inside `.setup(|app| {`, add:

```rust
            // Make the bundled daemon durable across AppImage updates.
            daemon_control::maybe_sync_bundled_daemon(
                &daemon_control::RealEnv,
                env!("CARGO_PKG_VERSION"),
            );
```

- [ ] **Step 9: Build the UI crate to confirm it compiles**

Run: `cargo build -p arctis-sound-manager-ui`
Expected: builds clean (warnings ok).

- [ ] **Step 10: Commit**

```bash
git add src-tauri/src/daemon_control.rs src-tauri/src/lib.rs
git commit -m "feat(updater): sync bundled asm-cli to ~/.local/bin on AppImage launch"
```

---

### Task 4: Point the updater at GitHub Releases

Replace the placeholder endpoint so the in-app banner actually checks for updates.

**Files:**
- Modify: `src-tauri/tauri.conf.json` (`plugins.updater.endpoints`)

**Interfaces:**
- Produces: the app queries `https://github.com/Chappy202/arctis-sound-manager/releases/latest/download/latest.json` on startup.

- [ ] **Step 1: Replace the placeholder endpoint**

In `src-tauri/tauri.conf.json`, change the `endpoints` array under `plugins.updater` from the `REPLACE-ME.example.com` value to:

```json
      "endpoints": [
        "https://github.com/Chappy202/arctis-sound-manager/releases/latest/download/latest.json"
      ],
```

- [ ] **Step 2: Verify it parses and no placeholder remains**

Run:
```bash
jq -e '.plugins.updater.endpoints[0] | contains("github.com/Chappy202")' src-tauri/tauri.conf.json
! grep -q "REPLACE-ME" src-tauri/tauri.conf.json && echo "no placeholder"
```
Expected: `true`, then `no placeholder`.

- [ ] **Step 3: Commit**

```bash
git add src-tauri/tauri.conf.json
git commit -m "feat(updater): point endpoint at GitHub Releases latest.json"
```

---

### Task 5: Version-assert script (tag == config version)

Prevents publishing a release the updater can't recognize.

**Files:**
- Create: `scripts/check-version.sh`

**Interfaces:**
- Produces: `scripts/check-version.sh vX.Y.Z` exits 0 iff `X.Y.Z` equals `tauri.conf.json.version`, else exits 1 with a message.

- [ ] **Step 1: Write the script**

Create `scripts/check-version.sh`:

```bash
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
```

- [ ] **Step 2: Make executable and test both branches**

Run:
```bash
chmod +x scripts/check-version.sh
./scripts/check-version.sh "v$(jq -r '.version' src-tauri/tauri.conf.json)"   # should pass
./scripts/check-version.sh v999.0.0 ; echo "exit=$?"                          # should fail, exit=1
```
Expected: first prints `Version OK: 0.1.0`; second prints the ERROR and `exit=1`.

- [ ] **Step 3: Commit**

```bash
git add scripts/check-version.sh
git commit -m "build: add release tag/version consistency check"
```

---

### Task 6: Release workflow on `v*` tags

Build the daemon, stage the sidecar, build + sign the bundles, publish to GitHub Releases.

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `scripts/stage-sidecar.sh` (Task 2), `scripts/check-version.sh` (Task 5), CI secret `TAURI_SIGNING_PRIVATE_KEY`.
- Produces: a GitHub Release for the tag with AppImage + `.AppImage.tar.gz` + `.sig` + `latest.json` + deb + rpm attached.

- [ ] **Step 1: Write the workflow**

Create `.github/workflows/release.yml`:

```yaml
name: Release
on:
  push:
    tags: ["v*"]
  workflow_dispatch:

permissions:
  contents: write

jobs:
  release:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Assert tag matches config version
        if: startsWith(github.ref, 'refs/tags/v')
        run: ./scripts/check-version.sh "${GITHUB_REF_NAME}"

      - name: Install system build deps
        run: |
          sudo apt-get update
          sudo apt-get install -y \
            libwebkit2gtk-4.1-dev \
            libsoup-3.0-dev \
            libjavascriptcoregtk-4.1-dev \
            libudev-dev \
            build-essential \
            file

      - uses: dtolnay/rust-toolchain@stable
      - uses: pnpm/action-setup@v4
        with:
          version: 9
      - uses: actions/setup-node@v4
        with:
          node-version: 20
          cache: pnpm

      - name: Install JS deps (root + frontend)
        run: |
          pnpm install --frozen-lockfile
          pnpm --dir frontend install --frozen-lockfile

      - name: Stage asm-cli sidecar
        run: ./scripts/stage-sidecar.sh

      - name: Build, sign, and publish bundles
        uses: tauri-apps/tauri-action@v0
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ""
        with:
          tagName: ${{ github.ref_name }}
          releaseName: "Arctis Sound Manager ${{ github.ref_name }}"
          releaseBody: "See the assets below. The AppImage self-updates via the in-app banner."
          releaseDraft: false
          prerelease: false
```

Notes for the implementer:
- `tauri-action` runs `tauri build` from the repo root (it auto-detects `src-tauri/`), uploads all bundle artifacts **and** the generated `latest.json` to the release, and fails if signing env vars are set but invalid. We stage the sidecar in the prior step so `externalBin` resolves.
- If `tauri-action` cannot find the config, set `with: projectPath: .` — but root detection is the default.

- [ ] **Step 2: Validate the workflow YAML**

Run (no network build, just syntax):
```bash
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release.yml')); print('YAML OK')"
```
Expected: `YAML OK`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci: add tagged-release workflow (build, sign, publish to GitHub Releases)"
```

> **Owner action (not a code step):** before the first tag, run
> `gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.signing/arctis-sound-manager.key`.
> End-to-end verification (real install + OTA) is owner-run per the live-hardware consent rule.

---

### Task 7: AppImage installer script (launcher registration)

Turn the downloaded AppImage into a real launcher app (desktop entry + icon).

**Files:**
- Create: `scripts/install-appimage.sh`

**Interfaces:**
- Produces: `scripts/install-appimage.sh <path-to.AppImage>` installs the AppImage to `~/Applications/`, writes a `.desktop` entry pointing at it, installs the icon, and refreshes the menu.

- [ ] **Step 1: Write the installer**

Create `scripts/install-appimage.sh`:

```bash
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

# Extract the bundled icon (best-effort; falls back to no icon).
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
( cd "$tmp" && "$dest" --appimage-extract '*.png' >/dev/null 2>&1 || true )
icon_src="$(find "$tmp" -name '*.png' -path '*128x128*' | head -n1 || true)"
[[ -z "$icon_src" ]] && icon_src="$(find "$tmp" -name '*.png' | head -n1 || true)"
if [[ -n "$icon_src" ]]; then
  install -m 0644 "$icon_src" "${icon_dir}/arctis-sound-manager.png"
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
```

- [ ] **Step 2: Make executable and shellcheck-parse it**

Run:
```bash
chmod +x scripts/install-appimage.sh
bash -n scripts/install-appimage.sh && echo "syntax OK"
./scripts/install-appimage.sh 2>&1 | grep -q usage && echo "usage guard OK"
```
Expected: `syntax OK` then `usage guard OK`.

- [ ] **Step 3: Commit**

```bash
git add scripts/install-appimage.sh
git commit -m "feat(install): AppImage launcher-registration script"
```

---

### Task 8: Rewrite `docs/PACKAGING.md` to match reality

The current doc claims the key "already exists", shows the placeholder endpoint, and predates the sidecar + workflow. Bring it in line.

**Files:**
- Modify: `docs/PACKAGING.md`

**Interfaces:** Documentation only — no code consumes it.

- [ ] **Step 1: Update the signing section**

Replace the "Signing keypair — OWNER-ONLY" section's generation note with the actual state: keypair generated 2026-06-29 (empty passphrase), private key at `~/.signing/arctis-sound-manager.key` (mode 600), new pubkey committed (`4aa5f8c`). CI secret command:

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.signing/arctis-sound-manager.key
```
State that `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` is unset (empty passphrase).

- [ ] **Step 2: Update the endpoint section**

Replace the `REPLACE-ME`/`YOUR-UPDATE-HOST` text with the committed real endpoint
`https://github.com/Chappy202/arctis-sound-manager/releases/latest/download/latest.json`
and note that the GitHub Releases pipeline uploads `latest.json` automatically.

- [ ] **Step 3: Add a "Releasing" section**

Document the actual release flow:

```md
## Releasing

1. Bump `version` in `src-tauri/tauri.conf.json`.
2. Commit, then tag: `git tag vX.Y.Z && git push origin vX.Y.Z`.
3. `.github/workflows/release.yml` builds, signs, and publishes to GitHub Releases.
   (Requires the `TAURI_SIGNING_PRIVATE_KEY` secret to be set once.)
4. Users on an older AppImage see the in-app update banner on next launch.
```

- [ ] **Step 4: Add a "Daemon bundling" note**

Document that `asm-cli` ships as a Tauri `externalBin` sidecar (staged by
`scripts/stage-sidecar.sh`); for AppImage installs the GUI copies it to
`~/.local/bin/asm-cli` on launch (so the systemd `ExecStart` is durable); for
deb/rpm it installs to `/usr/bin/asm-cli`.

- [ ] **Step 5: Add the AppImage install path**

Document `scripts/install-appimage.sh <file>` as the one-step launcher
registration, followed by `asm-cli setup-udev`.

- [ ] **Step 6: Verify no stale placeholders remain**

Run:
```bash
! grep -Eq "REPLACE-ME|YOUR-UPDATE-HOST|already done — DO NOT regenerate" docs/PACKAGING.md && echo "clean"
```
Expected: `clean`.

- [ ] **Step 7: Commit**

```bash
git add docs/PACKAGING.md
git commit -m "docs(packaging): align with sidecar bundling, real endpoint, release flow"
```

---

## Self-Review

**Spec coverage** (against `2026-06-29-distribution-and-auto-update-design.md`):
- §4 Release workflow → Task 6 ✓
- §5 Daemon bundling (sidecar + sync) → Tasks 2, 3 ✓
- §6 Updater endpoint + pubkey → Task 4 (endpoint); pubkey already done in `4aa5f8c` ✓
- §7 Install/first-run UX → Task 7; banner is pre-existing (verify in E2E) ✓
- §8 Version single-source → Task 5 (assert) + Task 1 (`--version`) ✓
- §10 Testing → unit tests in Tasks 1, 3; version-assert in Task 5; E2E is owner-run (noted in Task 6) ✓
- §12 Deliverables checklist → all items map to Tasks 1–8 ✓

**Placeholder scan:** No "TBD/TODO/handle edge cases" — every code/script step shows full content. ✓

**Type consistency:** `sync_daemon_binary(env, bundled, dest, marker, app_version)` and `maybe_sync_bundled_daemon(env, app_version)` are used identically where referenced; `copy_file` added to trait + both impls (`RealEnv`, `MockEnv`); `candidate_binaries()`/`home_dir()` reused, not redefined. ✓

**Note:** Tasks 1–8 are largely independent and touch files disjoint from the concurrent engine work (`crates/engine/`). Task order 1→8 is recommended but only Task 6 hard-depends on Tasks 2 & 5 (the scripts it calls).
