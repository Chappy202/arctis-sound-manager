mod coexist;
mod daemon;

use arctis_audio::{
    AppMatch, AudioBackend, BandKind, ChannelManager, ChannelSetConfig, EqBand, EqModel,
    RealRunner, RouteRule, Router, SinkSpec,
};
use arctis_config::store as config_store;
use arctis_device::{discover, read_status, HidrawTransport, Registry};
use arctis_domain::StatusValue;
use arctis_engine::Engine;
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
    /// Manage the full set of submix channels (Game / Chat / Media).
    Channels {
        #[command(subcommand)]
        action: ChannelsAction,
    },
    /// Per-application routing (live + persistent).
    Route {
        #[command(subcommand)]
        action: RouteAction,
    },
    /// Per-channel output device control.
    Channel {
        #[command(subcommand)]
        action: ChannelCmd,
    },
    /// Profile management.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Reconcile the live graph to the active profile in config.
    Apply,
    /// Run the resident daemon (default: foreground).
    Daemon {
        #[arg(long, default_value_t = true)]
        foreground: bool,
    },
    /// Headset hardware control (live reads; gated writes).
    Device {
        #[command(subcommand)]
        action: DeviceAction,
    },
    /// Microphone DSP chain (Clean Mic virtual source).
    Mic {
        #[command(subcommand)]
        action: MicAction,
    },
    /// Virtual surround / HRIR (spatial audio via PipeWire convolver).
    Surround {
        #[command(subcommand)]
        action: SurroundAction,
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
        #[arg(long, allow_negative_numbers = true)]
        gain: f32,
        #[arg(long, default_value = "peaking")]
        kind: String,
    },
    /// Show the resolved node id and confirm the sink is present.
    Show,
}

#[derive(Subcommand, Debug)]
enum ChannelsAction {
    /// Create all configured channels (idempotent).
    Up {
        /// Hardware sink node.name every channel feeds; omit to follow default.
        #[arg(long)]
        target: Option<String>,
    },
    /// Remove all configured channels (idempotent).
    Down,
}

#[derive(Subcommand, Debug)]
enum RouteAction {
    /// Route an app to a channel: live move + persistent WirePlumber rule.
    Set {
        /// Application matcher (application.process.binary by default).
        app: String,
        /// Channel id: game | chat | media.
        channel: String,
        /// Match application.name instead of process.binary.
        #[arg(long)]
        by_name: bool,
    },
    /// Print all persistent routing rules from routes.json.
    List,
}

#[derive(Subcommand, Debug)]
enum ChannelCmd {
    /// Set a channel's output device (enforced rebuild).
    Output {
        #[command(subcommand)]
        action: ChannelOutputAction,
    },
    /// Set the software volume for a channel in dB (-60..+6). 0 = unity.
    Volume {
        /// Channel id: game | chat | media.
        channel: String,
        /// Volume in dB, e.g. -6.0 for -6 dB.
        #[arg(allow_negative_numbers = true)]
        db: f32,
    },
    /// Mute or unmute a channel.
    Mute {
        /// Channel id: game | chat | media.
        channel: String,
        /// `on` to mute, `off` to unmute.
        state: String,
    },
}

#[derive(Subcommand, Debug)]
enum ChannelOutputAction {
    /// Retarget a channel to a hardware sink (`default` clears the pin).
    Set {
        /// Channel id: game | chat | media.
        channel: String,
        /// Hardware sink node.name, or `default` to follow the default sink.
        device: String,
    },
}

#[derive(Subcommand, Debug)]
enum ProfileAction {
    /// List available profiles.
    List,
    /// Show a profile's details (defaults to the active profile).
    Show { name: Option<String> },
    /// Switch the active profile.
    Switch { name: String },
    /// Persist the current in-memory config to disk (normalization pass).
    Save,
    /// Create a new profile as a copy of the active one.
    New { name: String },
}

#[derive(Subcommand, Debug)]
enum DeviceAction {
    /// Read and print live device status (battery, ANC, mic, dial). Read-only.
    Status,
    /// Set sidetone level 0..3.
    Sidetone {
        #[arg(allow_negative_numbers = true)]
        level: i64,
    },
    /// Set mic LED brightness 1..10.
    MicLed {
        #[arg(allow_negative_numbers = true)]
        level: i64,
    },
    /// Set ANC mode: off | transparency | on.
    Anc { mode: String },
    /// Set auto-off level 0..6 (0=never, 1=1min, 2=5min, 3=10min, 4=15min, 5=30min, 6=60min).
    AutoOff {
        #[arg(allow_negative_numbers = true)]
        level: i64,
    },
    /// Set transparency level 1..10.
    Transparency {
        #[arg(allow_negative_numbers = true)]
        level: i64,
    },
    /// Set mic volume 1..10.
    MicVolume {
        #[arg(allow_negative_numbers = true)]
        level: i64,
    },
    /// Set a raw control by name and integer value (generic escape hatch).
    Set {
        control: String,
        #[arg(allow_negative_numbers = true)]
        value: i64,
    },
}

