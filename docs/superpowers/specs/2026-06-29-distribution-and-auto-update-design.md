# Distribution & Auto-Update — Design

**Date:** 2026-06-29
**Status:** Approved (design), pending implementation plan
**Owner decisions:** AppImage + in-app OTA updater · GitHub Releases host · fresh minisign keypair · keep deb/rpm as bonus artifacts · "update available" banner UX

---

## 1. Goal

Make Arctis Sound Manager installable as a real desktop app on the owner's
Nobara (Fedora 43) machine, and updateable with one click as development
continues. "Done" means:

1. A tagged commit (`vX.Y.Z`) produces signed, published release artifacts with
   no manual build steps.
2. The owner installs once (an AppImage that registers itself as a launcher app)
   and thereafter updates by clicking an in-app banner.
3. A fresh install has a working daemon, udev access, and optional autostart —
   nothing silently missing.

This is **gap-closing, not greenfield.** The Tauri updater, bundle targets,
committed pubkey, `packaging/` files, the GUI daemon-lifecycle controller, and
the frontend update banner already exist. This spec wires them into a working
end-to-end pipeline.

### Non-goals

- No Flatpak/Snap (ARCHITECTURE G9 — sandbox blocks `hidraw` + PipeWire rules).
- No dnf repo / COPR hosting (deb/rpm are emitted but not an update channel).
- No multi-arch (x86_64 only; the target machine is x86_64).
- No code-signing for Windows/macOS (Linux-only project).

---

## 2. Why AppImage enables all low-level functionality

An AppImage is **not a sandbox** — unlike Flatpak. It is a self-mounting
squashfs that `exec`s the binary as an ordinary, unconfined user process with
the same privileges a `dnf`-installed binary would have:

| Capability | Works | Mechanism |
|---|---|---|
| `/dev/hidraw*` HID writes | ✅ | udev `uaccess` ACL applies to the user process identically |
| PipeWire sinks / routing | ✅ | Full PipeWire socket + D-Bus access |
| `pw-cli`/`pw-record`/`pw-metadata`/`wpctl` subprocesses | ✅ | AppImage does not rewrite `$PATH`; host tools resolve normally |
| udev rule install via `pkexec` | ✅ | `asm-cli setup-udev` shells to polkit unchanged |
| systemd **user** unit autostart | ✅ | with the daemon-path handling in §5 |
| LADSPA mic plugins | ✅ | loaded by host PipeWire, not the app |

The **only** AppImage-specific constraint is that the internal mount path
(`/tmp/.mount_*`) is ephemeral, so a systemd `ExecStart=` cannot point inside
it. §5 handles this.

---

## 3. Components & boundaries

Five independent units, each separately testable/verifiable:

| Unit | Responsibility | Interface |
|---|---|---|
| **A. Release workflow** | Build + sign + publish on tag | `.github/workflows/release.yml`, triggered by `v*` tags |
| **B. Updater endpoint config** | Point the app at GitHub Releases; rotate pubkey | `src-tauri/tauri.conf.json` |
| **C. Daemon bundling** | Ship `asm-cli` inside every artifact; install to a stable path | Tauri `externalBin` sidecar + a GUI "sync daemon binary" step |
| **D. Install/first-run UX** | Make the AppImage a launcher app; udev + autostart | `install.sh`, existing `setup-udev`, existing autostart |
| **E. Version single-source** | One version drives bundle + updater `current_version` | build-time wiring |

Units A, B, D, E are largely config/CI and touch files the feature agents don't
(`.github/`, `tauri.conf.json`, `packaging/`, `docs/`). Unit C is the only one
touching Rust (`src-tauri`, possibly a tiny `crates/cli` addition) — scoped away
from `crates/engine`.

---

## 4. Unit A — Release workflow

New `.github/workflows/release.yml`, separate from the existing test-only
`ci.yml`.

**Trigger:** push of a tag matching `v*` (e.g. `v0.2.0`). Manual
`workflow_dispatch` also allowed for dry runs.

**Runner:** `ubuntu-latest` (Tauri's AppImage tooling targets a broad glibc;
building on an older Ubuntu maximizes AppImage portability — acceptable here
since the only consumer is Nobara 43, which is newer than the runner).

**Steps:**
1. Checkout.
2. Install build deps: `libwebkit2gtk-4.1-dev`, `libudev-dev`,
   `libsoup-3.0-dev`, `libjavascriptcoregtk-4.1-dev`, `build-essential`,
   plus AppImage tooling pulled by Tauri.
