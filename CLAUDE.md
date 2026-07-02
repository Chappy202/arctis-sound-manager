# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this is

A from-scratch **Linux** desktop app to manage SteelSeries Arctis headsets (primary target: **Arctis
Nova Pro Wireless**) with a SteelSeries Sonar-style experience: per-app audio routing, per-channel
parametric EQ, per-profile HRIR surround, and headset hardware control. It replaces a community Python
app the owner found unmaintainable.

**Status:** functional (v0.2.7 + the 2026-07-02 deep-review hardening on master). Engine/daemon, the
`asm-cli` CLI, and the Tauri v2 GUI all work — per-app routing (persisted as PipeWire
`stream.rules`/`pulse.rules` fragments), per-channel EQ with auto-preamp, mic DSP chain with always-on
limiter, virtual surround/HRIR (input-adaptive Auto mode), hardened daemon IPC (timeouts, line caps,
`daemon_version` handshake), and device reads are live. Device **writes** remain gated until each
control is validated on real hardware. Build order is still engine-first (headless): build/test the
Rust core + `asm-cli` before the UI.

## Authoritative documents (read these first)

- `docs/superpowers/specs/2026-06-20-arctis-sound-manager-design.md` — the full design spec.
- `ARCHITECTURE.md` — architecture diagrams + **binding engineering guardrails (G1–G10)**.
- `DESIGN.md` — UI/visual design system (Sonar look & feel).
- Project memory (`~/.claude/.../memory/`) — validated system facts, protocol, Sonar model, pitfalls.

## Stack

- **Rust** Cargo workspace (engine) + **Tauri v2** web UI. Audio via **PipeWire** (`pipewire-rs` on a
  dedicated thread; `pw-metadata`/`wpctl` as subprocess for discrete actions). HID via `hidraw` using
  the `hidapi` **C backend** (`linux-static-hidraw`); the pure-Rust `linux-native` backend does NOT
  enumerate the Nova Pro Wireless. Build deps: `libudev` (`systemd-devel`) + a C toolchain (`gcc`); the
  optional `pw-watcher` feature (live route re-apply) additionally needs `pipewire-devel` + `clang`. The system tray additionally needs `libayatana-appindicator3-dev` (`libayatana-appindicator-gtk3-devel` on Fedora/Nobara).
- Crates: `domain`, `device`, `audio`, `config`, `engine`, `client` (daemon IPC), `cli` (hosts the
  resident daemon: `asm-cli daemon`); `src-tauri` + `frontend`.
- Dependency rule: `tauri` only in `src-tauri`; `engine` and below are UI-agnostic. See ARCHITECTURE §2.

## Non-negotiable safety rules (from ARCHITECTURE G2)

- **Never write the headset OLED.** **Never replay unverified firmware init opcodes.**
- Validate every **write** opcode against real hardware before enabling it; reads are safe by default.
- All device writes go through one serialized writer; surface failures, never swallow USB errors.

## Validated environment (target machine, 2026-06-20)

- Nobara Linux 43, KDE/Wayland, kernel 7.0.x. PipeWire 1.4.11 + WirePlumber 0.5.13; **no PulseAudio**.
- Audio is **48 kHz only** — design for 48 kHz, no resampling.
- Device `1038:12e5` exposes **one stereo sink + one mono mic**; all Game/Chat/Media channels are
  software virtual sinks. Needs a udev rule for non-root hidraw access.
- `12e5` vs `12e0` (Xbox "X" vs standard) is unresolved — validate protocol before trusting opcodes.

## Build / run / test

- `cargo build --workspace` / `cargo test --workspace` (prefer `-- --test-threads=1`: a shared
  `/tmp` surround-conf test can race under parallelism).
- `cargo run -p arctis-cli -- <cmd>` — drive the engine headless (installed binary: `asm-cli`).
- GUI: `pnpm gui` from the repo root, daemon running. `src-tauri` needs the asm-cli sidecar staged
  first: `./scripts/stage-sidecar.sh`.
- Optional `pw-watcher` feature: `cargo build -p arctis-cli --features pw-watcher` (needs
  `pipewire-devel` + `clang`; off by default → no-op stub; re-applies routes on stream resume).
- Live-PipeWire/real-hardware tests are out of the default CI gate. CI runs clippy `-D warnings` +
  `cargo test`; **rustfmt is advisory/non-blocking** (the codebase uses a deliberately compact style —
  no rustfmt.toml preserves it, so it is not enforced).
- Releases: bump `crates/cli/Cargo.toml` **as well as** `src-tauri/tauri.conf.json` — the IPC
  `daemon_version` handshake reports the cli crate version (still `0.1.0` as of v0.2.7).

## Working conventions

- Follow ARCHITECTURE guardrails (G1 reuse-over-duplication; G3 live EQ, no service restarts; G4 single
  source of truth; G6 small focused files; G7 typed errors, no `unwrap` on runtime paths).
- Use the superpowers workflow: brainstorm → spec → plan → implement; TDD for features.
- Reference (do not run/modify) the old app for protocol/behavior: `/home/jj/src/Arctis-Sound-Manager`
  and `/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager`.

## Web searches

- Never include a date in searches, always pull the latest content.