# Implementation Plan — Device HID Control (Arctis Nova Pro Wireless)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to execute this plan task-by-task with review checkpoints. Each task is a self-contained TDD unit with explicit verification. Tasks marked **[CI]** are fully buildable and verifiable by a subagent against `MockTransport`/`MockRunner` with NO real hardware (assert exact bytes against the documented opcodes). Tasks marked **[OWNER-RUN]** require the real headset and are **SAFETY GATES** — they are **manual and NOT auto-executed**. A subagent MAY write the code and the CI tests for a write capability, but it MUST leave that capability **disabled** (its `Capability` flag absent from the live descriptor / the command commented out of the shipping TOML) until the owner runs the validation step on the real device and confirms the headset reacts correctly. SAFETY IS PARAMOUNT: the owner fears bricking the device. When in doubt, do less and surface the question.

---

## Goal

Bring the Arctis Nova Pro Wireless **alive** in the app: read live status on the resident daemon (battery, ANC mode, mic-mute, dial mix) so the GUI Device panel and topbar battery populate, and add a **data-driven, capability-gated WRITE layer** for a small set of safe hardware controls (sidetone, mic-LED brightness, ANC mode, transparency level, auto-off, and — gated — mic volume), each individually validated on the real headset before it is enabled.

The write layer mirrors the existing read layer's data-driven model (G1): commands are declared in TOML, encoded by **one generic encoder**, and sent by **one serialized writer**. No per-control hardcoded logic. No init bursts. No OLED. Reads come first and are safe.

This is the first plan that sends bytes *to* the device. Reads (Plan "foundation-and-device-read") already work and are validated on real HW.

---

## Architecture

### Data flow (after this plan)

```
                         ┌────────────────────────────────────────────┐
   real headset (HID)    │  crates/device  (data-driven, capability-gated) │
   1038:12e5 iface 4  ◄──┤   DeviceDescriptor (TOML):                  │
                         │     [status]   request + fields + parsers   │ (exists)
                         │     [commands] opcode + value encoding      │ (NEW)
                         │   codec::encode_command  (mirror decode)    │ (NEW)
                         │   codec::write_command   (1 serialized wr)  │ (NEW)
                         │   DeviceController (capability gate + read) │ (NEW)
                         └───────────────┬─────────────────────────────┘
                                         │ owns 1 HidrawTransport, &mut, serialized
                         ┌───────────────▼─────────────────────────────┐
                         │  crates/engine                              │
                         │   DeviceWorker: read-loop thread            │ (NEW)
                         │     - opens device (best-effort)            │
                         │     - polls read_status every N s           │
                         │     - merges unsolicited ANC/dial frames    │
                         │     - pushes DeviceSnapshot → engine        │
                         │   Engine.device_* setters → write_command   │ (NEW)
                         │   EngineState.device_present/device_fields  │ (populated)
                         └───────────────┬─────────────────────────────┘
                                         │ Unix socket JSON (arctis-client)
                  ┌──────────────────────┼───────────────────────────────┐
                  ▼                      ▼                                ▼
          asm-cli device ...     src-tauri commands              (Event stream:
          (status / sidetone /   device_set_* + get_state         DeviceState{fields})
           anc / mic-led / ...)  → frontend DevicePage live
```

### Key design points

