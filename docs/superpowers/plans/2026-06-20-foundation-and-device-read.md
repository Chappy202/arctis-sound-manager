# Foundation & Safe Device Read Path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the Cargo workspace and a data-driven, fully-tested **read-only** device layer that can safely query a SteelSeries Arctis Nova Pro Wireless's status (battery, ChatMix dial, ANC, mic-mute) over hidraw, exposed through an `asm-cli probe` command.

**Architecture:** A workspace with three crates for this plan — `domain` (pure types), `device` (descriptor-driven HID codec + `Transport` trait + hidraw impl), and `cli` (`asm-cli`). Device behavior is defined by declarative TOML **descriptors**, decoded by one generic codec. All decoding/encoding logic is unit-tested against recorded byte fixtures via a `MockTransport`; the hidraw path is validated on real hardware.

**Tech Stack:** Rust (edition 2021, stable toolchain), `serde` + `toml` (descriptors), `hidapi` with the pure-Rust `linux-native` feature (hidraw), `clap` (CLI), `thiserror` (errors).

## Global Constraints

- **Hardware safety (ARCHITECTURE G2):** This plan performs **reads only**. No write opcodes are sent. Never write the OLED. Never replay init opcodes.
- **Transport:** Use `hidraw` via `hidapi` `linux-native` feature (no libusb, no kernel-driver detach).
- **Device identity:** vendor `0x1038`; product may be `0x12e0` (standard) **or** `0x12e5` (X). Match both.
- **Report format:** control interface `4`; report id `0x06`; reports padded to `64` bytes.
- **Sample rate / audio:** N/A to this plan (no audio yet) but the project is 48 kHz only.
- **Crate naming:** package `arctis-<name>`, library `arctis_<name>` (e.g. `arctis-device` / `arctis_device`).
- **Error handling (G7):** typed `thiserror` errors across boundaries; no `unwrap()`/`expect()` on runtime-fallible paths.
- **Reuse (G1):** one generic descriptor-driven codec; per-device differences live in TOML descriptors only.

---

### Task 1: Workspace scaffold

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `crates/domain/Cargo.toml`
- Create: `crates/domain/src/lib.rs`
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: a buildable workspace with member `arctis-domain`.

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/domain", "crates/device", "crates/cli"]

[workspace.package]
edition = "2021"
license = "MIT OR Apache-2.0"
rust-version = "1.78"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
toml = "0.8"
thiserror = "1"
clap = { version = "4", features = ["derive"] }
hidapi = { version = "2", default-features = false, features = ["linux-native"] }
arctis-domain = { path = "crates/domain" }
arctis-device = { path = "crates/device" }
```

- [ ] **Step 2: Create `rust-toolchain.toml`**

```toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 3: Create `.gitignore`**

```gitignore
/target
**/*.rs.bk
*.pdb
```

- [ ] **Step 4: Create the `domain` crate manifest** (`crates/domain/Cargo.toml`)

```toml
[package]
name = "arctis-domain"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
```

- [ ] **Step 5: Create a placeholder lib** (`crates/domain/src/lib.rs`)

```rust
//! Pure domain types for Arctis Sound Manager. No I/O.
```

- [ ] **Step 6: Create CI** (`.github/workflows/ci.yml`)

```yaml
name: CI
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: clippy, rustfmt
      - run: cargo fmt --all -- --check
      - run: cargo clippy --workspace --all-targets -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 7: Verify it builds**

Run: `cargo build --workspace`
Expected: compiles (device & cli members are empty dirs — add their manifests in later tasks; if cargo errors on missing members, create the manifests from Tasks 3 and 7 first, or temporarily trim `members` to `["crates/domain"]` and restore it in Task 3).

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml rust-toolchain.toml .gitignore crates/domain .github
git commit -m "chore: scaffold cargo workspace + domain crate + CI"
```

---

### Task 2: Domain types — capabilities, device id, status values

**Files:**
- Create: `crates/domain/src/capability.rs`
- Create: `crates/domain/src/device.rs`
- Create: `crates/domain/src/status.rs`
- Modify: `crates/domain/src/lib.rs`

