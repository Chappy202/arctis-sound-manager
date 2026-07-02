# Arctis Sound Manager — Design Spec

**Date:** 2026-06-20
**Status:** Approved (brainstorming phase)
**Author:** JJ (with Claude as orchestrator)

> **Superseded in part (2026-07-02):** this is a historical design record. Two mechanisms in §6
> changed in implementation: persistent per-app routing is written as PipeWire `stream.rules` /
> `pulse.rules` fragments (WirePlumber 0.5 has no `node.rules` section), and `restore-stream`
> cannot be managed per-app without a WirePlumber restart — a cleared route carries an advisory
> note instead. See `ARCHITECTURE.md` §3/§5 and `KNOWN_ISSUES.md` KI-6 for the current behaviour.

A from-scratch Linux replacement for the SteelSeries Arctis / Sonar sound manager, focused on
the Sonar-style submix/EQ experience and managing a SteelSeries Arctis Nova Pro Wireless — built
with strong engineering practices, multi-device support, and OTA updates.

This spec is the authoritative design. It is grounded in research validated against the target
machine (read-only audit, 2026-06-20) and web research of the official SteelSeries software. It
supersedes assumptions; open items requiring on-hardware validation are called out in §11.

---

## 1. Goals & non-goals

### Goals
- Per-application audio routing to channels (Game, Chat, Media, Speakers/Aux), including routing
  to different physical devices (e.g. browser/Spotify → speakers while games → headset).
- Per-channel parametric EQ that applies **instantly** (no audio-stack restart).
- Per-profile HRIR / virtual-surround that can keep a Dolby-Atmos-style profile and toggle per channel.
- Fine microphone tuning that does **not** make the mic sound tinny.
- Manage the headset like the official app: battery/status, mic volume, sidetone, ANC/transparency,
  mic-mute LED, auto-off, wireless mode, hardware EQ + preset.
- Multi-device support via a data-driven device registry (not just the Nova Pro Wireless).
- Modular, reusable, expandable codebase; OTA / automated updates.

### Non-goals (for now)
- **Streamer mode** (dual monitoring/stream mixes feeding OBS). Designed-for but **deferred** to a
  future phase; the architecture must not preclude it.
- Writing to the base-station **OLED** display. Explicitly excluded by design (brick-risk aversion;
  let SteelSeries firmware own the device).