- **One owner of the device handle.** The engine spawns a single `DeviceWorker` thread that owns the one `HidrawTransport`. All reads *and* writes go through it via a command channel (`mpsc`), so the writer is inherently serialized and never races the read-loop. There is exactly one writer (G2).
- **Capability gate.** A command in the TOML `[commands]` map is only callable if (a) it is present in the descriptor and (b) its associated `Capability` is in `descriptor.capabilities`. A write to an absent/ungated command returns a typed `DeviceError::Unsupported` — it never silently no-ops and never sends arbitrary bytes.
- **Send ONLY the requested opcode.** `write_command` builds exactly one report (report_id + opcode bytes + encoded value, zero-padded to `report_length`) and writes it once. No preamble, no init burst, no "correction" frames. An optional per-command `save = true` flag appends a *second* discrete write of the documented save opcode (`0x06 0x09`) — and ONLY that — mirroring HeadsetControl's `sendCommandWithSave`. The save write is itself a single declared opcode, never a reverse-engineered burst.
- **Reads first, failures surfaced.** The read-loop is best-effort and degrades gracefully (device absent → `device_present:false`, daemon keeps running). But *writes* never swallow errors: a failed `write_command` propagates a typed error all the way to the CLI/GUI so the user sees it (the old app's silent failures made controls feel "hit or miss").

---

## Tech Stack

- **Rust** Cargo workspace (existing crates: `domain`, `device`, `audio`, `config`, `engine`, `cli`, `client`, `src-tauri`).
- **HID:** `hidapi` 2.x. **Task 1 switches the backend** from the pure-Rust `linux-native` (which does not enumerate this device) to the C `linux-static-hidraw` backend (which does). Build deps: `libudev` (provided by `systemd-devel`) + a C toolchain — both confirmed present on the target.
- **Serde + TOML** for the descriptor (existing model, extended).
- **Tauri v2 + Svelte** for the GUI Device panel (existing).
- Tests: `MockTransport` (device crate), `MockRunner` (engine), `cargo test --workspace`, `vitest`/`pnpm` for frontend pure-logic.

---

## Research basis (cross-reference; the binding validation is OWNER-RUN on the real device)

- **HeadsetControl** (`Sapd/HeadsetControl`, `lib/devices/steelseries_arctis_nova_pro_wireless.hpp`) — the authoritative cross-reference for *safe, routinely-sent* writes for THIS exact model. Confirmed there:
  - **Sidetone** `{ 0x06, 0x39, level }`, level **0–3** (user 0–128 mapped to 4 device levels). Followed by save.
  - **Inactive time / auto-off** `{ 0x06, 0xc1, level }`, level **0–6** (minutes bucketed). Followed by save.
  - **Lights / LED strength** `{ 0x06, 0xbf, strength }`, strength **1–10** (we expose this as "mic LED brightness"). Followed by save.
  - **Equalizer preset** `{ 0x06, 0x2e, preset }`, preset **0–3** (custom = 4). Followed by save.
  - **Equalizer bands** `{ 0x06, 0x33, <10 bands> }`, each band `0x14 + 2*value`, value −10..+10. (DEFERRED — overlaps software EQ.)
  - **Save / commit** `{ 0x06, 0x09 }`.
  - **Note:** HeadsetControl does **not** implement mic-volume, ANC-mode (0xbd), or transparency-level (0xb9) for this model.
- **The reference community project + project notes** supplied these *additional* opcodes (report id `0x06`): ANC `0xbd <0=off,1=transparency,2=on>`; transparency level `0xb9 <0x01–0x0a>`; mic volume `0x37 <0x01–0x0a>`; 2.4G mode `0xc3`. Because HeadsetControl does **not** corroborate these for this exact model, they are flagged **HIGHER RISK** and gated behind their own explicit OWNER-RUN steps; ANC mode is cross-checked against the already-validated **read** path (we read `anc_mode` from the `[0x07,0xbd]` frame today, so a write to `0xbd` is verifiable by re-reading).
- **Open encoding question (report length):** HeadsetControl uses short 3-byte arrays in source; whether the kernel/device requires the report padded to the full 64-byte `report_length` is unresolved. Our existing **read** writer pads the status request to `report_length` and it WORKS on real HW — so we follow the same convention (pad to `report_length`). The OWNER-RUN gate for the *first* write capability (sidetone) doubles as the validation of this padding choice; if the padded form misbehaves, the fallback (send the unpadded short report) is documented in Task 5's gate.

---

## Non-goals (DEFERRED — state explicitly)

- **The OLED display: NEVER written.** Not in this plan, not ever. No screen/image/text opcodes.
- **Replaying `device_init` / "# Correction?" / any reverse-engineered opcode burst: NEVER.** We send only the single specific opcode for a requested control (plus, optionally, the one documented save opcode).
- **Hardware EQ band writes (`0x33`):** DEFERRED. They overlap/conflict with the software per-channel parametric EQ (the headline feature). Not implemented here. The hardware EQ **preset** switch (`0x2e`) is *also* deferred in this plan (its interaction with the software EQ chain needs its own design); a stub TOML entry may be authored but its capability stays OFF.
- **Chat-mix dial WRITE:** DEFERRED. We READ the dial (`media_mix`/`chat_mix` from `[0x07,0x45]`) only; mixing stays software per the existing design.
- **2.4G wireless mode (`0xc3`), Bluetooth, firmware updates:** DEFERRED. Not corroborated by HeadsetControl for safe routine use; out of scope.
- **Mic volume (`0x37`):** code + CI test authored, but capability ships **OFF** pending its OWNER-RUN gate (HeadsetControl does not implement it for this model → higher risk).

---

## Global Constraints

- **NEVER write the OLED display — no OLED/screen/image opcode appears anywhere in this plan, ever.**
- **NEVER replay reverse-engineered init/"# Correction?" opcode bursts — only single declared opcodes are sent.**
- **Send ONLY the specific opcode for the requested control (plus at most the one documented save opcode `0x06 0x09` when `save = true`).**
- **There is exactly ONE serialized writer: the engine's `DeviceWorker` thread owns the sole `HidrawTransport`; all writes funnel through it.**
- **Every WRITE capability is VALIDATED on the real headset (OWNER-RUN) before its `Capability` flag is enabled in the shipping descriptor.**
- **Surface every write failure to the user as a typed error (CLI stderr / GUI error) — never swallow a USB/IO error.**
- **Reads are safe and come first; the read-loop must degrade gracefully on device absence/disconnect and must never crash the daemon.**
- **No `unwrap`/`expect`/`panic!` on any runtime device path (reads, writes, worker loop). Typed errors only.**
- **Data-driven (G1): commands are declared in TOML and encoded by one generic encoder — no per-control hardcoded byte logic.**
- **CI tests assert EXACT bytes against the documented opcodes via `MockTransport`; the real-device writes are OWNER-RUN safety gates.**

---

## Tasks

### Task 1 — KI-1 fix: switch hidapi to the C `linux-static-hidraw` backend + document build deps  **[CI]**

**Why:** the pure-Rust `linux-native` backend does not enumerate `1038:12e5`; the C backend does. (Owner already confirmed `asm-cli list`/`probe` work after the switch on real HW — note that in KNOWN_ISSUES.)

**Step 1.1 — change the dependency.** Edit `Cargo.toml` (workspace root), the `[workspace.dependencies]` line:

```toml
# was: hidapi = { version = "2", default-features = false, features = ["linux-native"] }
hidapi = { version = "2", default-features = false, features = ["linux-static-hidraw"] }
```

**Step 1.2 — build + test.** Run exactly:

```bash
cargo build --workspace
cargo test --workspace
```

Both must pass (the C backend pulls `libudev` + a C compiler; both are present on target). If the build fails with a missing `libudev`/`udev.h`, that is a host-setup error, not a code error — STOP and report it (`sudo dnf install systemd-devel` is the fix, OWNER-RUN).

**Step 1.3 — document build deps.** In `CLAUDE.md` under "## Stack", append to the HID line:

```markdown
- HID via `hidraw` using the `hidapi` **C backend** (`linux-static-hidraw`); the pure-Rust
  `linux-native` backend does NOT enumerate the Nova Pro Wireless. Build deps: `libudev`
  (`systemd-devel`) + a C toolchain (`gcc`/`clang`).
```

In `ARCHITECTURE.md`, find the device/HID section and add the same build-deps note (one sentence).

**Step 1.4 — mark KI-1 RESOLVED.** Edit `KNOWN_ISSUES.md` heading `## KI-1 …` from `(OPEN)` to `(RESOLVED 2026-06-20)` and append:

```markdown
**RESOLUTION (2026-06-20):** root cause was the `hidapi` `linux-native` (pure-Rust) backend
not enumerating this composite wireless device. Switching to the C `linux-static-hidraw`
backend fixes it. Validated on real HW (owner-run): `asm-cli list` finds
"Arctis Nova Pro Wireless (1038:12e5) on interface 4"; `asm-cli probe` reads
`battery_charge: 100%` and `mic_muted: false`. Build deps: `systemd-devel` + C toolchain.
```

**Verify:** `cargo test --workspace` green; `git diff` shows the four edits. (Owner has already confirmed `list`/`probe` work; do not re-run on hardware.)

---

### Task 2 — Descriptor `[commands]` model + Nova Pro command entries (TOML)  **[CI]**

**Why:** declare write commands the same data-driven way status fields are declared (G1).

**Step 2.1 — extend `crates/device/src/descriptor.rs`.** Add the command types and wire them into `DeviceDescriptor`. Write the test FIRST (TDD), then the types.

Add to `DeviceDescriptor` (after `pub status: StatusSpec,`):

```rust
    /// Declarative write commands, keyed by control name. Default empty so
    /// existing read-only descriptors parse unchanged.
    #[serde(default)]
    pub commands: std::collections::BTreeMap<String, CommandSpec>,
```

Add the new types:

```rust
/// A single declarative write command: which opcode bytes to send and how to
/// encode the user-supplied value into the final wire byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Bytes appended after the report id, BEFORE the encoded value byte.
    /// e.g. [0x39] for sidetone -> wire = [report_id, 0x39, <value>].
    pub opcode: Vec<u8>,
    /// The Capability that must be present in `capabilities` for this command
    /// to be enabled. A command whose capability is absent is NOT callable.
    pub capability: Capability,
    /// How to turn the user's requested value into the single wire value byte.
    pub encoding: ValueEncoding,
    /// When true, append a SECOND discrete write of the documented save opcode
    /// after the command write. SAFETY: this is the one allowed extra write and
    /// it is ALWAYS exactly the descriptor's `save_command` bytes — never a burst.
    #[serde(default)]
    pub save: bool,
}

/// Value encoding for a command's single value byte. Mirrors the read `Parser`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ValueEncoding {
    /// Clamp the integer to [min, max] and send it directly as one byte.
    IntRange { min: u8, max: u8 },
    /// Map a named choice to a fixed byte. Used for enums like ANC mode.
    Enum { entries: Vec<EnumEntry> },
}
```

Add to `StatusSpec`'s sibling area an optional save command on the descriptor:

```rust
// In DeviceDescriptor, after `commands`:
    /// The documented save/commit opcode bytes (after report id), e.g. [0x09].
    /// Sent as its own single report when a command has `save = true`.
    #[serde(default)]
    pub save_command: Option<Vec<u8>>,
```

**Step 2.2 — failing test** in `descriptor.rs` `mod tests`:

```rust
#[test]
fn parses_a_command_entry_with_int_range_and_save() {
    let src = r#"
        name = "T"
        vendor_id = 0x1038
        product_ids = [0x12e5]
        interface = 4
        report_id = 0x06
        report_length = 64
        capabilities = ["sidetone"]
        save_command = [0x09]

        [status]
        request = [0xb0]

        [commands.sidetone]
        opcode = [0x39]
        capability = "sidetone"
        save = true
        encoding = { type = "int_range", min = 0, max = 3 }
    "#;
    let d = parse_descriptor(src).expect("should parse");
    let c = d.commands.get("sidetone").expect("sidetone command");
    assert_eq!(c.opcode, vec![0x39]);
    assert_eq!(c.capability, Capability::Sidetone);
    assert!(c.save);
    assert_eq!(c.encoding, ValueEncoding::IntRange { min: 0, max: 3 });
    assert_eq!(d.save_command, Some(vec![0x09]));
}

#[test]
fn existing_read_only_descriptor_parses_with_empty_commands() {
    // The minimal descriptor (no [commands]) must still parse; commands empty.
    let src = r#"
        name = "T"
        vendor_id = 0x1038
        product_ids = [0x12e5]
        interface = 4
        report_id = 0x06
        report_length = 64
        capabilities = []
        [status]
        request = [0xb0]
    "#;
    let d = parse_descriptor(src).expect("should parse");
    assert!(d.commands.is_empty());
    assert_eq!(d.save_command, None);
}
```

**Step 2.3 — export the new types** in `crates/device/src/lib.rs`:

```rust
pub use descriptor::{
    parse_descriptor, CommandSpec, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec,
    ValueEncoding,
};
```

**Step 2.4 — add the command entries to `devices/steelseries_nova_pro_wireless.toml`.**
SAFETY: ship ONLY the HeadsetControl-corroborated commands' capabilities live; author the higher-risk ones but DO NOT add their capability to the `capabilities` list yet (gated to their OWNER-RUN tasks). Append:

```toml
# Documented save/commit opcode (HeadsetControl sendCommandWithSave).
save_command = [0x09]

# ── WRITE COMMANDS ─────────────────────────────────────────────────────────
# Each is a SINGLE opcode. No bursts. No OLED. Capability-gated.
# HeadsetControl-corroborated for THIS model (lib/devices/..nova_pro_wireless.hpp):

[commands.sidetone]                 # { 0x06, 0x39, 0..3 } + save
opcode = [0x39]
capability = "sidetone"
save = true
encoding = { type = "int_range", min = 0, max = 3 }

[commands.mic_led]                  # { 0x06, 0xbf, 1..10 } + save  (LED strength)
opcode = [0xbf]
capability = "mic_led"
save = true
encoding = { type = "int_range", min = 1, max = 10 }

[commands.inactive_time]           # { 0x06, 0xc1, 0..6 } + save   (auto-off level)
opcode = [0xc1]
capability = "inactive_time"
save = true
encoding = { type = "int_range", min = 0, max = 6 }

# Cross-check against the validated READ path ([0x07,0xbd] -> anc_mode):
[commands.anc]                     # { 0x06, 0xbd, 0=off|1=transparency|2=on }
opcode = [0xbd]
capability = "anc"
save = true
encoding = { type = "enum", entries = [
    { value = 0, label = "off" },
    { value = 1, label = "transparency" },
    { value = 2, label = "on" },
] }

# HIGHER RISK — NOT corroborated by HeadsetControl for this model. Authored but
# their capability must NOT be added to `capabilities` until their OWNER-RUN gate
# (Tasks 7d/7e) passes. Keep them here for the encoder; the gate flips them on.
[commands.transparency_level]      # { 0x06, 0xb9, 1..10 }
opcode = [0xb9]
capability = "anc"                  # reuses anc capability; still gated by Task 7d
save = true
encoding = { type = "int_range", min = 1, max = 10 }

[commands.mic_volume]              # { 0x06, 0x37, 1..10 }  -- ships OFF (Task 7f)
opcode = [0x37]
capability = "mic_volume"
save = true
encoding = { type = "int_range", min = 1, max = 10 }
```

Leave the existing `capabilities = [...]` line **as-is for this task** — it already lists `sidetone, anc, mic_volume, inactive_time, mic_led` etc. The *gate* for enabling each write is enforced in the CONTROLLER (Task 4) + OWNER-RUN tasks, not by editing this list per-task. (Rationale: the read capabilities already legitimately list these; the controller refuses to *write* a command until that command's OWNER-RUN gate is signed off — see Task 4's `enabled_writes` allowlist.)

**Verify:** `cargo test -p arctis-device` green; `Registry::builtin()` still loads.

---

### Task 3 — Command encoder + serialized `write_command` over Transport  **[CI]**

**Why:** one generic encoder (mirror of `decode_frame`) + one writer that sends exactly one report.

**Step 3.1 — write failing tests** in `crates/device/src/codec.rs` `mod tests`.

```rust
#[test]
fn encode_command_builds_padded_report_with_report_id_opcode_value() {
    let d = nova();
    // sidetone level 2 -> [0x06, 0x39, 0x02, 0,0,...] len 64
    let report = encode_command(&d, "sidetone", 2).expect("encode");
    assert_eq!(report.len(), d.report_length);
    assert_eq!(report[0], 0x06, "report_id first");
    assert_eq!(report[1], 0x39, "opcode");
    assert_eq!(report[2], 0x02, "encoded value");
    assert!(report[3..].iter().all(|&b| b == 0), "rest zero-padded");
}

#[test]
fn encode_command_int_range_clamps() {
    let d = nova();
    // sidetone max is 3; request 9 -> clamps to 3
    let report = encode_command(&d, "sidetone", 9).expect("encode");
    assert_eq!(report[2], 3, "value clamps to max");
}

#[test]
fn encode_command_enum_maps_label_value() {
    let d = nova();
    // anc enum: value 1 == transparency. encode_command takes the wire value (i64).
    let report = encode_command(&d, "anc", 1).expect("encode");
    assert_eq!(report[1], 0xbd);
    assert_eq!(report[2], 1);
}

#[test]
fn encode_command_unknown_returns_error() {
    let d = nova();
    let err = encode_command(&d, "oled", 0).unwrap_err();
    assert!(matches!(err, DeviceError::Unsupported(_)));
}

#[test]
fn write_command_sends_exactly_one_report_then_save() {
    let d = nova();
    let mut t = MockTransport::new();
    // sidetone has save = true -> expect 2 writes: the command, then save.
    write_command(&mut t, &d, "sidetone", 1).expect("write");
    assert_eq!(t.written.len(), 2, "command + save = exactly two writes");
    assert_eq!(t.written[0][0], 0x06);
    assert_eq!(t.written[0][1], 0x39);
    assert_eq!(t.written[0][2], 1);
    // save report = [0x06, 0x09, 0, ...]
    assert_eq!(t.written[1][0], 0x06);
    assert_eq!(t.written[1][1], 0x09);
    assert!(t.written[1][2..].iter().all(|&b| b == 0));
    assert_eq!(t.written[1].len(), d.report_length);
}

#[test]
fn write_command_no_save_sends_one_report() {
    // Build a descriptor with a save=false command to prove no extra write.
    let d = parse_descriptor(r#"
        name="T" vendor_id=0x1038 product_ids=[0x12e5] interface=4
        report_id=0x06 report_length=64 capabilities=["sidetone"]
        [status]
        request=[0xb0]
        [commands.sidetone]
        opcode=[0x39]
        capability="sidetone"
        encoding = { type = "int_range", min = 0, max = 3 }
    "#).unwrap();
    let mut t = MockTransport::new();
    write_command(&mut t, &d, "sidetone", 2).unwrap();
    assert_eq!(t.written.len(), 1, "no save -> exactly one write");
}

#[test]
fn write_command_surfaces_transport_error() {
    // A transport that errors on write must propagate (never swallow).
    struct FailWrite;
    impl Transport for FailWrite {
        fn write_report(&mut self, _d: &[u8]) -> Result<(), TransportError> {
            Err(TransportError::Io("boom".into()))
        }
        fn read_report(&mut self, _b: &mut [u8], _t: i32) -> Result<usize, TransportError> {
            Err(TransportError::Timeout)
        }
    }
    let d = nova();
    let err = write_command(&mut FailWrite, &d, "sidetone", 1).unwrap_err();
    assert!(matches!(err, DeviceError::Transport(_)));
}
```

**Step 3.2 — add a typed `DeviceError`** in a new `crates/device/src/error.rs`:

```rust
use crate::transport::TransportError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("device transport error: {0}")]
    Transport(#[from] TransportError),
    #[error("unsupported command: {0}")]
    Unsupported(String),
    #[error("invalid value for command '{cmd}': {detail}")]
    InvalidValue { cmd: String, detail: String },
    #[error("device not connected")]
    NotConnected,
}
```

Add `pub mod error;` + `pub use error::DeviceError;` to `lib.rs`.

**Step 3.3 — implement the encoder + writer** in `codec.rs`:

```rust
use crate::descriptor::ValueEncoding;
use crate::error::DeviceError;

/// Encode a single write command into a fully-formed, zero-padded report.
/// `value` is the wire value (enum: the numeric value, int_range: the user int).
/// SAFETY: builds exactly ONE report — report_id + opcode + one value byte,
/// padded to report_length. No init bytes, no extra opcodes.
pub fn encode_command(
    desc: &DeviceDescriptor,
    name: &str,
    value: i64,
) -> Result<Vec<u8>, DeviceError> {
    let spec = desc
        .commands
        .get(name)
        .ok_or_else(|| DeviceError::Unsupported(name.to_string()))?;

    let wire_value: u8 = match &spec.encoding {
        ValueEncoding::IntRange { min, max } => {
            let clamped = value.clamp(i64::from(*min), i64::from(*max));
            clamped as u8
        }
        ValueEncoding::Enum { entries } => {
            let v = u8::try_from(value).map_err(|_| DeviceError::InvalidValue {
                cmd: name.to_string(),
                detail: format!("{value} out of byte range"),
            })?;
            if !entries.iter().any(|e| e.value == v) {
                return Err(DeviceError::InvalidValue {
                    cmd: name.to_string(),
                    detail: format!("{v} is not a valid choice"),
                });
            }
            v
        }
    };

    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(&spec.opcode);
    report.push(wire_value);
    if report.len() > desc.report_length {
        return Err(DeviceError::InvalidValue {
            cmd: name.to_string(),
            detail: "opcode longer than report_length".into(),
        });
    }
    report.resize(desc.report_length, 0);
    Ok(report)
}

/// Build a save/commit report from `desc.save_command` (single opcode, padded).
fn encode_save(desc: &DeviceDescriptor) -> Option<Vec<u8>> {
    let save = desc.save_command.as_ref()?;
    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(save);
    report.resize(desc.report_length, 0);
    Some(report)
}

/// Send exactly one command (and at most one save report). SERIALIZED by the
/// single &mut Transport. Surfaces every transport error — never swallows.
pub fn write_command<T: Transport>(
    transport: &mut T,
    desc: &DeviceDescriptor,
    name: &str,
    value: i64,
) -> Result<(), DeviceError> {
    let report = encode_command(desc, name, value)?;
    transport.write_report(&report)?;

    let spec = desc
        .commands
        .get(name)
        .ok_or_else(|| DeviceError::Unsupported(name.to_string()))?;
    if spec.save {
        if let Some(save_report) = encode_save(desc) {
            transport.write_report(&save_report)?;
        }
    }
    Ok(())
}
```

**Step 3.4 — export** `encode_command`, `write_command` from `lib.rs`.

**Verify:** `cargo test -p arctis-device` green. Confirm the exact-byte assertions pass against the documented sidetone/anc opcodes.

---

### Task 4 — Capability-gated `DeviceController` (read + write + enabled-writes allowlist)  **[CI]**

**Why:** a single object owning a `Transport`, exposing `read()` and a `set(name, value)` that refuses any write whose **OWNER-RUN gate has not been signed off** (the `enabled_writes` allowlist) AND whose capability is absent.

**Step 4.1 — failing tests** in a new `crates/device/src/controller.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockTransport;
    use crate::registry::Registry;
    use arctis_domain::DeviceId;

    fn nova() -> crate::DeviceDescriptor {
        Registry::builtin().unwrap().find(DeviceId::new(0x1038, 0x12e5)).unwrap().clone()
    }

    #[test]
    fn set_refuses_command_not_in_enabled_writes() {
        let d = nova();
        let mut c = DeviceController::new(MockTransport::new(), d)
            .with_enabled_writes(&[]); // nothing enabled yet
        let err = c.set("sidetone", 1).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)),
            "a write must be refused until its OWNER-RUN gate enables it");
        // ...and NOTHING was written.
        assert!(c.transport().written.is_empty());
    }

    #[test]
    fn set_writes_when_enabled_and_capability_present() {
        let d = nova();
        let mut c = DeviceController::new(MockTransport::new(), d)
            .with_enabled_writes(&["sidetone"]);
        c.set("sidetone", 2).expect("enabled write succeeds");
        assert_eq!(c.transport().written[0][1], 0x39);
        assert_eq!(c.transport().written[0][2], 2);
    }

    #[test]
    fn set_refuses_when_capability_absent_even_if_enabled() {
        // Descriptor without `mic_led` capability but command present + enabled.
        let mut d = nova();
        d.capabilities.retain(|c| *c != arctis_domain::Capability::MicLed);
        let mut c = DeviceController::new(MockTransport::new(), d)
            .with_enabled_writes(&["mic_led"]);
        let err = c.set("mic_led", 5).unwrap_err();
        assert!(matches!(err, DeviceError::Unsupported(_)));
    }

    #[test]
    fn read_delegates_to_read_status() {
        let d = nova();
        let frame = {
            let mut f = vec![0u8; 64];
            f[0] = 0x06; f[1] = 0xb0; f[6] = 8; f[9] = 0;
            f
        };
        let mut c = DeviceController::new(MockTransport::new().with_response(frame), d);
        let state = c.read().expect("read ok");
        assert_eq!(state.fields.get("battery_charge"),
            Some(&arctis_domain::StatusValue::Percentage(100)));
    }
}
```

**Step 4.2 — implement** `controller.rs`:

```rust
use crate::codec::{read_status, write_command};
use crate::descriptor::DeviceDescriptor;
use crate::error::DeviceError;
use crate::transport::Transport;
use arctis_domain::DeviceState;

/// Owns the single device transport. The ONLY thing that reads/writes the device.
/// Writes are gated twice: by the descriptor capability AND by the runtime
/// `enabled_writes` allowlist (which the OWNER-RUN validation gates populate).
pub struct DeviceController<T: Transport> {
    transport: T,
    descriptor: DeviceDescriptor,
    enabled_writes: Vec<String>,
}

impl<T: Transport> DeviceController<T> {
    pub fn new(transport: T, descriptor: DeviceDescriptor) -> Self {
        Self { transport, descriptor, enabled_writes: Vec::new() }
    }

    /// Builder: declare which write command names are OWNER-VALIDATED + enabled.
    pub fn with_enabled_writes(mut self, names: &[&str]) -> Self {
        self.enabled_writes = names.iter().map(|s| s.to_string()).collect();
        self
    }

    #[cfg(test)]
    pub(crate) fn transport(&self) -> &T { &self.transport }

    /// Read a full status snapshot. Safe; best-effort merge of frames.
    pub fn read(&mut self) -> Result<DeviceState, DeviceError> {
        Ok(read_status(&mut self.transport, &self.descriptor)?)
    }

    /// Send a single write command. Refuses unless (a) it is in enabled_writes
    /// AND (b) its capability is present in the descriptor.
    pub fn set(&mut self, name: &str, value: i64) -> Result<(), DeviceError> {
        if !self.enabled_writes.iter().any(|n| n == name) {
            return Err(DeviceError::Unsupported(format!(
                "{name} is not enabled (no validated OWNER-RUN gate)"
            )));
        }
        let spec = self.descriptor.commands.get(name)
            .ok_or_else(|| DeviceError::Unsupported(name.to_string()))?;
        if !self.descriptor.capabilities.contains(&spec.capability) {
            return Err(DeviceError::Unsupported(format!(
                "{name} capability not advertised by device"
            )));
        }
        write_command(&mut self.transport, &self.descriptor, name, value)
    }
}
```

Add `pub mod controller;` + `pub use controller::DeviceController;` to `lib.rs`.

**The `enabled_writes` allowlist is the SAFETY GATE in code.** It ships **EMPTY**. Each OWNER-RUN task in Task 7 adds exactly one name to the list the daemon constructs (Task 5). Until an owner signs off, the daemon refuses that write with a clear error.

**Verify:** `cargo test -p arctis-device` green.

---

### Task 5 — Engine `DeviceWorker` read-loop + graceful absence  **[CI]**

**Why:** populate `EngineState.device_present`/`device_fields` and emit `DeviceState` events from the resident daemon, without crashing on device absence.

**Step 5.1 — make the worker testable with a transport factory.** The worker must be unit-testable with `MockTransport`. Define a trait the engine depends on:

```rust
// crates/engine/src/device.rs  (NEW)
use arctis_device::{DeviceController, DeviceError, Transport};
use std::collections::BTreeMap;

/// Opens the device transport on demand. Real impl uses HidrawTransport +
/// discover(); tests inject a closure returning a MockTransport.
pub trait DeviceOpener: Send + 'static {
    type T: Transport;
    /// Returns Ok(None) when no device is connected (graceful), Err on real IO fault.
    fn open(&self) -> Result<Option<(DeviceController<Self::T>, Vec<String>)>, DeviceError>;
}
```

`open` returns the controller **already configured with the enabled-writes allowlist** (so the allowlist lives in one place — the real opener — and OWNER-RUN tasks edit exactly that list).

**Step 5.2 — the snapshot type + flattening.** Add to `crates/engine/src/state.rs` a helper that converts a `DeviceState` (`BTreeMap<String, StatusValue>`) into the `BTreeMap<String, String>` the existing `device_fields` already uses, plus a `device_present` bool. Reuse the rendering already in `asm-cli probe`:

```rust
// in state.rs
pub fn render_device_fields(
    state: &arctis_domain::DeviceState,
) -> std::collections::BTreeMap<String, String> {
    use arctis_domain::StatusValue;
    state.fields.iter().map(|(k, v)| {
        let s = match v {
            StatusValue::Percentage(p) => format!("{p}"),
            StatusValue::Bool(b) => b.to_string(),
            StatusValue::Enum(e) => e.clone(),
            StatusValue::Int(i) => i.to_string(),
        };
        (k.clone(), s)
    }).collect()
}
```

(Battery is rendered as a bare number string so the existing `DevicePage` `batteryColor`/`{row.value}%` logic works — see DevicePage.svelte `kindForKey("battery")`.)

**Step 5.3 — shared device state on the engine.** Add to `Engine`:

```rust
device: std::sync::Arc<std::sync::Mutex<DeviceShared>>,
```

where

```rust
#[derive(Default, Clone)]
pub struct DeviceShared {
    pub present: bool,
    pub fields: std::collections::BTreeMap<String, String>,
}
```

`Engine::state()` now reads this:

```rust
let dev = self.device.lock().map(|d| d.clone()).unwrap_or_default();
// ...
device_present: dev.present,
device_fields: dev.fields,
```

(Replace the hardcoded `device_present: false` / empty map.)

**Step 5.4 — the read-loop.** A function that owns a controller and loops:

```rust
// crates/engine/src/device.rs
pub fn run_read_loop<O: DeviceOpener>(
    opener: O,
    shared: std::sync::Arc<std::sync::Mutex<crate::DeviceShared>>,
    events: Option<std::sync::mpsc::Sender<crate::state::Event>>,
    poll: std::time::Duration,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::sync::atomic::Ordering;
    while !stop.load(Ordering::Relaxed) {
        match opener.open() {
            Ok(Some((mut controller, _enabled))) => {
                // Read until error/disconnect, then fall back to re-open.
                while !stop.load(Ordering::Relaxed) {
                    match controller.read() {
                        Ok(state) => {
                            let fields = crate::state::render_device_fields(&state);
                            if let Ok(mut g) = shared.lock() {
                                g.present = true;
                                g.fields = fields.clone();
                            }
                            if let Some(tx) = &events {
                                let _ = tx.send(crate::state::Event::DeviceState { fields });
                            }
                        }
                        Err(_) => {
                            // disconnect / transient: mark absent, break to re-open.
                            if let Ok(mut g) = shared.lock() { g.present = false; }
                            break;
                        }
                    }
                    std::thread::sleep(poll);
                }
            }
            Ok(None) => {
                if let Ok(mut g) = shared.lock() { g.present = false; }
            }
            Err(_) => {
                if let Ok(mut g) = shared.lock() { g.present = false; }
            }
        }
        std::thread::sleep(poll);
    }
}
```

No `unwrap`; every lock/read/IO failure degrades to "absent" and retries. Never panics, never exits the daemon.

**Step 5.5 — failing tests** in `device.rs`:

```rust
#[test]
fn read_loop_populates_shared_state_then_stops() { /* MockOpener returns a controller
    whose MockTransport yields one battery frame; run loop in a thread with a short poll,
    assert shared.present == true and fields["battery_charge"] == "100", then set stop. */ }

#[test]
fn read_loop_marks_absent_when_opener_returns_none() { /* MockOpener -> Ok(None);
    assert shared.present == false; loop must not panic. */ }

#[test]
fn read_loop_survives_opener_error() { /* MockOpener -> Err(NotConnected);
    present stays false, no panic, loop exits on stop. */ }
```

(Use a `MockOpener` struct in the test module wrapping a `MockTransport` factory.)

**Step 5.6 — wire it in the daemon.** In `crates/cli/src/daemon.rs` `run_daemon()`, after building the engine, construct the real opener and spawn the read-loop thread:

```rust
// real opener
struct HidOpener;
impl arctis_engine::DeviceOpener for HidOpener {
    type T = arctis_device::HidrawTransport;
    fn open(&self) -> Result<Option<(arctis_device::DeviceController<Self::T>, Vec<String>)>, arctis_device::DeviceError> {
        let registry = arctis_device::Registry::builtin()
            .map_err(|e| arctis_device::DeviceError::Unsupported(e.to_string()))?;
        match arctis_device::discover(&registry)? {
            Some((id, iface)) => {
                let desc = registry.find(id)
                    .ok_or(arctis_device::DeviceError::NotConnected)?.clone();
                let transport = arctis_device::HidrawTransport::open(id, iface)?;
                // SAFETY GATE: enabled_writes starts EMPTY. OWNER-RUN tasks (Task 7)
                // add one name at a time AFTER real-HW validation. Do NOT add a name
                // here unless its OWNER-RUN gate in this plan is signed off.
                let enabled: Vec<String> = vec![/* filled by Task 7 gates */];
                let controller = arctis_device::DeviceController::new(transport, desc)
                    .with_enabled_writes(&enabled.iter().map(|s| s.as_str()).collect::<Vec<_>>());
                Ok(Some((controller, enabled)))
            }
            None => Ok(None),
        }
    }
}
```

Spawn the thread, hold the `stop` flag + `JoinHandle` so daemon shutdown joins it. (The worker also needs to receive *write* commands — see Task 6 for the command channel; in Task 5 the worker is read-only.)

**Verify:** `cargo test -p arctis-engine` green; `cargo build --workspace` green. With a device connected (owner), `asm-cli daemon` + the GUI shows live battery (validated visually in Task 8 / Task 7a).

> **[OWNER-RUN] Task 5 validation gate — READS.** With the headset on the base station, run the daemon and `asm-cli device status` (Task 7). Toggle ANC on the headset's dial/base, twist the chat-mix dial, mute/unmute the mic — re-run `device status` (or watch the GUI) and confirm `anc_mode`, `media_mix`/`chat_mix`, and `mic_muted` change accordingly, and `battery_charge` is plausible. Reads are non-destructive; this gate just confirms the loop is live.

---

### Task 6 — Engine device-control methods + command channel + daemon verbs + client Request variants  **[CI]**

**Why:** route validated writes from CLI/GUI → daemon → the single worker thread (the one serialized writer).

**Step 6.1 — command channel to the worker.** The worker owns the controller, so writes must be sent to it, not done on the engine thread. Add an `mpsc` of write requests handled inside the read-loop between reads:

```rust
// device.rs
pub enum DeviceCommand {
    Set { name: String, value: i64, reply: std::sync::mpsc::Sender<Result<(), String>> },
}
```

Extend `run_read_loop` to accept a `Receiver<DeviceCommand>` and, each iteration, drain pending commands and call `controller.set(...)`, sending the stringified result back on `reply`. SAFETY: the controller's `enabled_writes` gate still applies — a not-yet-validated command returns `Unsupported`, surfaced as the reply error. Writes and reads on the same thread = serialized writer (Global Constraint).

**Step 6.2 — engine setter.** Add to `Engine`:

```rust
device_tx: Option<std::sync::mpsc::Sender<crate::device::DeviceCommand>>,
```

and a method (TDD'd with a fake receiver):

```rust
/// Send a validated device write through the worker. Surfaces failures (never swallows).
pub fn device_set(&self, name: &str, value: i64) -> Result<(), EngineError> {
    let tx = self.device_tx.as_ref().ok_or_else(||
        EngineError::BadRequest("device worker not running".into()))?;
    let (reply_tx, reply_rx) = std::sync::mpsc::channel();
    tx.send(crate::device::DeviceCommand::Set {
        name: name.to_string(), value, reply: reply_tx,
    }).map_err(|_| EngineError::BadRequest("device worker gone".into()))?;
    reply_rx.recv()
        .map_err(|_| EngineError::BadRequest("no reply from device worker".into()))?
        .map_err(EngineError::Device)
}
```

Add an `EngineError::Device(String)` variant (or wrap `arctis_device::DeviceError`) in `crates/engine/src/error.rs`.

**Step 6.3 — client Request variant.** In `crates/client/src/protocol.rs`, add ONE generic verb (avoids a verb per control — data-driven, G1):

```rust
/// Set a single device hardware control by name (sidetone|mic_led|anc|inactive_time|...).
DeviceSet { control: String, value: i64 },
```

with kebab-case `device-set`. Add round-trip + parse tests mirroring the existing ones:

```rust
#[test]
fn parse_device_set() {
    let req: Request = serde_json::from_str(
        r#"{"cmd":"device-set","control":"sidetone","value":2}"#).unwrap();
    assert_eq!(req, Request::DeviceSet { control: "sidetone".into(), value: 2 });
}
```

**Step 6.4 — daemon dispatch.** In `crates/cli/src/daemon.rs` `handle_request`:

```rust
Request::DeviceSet { control, value } => match engine.device_set(&control, value) {
    Ok(()) => Response::ok_with_state(engine.state()),
    Err(e) => Response::err(e.to_string()),
},
```

Add a daemon test with a MockRunner engine wired to a fake worker channel asserting that `device-set` for a non-enabled control returns `ok:false` with the gate error, and an enabled one returns `ok:true`.

**Step 6.5 — Tauri command.** In `src-tauri/src/commands.rs` add:

```rust
#[tauri::command]
pub async fn device_set(
    control: String,
    value: i64,
    state: State<'_, Mutex<DaemonState>>,
) -> Result<EngineState, CommandError> {
    call(&state, Request::DeviceSet { control, value }).await
}
```

Register it in the `invoke_handler` list. Add the JS wrapper in `frontend/src/lib/ipc.ts`:

```ts
export const deviceSet = (control: string, value: number): Promise<EngineState> =>
  invoke<EngineState>("device_set", { control, value });
```

**Verify:** `cargo test --workspace` green; `pnpm -C frontend test` green (ipc wrapper unit test).

---

### Task 7 — `asm-cli device` subcommands + per-control OWNER-RUN safety gates

**Step 7.1 — CLI subcommands** in `crates/cli/src/main.rs`. Add a `Device` command:

```rust
/// Headset hardware control (live reads; gated writes).
Device {
    #[command(subcommand)]
    action: DeviceAction,
},
```

```rust
#[derive(Subcommand, Debug)]
enum DeviceAction {
    /// Read and print live device status (battery, ANC, mic, dial). Read-only.
    Status,
    /// Set sidetone level 0..3.
    Sidetone { level: i64 },
    /// Set mic LED brightness 1..10.
    MicLed { level: i64 },
    /// Set ANC mode: off | transparency | on.
    Anc { mode: String },
    /// Set auto-off level 0..6 (0=never .. 6=60min).
    AutoOff { level: i64 },
    /// Set transparency level 1..10. (HIGHER RISK — gated.)
    Transparency { level: i64 },
    /// Set mic volume 1..10. (HIGHER RISK — gated.)
    MicVolume { level: i64 },
}
```

Dispatch: `Status` → `Request::GetState` (or a direct one-shot read via `discover` + `DeviceController::read` when no daemon) and print `device_fields`; the setters → `Request::DeviceSet { control, value }` (mapping `anc` mode string → 0/1/2). All go through the daemon first (live worker), falling back to a clear "start the daemon" error for writes (writes must use the single worker; do NOT open a second transport for writes). Add clap parse tests mirroring existing ones.

`asm-cli device status` prints each field; the setters print the returned battery/anc/etc. for confirmation and **print the daemon's error verbatim on failure** (surface failures).

**Verify (CI):** `cargo test -p arctis-cli` (clap parse tests) green.

---

The remaining Task 7 sub-steps are **[OWNER-RUN] SAFETY GATES**. Each enables exactly ONE write by adding its name to the `enabled` vec in `HidOpener::open` (Task 5.6), rebuilding, and validating on the real headset. **Do these one at a time, in this order (safest first). Do NOT enable the next until the current one is confirmed.**

> **[OWNER-RUN] Task 7a — SIDETONE gate (FIRST write — also validates report padding).**
> 1. Edit `enabled` in `HidOpener::open` to `vec!["sidetone".into()]`. `cargo build --workspace`.
> 2. Start the daemon: `asm-cli daemon`. In another terminal: `asm-cli device sidetone 1`, then `2`, then `3`, then `0`.
> 3. **Listen:** with the mic active, sidetone should make your own voice audible in the headset, increasing with the level, off at 0.
> 4. If it works → sidetone is validated; keep it enabled. If NOTHING changes, try the unpadded fallback: temporarily make `write_command` send `report[..1+opcode+1]` (the short 3-byte form) instead of the padded 64; rebuild and retest. Record which form works in `KNOWN_ISSUES.md` and set the convention for ALL commands accordingly.
> 5. If the headset misbehaves in ANY unexpected way (disconnects, weird state), STOP, unplug/replug, and report before proceeding.

> **[OWNER-RUN] Task 7b — MIC-LED gate.** Add `"mic_led"` to `enabled`, rebuild. `asm-cli device mic-led 10`, `5`, `1`. **Watch:** the mic LED brightness should change. Confirm, then keep enabled.

> **[OWNER-RUN] Task 7c — AUTO-OFF gate.** Add `"inactive_time"`, rebuild. `asm-cli device auto-off 3`. **Verify:** re-read via `asm-cli device status` (if the device reports it) and/or check the setting persists in SteelSeries GG / base-station menu. Confirm no adverse effect, keep enabled.

> **[OWNER-RUN] Task 7d — ANC gate (cross-checked against the READ path).** Add `"anc"` to `enabled`, rebuild. `asm-cli device anc transparency`, then `on`, then `off`. **Verify both ways:** (a) audibly — transparency lets ambient sound in, "on" cancels it; (b) `asm-cli device status` `anc_mode` field must now reflect what you just set (this is the strongest validation — the write round-trips through the independent read path). Confirm, keep enabled. THEN test `transparency_level`: with ANC in transparency mode, `asm-cli device transparency 1`..`10` and listen for the ambient level changing. (transparency_level is HIGHER RISK / not in HeadsetControl — only keep it enabled if it clearly works and the device stays healthy.)

> **[OWNER-RUN] Task 7e — (HIGHER RISK, OPTIONAL) confirm transparency_level / 2.4G remain DEFERRED if uncertain.** If 7d's transparency-level test is ambiguous, leave `transparency_level` effectively unused (the `anc` capability gates it; document it as "untested" in KNOWN_ISSUES and don't surface it in the GUI). Do NOT enable `0xc3` 2.4G in this plan.

> **[OWNER-RUN] Task 7f — MIC-VOLUME gate (HIGHEST RISK — HeadsetControl does NOT implement this for this model).** Only if you want it: add `"mic_volume"`, rebuild. `asm-cli device mic-volume 5`, then `10`, then `1`. **Verify:** record yourself / watch input level in `pavucontrol`/`wpctl` — gain should change. If the opcode does nothing or the device acts oddly, REMOVE it from `enabled` and document `mic_volume` as unsupported in `KNOWN_ISSUES.md`. This control ships OFF by default.

After each gate passes, the corresponding write is live in the daemon; failures (e.g. before a gate is signed off) return a clear `Unsupported`/transport error to the CLI/GUI.

---

### Task 8 — GUI Device panel: live data + validated controls  **[OWNER-RUN visual]**

**Why:** the headline payoff — battery/ANC/mic come alive and the user can drive the validated controls.

**Step 8.1 — live status (CI-buildable logic).** `DevicePage.svelte` already renders `device_fields` when `device_present`. With Task 5 live, the existing path "just works." Extend `frontend/src/lib/components/DevicePage.svelte` to add **control widgets** for the enabled writes, calling `deviceSet` from `ipc.ts`:
- Sidetone: a 0–3 segmented control → `deviceSet("sidetone", level)`.
- Mic LED: a 1–10 slider → `deviceSet("mic_led", level)`.
- ANC: off/transparency/on toggle → `deviceSet("anc", 0|1|2)`.
- Auto-off: a 0–6 select → `deviceSet("inactive_time", level)`.
- (transparency_level / mic_volume: render ONLY if their gate passed.)

Each control: optimistic apply from the returned `EngineState`, and on a rejected promise show the error inline (surface failures — do not silently fail). Gate each control's visibility behind `device_present` AND a small `enabledControls` set the frontend derives (e.g. only show ANC if `anc_mode` is present in `device_fields`).

**Step 8.2 — pure-logic tests** in `frontend/src/lib/device.test.ts`: extend `mapDeviceFields` coverage for the new live fields; add a test for the ANC mode→value mapping helper. `pnpm -C frontend test` green (CI).

> **[OWNER-RUN] Task 8 visual gate.** Owner runs `pnpm tauri dev` with the headset connected and the daemon running. Confirm: topbar/battery shows the real percentage; the Device panel shows live ANC/mic/dial; each enabled control drives the headset (matching the Task 7 gates) and shows an error toast/inline message if the daemon returns an error. Controls for un-validated writes must NOT appear (or appear disabled).

---

## Self-Review

**Coverage of the brief:**
- Task 1 fixes KI-1 (hidapi C backend), updates KNOWN_ISSUES/CLAUDE/ARCHITECTURE, build verified. ✔
- Task 2 adds the data-driven `[commands]` TOML model + real Nova Pro entries (exact documented opcodes). ✔ (G1 — no per-control hardcoding.)
- Task 3 adds the generic encoder + the single serialized `write_command` (one report + optional one save), with exact-byte MockTransport tests and error-surfacing. ✔
- Task 4 adds the capability-gated `DeviceController` with an `enabled_writes` allowlist — the in-code safety gate. ✔
- Task 5 adds the engine read-loop populating `EngineState.device_*` + events, graceful on absence, MockTransport-tested. ✔
- Task 6 adds the command channel (single writer), engine setter, generic `device-set` verb, daemon dispatch, Tauri command, JS wrapper. ✔
- Task 7 adds `asm-cli device` subcommands and one explicit OWNER-RUN safety gate PER write control, ordered safest-first. ✔
- Task 8 brings the GUI Device panel alive with live data + validated controls. ✔
- Non-goals stated: OLED (never), init-replay (never), HW-EQ bands + preset, chat-mix dial write, 2.4G, Bluetooth, firmware. ✔

**Safety-gate model (two independent gates per write):**
1. **Compile/runtime gate:** `DeviceController.enabled_writes` ships EMPTY; the daemon's `HidOpener` adds one name only after that control's OWNER-RUN step. An un-gated write returns `Unsupported` — it never sends bytes.
2. **Human gate:** each OWNER-RUN task is a manual real-HW validation that must be confirmed before its name is added. ANC and auto-off are additionally cross-checked against the independent read path.
Plus the structural guarantees: one serialized writer (worker thread owns the sole transport), only single declared opcodes (+ one save), never OLED, never init bursts, all failures surfaced.

**Tasks that are OWNER-RUN safety gates:** Task 5 validation (reads), Task 7a–7f (one per write control), Task 8 visual gate. Task 1's hardware confirm is already done by the owner. Everything else (Tasks 1–6 code, 7.1 CLI parsing, 8.1/8.2 logic) is CI-buildable/verifiable by a subagent.

**Open questions for the executor / owner:**
1. **Report length for writes** — pad to 64 (our read convention, works for reads) vs. send the short 3-byte report HeadsetControl uses in source. Resolved empirically in Task 7a; convention then applied to all commands.
2. **ANC (0xbd) / transparency (0xb9) / mic-volume (0x37)** are NOT in HeadsetControl for this exact model — they rely on the community reference and are the higher-risk gates (7d/7f). If any behaves oddly, leave it disabled and document it; the app is fully useful with just sidetone/mic-led/auto-off/ANC.
3. **EQ preset (0x2e)** is authored-but-deferred; whether to surface a hardware-EQ-preset switch alongside the software EQ needs its own design (future plan).
4. **Auto-off level → minutes** mapping for the GUI label (HeadsetControl buckets minutes into 0–6); confirm the exact minute labels against the device menu during Task 7c.