**Interfaces:**
- Produces:
  - `enum Capability` (serde `snake_case`): `Battery, Sidetone, Anc, MicVolume, InactiveTime, HardwareEq, EqPreset, ChatMix, WirelessMode, MicLed`
  - `struct DeviceId { vendor_id: u16, product_id: u16 }` with `new()` and `Display` as `"1038:12e5"`
  - `enum StatusValue { Percentage(u8), Bool(bool), Enum(String), Int(i64) }`
  - `struct DeviceState { fields: BTreeMap<String, StatusValue> }`

- [ ] **Step 1: Write the failing test** (`crates/domain/src/device.rs`)

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceId {
    pub vendor_id: u16,
    pub product_id: u16,
}

impl DeviceId {
    pub fn new(vendor_id: u16, product_id: u16) -> Self {
        Self { vendor_id, product_id }
    }
}

impl std::fmt::Display for DeviceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04x}:{:04x}", self.vendor_id, self.product_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_displays_as_colon_separated_hex() {
        assert_eq!(DeviceId::new(0x1038, 0x12e5).to_string(), "1038:12e5");
    }
}
```

- [ ] **Step 2: Create capability + status modules**

`crates/domain/src/capability.rs`:

```rust
use serde::{Deserialize, Serialize};

/// A discrete feature a device may support. Drives both what the engine sends
/// and what the UI renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Battery,
    Sidetone,
    Anc,
    MicVolume,
    InactiveTime,
    HardwareEq,
    EqPreset,
    ChatMix,
    WirelessMode,
    MicLed,
}
```

`crates/domain/src/status.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A decoded status field value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StatusValue {
    Percentage(u8),
    Bool(bool),
    Enum(String),
    Int(i64),
}

/// A snapshot of decoded device state, keyed by field name.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceState {
    pub fields: BTreeMap<String, StatusValue>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_state_round_trips_through_json_like_map() {
        let mut s = DeviceState::default();
        s.fields.insert("battery".into(), StatusValue::Percentage(75));
        assert_eq!(s.fields.get("battery"), Some(&StatusValue::Percentage(75)));
    }
}
```

- [ ] **Step 3: Wire up `lib.rs`** (`crates/domain/src/lib.rs`)

```rust
//! Pure domain types for Arctis Sound Manager. No I/O.
pub mod capability;
pub mod device;
pub mod status;

pub use capability::Capability;
pub use device::DeviceId;
pub use status::{DeviceState, StatusValue};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p arctis-domain`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/domain/src
git commit -m "feat(domain): add Capability, DeviceId, StatusValue, DeviceState"
```

---

### Task 3: Device descriptor model + parsing

**Files:**
- Create: `crates/device/Cargo.toml`
- Create: `crates/device/src/lib.rs`
- Create: `crates/device/src/descriptor.rs`

**Interfaces:**
- Consumes: `arctis_domain::Capability`
- Produces:
  - `struct DeviceDescriptor { name, vendor_id: u16, product_ids: Vec<u16>, interface: u8, report_id: u8, report_length: usize, capabilities: Vec<Capability>, status: StatusSpec }`
  - `struct StatusSpec { request: Vec<u8>, fields: Vec<StatusField> }`
  - `struct StatusField { name: String, match_prefix: Vec<u8>, offset: usize, parser: Parser }`
  - `enum Parser { Percentage{min,max}, Bool{true_value}, Enum{entries: Vec<EnumEntry>}, Int }`
  - `struct EnumEntry { value: u8, label: String }`
  - `fn parse_descriptor(&str) -> Result<DeviceDescriptor, toml::de::Error>`

- [ ] **Step 1: Create the `device` crate manifest** (`crates/device/Cargo.toml`)

```toml
[package]
name = "arctis-device"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
arctis-domain = { workspace = true }
serde = { workspace = true }
toml = { workspace = true }
thiserror = { workspace = true }
hidapi = { workspace = true }
```