3. Rust stable toolchain; Node + `pnpm`.
4. `pnpm install` (root + `frontend/`).
5. Build the daemon: `cargo build --release -p arctis-cli` (produces `asm-cli`),
   then stage it where the sidecar config (§5) expects it.
6. `pnpm tauri build` — emits AppImage (+ `.AppImage.tar.gz` + `.sig`),
   deb, rpm, and `latest.json`, with the AppImage updater artifact signed via
   the injected key.
7. Publish all artifacts to the GitHub Release for the tag, using the built-in
   `GITHUB_TOKEN` (e.g. `softprops/action-gh-release` or `gh release upload`).

**Secrets (owner-provided, repo settings → Actions secrets):**
- `TAURI_SIGNING_PRIVATE_KEY` — contents of the fresh private key (§6).
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — passphrase, or empty.

**Failure surfacing:** the workflow fails loudly if signing env vars are
missing or the bundle step omits the updater artifact — never publish an
unsigned/un-updatable release.

---

## 5. Unit C — Daemon (`asm-cli`) bundling

**Problem:** every bundle today ships only the GUI binary
(`arctis-sound-manager-ui`). A fresh install therefore has no `asm-cli`, so the
GUI's daemon controller finds no binary and the app is inert.

**Approach — Tauri `externalBin` sidecar:**
- Add `bundle.externalBin: ["binaries/asm-cli"]` (or equivalent) to
  `tauri.conf.json`. Tauri requires the sidecar to exist at
  `binaries/asm-cli-<target-triple>` at build time; the release workflow stages
  the `cargo build` output there.
- Tauri then places `asm-cli` next to the GUI binary inside every artifact
  (AppImage AppDir root, `/usr/bin` for deb/rpm).

**Daemon path resolution (already supported by `daemon_control.rs`):**
- **Manual spawn** (no autostart): `candidate_binaries()` already checks the
  *sibling of the current exe* — so inside the AppImage, the GUI spawns the
  co-located `asm-cli` directly. Works out of the box once the sidecar exists.