- Replaying reverse-engineered, unverified firmware init opcodes.
- Auto-switching profiles by running game (the official Sonar doesn't do this; not worth the complexity).
- Windows/macOS support; this is Linux-first.

---

## 2. Key decisions (locked)

| Decision | Choice | Rationale |
|---|---|---|
| Relationship to existing app | **Full replacement** of the RPM `arctis-sound-manager` + `hrir-switch` | Avoids USB-endpoint/sink-name collisions; clean architecture |
| Language / stack | **Rust engine + Tauri v2 web UI** | Rust is the only language with a mature PipeWire binding (Python's is archived); Tauri gives polished UI + best self-distributed OTA on Linux |
| Build order | **Engine-first (headless)** | Complete, tested core + CLI before UI |
| Audio backend | **PipeWire** | Settled 2026 Linux standard; the machine runs 1.4.11 / WirePlumber 0.5.13; no better alternative |
| Device support | **Data-driven registry** | Reuses the old app's best idea; add a descriptor, not code |
| Packaging / OTA | **AppImage + Tauri signed updater** (primary); .deb/.rpm convenience; **not Flatpak** | Flatpak sandbox fights hidraw + PipeWire routing |

---

## 3. System context (validated on target machine, 2026-06-20)

- OS: Nobara Linux 43 (Fedora-based), KDE Plasma / Wayland, kernel 7.0.9.
- Audio: PipeWire 1.4.11 + WirePlumber 0.5.13; no PulseAudio (pipewire-pulse shim only); rtkit active.
- Sample rate: **48000 Hz only** (`clock.allowed-rates=[48000]`). Design for 48 kHz throughout; no resampling.
- Device: USB `1038:12e5`, name "Arctis Nova Pro Wireless". **Hardware exposes one stereo sink + one
  mono mic** — there are **no native Game/Chat hardware sinks**; all channel splitting is software.
- HID: `/dev/hidraw0` is the device but currently root-only (ACL missing). A udev rule explicitly
  covering `1038:12e5` with `TAG+="uaccess"` is required for non-root access.
- Existing stack to replace (see §10): RPM daemon+GUI+router, a dedicated filter-chain PipeWire
  instance, 3 `pw-loopback` sinks (`Arctis_Game/Chat/Media`), and `~/.local/bin/hrir-switch`.

---

## 4. Architecture

A Cargo **workspace** with a hard split between a reusable, UI-agnostic engine and the Tauri shell.

```
arctis-sound-manager/
├── crates/
│   ├── domain/    # pure types, no I/O: Device, Capability, Channel, Profile, EqBand,
│   │              #   MicChain, DeviceState, value/unit types
│   ├── device/    # HID layer: Transport trait + hidraw impl; data-driven device REGISTRY;
│   │              #   descriptor-driven command encode + status decode; single serialized writer
│   ├── audio/     # PipeWire engine: virtual-sink factory, live-EQ filter chains, convolver
│   │              #   surround, per-app routing, submix→master graph
│   ├── config/    # single source of truth: profiles, persistence, schema versioning + migration
│   ├── engine/    # orchestrator ("core"): composes device+audio+config; async UI-agnostic API
│   │              #   + event stream (state changes, battery, levels)
│   ├── cli/       # asm-cli: drives the engine headless — engine-first deliverable + test harness
│   └── daemon/    # (future) headless service exposing engine over D-Bus / socket
├── src-tauri/     # Tauri v2 shell: maps engine API → commands/events/channels; OTA updater; bundling
├── ui/            # web frontend (Sonar-like)
├── devices/       # declarative device descriptors (data, not code)
└── docs/
```

**Dependency rule:** `domain` depends on nothing app-specific; `device`/`audio`/`config` depend only
on `domain`; `engine` composes them; `cli`, `daemon`, and `src-tauri` depend on `engine`. **Nothing
below `src-tauri` may depend on `tauri`.**

**Reuse principles (enforced in review):**
- Generic utilities with dynamic parameters over duplicated bespoke code: one biquad-band builder,
  one descriptor-driven HID codec, one virtual-sink factory, one filter-chain config generator.
- Data-driven device + profile definitions; capability-gated behavior so one code path serves all devices.
- Files stay focused and small; engine logic never lives in UI components.

---

## 5. Domain model — channels / submixes

Adopts the canonical SteelSeries Sonar model (the mental model the user wants; cleanly supports the
headset's native Game/Chat dial).

- **Source submixes:** `Game`, `Chat`, `Media` (optional), `Aux` (optional), `Mic`.
- All source submixes sum into **`Master`** → physical output device(s).
- **Output submix** properties: volume, mute, parametric EQ (§6), optional spatial/surround (§7),
  gain, optional Smart-Volume (loudness leveling).
- **Mic submix** properties: the mic chain (§8).
- **Per-channel output-device override is first-class and enforced** — the routing graph is actually
  rebuilt/retargeted when a channel's output changes (fixes the old dead selector). Example: Media →
  speakers while Game/Chat → headset, simultaneously.
- **Channels are user-customizable**: rename, enable/disable Media/Aux. One coherent label set used
  in every view (fixes the old two-views-disagree confusion).
- **Physical Game/Chat dial**: read over HID, mapped to the Game↔Chat balance (software volume on the
  two virtual sinks). The native switch keeps working, now correctly wired. Never written back to the device.
- **Profiles** (§9) bundle the full state and are switched from a top dropdown.

Virtual devices are named to mirror Sonar (e.g. `Arctis_Game`, `Arctis_Chat`, `Arctis_Media`) so apps
present familiar choices.

---

## 6. Audio engine

- **Virtual sinks:** one PipeWire `filter-chain` `Audio/Sink` per active submix, built by a single
  parameterized factory. 48 kHz end-to-end.
- **Live parametric EQ (headline fix):** bands are builtin biquad nodes (`bq_peaking`, `bq_lowshelf`,
  `bq_highshelf`, plus `bq_highpass`/`bq_lowpass`/`bq_notch` as filter types) with **named controls**.
  EQ edits update controls in place via `pw_node_set_param(SPA_PARAM_Props, { params = [name value …] })`
  (CLI-equivalent `pw-cli s <id> Props …`). **No `.conf` rewrite, no service restart, instant apply.**
  Runtime Props are not persisted by PipeWire → the engine re-applies all EQ on session start.
  - EQ spec: up to 10 bands per channel; default range ±12 dB, Q ~0.3–10 (SteelSeries' exact ranges are
    unpublished; these are our defaults). Per-band filter type selectable. A simplified 3-band mode
    (Bass/Voice/Treble) maps onto the parametric bands.
  - Preset management: save/load/favorite/import per-channel EQ presets.
- **Per-profile HRIR / virtual surround:** a `convolver` filter-chain sink loaded from a HeSuVi/SOFA
  impulse, fed **only** by the channels the profile selects (e.g. Game/Media surround; Chat stays clean
  stereo). Per-profile toggle and a "None"/bypass option. Profiles supply the impulse (keeps a
  Dolby-Atmos-style HRIR). **Subsumes `hrir-switch`** — same capability, built in, no external script.
- **Per-app routing:** persistent `WirePlumber node.rules` (match `application.process.binary` /
  `application.name` → `node.target`) + live moves via `pw-metadata <stream> target.object <sink>`.
  Respects streams the user manually pinned; manages `restore-stream` so it doesn't fight our rules.
- **Signal path:** app → submix sink (EQ) → [optional surround] → Master → hardware sink.
- **Idempotency:** declarative startup config (stable `node.name`) + live runtime control; the engine
  reconciles existing objects rather than blindly recreating them.

---

## 7. Device layer (multi-device, data-driven)

- **Device descriptor** (one declarative file per device under `devices/`) holds: identity (VID/PID),
  capability flags, the HID command map (opcode + arg encoding), the status response map (match by
  header prefix, slice bytes by offset), and value parsers (percentage, on/off, enum mappings).
- **Registry** loads descriptors; the engine resolves the connected device → its descriptor.
- **Capability flags drive everything:** the UI renders only supported controls; the engine only sends
  commands the device declares. Adding a device = add a descriptor file, **no code changes**.
- **Transport:** a `Transport` trait with a hidraw implementation (preferred over libusb +
  kernel-driver detach to avoid the EBUSY/stale-handle class of bugs). A **single serialized,
  prioritized writer** (no concurrent USB writes).
- **Safety guardrails (hard rules):**
  - Never write the OLED. Never replay unverified init opcodes.
  - Every write capability is **validated against real hardware before being enabled** (§11).
  - Reads are always safe and used by default. Write failures are surfaced, never silently swallowed.
- **Reads:** battery %, charge state, connection, chat-mix dial position, ANC state, mic-mute state.
- **Writes (capability-gated):** sidetone, mic volume, ANC/transparency mode + level, mic-mute LED,
  auto-off timer, wireless Speed/Range mode, hardware (graphic) EQ + preset.
- **EQ distinction:** the device's on-board EQ is **10-band graphic**; the app's primary EQ is the
  **software parametric** per-channel EQ (§6). The device graphic EQ is an optional capability, not the
  main EQ surface.

First device shipped: **Arctis Nova Pro Wireless**. Architecture carries the ~16 devices the old app
supported, and more.

---

## 8. Microphone chain (fixes "tinny")

Default = **clean passthrough**. Each stage is **opt-in** with conservative defaults, in order:

1. Gain (`linear`)
2. High-pass (~80–100 Hz, gentle) — off by default
3. Noise suppression — **DeepFilterNet preferred** (RNNoise fallback). Exposed as
   **Minimal / Medium / Maximum**, mapped to a **capped attenuation** (the key anti-tinny control).
4. Noise gate (conservative threshold) — off by default
5. Parametric mic EQ (biquad bands, like §6) — off by default
6. Compressor / limiter (LSP plugins) — off by default

Stereo-aware where the source supports it; 48 kHz throughout. This is the inverse of the old app's
always-on mono 5-stage chain that thinned the voice.

---

## 9. Config, profiles & state

- **One authoritative, schema-versioned config store** with migrations (not the old ~7 scattered
  dotfiles that caused view/data desync).
- A **Profile** = a full bundle: per-channel EQ, surround selection, routing rules, volumes, mic chain,
  and device settings. Switchable from a top dropdown. Switching is manual.
- Import/export of profiles and per-channel EQ presets.

---

## 10. Coexistence / migration

On **first run**, detect the existing stack — RPM `arctis-sound-manager` (daemon/gui/router), its
dedicated filter-chain PipeWire instance, the `Arctis_Game/Chat/Media` loopbacks, and
`~/.local/bin/hrir-switch` — and **offer to disable/uninstall it** before taking over the single USB
control endpoint and virtual-sink namespace. Provide a clean teardown of our own objects on exit.

---

## 11. Validation & risks (engine-first phase 0)

Resolve before building dependent features:

1. **Protocol validation for `12e5`.** The unit reports `1038:12e5` named "Arctis Nova Pro Wireless",
   but upstream docs call `12e5` the Xbox "X" variant (`12e0` = standard). Validate the command/status
   protocol with **safe read-only probes** before enabling any write opcode.
2. **Live-EQ `Props` behavior** on PipeWire 1.4.11 — confirm in-place control updates apply without
   node recreation or audible glitches.
3. **Empirical ranges** — pin exact EQ gain/Q, mic volume unit, gate threshold, sidetone range against
   the real device (SteelSeries never published them).
4. **udev** — ship and verify a rule covering `1038:12e5`; first-run `pkexec` installer.
5. **WebKitGTK** rendering quirks on the target (NVIDIA/DMABUF) — apply `WEBKIT_DISABLE_*` mitigations.
6. **`hidraw` vs in-kernel `hid-steelseries`** — detect if a kernel driver claims the device; prefer
   sysfs attributes if present, else hidraw.

---

## 12. UI (Tauri v2, Sonar look & feel)

Dark, Sonar-style layout:
- **Mixer** page: channel strips (volume, mute, active preset label, output-device selector).
- **Per-channel EQ / Spatial** pages: parametric curve editor + surround controls.
- **Mic** page: the opt-in chain (§8).
- **Device** page: capability-driven hardware controls + battery/status/dial readout.
- **Profiles** dropdown (top): switch full configs.

Ship WebKitGTK mitigations from day one. The UI talks only to the `engine` API via Tauri
commands/events/channels (channels for streamed telemetry: battery, level meters, live EQ).

---

## 13. Packaging & OTA

- **Primary:** auto-updating **AppImage** + Tauri signed (minisign/Ed25519) updater against a static
  `latest.json`. First-run helper installs the udev rule via one-time `pkexec`.
- **Convenience:** `.deb` / `.rpm` (won't silently auto-update; acceptable).
- **Not Flatpak** — sandbox blocks hidraw (no portal; needs `--device=all` + host udev) and withholds
  the PipeWire manager permission needed for routing.
- Unsandboxed process automatically gets full PipeWire permissions (virtual sinks, routing, EQ).

---

## 14. Testing strategy

- `domain` / EQ math (biquad coefficients) / profile (de)serialization — pure unit tests.
- `device` — `Transport` trait mocked with recorded byte fixtures; command encode + status decode tested
  with no hardware in CI.
- `audio` — unit-test generated filter-chain configs and `Props` payloads + routing requests without a
  live daemon; gate true integration tests (spin PipeWire, assert nodes via `pw-dump`) behind a flag.
- `cli` — end-to-end harness against real hardware/PipeWire, kept out of the default CI gate.

---

## 15. Companion documents (to be produced next)

- `DESIGN.md` — architecture guardrails & guidelines (data-driven device model, reuse rules, safety
  guardrails, dependency rule, file-size discipline). Living document.
- `CLAUDE.md` — project context, system facts, conventions, how to run/test, the hard safety rules.
- Implementation plan (via the writing-plans skill) — decomposed, parallelizable tasks following the
  engine-first order.

---

## 16. Open questions / future

- Streamer mode (deferred) — keep the audio graph factorable so a parallel Stream mix + OBS sink can be
  added without rework.
- Daemon crate — promote engine to a background service once the GUI path is proven.
- Additional device descriptors beyond the Nova Pro Wireless.