- [ ] **Step 2: Write the failing test** (`crates/device/src/descriptor.rs`)

```rust
use arctis_domain::Capability;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub name: String,
    pub vendor_id: u16,
    pub product_ids: Vec<u16>,
    pub interface: u8,
    pub report_id: u8,
    pub report_length: usize,
    pub capabilities: Vec<Capability>,
    pub status: StatusSpec,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusSpec {
    /// Bytes appended after the report id to request a status frame.
    pub request: Vec<u8>,
    pub fields: Vec<StatusField>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusField {
    pub name: String,
    pub match_prefix: Vec<u8>,
    pub offset: usize,
    pub parser: Parser,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Parser {
    Percentage { min: u8, max: u8 },
    Bool { true_value: u8 },
    Enum { entries: Vec<EnumEntry> },
    Int,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumEntry {
    pub value: u8,
    pub label: String,
}

/// Parse a device descriptor from TOML source.
pub fn parse_descriptor(src: &str) -> Result<DeviceDescriptor, toml::de::Error> {
    toml::from_str(src)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_descriptor() {
        let src = r#"
            name = "Test Device"
            vendor_id = 0x1038
            product_ids = [0x12e0, 0x12e5]
            interface = 4
            report_id = 0x06
            report_length = 64
            capabilities = ["battery"]

            [status]
            request = [0xb0]

            [[status.fields]]
            name = "battery_charge"
            match_prefix = [0x06, 0xb0]
            offset = 6
            parser = { type = "percentage", min = 0, max = 8 }
        "#;
        let d = parse_descriptor(src).expect("should parse");
        assert_eq!(d.name, "Test Device");
        assert_eq!(d.product_ids, vec![0x12e0, 0x12e5]);
        assert_eq!(d.capabilities, vec![Capability::Battery]);
        assert_eq!(d.status.request, vec![0xb0]);
        assert_eq!(d.status.fields[0].offset, 6);
        assert_eq!(
            d.status.fields[0].parser,
            Parser::Percentage { min: 0, max: 8 }
        );
    }
}
```

- [ ] **Step 3: Create `lib.rs`** (`crates/device/src/lib.rs`)

```rust
//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod descriptor;

pub use descriptor::{parse_descriptor, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec};
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p arctis-device descriptor`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/device
git commit -m "feat(device): add data-driven device descriptor model + TOML parsing"
```

---

### Task 4: Device registry + Nova Pro Wireless descriptor

**Files:**
- Create: `devices/steelseries_nova_pro_wireless.toml`
- Create: `crates/device/src/registry.rs`
- Modify: `crates/device/src/lib.rs`

**Interfaces:**
- Consumes: `DeviceDescriptor`, `parse_descriptor`, `arctis_domain::DeviceId`
- Produces:
  - `struct Registry` with `Registry::builtin() -> Result<Registry, RegistryError>`, `Registry::from_descriptors(Vec<DeviceDescriptor>) -> Registry`, and `fn find(&self, id: DeviceId) -> Option<&DeviceDescriptor>`
  - `enum RegistryError` (thiserror)

- [ ] **Step 1: Create the Nova Pro descriptor** (`devices/steelseries_nova_pro_wireless.toml`)

Status offsets are from reverse-engineering (see project memory `arctis-nova-pro-protocol`); they are validated against real hardware in Task 7. Reads only.

```toml
name = "Arctis Nova Pro Wireless"
vendor_id = 0x1038
product_ids = [0x12e0, 0x12e5]
interface = 4
report_id = 0x06
report_length = 64
capabilities = [
    "battery", "sidetone", "anc", "mic_volume",
    "inactive_time", "hardware_eq", "eq_preset", "chat_mix", "mic_led",
]

[status]
# 0x06 0xb0 = status request. Writer prepends report_id, so request starts at 0xb0.
request = [0xb0]

[[status.fields]]
name = "battery_charge"
match_prefix = [0x06, 0xb0]
offset = 6
parser = { type = "percentage", min = 0, max = 8 }