#[derive(Subcommand, Debug)]
enum MicAction {
    /// Print the mic DSP chain status (enabled flag, per-stage, params, EQ bands).
    Status,
    /// Enable the whole mic chain (master switch on).
    On,
    /// Disable the whole mic chain (master switch off).
    Off,
    /// Enable a mic DSP stage (gain|highpass|suppression|compressor|gate|eq).
    Enable {
        /// Stage name: gain|highpass|suppression|compressor|gate|eq (alias: rnnoise)
        stage: String,
    },
    /// Disable a mic DSP stage (gain|highpass|suppression|compressor|gate|eq).
    Disable {
        /// Stage name: gain|highpass|suppression|compressor|gate|eq (alias: rnnoise)
        stage: String,
    },
    /// Set a mic DSP parameter live (no restart).
    Set {
        /// Param name: gain_db|highpass_freq|attenuation_limit_db|vad_threshold|vad_grace_ms|vad_retro_grace_ms|gate_threshold|comp_threshold_db|comp_ratio|comp_makeup_db
        param: String,
        /// Parameter value (float; negative values accepted for dB params)
        #[arg(allow_negative_numbers = true)]
        value: f32,
    },
    /// Set one mic EQ band live (no restart).
    Eq {
        /// Band index (0-based)
        #[arg(long)]
        band: usize,
        /// Center/corner frequency in Hz
        #[arg(long)]
        freq: f32,
        /// Q factor
        #[arg(long)]
        q: f32,
        /// Gain in dB (negative accepted)
        #[arg(long, allow_negative_numbers = true)]
        gain: f32,
        /// Filter kind: peaking|lowshelf|highshelf
        #[arg(long, default_value = "peaking")]
        kind: String,
    },
    /// Set (or clear) the hardware mic capture source.
    HwMic {
        /// Hardware mic node.name to capture from; omit to clear the pin.
        device: Option<String>,
    },
    /// Select the noise-suppression backend (deep_filter|rnnoise).
    Backend {
        /// Backend name: deep_filter|rnnoise
        backend: String,
    },
}

