# Arctis Sound Manager

A from-scratch Linux desktop app and headless engine to manage SteelSeries Arctis headsets
(primary target: Arctis Nova Pro Wireless, 1038:12e5): per-app audio routing, per-channel
parametric EQ, mic DSP chain, virtual surround/HRIR, and headset hardware control via
PipeWire + hidraw.

---

## Status

What works today:

- Engine/daemon, `asm-cli` CLI, Sonar-style Tauri v2 GUI scaffold
- Multi-channel software routing: Game/Chat/Media virtual PipeWire sinks
- Per-channel parametric EQ (10 bands, live, no restart)
- Channel volume (dB) and mute; per-channel output device pinning
- Channel add/remove; route set/clear/list with live WirePlumber rules. Routes re-apply
  automatically when an app's stream reappears (idle → resume) with the optional `pw-watcher`
  build feature (see Build, step 7)
- Profiles: list/show/switch/new/save/rename/delete/export/import; EQ presets
- Mic DSP chain (Clean Mic virtual source): gain, highpass, suppression, compressor, gate, EQ
  — DeepFilterNet default suppressor; RNNoise fallback; all stages opt-in
- Virtual surround/HRIR via PipeWire convolver with HeSuVi .wav profiles
- Device live reads: battery, ANC state, ChatMix dial, mic-mute flag
- Dial-to-balance mapping; real signal peak meters in the GUI
- Coexistence teardown of the legacy arctis-sound-manager RPM stack

Gated / owner-run:

- Device writes (sidetone, mic LED, ANC mode, auto-off, etc.) are dispatched through the
  daemon but are refused by a gate layer until each control is validated on real hardware.
- Audible mic DSP end-to-end validation is owner-run (hardware required).
- OTA updater needs a signed hosting endpoint configured; see [docs/PACKAGING.md](docs/PACKAGING.md).

Out of scope by design: OLED writes, streamer mode.

---

## QUICK START — Fedora / Nobara

These steps assume a clean Nobara 43 (or Fedora 40+) machine with no Rust toolchain yet.
Run them in order.

### 1. Build dependencies

```sh
# libudev (for the hidapi C backend) + C compiler (gcc or clang)
sudo dnf install systemd-devel gcc

# Rust toolchain — either via rustup (recommended) or the distro package
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# After rustup: open a new shell or:
source "$HOME/.cargo/env"

# Alternatively, the distro package (may lag behind MSRV):
# sudo dnf install rust cargo
```

Minimum Rust version: **1.78** (enforced in `Cargo.toml`).

### 2. GUI build dependencies (skip if using the CLI only)

```sh
sudo dnf install webkit2gtk4.1-devel libsoup3-devel javascriptcoregtk4.1-devel openssl-devel
```

Node + pnpm for the frontend:

```sh
sudo dnf install nodejs
sudo npm install -g pnpm
```

### 3. Runtime audio

On Nobara 43 these are installed by default. Verify:

```sh
systemctl --user status pipewire wireplumber
```

If missing:

```sh
sudo dnf install pipewire wireplumber pipewire-utils
```

`pipewire-utils` provides `pw-cli`, `pw-metadata`, and `pw-record`, all of which the daemon
calls as subprocesses. The engine pins all nodes at **48 kHz**; the system must be running
at 48 kHz (Nobara default).

### 4. Optional mic/surround plugins (recommended)

All plugins are auto-detected at runtime. If a plugin is missing the app reports it and
disables that stage — nothing crashes. Plugin `.so` files are searched in order:
`$LADSPA_PATH` (colon-separated), `/usr/lib64/ladspa`, `/usr/lib/ladspa`,
`/usr/lib/x86_64-linux-gnu/ladspa`.

To verify a plugin after install:

```sh
analyseplugin /usr/lib64/ladspa/<file>.so
# package: ladspa on Fedora/Nobara, ladspa-sdk on Debian/Ubuntu
sudo dnf install ladspa
```

**DeepFilterNet** — default noise suppressor (recommended; has an attenuation cap knob):

```sh
sudo dnf copr enable mavit/DeepFilterNet
sudo dnf install deep-filter-ladspa
# installs: /usr/lib64/ladspa/libdeep_filter_ladspa.so
```

**swh-plugins** — provides the `sc4m_1916` compressor stage and `gate_1410` gate fallback
for PipeWire < 1.6 (PipeWire >= 1.6 uses the builtin `noisegate` for the gate stage):