[[status.fields]]
name = "mic_muted"
match_prefix = [0x06, 0xb0]
offset = 9
parser = { type = "bool", true_value = 1 }

[[status.fields]]
name = "anc_mode"
match_prefix = [0x07, 0xbd]
offset = 2
parser = { type = "enum", entries = [
    { value = 0, label = "off" },
    { value = 1, label = "transparency" },
    { value = 2, label = "on" },
] }

[[status.fields]]
name = "media_mix"
match_prefix = [0x07, 0x45]
offset = 2
parser = { type = "int" }

[[status.fields]]
name = "chat_mix"
match_prefix = [0x07, 0x45]
offset = 3
parser = { type = "int" }
```

- [ ] **Step 2: Write the failing test** (`crates/device/src/registry.rs`)

```rust
use crate::descriptor::{parse_descriptor, DeviceDescriptor};
use arctis_domain::DeviceId;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("failed to parse built-in descriptor '{0}': {1}")]
    Parse(String, String),
}

/// Built-in descriptors compiled into the binary.
const NOVA_PRO_WIRELESS: &str =
    include_str!("../../../devices/steelseries_nova_pro_wireless.toml");

pub struct Registry {
    descriptors: Vec<DeviceDescriptor>,
}

impl Registry {
    pub fn from_descriptors(descriptors: Vec<DeviceDescriptor>) -> Self {
        Self { descriptors }
    }

    /// Load all descriptors compiled into the binary.
    pub fn builtin() -> Result<Self, RegistryError> {
        let nova = parse_descriptor(NOVA_PRO_WIRELESS)
            .map_err(|e| RegistryError::Parse("nova_pro_wireless".into(), e.to_string()))?;
        Ok(Self::from_descriptors(vec![nova]))
    }

