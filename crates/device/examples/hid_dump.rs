//! Raw HID frame diagnostic for the Arctis device.
//!
//! Prints every raw status frame the headset sends, grouped by its 2-byte prefix,
//! and highlights which byte indices changed since the previous frame of that prefix.
//! Use it to find which byte (and value) carries a given state — e.g. the physical
//! mic-mute button.
//!
//! Run with the daemon STOPPED (only one process can read the hidraw node cleanly):
//!     ~/.cargo/bin/cargo run -p arctis-device --example hid_dump
//!
//! Then physically toggle the thing you want to map (e.g. press the mic-mute button a
//! few times). Watch the `changed=[...]` indices and the byte values. Ctrl-C to stop.

use std::collections::HashMap;

use arctis_device::{discover, HidrawTransport, Registry, Transport};

fn main() {
    let registry = match Registry::builtin() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("registry error: {e}");
            std::process::exit(1);
        }
    };
    let (id, iface) = match discover(&registry) {
        Ok(Some(v)) => v,
        Ok(None) => {
            eprintln!("no recognized SteelSeries device connected");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("discover error: {e}");
            std::process::exit(1);
        }
    };
    let mut transport = match HidrawTransport::open(id, iface) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error opening {id}: {e}");
            eprintln!("hint: stop the daemon first, and ensure the udev rule grants hidraw access.");
            std::process::exit(1);
        }
    };

    eprintln!(
        "Reading raw frames from {id} (interface {iface}). \
         Toggle the mic-mute button (or whatever you want to map) now. Ctrl-C to stop.\n\
         Columns: prefix = first 2 bytes; changed = byte indices that differ from the previous \
         frame of the same prefix; the full frame is printed as `idx:hex` pairs.\n"
    );

    let mut buf = [0u8; 64];
    let mut last_by_prefix: HashMap<(u8, u8), Vec<u8>> = HashMap::new();
    let mut seq: u64 = 0;

    loop {
        match transport.read_report(&mut buf, 5000) {
            Ok(0) => continue, // timeout, no frame this interval
            Ok(n) => {
                seq += 1;
                let frame = &buf[..n];
                let prefix = (frame.first().copied().unwrap_or(0), frame.get(1).copied().unwrap_or(0));

                let changed: Vec<String> = match last_by_prefix.get(&prefix) {
                    Some(prev) if prev.len() == frame.len() => prev
                        .iter()
                        .zip(frame.iter())
                        .enumerate()
                        .filter(|(_, (a, b))| a != b)
                        .map(|(i, (a, b))| format!("{i}:{a:#04x}->{b:#04x}"))
                        .collect(),
                    Some(_) => vec!["(length changed)".to_string()],
                    None => vec!["(first frame of this prefix)".to_string()],
                };

                let hex: String = frame
                    .iter()
                    .enumerate()
                    .map(|(i, b)| format!("{i:02}:{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ");

                println!(
                    "#{seq} prefix={:#04x},{:#04x} len={n} byte9={:#04x}  changed=[{}]",
                    prefix.0,
                    prefix.1,
                    frame.get(9).copied().unwrap_or(0),
                    changed.join(", ")
                );
                println!("    {hex}");

                last_by_prefix.insert(prefix, frame.to_vec());
            }
            Err(e) => {
                eprintln!("read error: {e}");
            }
        }
    }
}
