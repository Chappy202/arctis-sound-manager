# Arctis Sound Manager

A from-scratch Linux desktop app and headless engine to manage SteelSeries Arctis headsets
(primary target: **Arctis Nova Pro Wireless**, 1038:12e5): per-app audio routing, per-channel
parametric EQ, a "Clean Mic" DSP chain, and headset hardware control, via PipeWire + hidraw.
Replaces a community Python app that became unmaintainable.

---

## Status

- **Working:** engine/daemon, `asm-cli`, and Sonar-style Tauri GUI scaffold; multi-channel
  software routing (Game/Chat/Media virtual sinks) + live parametric EQ; mic DSP chain
  (gain, highpass, suppression, compressor, gate, EQ stages); device live reads (battery,
  ANC, ChatMix dial, mic mute).
- **Owner-run / gated:** device _writes_ (sidetone, mic LED, ANC mode, etc.) are dispatched
  through the daemon but ship disabled until each control is validated on real hardware.
  Audible mic DSP end-to-end is owner-run (hardware required).
- **Not yet built:** packaging/OTA (AppImage + signed updater, Plan 8); spatial/surround
  convolver; full GUI page set (UI scaffold exists, individual pages incomplete).

---

## Architecture

A Rust Cargo workspace (engine-first, UI-agnostic) + Tauri v2 GUI
(`identifier: com.oxibux.arctis-sound-manager`).

**Crates:** `domain` (pure types), `device` (HID codec + registry), `audio` (PipeWire
virtual sinks, EQ, routing), `config` (schema-versioned store), `engine` (orchestrator),
`client` (daemon IPC client), `cli` (`asm-cli`); `src-tauri` + `frontend` (Tauri shell).

Audio is driven via PipeWire subprocess (`pw-cli`) at **48 kHz throughout** — no resampling.
HID access uses `hidapi` with the C `linux-static-hidraw` backend; the pure-Rust
`linux-native` backend does not enumerate the Nova Pro Wireless.

See [ARCHITECTURE.md](ARCHITECTURE.md) for diagrams and binding guardrails (G1–G10).
See [DESIGN.md](DESIGN.md) for the Sonar-style visual design system.

---

## Requirements

### Runtime (core)

- **PipeWire 1.4+** — 1.6+ recommended. The mic noise gate uses PipeWire's builtin
  `noisegate` plugin on ≥ 1.6; on older PipeWire it falls back to the `swh` `gate_1410`
  LADSPA plugin (see below).
- **WirePlumber** (any version compatible with your PipeWire; 0.5.x tested).
- **48 kHz audio setup** — the engine pins all nodes at 48 kHz.

### Optional mic-DSP LADSPA plugins

All plugins are auto-detected at runtime. If a plugin is missing the app reports it and
disables that stage — nothing crashes. Plugin `.so` files are searched in order:
`$LADSPA_PATH` (colon-separated), `/usr/lib64/ladspa`, `/usr/lib/ladspa`,
`/usr/lib/x86_64-linux-gnu/ladspa`.

To verify a plugin after install: `analyseplugin /usr/lib64/ladspa/<file>.so`
(package: `ladspa` on Fedora, `ladspa-sdk` on Debian/Ubuntu).

#### DeepFilterNet (default noise suppressor — recommended)

Has an Attenuation Limit knob that caps how much it suppresses, making it the
anti-tinny choice.