#[derive(Subcommand, Debug)]
enum SurroundAction {
    /// Print the virtual surround status (enabled, HRIR, channels, hw_sink).
    Status,
    /// Enable virtual surround (master switch on).
    On,
    /// Disable virtual surround (master switch off).
    Off,
    /// HRIR profile management (list or set).
    Hrir {
        #[command(subcommand)]
        action: HrirAction,
    },
    /// Set which channels are routed through surround (comma-separated, e.g. game,media).
    Channels {
        /// Channel ids, comma-separated: game,media (or a single id).
        #[arg(value_delimiter = ',')]
        channels: Vec<String>,
    },
    /// Pin (or clear) the surround output to a specific hardware sink node.name.
    HwSink {
        /// Hardware sink node.name; omit to clear the pin.
        device: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
enum HrirAction {
    /// List available HRIR profiles from ~/.local/share/pipewire/hrir_hesuvi/profiles/.
    List,
    /// Set the active HRIR profile by stem (filename without .wav).
    Set {
        /// Profile stem, e.g. 02-dh-dolby-headphone
        name: String,
    },
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

/// Parse an ANC mode string ("off" | "transparency" | "on") to its wire integer.
fn parse_anc_mode(mode: &str) -> Result<i64, String> {
    match mode {
        "off" => Ok(0),
        "transparency" => Ok(1),
        "on" => Ok(2),
        other => Err(format!(
            "unknown ANC mode '{other}' (use: off | transparency | on)"
        )),
    }
}

/// Send a DeviceSet request to the daemon and print the result.
/// On daemon error (gate refused, etc.) the daemon's error message is surfaced
/// clearly — it is NOT treated as a crash.
fn device_set_via_daemon(control: &str, value: i64) -> ExitCode {
    if !daemon::socket_path().exists() {
        eprintln!("error: daemon is not running — start it with `asm-cli daemon`");
        eprintln!(
            "note: device writes require the daemon (single worker enforces HID serialisation)"
        );
        return ExitCode::FAILURE;
    }
    let req = daemon::Request::DeviceSet {
        control: control.to_string(),
        value,
    };
    match daemon::send_request(&req) {
        Ok(resp) if resp.ok => {
            println!("ok: {control} set to {value}");
            // If state came back, print device fields for confirmation.
            if let Some(state) = resp.state {
                if state.device_present {
                    for (k, v) in &state.device_fields {
                        println!("  {k}: {v}");
                    }
                }
            }
            ExitCode::SUCCESS
        }
        Ok(resp) => {
            // Surface the daemon's gate/validation error verbatim.
            let msg = resp.error.unwrap_or_else(|| "unknown error".to_string());
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error communicating with daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn suppression_backend_str(backend: arctis_engine::SuppressionBackend) -> &'static str {
    use arctis_engine::SuppressionBackend;
    match backend {
        SuppressionBackend::DeepFilter => "deep_filter",
        SuppressionBackend::Rnnoise => "rnnoise",
    }
}

fn stage_canonical(kind: &arctis_engine::StageName) -> &'static str {
    use arctis_engine::StageName;
    match kind {
        StageName::Gain => "gain",
        StageName::Highpass => "highpass",
        StageName::Suppression => "suppression",
        StageName::Compressor => "compressor",
        StageName::Gate => "gate",
        StageName::MicEq => "eq",
    }
}

fn dispatch_mic(action: MicAction) -> ExitCode {
    if !daemon::socket_path().exists() {
        eprintln!("error: daemon is not running — start it with `asm-cli daemon`");
        eprintln!(
            "note: mic commands require the daemon (single worker enforces PipeWire serialisation)"
        );
        return ExitCode::FAILURE;
    }

    let is_status = matches!(action, MicAction::Status);
    let req = match action {
        MicAction::Status => daemon::Request::MicStatus,
        MicAction::On => daemon::Request::MicEnable { enabled: true },
        MicAction::Off => daemon::Request::MicEnable { enabled: false },
        MicAction::Enable { stage } => daemon::Request::MicStage {
            stage,
            enabled: true,
        },
        MicAction::Disable { stage } => daemon::Request::MicStage {
            stage,
            enabled: false,
        },
        MicAction::Set { param, value } => daemon::Request::MicSet { param, value },
        MicAction::Eq {
            band,
            freq,
            q,
            gain,
            kind,
        } => daemon::Request::MicEqBand {
            band,
            kind,
            freq_hz: freq,
            q,
            gain_db: gain,
        },
        MicAction::HwMic { device } => daemon::Request::MicHwMic { device },
        MicAction::Backend { backend } => daemon::Request::MicSuppressionBackend { backend },
    };

    match daemon::send_request(&req) {
        Ok(resp) if resp.ok => {
            if is_status {
                if let Some(state) = resp.state {
                    let mic = &state.mic;
                    println!("mic: {}", if mic.enabled { "enabled" } else { "disabled" });
                    for stage in &mic.stages {
                        let avail_str = if stage.available {
                            String::new()
                        } else {
                            format!(
                                " (unavailable: {} plugin not found)",
                                stage_canonical(&stage.kind)
                            )
                        };
                        println!(
                            "  {}: {}{}",
                            stage_canonical(&stage.kind),
                            if stage.enabled { "enabled" } else { "disabled" },
                            avail_str
                        );
                        // For the suppression stage, show backend info inline.
                        if stage.kind == arctis_engine::StageName::Suppression {
                            let active = suppression_backend_str(mic.suppression_backend);
                            let avail_backends: Vec<&str> = mic
                                .available_suppression_backends
                                .iter()
                                .map(|b| suppression_backend_str(*b))
                                .collect();
                            let avail_display = if avail_backends.is_empty() {
                                "none found".to_string()
                            } else {
                                avail_backends.join(", ")
                            };
                            println!(
                                "    suppression_backend: {active} (available: {avail_display})"
                            );
                        }
                        for (k, v) in &stage.params {
                            println!("    {k}: {v}");
                        }
                    }
                    if !mic.eq_bands.is_empty() {
                        println!("  eq bands:");
                        for (i, b) in mic.eq_bands.iter().enumerate() {
                            println!(
                                "    band {i}: {} {:.1} Hz Q {:.2} {:.1} dB",
                                b.kind, b.freq_hz, b.q, b.gain_db
                            );
                        }
                    }
                }
            } else {
                println!("ok");
            }
            ExitCode::SUCCESS
        }
        Ok(resp) => {
            let msg = resp.error.unwrap_or_else(|| "unknown error".to_string());
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error communicating with daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch_device(action: DeviceAction) -> ExitCode {
    match action {
        DeviceAction::Status => {
            // Try the daemon first (preferred — uses the live DeviceWorker).
            if daemon::socket_path().exists() {
                match daemon::send_request(&daemon::Request::GetState) {
                    Ok(resp) if resp.ok => {
                        if let Some(state) = resp.state {
                            if !state.device_present {
                                println!("device: not connected");
                            } else {
                                println!("device: connected");
                                for (k, v) in &state.device_fields {
                                    println!("  {k}: {v}");
                                }
                            }
                            return ExitCode::SUCCESS;
                        }
                    }
                    Ok(resp) => {
                        eprintln!("error: {}", resp.error.unwrap_or_default());
                        return ExitCode::FAILURE;
                    }
                    Err(_) => {
                        // Daemon failed — fall through to direct read.
                    }
                }
            }
            // Fall back: direct one-shot read via discover + read_status.
            let registry = match Registry::builtin() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let (id, iface) = match discover(&registry) {
                Ok(Some(v)) => v,
                Ok(None) => {
                    println!("device: not connected");
                    return ExitCode::SUCCESS;
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
                Ok(device_state) => {
                    println!("device: {} ({id})", desc.name);
                    for (k, v) in &device_state.fields {
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
                    eprintln!("error reading device status: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        DeviceAction::Sidetone { level } => device_set_via_daemon("sidetone", level),
        DeviceAction::MicLed { level } => device_set_via_daemon("mic_led", level),
        DeviceAction::Anc { mode } => {
            let value = match parse_anc_mode(&mode) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            device_set_via_daemon("anc", value)
        }
        DeviceAction::AutoOff { level } => device_set_via_daemon("inactive_time", level),
        DeviceAction::Transparency { level } => device_set_via_daemon("transparency_level", level),
        DeviceAction::MicVolume { level } => device_set_via_daemon("mic_volume", level),
        DeviceAction::Set { control, value } => device_set_via_daemon(&control, value),
    }
}

fn dispatch_surround(action: SurroundAction) -> ExitCode {
    if !daemon::socket_path().exists() {
        eprintln!("error: daemon is not running — start it with `asm-cli daemon`");
        eprintln!(
            "note: surround commands require the daemon (single worker enforces PipeWire serialisation)"
        );
        return ExitCode::FAILURE;
    }

    let is_status = matches!(action, SurroundAction::Status);
    let is_hrir_list = matches!(
        action,
        SurroundAction::Hrir {
            action: HrirAction::List
        }
    );
    let req = match action {
        SurroundAction::Status => daemon::Request::SurroundStatus,
        SurroundAction::On => daemon::Request::SurroundEnable { enabled: true },
        SurroundAction::Off => daemon::Request::SurroundEnable { enabled: false },
        SurroundAction::Hrir {
            action: HrirAction::List,
        } => daemon::Request::SurroundStatus,
        SurroundAction::Hrir {
            action: HrirAction::Set { name },
        } => daemon::Request::SurroundSetHrir { name },
        SurroundAction::Channels { channels } => daemon::Request::SurroundSetChannels { channels },
        SurroundAction::HwSink { device } => daemon::Request::SurroundSetHwSink { hw_sink: device },
    };

    match daemon::send_request(&req) {
        Ok(resp) if resp.ok => {
            if is_status {
                if let Some(state) = resp.state {
                    let s = &state.surround;
                    println!(
                        "surround: {}",
                        if s.enabled { "enabled" } else { "disabled" }
                    );
                    match &s.hrir {
                        Some(h) => println!("  hrir: {h}"),
                        None => println!("  hrir: (default)"),
                    }
                    if s.available_hrirs.is_empty() {
                        println!(
                            "  available_hrirs: none found in ~/.local/share/pipewire/hrir_hesuvi/profiles/"
                        );
                    } else {
                        println!("  available_hrirs:");
                        for h in &s.available_hrirs {
                            println!("    {h}");
                        }
                    }
                    println!("  channels: {}", s.channels.join(", "));
                    match &s.hw_sink {
                        Some(sink) => println!("  hw_sink: {sink}"),
                        None => println!("  hw_sink: (auto-detected)"),
                    }
                }
            } else if is_hrir_list {
                if let Some(state) = resp.state {
                    let s = &state.surround;
                    if s.available_hrirs.is_empty() {
                        println!("none found in ~/.local/share/pipewire/hrir_hesuvi/profiles/");
                    } else {
                        for h in &s.available_hrirs {
                            println!("{h}");
                        }
                    }
                }
            } else {
                println!("ok");
            }
            ExitCode::SUCCESS
        }
        Ok(resp) => {
            let msg = resp.error.unwrap_or_else(|| "unknown error".to_string());
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error communicating with daemon: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::List => {
            let registry = match Registry::builtin() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match discover(&registry) {
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
            }
        }
        Command::Probe => {
            let registry = match Registry::builtin() {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            };
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
        Command::Channels { action } => {
            let target = match &action {
                ChannelsAction::Up { target } => target.clone(),
                ChannelsAction::Down => None,
            };
            let cfg = ChannelSetConfig::default_sonar(target.as_deref());
            let mut mgr = ChannelManager::new(RealRunner, cfg);
            match action {
                ChannelsAction::Up { .. } => match mgr.up(&EqModel::default_10band()) {
                    Ok(handles) => {
                        println!("channels up: {} sinks ready", handles.len());
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error bringing channels up: {e}");
                        ExitCode::FAILURE
                    }
                },
                ChannelsAction::Down => match mgr.down() {
                    Ok(()) => {
                        println!("channels down");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error bringing channels down: {e}");
                        ExitCode::FAILURE
                    }
                },
            }
        }
        Command::Route { action } => match action {
            RouteAction::Set {
                app,
                channel,
                by_name,
            } => {
                let cfg = ChannelSetConfig::default_sonar(None);
                let sink = match cfg.find(&channel) {
                    Some(c) => c.node_name.clone(),
                    None => {
                        eprintln!("error: unknown channel '{channel}' (use game|chat|media)");
                        return ExitCode::FAILURE;
                    }
                };
                let matcher = if by_name {
                    AppMatch::Name(app.clone())
                } else {
                    AppMatch::Binary(app.clone())
                };
                let mut router = Router::new(RealRunner);
                // Load existing rules so we never clobber them.
                if let Err(e) = router.load_persistent() {
                    eprintln!("warning: could not load existing routes: {e}");
                }
                // Live move first (best-effort — warn on failure, continue).
                match router.apply_live(&matcher, &sink) {
                    Ok(id) => println!("live: moved stream {id} ({app}) → {sink}"),
                    Err(e) => {
                        eprintln!("warning: live move failed (is the app playing?): {e}");
                        // Still persist the rule so it applies next launch.
                    }
                }
                // Upsert and persist all rules.
                router.set_rule(RouteRule::new(&app, &sink));
                match router.save_persistent() {
                    Ok(()) => {
                        println!("persistent: rule saved ({app} → {sink})");
                        println!("note: run `systemctl --user restart wireplumber` to load it now");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("error saving persistent rule: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            RouteAction::List => {
                let mut router = Router::new(RealRunner);
                if let Err(e) = router.load_persistent() {
                    eprintln!("error loading routes: {e}");
                    return ExitCode::FAILURE;
                }
                let rules = router.list();
                if rules.is_empty() {
                    println!("no persistent routes");
                } else {
                    for rule in rules {
                        println!("{} → {}", rule.app_binary, rule.target_sink);
                    }
                }
                ExitCode::SUCCESS
            }
        },
        Command::Channel { action } => match action {
            ChannelCmd::Output { action } => match action {
                ChannelOutputAction::Set { channel, device } => {
                    let cfg = ChannelSetConfig::default_sonar(None);
                    let mut mgr = ChannelManager::new(RealRunner, cfg);
                    let dev = if device == "default" {
                        None
                    } else {
                        Some(device.clone())
                    };
                    match mgr.set_output(&channel, dev, &EqModel::default_10band()) {
                        Ok(h) => {
                            println!(
                                "channel '{channel}' output set to {device} (conf {})",
                                h.conf_path.display()
                            );
                            ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("error setting channel output: {e}");
                            ExitCode::FAILURE
                        }
                    }
                }
            },
            ChannelCmd::Volume { channel, db } => {
                let req = daemon::Request::SetChannelVolume {
                    channel: channel.clone(),
                    volume_db: db,
                };
                match daemon::send_request(&req) {
                    Ok(resp) if resp.ok => {
                        println!("channel '{channel}' volume set to {db} dB");
                        ExitCode::SUCCESS
                    }
                    Ok(resp) => {
                        eprintln!(
                            "error: {}",
                            resp.error.unwrap_or_else(|| "unknown error".to_string())
                        );
                        ExitCode::FAILURE
                    }
                    Err(e) => {
                        eprintln!("error sending request: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            ChannelCmd::Mute { channel, state } => {
                let muted = match state.as_str() {
                    "on" => true,
                    "off" => false,
                    other => {
                        eprintln!("mute state must be 'on' or 'off', got: {other}");
                        return ExitCode::FAILURE;
                    }
                };
                let req = daemon::Request::SetChannelMute {
                    channel: channel.clone(),
                    muted,
                };
                match daemon::send_request(&req) {
                    Ok(resp) if resp.ok => {
                        let mute_str = if muted { "muted" } else { "unmuted" };
                        println!("channel '{channel}' {mute_str}");
                        ExitCode::SUCCESS
                    }
                    Ok(resp) => {
                        eprintln!(
                            "error: {}",
                            resp.error.unwrap_or_else(|| "unknown error".to_string())
                        );
                        ExitCode::FAILURE
                    }
                    Err(e) => {
                        eprintln!("error sending request: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
        },
        Command::Device { action } => dispatch_device(action),
        Command::Mic { action } => dispatch_mic(action),
        Command::Surround { action } => dispatch_surround(action),
        Command::Profile { action } => dispatch_profile(action),
        Command::Apply => dispatch_apply(),
        Command::Daemon { foreground: _ } => {
            // Coexist check: scan for legacy nodes using pw-cli ls Node output.
            let node_stdout = std::process::Command::new("pw-cli")
                .args(["ls", "Node"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            let home = std::env::var("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| std::path::PathBuf::from("/root"));
            let report = coexist::detect_from(&node_stdout, &home);
            if let Some(w) = coexist::warning(&report) {
                eprintln!("{w}");
            }
            match daemon::run_daemon() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("daemon error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn dispatch_profile(action: ProfileAction) -> ExitCode {
    match action {
        ProfileAction::List => {
            let cfg = match config_store::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error loading config: {e}");
                    return ExitCode::FAILURE;
                }
            };
            for name in cfg.profile_names() {
                let marker = if name == cfg.active_profile { " *" } else { "" };
                println!("{name}{marker}");
            }
            ExitCode::SUCCESS
        }
        ProfileAction::Show { name } => {
            let cfg = match config_store::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error loading config: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let target = name.as_deref().unwrap_or(&cfg.active_profile);
            match cfg.profiles.iter().find(|p| p.name == target) {
                Some(p) => {
                    println!("profile: {}", p.name);
                    println!("  channels: {}", p.channels.len());
                    println!("  routes: {}", p.routes.len());
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("error: profile '{target}' not found");
                    ExitCode::FAILURE
                }
            }
        }
        ProfileAction::Switch { name } => {
            // Try daemon first.
            if daemon::socket_path().exists() {
                let req = daemon::Request::SwitchProfile { name: name.clone() };
                match daemon::send_request(&req) {
                    Ok(resp) if resp.ok => {
                        println!("switched to profile '{name}'");
                        return ExitCode::SUCCESS;
                    }
                    Ok(resp) => {
                        eprintln!("error: {}", resp.error.unwrap_or_default());
                        return ExitCode::FAILURE;
                    }
                    Err(_) => {
                        // Fall through to direct engine path.
                    }
                }
            }
            // Direct engine path.
            let cfg = match config_store::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error loading config: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let mut engine = Engine::new(RealRunner, cfg);
            match engine.switch_profile(&name) {
                Ok(()) => {
                    println!("switched to profile '{name}'");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        ProfileAction::Save => {
            let cfg = match config_store::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error loading config: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match config_store::save(&cfg) {
                Ok(()) => {
                    println!("config saved");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error saving config: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        ProfileAction::New { name } => {
            let mut cfg = match config_store::load() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error loading config: {e}");
                    return ExitCode::FAILURE;
                }
            };
            match cfg.new_profile_from_active(&name) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("error creating profile: {e}");
                    return ExitCode::FAILURE;
                }
            }
            match config_store::save(&cfg) {
                Ok(()) => {
                    println!("profile '{name}' created");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error saving config: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn dispatch_apply() -> ExitCode {
    // Try daemon first via Reload.
    if daemon::socket_path().exists() {
        match daemon::send_request(&daemon::Request::Reload) {
            Ok(resp) if resp.ok => {
                println!("applied active profile");
                return ExitCode::SUCCESS;
            }
            Ok(resp) => {
                eprintln!("error: {}", resp.error.unwrap_or_default());
                return ExitCode::FAILURE;
            }
            Err(_) => {
                // Fall through to direct engine path.
            }
        }
    }
    // Direct engine path.
    let cfg = match config_store::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error loading config: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut engine = Engine::new(RealRunner, cfg);
    match engine.reconcile() {
        Ok(()) => {
            println!("applied active profile");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
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
        // Use `--gain=-6` (= form) — still works alongside the space form.
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
    fn eq_set_negative_gain_space_form() {
        // `--gain -6` (space form) must parse now that allow_negative_numbers = true.
        let cmd = parse(&[
            "eq", "set", "--band", "3", "--freq", "1200", "--q", "1.0", "--gain", "-6",
        ])
        .expect("eq set --gain -6 (space form) should parse");
        match cmd {
            super::Command::Eq {
                action: super::EqAction::Set { gain, .. },
            } => assert!((gain - (-6.0)).abs() < f32::EPSILON),
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

    #[test]
    fn channels_up_with_target() {
        let cmd = parse(&["channels", "up", "--target", "alsa_output.arctis"])
            .expect("channels up --target should parse");
        match cmd {
            super::Command::Channels {
                action: super::ChannelsAction::Up { target: Some(t) },
            } => assert_eq!(t, "alsa_output.arctis"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn channels_down() {
        let cmd = parse(&["channels", "down"]).expect("channels down should parse");
        assert!(matches!(
            cmd,
            super::Command::Channels {
                action: super::ChannelsAction::Down
            }
        ));
    }

    #[test]
    fn route_set_binary_default() {
        let cmd = parse(&["route", "set", "firefox", "media"]).expect("route set should parse");
        match cmd {
            super::Command::Route {
                action:
                    super::RouteAction::Set {
                        app,
                        channel,
                        by_name,
                    },
            } => {
                assert_eq!(app, "firefox");
                assert_eq!(channel, "media");
                assert!(!by_name);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn route_set_by_name() {
        let cmd = parse(&["route", "set", "Firefox", "media", "--by-name"])
            .expect("route set --by-name should parse");
        match cmd {
            super::Command::Route {
                action: super::RouteAction::Set { by_name, .. },
            } => assert!(by_name),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn route_list() {
        let cmd = parse(&["route", "list"]).expect("route list should parse");
        assert!(matches!(
            cmd,
            super::Command::Route {
                action: super::RouteAction::List
            }
        ));
    }

    #[test]
    fn channel_output_set() {
        let cmd = parse(&["channel", "output", "set", "media", "alsa_output.speakers"])
            .expect("channel output set should parse");
        match cmd {
            super::Command::Channel {
                action:
                    super::ChannelCmd::Output {
                        action: super::ChannelOutputAction::Set { channel, device },
                    },
            } => {
                assert_eq!(channel, "media");
                assert_eq!(device, "alsa_output.speakers");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── F2.1: channel volume/mute subcommand parsing tests ───────────────────

    #[test]
    fn channel_volume_set() {
        let cmd =
            parse(&["channel", "volume", "game", "-6.0"]).expect("channel volume should parse");
        match cmd {
            super::Command::Channel {
                action: super::ChannelCmd::Volume { channel, db },
            } => {
                assert_eq!(channel, "game");
                assert!((db - (-6.0_f32)).abs() < 0.001, "db must be -6.0, got {db}");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn channel_mute_set() {
        let cmd = parse(&["channel", "mute", "chat", "on"]).expect("channel mute on should parse");
        match cmd {
            super::Command::Channel {
                action: super::ChannelCmd::Mute { channel, state },
            } => {
                assert_eq!(channel, "chat");
                assert_eq!(state, "on");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn channel_unmute_set() {
        let cmd =
            parse(&["channel", "mute", "media", "off"]).expect("channel mute off should parse");
        match cmd {
            super::Command::Channel {
                action: super::ChannelCmd::Mute { channel, state },
            } => {
                assert_eq!(channel, "media");
                assert_eq!(state, "off");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── device subcommand parsing tests ─────────────────────────────────────

    #[test]
    fn device_status_parses() {
        let cmd = parse(&["device", "status"]).expect("device status should parse");
        assert!(matches!(
            cmd,
            super::Command::Device {
                action: super::DeviceAction::Status
            }
        ));
    }

    #[test]
    fn device_sidetone_parses() {
        let cmd = parse(&["device", "sidetone", "2"]).expect("device sidetone should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Sidetone { level },
            } => assert_eq!(level, 2),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_sidetone_zero_parses() {
        let cmd = parse(&["device", "sidetone", "0"]).expect("device sidetone 0 should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Sidetone { level },
            } => assert_eq!(level, 0),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_mic_led_parses() {
        let cmd = parse(&["device", "mic-led", "10"]).expect("device mic-led should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::MicLed { level },
            } => assert_eq!(level, 10),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_anc_off_parses() {
        let cmd = parse(&["device", "anc", "off"]).expect("device anc off should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Anc { mode },
            } => assert_eq!(mode, "off"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_anc_transparency_parses() {
        let cmd = parse(&["device", "anc", "transparency"])
            .expect("device anc transparency should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Anc { mode },
            } => assert_eq!(mode, "transparency"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_anc_on_parses() {
        let cmd = parse(&["device", "anc", "on"]).expect("device anc on should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Anc { mode },
            } => assert_eq!(mode, "on"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_auto_off_parses() {
        let cmd = parse(&["device", "auto-off", "3"]).expect("device auto-off should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::AutoOff { level },
            } => assert_eq!(level, 3),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_transparency_parses() {
        let cmd =
            parse(&["device", "transparency", "5"]).expect("device transparency should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Transparency { level },
            } => assert_eq!(level, 5),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_mic_volume_parses() {
        let cmd = parse(&["device", "mic-volume", "7"]).expect("device mic-volume should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::MicVolume { level },
            } => assert_eq!(level, 7),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_set_generic_parses() {
        let cmd = parse(&["device", "set", "sidetone", "1"]).expect("device set should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Set { control, value },
            } => {
                assert_eq!(control, "sidetone");
                assert_eq!(value, 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn device_set_negative_value_parses() {
        let cmd =
            parse(&["device", "set", "mic_volume", "-1"]).expect("device set -1 should parse");
        match cmd {
            super::Command::Device {
                action: super::DeviceAction::Set { control, value },
            } => {
                assert_eq!(control, "mic_volume");
                assert_eq!(value, -1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── mic subcommand parsing tests ─────────────────────────────────────────

    #[test]
    fn mic_status_parses() {
        let cmd = parse(&["mic", "status"]).expect("mic status should parse");
        assert!(matches!(
            cmd,
            super::Command::Mic {
                action: super::MicAction::Status
            }
        ));
    }

    #[test]
    fn mic_on_parses() {
        let cmd = parse(&["mic", "on"]).expect("mic on should parse");
        assert!(matches!(
            cmd,
            super::Command::Mic {
                action: super::MicAction::On
            }
        ));
    }

    #[test]
    fn mic_off_parses() {
        let cmd = parse(&["mic", "off"]).expect("mic off should parse");
        assert!(matches!(
            cmd,
            super::Command::Mic {
                action: super::MicAction::Off
            }
        ));
    }

    #[test]
    fn mic_enable_parses() {
        let cmd = parse(&["mic", "enable", "suppression"]).expect("mic enable should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Enable { stage },
            } => assert_eq!(stage, "suppression"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_disable_parses() {
        let cmd = parse(&["mic", "disable", "gain"]).expect("mic disable should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Disable { stage },
            } => assert_eq!(stage, "gain"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_set_vad_threshold_parses() {
        let cmd = parse(&["mic", "set", "vad_threshold", "40"])
            .expect("mic set vad_threshold 40 should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Set { param, value },
            } => {
                assert_eq!(param, "vad_threshold");
                assert!((value - 40.0).abs() < f32::EPSILON);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_set_negative_gain_db_parses() {
        let cmd = parse(&["mic", "set", "gain_db", "-6"]).expect("mic set gain_db -6 should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Set { param, value },
            } => {
                assert_eq!(param, "gain_db");
                assert!((value - (-6.0)).abs() < f32::EPSILON);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_eq_parses() {
        let cmd = parse(&[
            "mic",
            "eq",
            "--band",
            "2",
            "--freq",
            "1000",
            "--q",
            "1.0",
            "--gain=-3.0",
        ])
        .expect("mic eq should parse");
        match cmd {
            super::Command::Mic {
                action:
                    super::MicAction::Eq {
                        band,
                        freq,
                        q,
                        gain,
                        kind,
                    },
            } => {
                assert_eq!(band, 2);
                assert!((freq - 1000.0).abs() < f32::EPSILON);
                assert!((q - 1.0).abs() < f32::EPSILON);
                assert!((gain - (-3.0)).abs() < f32::EPSILON);
                assert_eq!(kind, "peaking");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_hw_mic_with_device_parses() {
        let cmd = parse(&["mic", "hw-mic", "alsa_input.usb-SteelSeries"])
            .expect("mic hw-mic with device should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::HwMic { device },
            } => assert_eq!(device, Some("alsa_input.usb-SteelSeries".into())),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_hw_mic_no_device_parses() {
        let cmd = parse(&["mic", "hw-mic"]).expect("mic hw-mic (no device) should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::HwMic { device },
            } => assert_eq!(device, None),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── Task 3: mic backend + attenuation_limit_db CLI parse tests ──────────

    #[test]
    fn mic_backend_deep_filter_parses() {
        let cmd = parse(&["mic", "backend", "deep_filter"])
            .expect("mic backend deep_filter should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Backend { backend },
            } => assert_eq!(backend, "deep_filter"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_backend_rnnoise_parses() {
        let cmd = parse(&["mic", "backend", "rnnoise"]).expect("mic backend rnnoise should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Backend { backend },
            } => assert_eq!(backend, "rnnoise"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn mic_set_attenuation_limit_db_parses() {
        let cmd = parse(&["mic", "set", "attenuation_limit_db", "24"])
            .expect("mic set attenuation_limit_db 24 should parse");
        match cmd {
            super::Command::Mic {
                action: super::MicAction::Set { param, value },
            } => {
                assert_eq!(param, "attenuation_limit_db");
                assert!((value - 24.0).abs() < f32::EPSILON);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── F1.4: surround subcommand parsing tests ──────────────────────────────

    #[test]
    fn surround_status_parses() {
        let cmd = parse(&["surround", "status"]).expect("surround status should parse");
        assert!(matches!(
            cmd,
            super::Command::Surround {
                action: super::SurroundAction::Status
            }
        ));
    }

    #[test]
    fn surround_on_parses() {
        let cmd = parse(&["surround", "on"]).expect("surround on should parse");
        assert!(matches!(
            cmd,
            super::Command::Surround {
                action: super::SurroundAction::On
            }
        ));
    }

    #[test]
    fn surround_off_parses() {
        let cmd = parse(&["surround", "off"]).expect("surround off should parse");
        assert!(matches!(
            cmd,
            super::Command::Surround {
                action: super::SurroundAction::Off
            }
        ));
    }

    #[test]
    fn surround_hrir_list_parses() {
        let cmd = parse(&["surround", "hrir", "list"]).expect("surround hrir list should parse");
        assert!(matches!(
            cmd,
            super::Command::Surround {
                action: super::SurroundAction::Hrir {
                    action: super::HrirAction::List
                }
            }
        ));
    }

    #[test]
    fn surround_hrir_set_parses() {
        let cmd = parse(&["surround", "hrir", "set", "02-dh-dolby-headphone"])
            .expect("surround hrir set should parse");
        match cmd {
            super::Command::Surround {
                action:
                    super::SurroundAction::Hrir {
                        action: super::HrirAction::Set { name },
                    },
            } => assert_eq!(name, "02-dh-dolby-headphone"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn surround_channels_parses_comma_separated() {
        let cmd = parse(&["surround", "channels", "game,media"])
            .expect("surround channels game,media should parse");
        match cmd {
            super::Command::Surround {
                action: super::SurroundAction::Channels { channels },
            } => assert_eq!(channels, vec!["game".to_string(), "media".to_string()]),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn surround_channels_parses_single() {
        let cmd =
            parse(&["surround", "channels", "game"]).expect("surround channels game should parse");
        match cmd {
            super::Command::Surround {
                action: super::SurroundAction::Channels { channels },
            } => assert_eq!(channels, vec!["game".to_string()]),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn surround_hw_sink_with_device_parses() {
        let cmd = parse(&["surround", "hw-sink", "alsa_output.usb-SteelSeries"])
            .expect("surround hw-sink with device should parse");
        match cmd {
            super::Command::Surround {
                action: super::SurroundAction::HwSink { device },
            } => assert_eq!(device, Some("alsa_output.usb-SteelSeries".into())),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn surround_hw_sink_no_device_parses() {
        let cmd =
            parse(&["surround", "hw-sink"]).expect("surround hw-sink (no device) should parse");
        match cmd {
            super::Command::Surround {
                action: super::SurroundAction::HwSink { device },
            } => assert_eq!(device, None),
            other => panic!("unexpected: {other:?}"),
        }
    }

    // ── parse_anc_mode unit tests ────────────────────────────────────────────

    #[test]
    fn anc_mode_off_maps_to_zero() {
        assert_eq!(super::parse_anc_mode("off"), Ok(0));
    }

    #[test]
    fn anc_mode_transparency_maps_to_one() {
        assert_eq!(super::parse_anc_mode("transparency"), Ok(1));
    }

    #[test]
    fn anc_mode_on_maps_to_two() {
        assert_eq!(super::parse_anc_mode("on"), Ok(2));
    }

    #[test]
    fn anc_mode_unknown_errors() {
        assert!(super::parse_anc_mode("invalid").is_err());
        let e = super::parse_anc_mode("active").unwrap_err();
        assert!(
            e.contains("off | transparency | on"),
            "error should hint at valid values: {e}"
        );
    }
}