    /// Find a descriptor matching a connected device's id.
    pub fn find(&self, id: DeviceId) -> Option<&DeviceDescriptor> {
        self.descriptors.iter().find(|d| {
            d.vendor_id == id.vendor_id && d.product_ids.contains(&id.product_id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_registry_loads_and_finds_nova_pro_by_either_pid() {
        let reg = Registry::builtin().expect("builtin descriptors must parse");
        let std = reg.find(DeviceId::new(0x1038, 0x12e0));
        let x = reg.find(DeviceId::new(0x1038, 0x12e5));
        assert!(std.is_some(), "should match standard pid 12e0");
        assert!(x.is_some(), "should match X pid 12e5");
        assert_eq!(std.unwrap().name, "Arctis Nova Pro Wireless");
    }

    #[test]
    fn unknown_device_is_not_found() {
        let reg = Registry::builtin().unwrap();
        assert!(reg.find(DeviceId::new(0x1234, 0x5678)).is_none());
    }
}
```

- [ ] **Step 3: Export the registry** (`crates/device/src/lib.rs`)

```rust
//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod descriptor;
pub mod registry;

pub use descriptor::{parse_descriptor, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec};
pub use registry::{Registry, RegistryError};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p arctis-device registry`
Expected: PASS (2 tests). This also proves the shipped Nova Pro descriptor is valid TOML.

- [ ] **Step 5: Commit**

```bash
git add devices crates/device/src/registry.rs crates/device/src/lib.rs
git commit -m "feat(device): add registry + Nova Pro Wireless descriptor (read-only)"
```

---

### Task 5: Transport trait, MockTransport, and status codec

**Files:**
- Create: `crates/device/src/transport.rs`
- Create: `crates/device/src/mock.rs`
- Create: `crates/device/src/codec.rs`
- Modify: `crates/device/src/lib.rs`

**Interfaces:**
- Consumes: `DeviceDescriptor`, `Parser`, `StatusField`, `arctis_domain::{DeviceState, StatusValue}`
- Produces:
  - `trait Transport { fn write_report(&mut self, &[u8]) -> Result<(), TransportError>; fn read_report(&mut self, &mut [u8], timeout_ms: i32) -> Result<usize, TransportError>; }`
  - `enum TransportError { NotFound(String), Io(String), Timeout }` (thiserror)
  - `struct MockTransport` with `new()`, `with_response(Vec<u8>)`, and pub `written: Vec<Vec<u8>>`
  - `fn decode_frame(&DeviceDescriptor, &[u8]) -> DeviceState`
  - `fn read_status<T: Transport>(&mut T, &DeviceDescriptor) -> Result<DeviceState, TransportError>`

- [ ] **Step 1: Create the transport trait** (`crates/device/src/transport.rs`)

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("device not found: {0}")]
    NotFound(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("read timed out")]
    Timeout,
}

/// A raw HID byte transport. The report id is included as the first byte of
/// every written buffer. Reads return a single input report.
pub trait Transport {
    fn write_report(&mut self, data: &[u8]) -> Result<(), TransportError>;
    fn read_report(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError>;
}
```

- [ ] **Step 2: Create the MockTransport** (`crates/device/src/mock.rs`)

```rust
use crate::transport::{Transport, TransportError};
use std::collections::VecDeque;

/// An in-memory transport for tests. Records writes; replays queued responses.
#[derive(Default)]
pub struct MockTransport {
    pub written: Vec<Vec<u8>>,
    responses: VecDeque<Vec<u8>>,
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a frame to be returned by the next `read_report`.
    pub fn with_response(mut self, frame: Vec<u8>) -> Self {
        self.responses.push_back(frame);
        self
    }
}

impl Transport for MockTransport {
    fn write_report(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.written.push(data.to_vec());
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8], _timeout_ms: i32) -> Result<usize, TransportError> {
        let frame = self.responses.pop_front().ok_or(TransportError::Timeout)?;
        let n = frame.len().min(buf.len());
        buf[..n].copy_from_slice(&frame[..n]);
        Ok(n)
    }
}
```

- [ ] **Step 3: Write the failing codec test** (`crates/device/src/codec.rs`)

```rust
use crate::descriptor::{DeviceDescriptor, Parser, StatusField};
use crate::transport::{Transport, TransportError};
use arctis_domain::{DeviceState, StatusValue};

/// Decode a single status frame into device state using a descriptor.
pub fn decode_frame(desc: &DeviceDescriptor, frame: &[u8]) -> DeviceState {
    let mut state = DeviceState::default();
    for field in &desc.status.fields {
        if frame_matches(frame, &field.match_prefix) {
            if let Some(value) = parse_field(field, frame) {
                state.fields.insert(field.name.clone(), value);
            }
        }
    }
    state
}

fn frame_matches(frame: &[u8], prefix: &[u8]) -> bool {
    frame.len() >= prefix.len() && frame[..prefix.len()] == *prefix
}

fn parse_field(field: &StatusField, frame: &[u8]) -> Option<StatusValue> {
    let raw = *frame.get(field.offset)?;
    Some(match &field.parser {
        Parser::Percentage { min, max } => {
            let span = max.saturating_sub(*min).max(1) as u32;
            let clamped = raw.clamp(*min, *max);
            let pct = (clamped.saturating_sub(*min) as u32 * 100 / span) as u8;
            StatusValue::Percentage(pct)
        }
        Parser::Bool { true_value } => StatusValue::Bool(raw == *true_value),
        Parser::Enum { entries } => entries
            .iter()
            .find(|e| e.value == raw)
            .map(|e| StatusValue::Enum(e.label.clone()))
            .unwrap_or(StatusValue::Int(raw as i64)),
        Parser::Int => StatusValue::Int(raw as i64),
    })
}

/// Build the status-request report, send it, read one frame, and decode it.
pub fn read_status<T: Transport>(
    transport: &mut T,
    desc: &DeviceDescriptor,
) -> Result<DeviceState, TransportError> {
    let mut report = Vec::with_capacity(desc.report_length);
    report.push(desc.report_id);
    report.extend_from_slice(&desc.status.request);
    report.resize(desc.report_length, 0);
    transport.write_report(&report)?;

    let mut buf = vec![0u8; desc.report_length];
    let n = transport.read_report(&mut buf, 500)?;
    Ok(decode_frame(desc, &buf[..n]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockTransport;
    use crate::registry::Registry;
    use arctis_domain::StatusValue;

    fn nova() -> DeviceDescriptor {
        Registry::builtin().unwrap().find(arctis_domain::DeviceId::new(0x1038, 0x12e5)).unwrap().clone()
    }

    fn frame(prefix: &[u8], pairs: &[(usize, u8)]) -> Vec<u8> {
        let mut f = vec![0u8; 64];
        f[..prefix.len()].copy_from_slice(prefix);
        for (i, v) in pairs {
            f[*i] = *v;
        }
        f
    }

    #[test]
    fn decodes_battery_percentage_from_0to8_scale() {
        let d = nova();
        // raw battery 4 of 0..8 == 50%
        let f = frame(&[0x06, 0xb0], &[(6, 4)]);
        let state = decode_frame(&d, &f);
        assert_eq!(state.fields.get("battery_charge"), Some(&StatusValue::Percentage(50)));
    }

    #[test]
    fn decodes_anc_enum_from_separate_frame_header() {
        let d = nova();
        let f = frame(&[0x07, 0xbd], &[(2, 1)]);
        let state = decode_frame(&d, &f);
        assert_eq!(state.fields.get("anc_mode"), Some(&StatusValue::Enum("transparency".into())));
    }

    #[test]
    fn read_status_sends_request_then_decodes_response() {
        let d = nova();
        let response = frame(&[0x06, 0xb0], &[(6, 8), (9, 1)]);
        let mut t = MockTransport::new().with_response(response);

        let state = read_status(&mut t, &d).expect("should read");

        // request was padded to report_length and starts with report_id + request bytes
        assert_eq!(t.written.len(), 1);
        assert_eq!(t.written[0].len(), 64);
        assert_eq!(&t.written[0][..2], &[0x06, 0xb0]);
        // battery 8/8 == 100%, mic muted
        assert_eq!(state.fields.get("battery_charge"), Some(&StatusValue::Percentage(100)));
        assert_eq!(state.fields.get("mic_muted"), Some(&StatusValue::Bool(true)));
    }
}
```

- [ ] **Step 4: Export the new modules** (`crates/device/src/lib.rs`)

```rust
//! Data-driven HID device layer: descriptors, registry, transport, codec.
pub mod codec;
pub mod descriptor;
pub mod mock;
pub mod registry;
pub mod transport;

pub use codec::{decode_frame, read_status};
pub use descriptor::{parse_descriptor, DeviceDescriptor, EnumEntry, Parser, StatusField, StatusSpec};
pub use mock::MockTransport;
pub use registry::{Registry, RegistryError};
pub use transport::{Transport, TransportError};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p arctis-device`
Expected: PASS (descriptor + registry + codec tests)

- [ ] **Step 6: Commit**

```bash
git add crates/device/src
git commit -m "feat(device): add Transport trait, MockTransport, and descriptor-driven status codec"
```

---

### Task 6: Real hidraw transport + device discovery

**Files:**
- Create: `crates/device/src/hidraw.rs`
- Modify: `crates/device/src/lib.rs`

**Interfaces:**
- Consumes: `Transport`, `TransportError`, `Registry`, `arctis_domain::DeviceId`
- Produces:
  - `struct HidrawTransport` implementing `Transport`, with `HidrawTransport::open(DeviceId, interface: u8) -> Result<Self, TransportError>`
  - `fn discover(&Registry) -> Result<Option<(DeviceId, u8)>, TransportError>` — returns the first connected device a descriptor matches, with its control interface number

- [ ] **Step 1: Implement the hidraw transport + discovery** (`crates/device/src/hidraw.rs`)

```rust
use crate::registry::Registry;
use crate::transport::{Transport, TransportError};
use arctis_domain::DeviceId;
use hidapi::HidApi;

pub struct HidrawTransport {
    device: hidapi::HidDevice,
}

impl HidrawTransport {
    /// Open the matching HID interface for `id` (read/write capable, but this
    /// plan only ever writes status-request reports).
    pub fn open(id: DeviceId, interface: u8) -> Result<Self, TransportError> {
        let api = HidApi::new().map_err(|e| TransportError::Io(e.to_string()))?;
        let info = api
            .device_list()
            .find(|d| {
                d.vendor_id() == id.vendor_id
                    && d.product_id() == id.product_id
                    && d.interface_number() == i32::from(interface)
            })
            .ok_or_else(|| TransportError::NotFound(format!("{id} iface {interface}")))?;
        let device = info
            .open_device(&api)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { device })
    }
}

impl Transport for HidrawTransport {
    fn write_report(&mut self, data: &[u8]) -> Result<(), TransportError> {
        self.device
            .write(data)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError> {
        let n = self
            .device
            .read_timeout(buf, timeout_ms)
            .map_err(|e| TransportError::Io(e.to_string()))?;
        if n == 0 {
            return Err(TransportError::Timeout);
        }
        Ok(n)
    }
}

/// Scan connected HID devices for the first one a registry descriptor matches.
/// Returns its `DeviceId` and the descriptor's declared control interface.
pub fn discover(registry: &Registry) -> Result<Option<(DeviceId, u8)>, TransportError> {
    let api = HidApi::new().map_err(|e| TransportError::Io(e.to_string()))?;
    for info in api.device_list() {
        let id = DeviceId::new(info.vendor_id(), info.product_id());
        if let Some(desc) = registry.find(id) {
            if info.interface_number() == i32::from(desc.interface) {
                return Ok(Some((id, desc.interface)));
            }
        }
    }
    Ok(None)
}
```

- [ ] **Step 2: Export it** (`crates/device/src/lib.rs`) — add these two lines in the appropriate places:

```rust
pub mod hidraw;
pub use hidraw::{discover, HidrawTransport};
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p arctis-device`
Expected: compiles. (No unit test — this path needs real hardware; it is exercised in Task 7.)

- [ ] **Step 4: Commit**

```bash
git add crates/device/src
git commit -m "feat(device): add real hidraw transport + device discovery"
```

---

### Task 7: `asm-cli probe` command (hardware validation)

**Files:**
- Create: `crates/cli/Cargo.toml`
- Create: `crates/cli/src/main.rs`

**Interfaces:**
- Consumes: `arctis_device::{Registry, discover, HidrawTransport, read_status}`, `arctis_domain::StatusValue`
- Produces: the `asm-cli` binary with subcommands `list`, `probe`.

- [ ] **Step 1: Create the CLI manifest** (`crates/cli/Cargo.toml`)

```toml
[package]
name = "arctis-cli"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[[bin]]
name = "asm-cli"
path = "src/main.rs"

[dependencies]
arctis-domain = { workspace = true }
arctis-device = { workspace = true }
clap = { workspace = true }
```

- [ ] **Step 2: Implement the CLI** (`crates/cli/src/main.rs`)

```rust
use arctis_device::{discover, read_status, HidrawTransport, Registry};
use arctis_domain::StatusValue;
use clap::{Parser, Subcommand};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "asm-cli", about = "Arctis Sound Manager CLI (read-only probe)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List connected, recognized SteelSeries devices.
    List,
    /// Read and print device status (battery, ANC, mic, ChatMix). Read-only.
    Probe,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let registry = match Registry::builtin() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    match cli.command {
        Command::List => match discover(&registry) {
            Ok(Some((id, iface))) => {
                let name = registry.find(id).map(|d| d.name.as_str()).unwrap_or("?");
                println!("found: {name} ({id}) on interface {iface}");
                ExitCode::SUCCESS
            }
            Ok(None) => {
                println!("no recognized SteelSeries device connected");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Command::Probe => {
            let (id, iface) = match discover(&registry) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    eprintln!("no recognized device connected");
                    return ExitCode::FAILURE;
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let desc = registry.find(id).expect("discover returned a matched id");
            let mut transport = match HidrawTransport::open(id, iface) {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("error opening {id}: {e}");
                    eprintln!("hint: a udev rule granting hidraw access may be required.");
                    return ExitCode::FAILURE;
                }
            };
            match read_status(&mut transport, desc) {
                Ok(state) => {
                    println!("{} ({id}):", desc.name);
                    for (k, v) in &state.fields {
                        let rendered = match v {
                            StatusValue::Percentage(p) => format!("{p}%"),
                            StatusValue::Bool(b) => b.to_string(),
                            StatusValue::Enum(s) => s.clone(),
                            StatusValue::Int(i) => i.to_string(),
                        };
                        println!("  {k}: {rendered}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error reading status: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}
```

- [ ] **Step 3: Build the workspace**

Run: `cargo build --workspace`
Expected: compiles cleanly (warnings denied in CI, so fix any).

- [ ] **Step 4: Run the read-only probe on real hardware (VALIDATION GATE — spec §11.1)**

Run: `cargo run -p arctis-cli -- list`
Expected: prints the device id actually present (`1038:12e0` or `1038:12e5`) and interface 4. If "no recognized device", check the headset is on and connected.

Run: `cargo run -p arctis-cli -- probe`
Expected: prints decoded fields. **Validate against the physical headset:** `battery_charge` should roughly match the OLED battery readout; toggling ANC/Transparency on the headset and re-running should change `anc_mode`; muting the mic should flip `mic_muted`; turning the ChatMix dial should change `media_mix`/`chat_mix`.

If a field is wrong, the descriptor offsets/headers in `devices/steelseries_nova_pro_wireless.toml` need correction — fix the TOML (no code change), re-run, and update a test fixture in `codec.rs` to lock in the corrected mapping. **Record the validated mapping** in project memory (`arctis-nova-pro-protocol`).

If `probe` fails with a permission error, install a udev rule and reload:

```bash
echo 'KERNEL=="hidraw*", SUBSYSTEM=="hidraw", ATTRS{idVendor}=="1038", MODE="0660", TAG+="uaccess"' | sudo tee /etc/udev/rules.d/70-arctis-sound-manager.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```
(replug the device, then re-run `probe`). This rule ships with the app later (Plan 8).

- [ ] **Step 5: Commit**

```bash
git add crates/cli
git commit -m "feat(cli): add asm-cli list/probe for safe read-only device status"
```

---

## Self-Review

**Spec coverage (this plan's slice):**
- §4 workspace + crate boundaries → Tasks 1–7 (domain/device/cli; tauri rule N/A here). ✅
- §7 data-driven registry, capability flags, descriptor-driven codec, hidraw, single reader → Tasks 3–6. ✅
- §7 safety: reads only, no OLED, no init replay → enforced (only `status.request` is ever written). ✅
- §11.1 protocol read validation → Task 7 Step 4 validation gate. ✅
- §11.6 hidraw vs kernel driver → uses hidraw; `discover` surfaces what's present (kernel-driver contention to be handled in Plan 2 if observed). ✅
- §14 testing: pure unit tests for domain/codec; mock transport; hardware path validated manually → Tasks 2,3,4,5,7. ✅
- Out of scope here (correctly deferred to later plans): writes (Plan 2), audio (Plans 3–5), config/profiles/engine (Plan 6), UI (7), packaging/udev shipping (8).

**Placeholder scan:** No TBD/TODO; every code step contains complete, compilable code. ✅

**Type consistency:** `Transport::{write_report, read_report}` used identically in mock, hidraw, codec, and CLI. `read_status`/`decode_frame`/`discover`/`Registry::{builtin,find}` signatures match across tasks. `StatusValue` variants rendered in CLI match the domain definition. `Parser` variants in the descriptor match the codec match-arms. ✅

---

## Notes for the executor
- Work top-to-bottom; each task ends green and committed.
- The only step requiring the physical headset is Task 7 Step 4 — everything else is CI-testable with no hardware.
- Treat Task 7 Step 4 as a **validation gate**: do not assume the reverse-engineered offsets are correct; verify each field against the device and correct the descriptor if needed before declaring Plan 1 done.
