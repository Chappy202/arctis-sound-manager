# Known Issues

Tracked gaps deferred for later resolution (typically during end-to-end testing).

## KI-1 — Device not enumerated by hidapi even with hidraw access (RESOLVED 2026-06-20)

**Symptom:** `asm-cli list` reports "no recognized SteelSeries device connected" on the
target machine even though the Arctis Nova Pro Wireless (`1038:12e5`) is physically present.

**Status of investigation (2026-06-20):**
- Device present: `lsusb` shows `1038:12e5`. Control interface confirmed at `/dev/hidraw1`
  (`bInterfaceNumber=04`), which matches the descriptor's `interface = 4`.
- **Permission blocker RESOLVED:** installed `packaging/udev/70-arctis-sound-manager.rules`
  (the prior `99-steelseries.rules` applied `uaccess` after `73-seat-late`, so no ACL was
  granted). `getfacl /dev/hidraw1` now shows `user:jj:rw-` — the user can open the node.
- **Remaining problem:** with access granted, `asm-cli list` STILL finds nothing. So the
  cause is NOT (only) permissions — it is in hidapi (`linux-native`) enumeration of this
  device and/or our `discover()` interface-number matching.

**Leading hypotheses to test (when resumed):**
1. Re-run an unfiltered hidapi enumeration dump now that `jj` has access — does `1038:12e5`
   appear at all, and what `interface_number` does hidapi report for it? (Earlier, before the
   ACL, it was absent; a root-only sibling device WAS listed, suggesting `linux-native`
   enumeration is quirky for some composite/wireless devices.)
2. If hidapi reports `interface_number = -1` (or not 4) for the control node, our
   `discover()` filter `info.interface_number() == descriptor.interface` would reject it.
   Consider matching by usage_page/usage, or relaxing the interface match, or selecting the
   node by report-descriptor characteristics.
3. Possible `hidapi` `linux-native` limitation with this device — compare against the
   default `hidapi` backend (libusb/hidraw via the C lib) as a fallback.

**Diagnostic aid:** a throwaway `crates/device/examples/hid_dump.rs` was used (since removed);
re-add a `asm-cli doctor` command (planned for Plan 2 diagnostics) that dumps all enumerated
HID devices and, when none match, checks `lsusb`/sysfs for a `1038:` device and prints a
permission/enumeration hint instead of a bare "no device" message.

**Does not block:** building the audio engine (PipeWire, independent of HID), config, and
engine structure. Revisit during E2E hardware testing.

**RESOLUTION (2026-06-20):** root cause was the `hidapi` `linux-native` (pure-Rust) backend
not enumerating this composite wireless device. Switching to the C `linux-static-hidraw`
backend fixes it. Validated on real HW (owner-run): `asm-cli list` finds
"Arctis Nova Pro Wireless (1038:12e5) on interface 4"; `asm-cli probe` reads
`battery_charge: 100%` and `mic_muted: false`. Build deps: `systemd-devel` + C toolchain.

## KI-2 — Audio backend writes filter-chain conf to a predictable /tmp path (LOW, OPEN)

`AudioBackend::create` writes the filter-chain `.conf` to a predictable `/tmp/arctis_eq.<node_name>.conf`
path. Low severity (local, non-privileged audio utility), but a local user could pre-place a symlink
there. Follow-up: write to `~/.config/arctis-sound-manager/` or use `tempfile::NamedTempFile` with a
stable rename. Also: the conf filename doubles the name (`arctis_eq.arctis_eq.conf`) when node_name ==
the literal prefix — cosmetic. And `AudioError::Spawn{program:"write-conf"}` is reused for file-I/O
errors; add a dedicated `AudioError::Io` variant in a later pass. Revisit in the engine-orchestrator plan.
