use arctis_audio::{AudioBackend, BandKind, EqBand, EqModel, RealRunner, SinkSpec};
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

#[derive(Subcommand, Debug)]
enum Command {
    /// List connected, recognized SteelSeries devices.
    List,
    /// Read and print device status (battery, ANC, mic, ChatMix). Read-only.
    Probe,
    /// Manage the PipeWire virtual EQ sink.
    Sink {
        #[command(subcommand)]
        action: SinkAction,
    },
    /// Live parametric EQ control on the virtual sink.
    Eq {
        #[command(subcommand)]
        action: EqAction,
    },
}

#[derive(Subcommand, Debug)]
enum SinkAction {
    /// Create the virtual EQ sink (idempotent) with 10 flat bands.
    Create {
        /// Hardware sink node.name to feed; omit to follow the default sink.
        #[arg(long)]
        target: Option<String>,
    },
    /// Remove the virtual EQ sink (idempotent).
    Remove,
}

#[derive(Subcommand, Debug)]
enum EqAction {
    /// Set one band live (no restart).
    Set {
        #[arg(long)]
        band: usize,
        #[arg(long)]
        freq: f32,
        #[arg(long)]
        q: f32,
        #[arg(long)]
        gain: f32,
        #[arg(long, default_value = "peaking")]
        kind: String,
    },
    /// Show the resolved node id and confirm the sink is present.
    Show,
}

const SINK_NAME: &str = "arctis_eq";
const SINK_DESC: &str = "Arctis EQ Sink";

fn band_kind(s: &str) -> Result<BandKind, String> {
    match s {
        "peaking" => Ok(BandKind::Peaking),
        "lowshelf" => Ok(BandKind::LowShelf),
        "highshelf" => Ok(BandKind::HighShelf),
        other => Err(format!("unknown band kind: {other}")),
    }
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
        Command::Sink { action } => {
            let target = match &action {
                SinkAction::Create { target } => target.clone(),
                SinkAction::Remove => None,
            };
            let spec = SinkSpec {
                node_name: SINK_NAME.to_string(),
                description: SINK_DESC.to_string(),
                playback_target: target,
            };
            let mut be = AudioBackend::new(RealRunner, spec);
            match action {
                SinkAction::Create { .. } => match be.create(&EqModel::default_10band()) {
                    Ok(h) => {
                        println!("sink ready: {SINK_NAME} (conf {})", h.conf_path.display());
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error creating sink: {e}");
                        ExitCode::FAILURE
                    }
                },
                SinkAction::Remove => match be.remove() {
                    Ok(()) => {
                        println!("sink removed: {SINK_NAME}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error removing sink: {e}");
                        ExitCode::FAILURE
                    }
                },
            }
        }
        Command::Eq { action } => {
            let spec = SinkSpec {
                node_name: SINK_NAME.to_string(),
                description: SINK_DESC.to_string(),
                playback_target: None,
            };
            let mut be = AudioBackend::new(RealRunner, spec);
            match action {
                EqAction::Set {
                    band,
                    freq,
                    q,
                    gain,
                    kind,
                } => {
                    let kind = match band_kind(&kind) {
                        Ok(k) => k,
                        Err(e) => {
                            eprintln!("error: {e}");
                            return ExitCode::FAILURE;
                        }
                    };
                    let b = EqBand::new(kind, freq, q, gain);
                    match be.apply_band(band, &b) {
                        Ok(()) => {
                            println!("band {band} set: {freq} Hz Q {q} {gain} dB");
                            ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("error setting band: {e}");
                            ExitCode::FAILURE
                        }
                    }
                }
                EqAction::Show => match be.find_node_id() {
                    Ok(id) => {
                        println!("{SINK_NAME} present, node id {id}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error: {e}");
                        ExitCode::FAILURE
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Verify the CLI arg parser accepts all expected subcommand forms.
    /// These tests parse only — they do not touch PipeWire or hidraw.
    use clap::Parser;

    use super::Cli;

    fn parse(args: &[&str]) -> Result<super::Command, clap::Error> {
        let mut full = vec!["asm-cli"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full).map(|c| c.command)
    }

    #[test]
    fn sink_create_no_target() {
        let cmd = parse(&["sink", "create"]).expect("sink create should parse");
        assert!(matches!(
            cmd,
            super::Command::Sink {
                action: super::SinkAction::Create { target: None }
            }
        ));
    }

    #[test]
    fn sink_create_with_target() {
        let cmd = parse(&[
            "sink",
            "create",
            "--target",
            "alsa_output.pci-0000_00_1f.3.analog-stereo",
        ])
        .expect("sink create --target should parse");
        match cmd {
            super::Command::Sink {
                action: super::SinkAction::Create { target: Some(t) },
            } => assert_eq!(t, "alsa_output.pci-0000_00_1f.3.analog-stereo"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn sink_remove() {
        let cmd = parse(&["sink", "remove"]).expect("sink remove should parse");
        assert!(matches!(
            cmd,
            super::Command::Sink {
                action: super::SinkAction::Remove
            }
        ));
    }

    #[test]
    fn eq_set_defaults() {
        // Use `--gain=-6` (= form) to avoid clap treating "-6" as an unknown flag.
        let cmd = parse(&[
            "eq",
            "set",
            "--band",
            "3",
            "--freq",
            "1200",
            "--q",
            "1.0",
            "--gain=-6",
        ])
        .expect("eq set should parse");
        match cmd {
            super::Command::Eq {
                action:
                    super::EqAction::Set {
                        band,
                        freq,
                        q,
                        gain,
                        kind,
                    },
            } => {
                assert_eq!(band, 3);
                assert!((freq - 1200.0).abs() < f32::EPSILON);
                assert!((q - 1.0).abs() < f32::EPSILON);
                assert!((gain - (-6.0)).abs() < f32::EPSILON);
                assert_eq!(kind, "peaking");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn eq_set_highshelf() {
        let cmd = parse(&[
            "eq",
            "set",
            "--band",
            "9",
            "--freq",
            "8000",
            "--q",
            "0.7",
            "--gain",
            "3",
            "--kind",
            "highshelf",
        ])
        .expect("eq set --kind highshelf should parse");
        match cmd {
            super::Command::Eq {
                action: super::EqAction::Set { kind, .. },
            } => assert_eq!(kind, "highshelf"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn eq_show() {
        let cmd = parse(&["eq", "show"]).expect("eq show should parse");
        assert!(matches!(
            cmd,
            super::Command::Eq {
                action: super::EqAction::Show
            }
        ));
    }

    #[test]
    fn band_kind_roundtrip() {
        assert!(matches!(
            super::band_kind("peaking"),
            Ok(super::BandKind::Peaking)
        ));
        assert!(matches!(
            super::band_kind("lowshelf"),
            Ok(super::BandKind::LowShelf)
        ));
        assert!(matches!(
            super::band_kind("highshelf"),
            Ok(super::BandKind::HighShelf)
        ));
        assert!(super::band_kind("unknown").is_err());
    }
}