```sh
sudo dnf install ladspa-swh-plugins
```

**RNNoise** — lighter fallback suppressor; select with `asm-cli mic backend rnnoise`:

```sh
sudo dnf copr enable lkiesow/noise-suppression-for-voice
sudo dnf install ladspa-realtime-noise-suppression-plugin
```

### 5. Surround HRIRs (optional)

The surround feature convolves audio with HeSuVi-format HRIR `.wav` profiles. Drop profiles into:

```
~/.local/share/pipewire/hrir_hesuvi/profiles/
```

The daemon enumerates whatever is there; `asm-cli surround hrir list` shows them.
Source: [HeSuVi HRIR collection](https://sourceforge.net/projects/hesuvi/) or any HeSuVi-compatible `.wav`.

### 6. Device access (udev rule, one-time)

The headset's hidraw node is root-only by default. Install the shipped rule:

```sh
# Automatic (uses pkexec — prompts for authentication):
cargo run -p arctis-cli -- setup-udev

# Or manual:
sudo cp packaging/udev/70-arctis-sound-manager.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then replug or power-cycle the headset. Verify:

```sh
cargo run -p arctis-cli -- list
# expected: found: Arctis Nova Pro Wireless (1038:12e5) on interface N
```

Without this rule, `list` and `probe` will fail with a permission error on `/dev/hidraw*`.

### 7. Build

```sh
cargo build --workspace
```

Tests (unit + non-hardware integration):

```sh
cargo test --workspace
```

Live-PipeWire and real-hardware tests are out of the default CI gate.

#### Optional: the `pw-watcher` feature (auto re-apply routes on stream resume)

By default a remembered route is applied to an app's current stream once. When the app goes
idle PipeWire destroys that stream, so on resume it can fall back to the default sink. The
optional **`pw-watcher`** feature runs a `pipewire-rs` registry listener on a dedicated thread
that re-applies remembered routes by app binary whenever a stream (re)appears — so routes stick
across idle/resume. It is **off by default** because it links libpipewire and needs extra build
deps:

```sh
# Fedora / Nobara — bindgen (libspa-sys/pipewire-sys) needs the clang resource headers,
# and pkg-config needs the PipeWire dev headers:
sudo dnf install pipewire-devel clang

cargo build -p arctis-cli --release --features pw-watcher
```

Then restart the daemon from this binary. Without the feature, routing still works on explicit
moves; only the automatic re-apply-on-resume is inactive (it compiles to a no-op stub).

### 8. Start the daemon

The daemon must be running before the GUI and before any command that writes to PipeWire or
the device (mic, surround, volume, mute, device writes, channel add/remove).

```sh
cargo run -p arctis-cli -- daemon
# Listens on: $XDG_RUNTIME_DIR/arctis-sound-manager.sock
# Runs in the foreground; use a terminal multiplexer or the systemd unit below.
```

To run it as a persistent user service:

```sh
mkdir -p ~/.config/systemd/user/
cp packaging/systemd/arctis-sound-manager.service ~/.config/systemd/user/
# Edit ExecStart to point to the built binary, e.g.:
# ExecStart=/home/<you>/.../target/debug/asm-cli daemon
systemctl --user daemon-reload
systemctl --user enable --now arctis-sound-manager.service

# Status and logs:
systemctl --user status arctis-sound-manager.service
journalctl --user -u arctis-sound-manager.service -f
```

### 9. Run the GUI

Run the Tauri CLI from the **repo root** (not `frontend/`): `tauri.conf.json` lives in
`src-tauri/`, so the CLI must be invoked where `src-tauri/` is a subfolder. One-time install
of the two dependency sets:

```sh
pnpm install                 # repo root — installs the Tauri CLI (for `pnpm tauri`)
pnpm --dir frontend install  # the Svelte frontend's deps (vite, svelte, ...)
```

Then launch from the repo root (the daemon from step 8 must be running):

```sh
pnpm gui     # = WEBKIT_DISABLE_DMABUF_RENDERER=1 tauri dev
```

`pnpm gui` sets `WEBKIT_DISABLE_DMABUF_RENDERER=1`, which avoids a `Failed to create GBM
buffer` / blank-window failure on NVIDIA GPUs (harmless elsewhere). The plain form is
`pnpm tauri dev` (add the env var yourself on NVIDIA). Do **not** `cd frontend` first —
the CLI won't find the project there.

(The `.desktop` file applies the same flag automatically once installed via
`cp packaging/arctis-sound-manager.desktop ~/.local/share/applications/`.)

### 10. First use

```sh
# Bring the virtual channels up:
cargo run -p arctis-cli -- channels up

# Route an app to the Game channel:
cargo run -p arctis-cli -- route set firefox game

# Enable the mic DSP chain with DeepFilterNet:
cargo run -p arctis-cli -- mic on
cargo run -p arctis-cli -- mic enable suppression
cargo run -p arctis-cli -- mic backend deep_filter
cargo run -p arctis-cli -- mic set attenuation_limit_db 40
cargo run -p arctis-cli -- mic status

# Enable virtual surround on the Game channel:
cargo run -p arctis-cli -- surround on
cargo run -p arctis-cli -- surround channels game
cargo run -p arctis-cli -- surround hrir list
cargo run -p arctis-cli -- surround hrir set 02-dh-dolby-headphone
```

---

## Other distros

| Dependency | Fedora / Nobara | Debian / Ubuntu | Arch |
|---|---|---|---|
| Rust toolchain | `rustup` or `sudo dnf install rust cargo` | `rustup` or `sudo apt install rustc cargo` | `rustup` or `sudo pacman -S rust` |
| libudev + C compiler | `sudo dnf install systemd-devel gcc` | `sudo apt install libudev-dev gcc` | `sudo pacman -S systemd-libs gcc` |
| WebKitGTK (GUI) | `sudo dnf install webkit2gtk4.1-devel libsoup3-devel javascriptcoregtk4.1-devel openssl-devel` | `sudo apt install libwebkit2gtk-4.1-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev libssl-dev` | `sudo pacman -S webkit2gtk-4.1` |
| PipeWire + WirePlumber | `sudo dnf install pipewire wireplumber pipewire-utils` | `sudo apt install pipewire wireplumber pipewire-bin` | `sudo pacman -S pipewire wireplumber` |
| `pw-watcher` feature (optional, build-time) | `sudo dnf install pipewire-devel clang` | `sudo apt install libpipewire-0.3-dev clang` | `sudo pacman -S pipewire clang` |
| DeepFilterNet | `sudo dnf copr enable mavit/DeepFilterNet && sudo dnf install deep-filter-ladspa` | Download `.so` from [github.com/Rikorose/DeepFilterNet/releases](https://github.com/Rikorose/DeepFilterNet/releases) | AUR: `yay -S libdeep_filter_ladspa-bin` |
| RNNoise | `sudo dnf copr enable lkiesow/noise-suppression-for-voice && sudo dnf install ladspa-realtime-noise-suppression-plugin` | `sudo apt install noise-suppression-for-voice` | AUR: `yay -S noise-suppression-for-voice` |
| swh-plugins | `sudo dnf install ladspa-swh-plugins` | `sudo apt install swh-plugins` | `sudo pacman -S swh-plugins` |
| LADSPA tools (analyseplugin) | `sudo dnf install ladspa` | `sudo apt install ladspa-sdk` | `sudo pacman -S ladspa` |

LADSPA plugin search paths: `$LADSPA_PATH`, `/usr/lib64/ladspa` (Fedora),
`/usr/lib/ladspa`, `/usr/lib/x86_64-linux-gnu/ladspa` (Debian/Ubuntu).

---

## Using the CLI (asm-cli)

From a source checkout: `cargo run -p arctis-cli -- <command> [args]`
Installed binary name: `asm-cli`

Commands that touch PipeWire or the device require the daemon to be running.

### Device discovery

```sh
asm-cli list                    # list connected, recognized SteelSeries devices
asm-cli probe                   # read device status (battery, ANC, mic, ChatMix) — read-only
```

### Channels and volume

```sh
asm-cli channels up [--target <node.name>]   # create Game/Chat/Media virtual sinks (idempotent)
asm-cli channels down                        # remove them (idempotent)
asm-cli channels add aux                     # add a custom channel
asm-cli channels remove aux                  # remove it (any channel may be removed; min 1 remains)

asm-cli channel volume game -6.0             # set Game channel volume to -6 dB (−60..+6)
asm-cli channel volume chat 0                # unity gain
asm-cli channel mute media on               # mute the Media channel
asm-cli channel mute media off              # unmute
asm-cli channel output set game alsa_output.usb-SteelSeries_Arctis.stereo-fallback
                                             # pin Game to a specific hardware sink
asm-cli channel output set game default      # clear the pin
```

### Routing

```sh
asm-cli route set firefox game              # route Firefox (by process binary) to Game channel
asm-cli route set "Firefox" game --by-name  # match by application.name instead
asm-cli route list                          # print all persistent rules
asm-cli route clear firefox                 # remove rule and move stream back to default
```

### EQ

```sh
asm-cli eq set --band 3 --freq 1200 --q 1.0 --gain -6 [--kind peaking|lowshelf|highshelf]
asm-cli eq show                             # confirm the EQ sink node is present

# EQ presets
asm-cli eq preset save bass-boost --channel game
asm-cli eq preset list
asm-cli eq preset apply bass-boost --channel game
asm-cli eq preset delete bass-boost
```

### Profiles

```sh
asm-cli profile list                        # list profiles (* = active)
asm-cli profile show                        # show active profile
asm-cli profile show gaming                 # show a named profile
asm-cli profile new gaming                  # create from a copy of the active profile
asm-cli profile switch gaming
asm-cli profile save                        # persist in-memory config to disk
asm-cli profile rename gaming competitive
asm-cli profile delete competitive          # cannot delete active or last profile
asm-cli profile export gaming --out gaming.toml
asm-cli profile import gaming.toml
asm-cli apply                               # reconcile the live PipeWire graph to the active profile
```

### Mic DSP chain

The Clean Mic feature creates a virtual PipeWire source with opt-in DSP stages. Apps select
it as their capture input.

```sh
asm-cli mic on                              # master switch on
asm-cli mic off                             # master switch off
asm-cli mic status                          # show full chain state

# Enable / disable stages
asm-cli mic enable  <gain|highpass|suppression|compressor|gate|eq>
asm-cli mic disable <gain|highpass|suppression|compressor|gate|eq>

# Select suppression backend
asm-cli mic backend deep_filter             # DeepFilterNet (default)
asm-cli mic backend rnnoise                 # RNNoise (lighter)

# Set parameters live (no restart)
asm-cli mic set gain_db 6
asm-cli mic set highpass_freq 120
asm-cli mic set attenuation_limit_db 40     # DeepFilterNet: max suppression cap (0–100 dB)
asm-cli mic set vad_threshold 50            # RNNoise: VAD threshold %
asm-cli mic set vad_grace_ms 200            # RNNoise: VAD grace period
asm-cli mic set vad_retro_grace_ms 100      # RNNoise: retroactive grace
asm-cli mic set gate_threshold 0.02         # Gate open threshold (linear, 0–0.5)
asm-cli mic set comp_threshold_db -18       # Compressor threshold dB
asm-cli mic set comp_ratio 4                # Compressor ratio
asm-cli mic set comp_makeup_db 6            # Compressor makeup gain

# Mic EQ band (live)
asm-cli mic eq --band 2 --freq 1000 --q 1.0 --gain -3.0 [--kind peaking|lowshelf|highshelf]

# Pin the hardware capture source
asm-cli mic hw-mic alsa_input.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.mono-fallback
asm-cli mic hw-mic                          # clear the pin (follow WirePlumber default)
```

Chain order (when enabled): gain → highpass → suppression → compressor → gate → EQ.

DeepFilterNet's `attenuation_limit_db` caps how much it may suppress. Setting it to ~40 dB
instead of the 100 dB maximum reduces the over-suppressed "tinny" effect that aggressive
noise reduction can cause. RNNoise has no such cap.

### Virtual surround / HRIR

```sh
asm-cli surround on
asm-cli surround off
asm-cli surround status                     # enabled, active HRIR, channels, hw_sink
asm-cli surround hrir list                  # list .wav profiles in hrir_hesuvi/profiles/
asm-cli surround hrir set 02-dh-dolby-headphone
asm-cli surround channels game,media        # comma-separated channel ids to route through surround
asm-cli surround hw-sink alsa_output.usb-...  # pin output to a specific sink
asm-cli surround hw-sink                    # clear pin (auto-detect)
```

HRIR profiles live in `~/.local/share/pipewire/hrir_hesuvi/profiles/`. Drop HeSuVi `.wav`
files there; the daemon picks them up without a restart.

### Device hardware controls

All device reads are safe and used without gating. Writes are routed through the daemon's
HID serializer and are refused by a gate layer until each control is validated on real hardware.

```sh
asm-cli device status                       # live read: battery, ANC, dial, mic-mute (daemon or direct)
asm-cli device sidetone <0..3>
asm-cli device mic-led <1..10>
asm-cli device anc <off|transparency|on>
asm-cli device auto-off <0..6>              # 0=never, 1=1min, 2=5min, 3=10min, 4=15min, 5=30min, 6=60min
asm-cli device transparency <1..10>
asm-cli device mic-volume <1..10>
asm-cli device set <control> <value>        # generic escape hatch
```

### Daemon

```sh
asm-cli daemon                              # start the resident daemon (foreground)
# Socket: $XDG_RUNTIME_DIR/arctis-sound-manager.sock
```

### udev setup

```sh
asm-cli setup-udev                          # install udev rule via pkexec (prompts for auth)
asm-cli setup-udev --dry-run               # preview only
```

### Coexistence with the legacy RPM

```sh
asm-cli coexist status                      # detect legacy arctis-sound-manager stack
asm-cli coexist disable                     # stop+disable legacy services, destroy live nodes
asm-cli coexist disable --dry-run           # preview actions without executing
```

To fully remove the legacy package: `sudo dnf remove arctis-sound-manager`.

---

## Architecture

Rust Cargo workspace (engine-first, UI-agnostic) + Tauri v2 GUI
(`identifier: com.oxibux.arctis-sound-manager`).

Crates: `domain` (pure types), `device` (HID codec + registry), `audio` (PipeWire virtual
sinks, EQ, routing), `config` (schema-versioned store), `engine` (orchestrator), `client`
(daemon IPC), `cli` (`asm-cli`); `src-tauri` + `frontend` (Tauri shell).

HID access uses `hidapi` with the C `linux-static-hidraw` backend (requires libudev + a C
compiler at build time). The pure-Rust `linux-native` backend does not enumerate the Nova
Pro Wireless. Audio is driven via PipeWire subprocess at 48 kHz throughout — no resampling.

Not Flatpak: the sandbox blocks `hidraw` access and PipeWire routing rules. AppImage is
the primary portable artifact.

See [ARCHITECTURE.md](ARCHITECTURE.md) for diagrams and binding guardrails (G1–G10).
See [DESIGN.md](DESIGN.md) for the Sonar-style visual design system.
See [docs/PACKAGING.md](docs/PACKAGING.md) for AppImage/deb/rpm build, OTA signing, and hosting.

---

## Safety

Device writes go through a single serialized HID writer and ship disabled by default until
each control is validated against real hardware. The headset OLED is never written. Reads are
safe and used without gating. Write failures are surfaced explicitly — USB errors are never
silently swallowed.

---

## Troubleshooting

**GUI shows disconnected / commands fail with "daemon is not running"**
Start the daemon: `cargo run -p arctis-cli -- daemon` (or enable the systemd unit).

**`asm-cli list` finds no device or returns a permission error on /dev/hidraw***
Install the udev rule (`asm-cli setup-udev` or manual copy), then replug the headset.

**A mic or surround stage shows "unavailable" in `asm-cli mic status` or `surround status`**
Install the required LADSPA plugin (see section 4 above). The app degrades gracefully —
other stages still function.

**GUI: "Couldn't recognize the current folder as a Tauri project"**
You ran the Tauri CLI from `frontend/`. Run it from the **repo root** instead (where `src-tauri/`
is a subfolder): `pnpm gui` (or `pnpm tauri dev`) from the repo root. See section 9.

**NVIDIA blank window / "Failed to create GBM buffer ... Invalid argument"**
WebKitGTK's DMABUF renderer fails on some NVIDIA setups. Launch with the env var:
`WEBKIT_DISABLE_DMABUF_RENDERER=1 pnpm tauri dev` — `pnpm gui` already sets it.

**Gate refused on a device write**
The control has not been validated on hardware yet. Read `asm-cli device status` to confirm
the device is connected; writes are intentionally blocked until per-control validation is done.

**PipeWire gate not available (gate stage shows gate_1410 plugin missing)**
Install `ladspa-swh-plugins` for the `gate_1410` LADSPA fallback, or upgrade to PipeWire ≥ 1.6
which provides the builtin `noisegate` plugin used by the gate stage on newer versions.
