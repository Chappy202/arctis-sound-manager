# CLAUDE.md

Guidance for Claude Code when working in this repository.

## What this is

A from-scratch **Linux** desktop app to manage SteelSeries Arctis headsets (primary target: **Arctis
Nova Pro Wireless**) with a SteelSeries Sonar-style experience: per-app audio routing, per-channel
parametric EQ, per-profile HRIR surround, and headset hardware control. It replaces a community Python
app the owner found unmaintainable.

**Status:** greenfield, in setup. **Build order is engine-first (headless)** — build and test the Rust
core + `asm-cli` before any UI.

## Authoritative documents (read these first)

- `docs/superpowers/specs/2026-06-20-arctis-sound-manager-design.md` — the full design spec.
- `ARCHITECTURE.md` — architecture diagrams + **binding engineering guardrails (G1–G10)**.
- `DESIGN.md` — UI/visual design system (Sonar look & feel).
- Project memory (`~/.claude/.../memory/`) — validated system facts, protocol, Sonar model, pitfalls.

## Stack

- **Rust** Cargo workspace (engine) + **Tauri v2** web UI. Audio via **PipeWire** (`pipewire-rs` on a
  dedicated thread; `pw-metadata`/`wpctl` as subprocess for discrete actions). HID via `hidraw`.
- Crates: `domain`, `device`, `audio`, `config`, `engine`, `cli`, (future `daemon`); `src-tauri` + `ui`.
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

Not yet scaffolded. Once the workspace exists:
- `cargo build --workspace` / `cargo test --workspace`
- `cargo run -p cli -- <cmd>` — drive the engine headless
- Live-PipeWire integration tests are gated behind a flag; real-hardware tests live in `cli` and are out
  of the default CI gate.
(Update this section as commands become real — do not leave stale instructions.)

## Working conventions

- Follow ARCHITECTURE guardrails (G1 reuse-over-duplication; G3 live EQ, no service restarts; G4 single
  source of truth; G6 small focused files; G7 typed errors, no `unwrap` on runtime paths).
- Use the superpowers workflow: brainstorm → spec → plan → implement; TDD for features.
- Reference (do not run/modify) the old app for protocol/behavior: `/home/jj/src/Arctis-Sound-Manager`
  and `/home/jj/Dev/Personal/sound-manager/Arctis-Sound-Manager`.

## Web searches

- Never include a date in searches, always pull the latest content.