- **systemd autostart** (stable path needed): on "enable autostart", the GUI
  copies the resolved `asm-cli` to `~/.local/bin/asm-cli` and renders the unit's
  `ExecStart` to that stable path (`install_autostart()` + `render_unit()`
  already do this with whatever path they're handed). New work: a small
  "sync the daemon binary to `~/.local/bin`" step that (a) runs before
  autostart install and (b) re-runs after an OTA update if the bundled
  `asm-cli` version differs from the installed copy.

**deb/rpm path:** packages install `asm-cli` to `/usr/bin/asm-cli`
(candidate #4), and ship the udev rule, `.desktop`, and systemd unit via package
data/scriptlets — these are native installs and need no copy-out step.

---

## 6. Unit B — Updater endpoint + fresh keypair

**Endpoint:** replace the `REPLACE-ME.example.com` placeholder in
`tauri.conf.json` with the GitHub Releases "latest" URL:

```
https://github.com/Chappy202/arctis-sound-manager/releases/latest/download/latest.json
```

Tauri substitutes `{{target}}`/`{{arch}}`/`{{current_version}}`; with the
`releases/latest/download/` form the manifest is always the newest published
`latest.json`, and it contains the absolute artifact URLs Tauri downloads. The
endpoint returns 200 with the manifest (update available) or the client
compares versions and no-ops when current.

**Fresh keypair (owner action, documented in `PACKAGING.md`):**
```sh
pnpm tauri signer generate -w ~/.signing/arctis-sound-manager.key
```
- Commit the new **public** key into `tauri.conf.json` `plugins.updater.pubkey`,
  replacing the existing one.
- Put the **private** key + password into the two CI secrets (§4).
- The private key never enters the repo. (No existing installs, so rotating the
  pubkey breaks nothing.)

---

## 7. Unit D — Install & first-run UX

**Banner (already implemented):** `frontend/src/lib/updater.ts` +
`App.svelte` already do a non-blocking startup `check()`, store
`pendingUpdate`, and expose `installUpdate()` (`downloadAndInstall` →
relaunch). Work here is **verification** that the banner renders and installs
against a real signed release — not new UI.

**AppImage → launcher app:** ship a small `install.sh` (and document the manual
steps) that:
- Moves the AppImage to a stable location (`~/Applications/`),
- Installs `packaging/arctis-sound-manager.desktop` to
  `~/.local/share/applications/` with `Exec=` pointing at the stable AppImage
  path, plus the icon,
- Runs `update-desktop-database`.

So it appears in the app launcher like any installed app. (deb/rpm do this via
package scriptlets automatically.)

**udev + autostart:** unchanged — `asm-cli setup-udev` (pkexec) for the rule;
GUI's existing autostart toggle for the systemd user unit.

---

## 8. Unit E — Version single source of truth

Today versions disagree: `tauri.conf.json` = `0.1.0`, `frontend/package.json`
= `0.0.0`, `arctis_config::CURRENT_VERSION` is a *config-schema* version (not the
app version). The updater compares the running app's version
(`tauri.conf.json`) against the manifest, so that value must be authoritative
and match the git tag.

**Decision:** `tauri.conf.json.version` is the single app version. The release
workflow asserts the tag (`vX.Y.Z`) equals `tauri.conf.json.version` and fails
on mismatch (prevents publishing a release the updater can't recognize).
`asm-cli` gains `#[command(version)]` (reports `CARGO_PKG_VERSION`) for
diagnostics; aligning crate versions to the app version is a nice-to-have, not
required for OTA correctness. `frontend/package.json` version is cosmetic and
left out of the OTA path.

---

## 9. Data flow — a release and an update

**Release:** owner bumps `tauri.conf.json.version` → commits → tags `vX.Y.Z` →
pushes tag → workflow builds, signs, and publishes AppImage + sidecar +
`latest.json` (+ deb/rpm) to the GitHub Release.

**Update (owner's machine):** GUI startup → `check()` hits the latest-release
`latest.json` → newer version + valid minisign signature → banner appears →
owner clicks Install → signed `.AppImage.tar.gz` downloaded, verified against
committed pubkey, AppImage replaced in place, app relaunches → on next launch
the GUI re-syncs `~/.local/bin/asm-cli` if the bundled daemon version changed.

**Signature failure** at any point → update rejected, nothing unsigned installed.

---

## 10. Testing & verification

- **Unit:** daemon-binary-sync logic (version compare + copy) gets unit tests in
  the `DaemonEnv` seam style already used in `daemon_control.rs` (mockable fs).
- **Version-assert:** a workflow step (and a local script) verifying tag ==
  `tauri.conf.json.version`.
- **End-to-end (owner-run, real hardware/host):**
  1. Tag a `v0.1.1` test release; confirm artifacts publish.
  2. Install `v0.1.0` AppImage; confirm it launches, daemon starts, headset is
     controllable.
  3. With `v0.1.1` published, confirm the banner appears and one-click update
     succeeds and relaunches into `0.1.1`.
  These are gated behind owner consent per the project's
  no-unattended-writes/live-hardware rules.

---

## 11. Risks & mitigations

| Risk | Mitigation |
|---|---|
| glibc/webkit mismatch makes AppImage non-portable | Build on `ubuntu-latest` (older glibc than Nobara); the only target is newer, so forward-compat holds. Verify on the owner's machine before relying on it. |
| Sidecar target-triple naming wrong → bundle fails | Stage `asm-cli-<triple>` exactly; assert presence before `tauri build`. |
| Updater silently no-ops (placeholder URL, bad pubkey) | Endpoint + fresh pubkey are explicit deliverables; E2E test confirms a real update lands. |
| systemd unit points into ephemeral AppImage mount | Copy-out to `~/.local/bin` (§5) — never reference the mount path. |
| Signing secret missing in CI | Workflow fails loudly rather than publishing an unsigned release. |

---

## 12. Deliverables checklist

- [ ] `.github/workflows/release.yml` (build + sign + publish on `v*`)
- [ ] `tauri.conf.json`: real updater endpoint + fresh committed pubkey + `externalBin`
- [ ] Daemon sidecar staging in the workflow + GUI `~/.local/bin` sync step (+ unit tests)
- [ ] `asm-cli --version` flag
- [ ] Workflow version-assert (tag == config version)
- [ ] `install.sh` + launcher/desktop registration for the AppImage
- [ ] `docs/PACKAGING.md` rewritten to match reality (key generation, secrets, endpoint, runbook)
- [ ] Owner action items documented: generate keypair, add 2 CI secrets, first tagged release