| Distro | Install | `.so` path |
|--------|---------|------------|
| Fedora / Nobara | `sudo dnf copr enable mavit/DeepFilterNet && sudo dnf install deep-filter-ladspa` | `/usr/lib64/ladspa/libdeep_filter_ladspa.so` |
| Debian / Ubuntu | Download release `.so` from [github.com/Rikorose/DeepFilterNet](https://github.com/Rikorose/DeepFilterNet/releases) | `/usr/lib/ladspa/libdeep_filter_ladspa.so` |
| Arch | `yay -S libdeep_filter_ladspa-bin` | `/usr/lib/ladspa/libdeep_filter_ladspa.so` |

#### RNNoise (fallback suppressor — lighter, can sound tinnier)

No attenuation cap; lighter CPU load. Select with `asm-cli mic backend rnnoise`.

| Distro | Install |
|--------|---------|
| Fedora / Nobara | `sudo dnf copr enable lkiesow/noise-suppression-for-voice && sudo dnf install ladspa-realtime-noise-suppression-plugin` |
| Debian / Ubuntu | `sudo apt install noise-suppression-for-voice` |
| Arch | AUR: `yay -S noise-suppression-for-voice` |

#### swh-plugins (compressor + gate fallback)

Provides the `sc4m_1916` compressor stage and `gate_1410` gate fallback for
PipeWire < 1.6.

| Distro | Install |
|--------|---------|
| Fedora / Nobara | `sudo dnf install ladspa-swh-plugins` |
| Debian / Ubuntu | `sudo apt install swh-plugins` |
| Arch | `sudo pacman -S swh-plugins` |

### Device access (udev)

The headset's hidraw node is root-only by default. Install the shipped udev rule once:

```sh
sudo cp packaging/udev/70-arctis-sound-manager.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
sudo udevadm trigger
```

Then replug or power-cycle the headset. Without this rule, `asm-cli probe` will fail
with a permission error on `/dev/hidraw*`.

### GUI runtime

- **webkit2gtk-4.1**: Fedora `sudo dnf install webkit2gtk4.1`, Debian `sudo apt install
  libwebkit2gtk-4.1-0`. Not required for headless `asm-cli` use.

### Build dependencies

- **Rust** (via [rustup](https://rustup.rs/); `cargo` may be at `~/.cargo/bin/cargo`
  if not on `PATH`).
- **libudev**: Fedora `sudo dnf install systemd-devel`, Debian `sudo apt install
  libudev-dev`. Required by the hidapi C backend.
- **C compiler** (`gcc` or `clang`): required for the hidapi C backend.
- **Node ≥ 20 + pnpm** and `webkit2gtk-4.1-devel` (Fedora) / `libwebkit2gtk-4.1-dev`
  (Debian): required only for building the GUI.

---

## Build / Test

```sh
cargo build --workspace
cargo test --workspace
```

Live-PipeWire integration tests are gated behind a feature flag. Real-hardware CLI tests
are out of the default CI gate.

---

## Run

**Start the daemon** (required before the GUI and before any write or live mic command):

```sh
cargo run -p arctis-cli -- daemon
```

The daemon listens on a Unix socket at `$XDG_RUNTIME_DIR/arctis-sound-manager.sock`.
Run in the foreground (default); use a terminal multiplexer or systemd user unit to keep
it running.

**GUI:**

```sh
cd frontend && pnpm install && pnpm tauri dev
```

The daemon must be running first. On NVIDIA with a blank window, set
`WEBKIT_DISABLE_DMABUF_RENDERER=1`.

---

## CLI usage (`asm-cli`)

`asm-cli` is the installed binary name of the `arctis-cli` crate; from a source checkout run it as `cargo run -p arctis-cli -- <args>`.

All commands that touch PipeWire or the device require the daemon to be running.

### Device discovery

```sh
asm-cli list              # list connected, recognized SteelSeries devices
asm-cli probe             # read and print device status (battery, ANC, mic, ChatMix)
```

### Audio

```sh
# Virtual EQ sink
asm-cli sink create [--target <node.name>]   # create the virtual EQ sink (idempotent)
asm-cli sink remove                           # remove it

# Live parametric EQ on the EQ sink
asm-cli eq set --band 3 --freq 1200 --q 1.0 --gain -6 [--kind peaking|lowshelf|highshelf]
asm-cli eq show

# Submix channels (Game / Chat / Media)
asm-cli channels up [--target <node.name>]   # create all configured channels
asm-cli channels down

# Per-channel output device
asm-cli channel output set <game|chat|media> <node.name|default>
```

### Routing

```sh
asm-cli route set <app-binary> <game|chat|media> [--by-name]
asm-cli route list
```

### Profiles

```sh
asm-cli profile list
asm-cli profile show [<name>]
asm-cli profile switch <name>
asm-cli profile save
asm-cli profile new <name>
asm-cli apply              # reconcile the live graph to the active profile
```

### Daemon

```sh
asm-cli daemon             # start the resident daemon (foreground)
```

### Device (hardware controls — writes gated pending per-control validation)

```sh
asm-cli device status                          # live read: battery, ANC, dial, mic-mute
asm-cli device sidetone <0..3>
asm-cli device mic-led <1..10>
asm-cli device anc <off|transparency|on>
asm-cli device auto-off <0..6>                 # 0=never, 1=1min, 2=5min, …, 6=60min
asm-cli device transparency <1..10>
asm-cli device mic-volume <1..10>
asm-cli device set <control> <value>           # generic escape hatch
```

Device writes are routed through the daemon, which enforces single-writer HID
serialization. A gating layer refuses writes that have not been validated on hardware.

### Mic DSP chain

The Clean Mic feature creates a virtual PipeWire source with opt-in DSP stages that apps
can select as their capture input.

```sh
# Master switch
asm-cli mic on
asm-cli mic off

# Enable / disable individual stages
asm-cli mic enable  <gain|highpass|suppression|compressor|gate|eq>
asm-cli mic disable <gain|highpass|suppression|compressor|gate|eq>

# Choose the noise-suppression backend
asm-cli mic backend deep_filter    # DeepFilterNet (default)
asm-cli mic backend rnnoise        # RNNoise (lighter fallback)

# Set parameters live (no restart)
asm-cli mic set <param> <value>
```

Available `mic set` parameters:

| Parameter | Description |
|-----------|-------------|
| `gain_db` | Input gain in dB |
| `highpass_freq` | Highpass filter corner frequency (Hz) |
| `attenuation_limit_db` | DeepFilterNet: max suppression cap 0–100 dB (lower = fewer artifacts) |
| `vad_threshold` | RNNoise: VAD threshold % (0–99) |
| `vad_grace_ms` | RNNoise: VAD grace period (0–1000 ms) |
| `vad_retro_grace_ms` | RNNoise: retroactive VAD grace (0–200 ms) |
| `gate_threshold` | Gate open threshold (linear 0–0.5) |
| `comp_threshold_db` | Compressor threshold (-30–0 dB) |
| `comp_ratio` | Compressor ratio (1–20) |
| `comp_makeup_db` | Compressor makeup gain (0–24 dB) |

```sh
# Mic EQ band (live, no restart)
asm-cli mic eq --band 2 --freq 1000 --q 1.0 --gain -3.0 [--kind peaking|lowshelf|highshelf]

# Pin (or clear) the hardware capture source
asm-cli mic hw-mic alsa_input.usb-SteelSeries_Arctis_Nova_Pro_Wireless-00.mono-fallback
asm-cli mic hw-mic     # clear the pin (follow WirePlumber default)

# Show full chain status
asm-cli mic status
```

**Example: enable the chain with DeepFilterNet and tune it**

```sh
asm-cli mic on
asm-cli mic enable suppression
asm-cli mic backend deep_filter
asm-cli mic set attenuation_limit_db 40    # lower = less suppression, fewer artifacts
asm-cli mic enable highpass
asm-cli mic set highpass_freq 120
asm-cli mic enable gate
asm-cli mic status
```

---

## Mic chain design notes

The default mic path is clean passthrough — no plugin loaded, no external dependency.
Every DSP stage is opt-in with conservative defaults.

Chain order (when enabled): gain → highpass → suppression → compressor → gate → EQ.

**DeepFilterNet is the recommended suppressor** because its Attenuation Limit knob
(`attenuation_limit_db`) caps how much it may attenuate the signal. Setting it lower
(e.g. 40 dB instead of the 100 dB maximum) reduces the risk of the over-suppressed,
thin, "tinny" sound that naive noise reduction produces. RNNoise is a lighter-weight
fallback but has no such cap — it can sound tinnier on voices, especially at higher
suppression levels.

The gate uses PipeWire's builtin `noisegate` on PipeWire ≥ 1.6, falling back to the
`swh` `gate_1410` LADSPA plugin on older versions. All processing runs at 48 kHz.

---

## Safety

Device writes go through a single serialized writer and ship **disabled by default**
until each control is validated against real hardware. The headset OLED is never written.
Reads are safe and used by default. Write failures are surfaced explicitly — USB errors
are never silently swallowed.

---

## Further reading

- [ARCHITECTURE.md](ARCHITECTURE.md) — system diagrams, crate dependency rules, and
  binding guardrails (G1–G10).
- [DESIGN.md](DESIGN.md) — Sonar-style visual design system (color tokens, typography,
  component specs).
- `docs/superpowers/specs/2026-06-20-arctis-sound-manager-design.md` — full design spec.
