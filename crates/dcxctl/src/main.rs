//! Offline-by-default operator CLI. The explicit Darwin `live-discovery`
//! feature adds one Search-only serial command.

#[cfg(all(feature = "live-discovery", target_os = "macos"))]
mod live_discovery;

use std::{
    error::Error,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use clap::{Args, Parser, Subcommand, ValueEnum};
use dcx_core::{
    LabProfileV1,
    discovery::QueryOnlyDiscovery,
    protocol::{self, DeviceId, DumpPart, MAX_FRAME_LEN, Query},
    rew::{self, MAX_REW_BYTES},
};

const MAX_DECODE_FILE_BYTES: usize = MAX_FRAME_LEN * 4;
const MAX_PROFILE_FILE_BYTES: usize = 128 * 1024;

#[derive(Debug, Parser)]
#[command(name = "dcxctl", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Decode one complete offline frame into strict JSON.
    Decode(DecodeArgs),
    /// Construct a read-only query frame and print uppercase hexadecimal.
    Query {
        #[command(subcommand)]
        command: QueryCommand,
    },
    /// Inspect or validate the pure query-only discovery plan.
    Discovery {
        #[command(subcommand)]
        command: DiscoveryCommand,
    },
    /// Validate or compare strict versioned profiles.
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Translate an offline Room EQ Wizard text export.
    Rew {
        #[command(subcommand)]
        command: RewCommand,
    },
}

#[derive(Debug, Args)]
struct DecodeArgs {
    /// Hexadecimal frame. Whitespace, `_`, `:`, and `-` separators are ignored.
    #[arg(long, conflicts_with = "file", required_unless_present = "file")]
    hex: Option<String>,
    /// Raw binary or ASCII-hex fixture file.
    #[arg(long, conflicts_with = "hex", required_unless_present = "hex")]
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Subcommand)]
enum QueryCommand {
    /// Construct the broadcast discovery query.
    Search,
    /// Construct a single-device ping query.
    Ping { device: u8 },
    /// Construct a request for one state-dump part.
    Dump {
        device: u8,
        #[arg(value_enum)]
        part: PartArg,
    },
}

#[derive(Debug, Subcommand)]
enum DiscoveryCommand {
    /// Print the exact offline 115200-then-38400 search plan.
    Plan {
        /// Reviewed DCX profile device address, 0 through 15.
        #[arg(long)]
        expected_device: u8,
    },
    /// Validate one offline 26-byte response against an expected address.
    ValidateResponse {
        file: PathBuf,
        /// Reviewed DCX profile device address, 0 through 15.
        #[arg(long)]
        expected_device: u8,
    },
    /// Run one DCX Search and nine repeats at the discovered baud.
    #[cfg(all(feature = "live-discovery", target_os = "macos"))]
    LiveSearch {
        /// Exact Darwin FTDI callout node; ports are never enumerated.
        #[arg(long)]
        tty: PathBuf,
        /// Expected DCX device address, 0 through 15.
        #[arg(long)]
        expected_device: u8,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PartArg {
    Part0,
    Part1,
}

#[derive(Debug, Subcommand)]
enum ProfileCommand {
    /// Parse and validate a strict complete-sink profile.
    Validate { file: PathBuf },
    /// Emit a stable semantic diff between two valid profiles.
    Diff { baseline: PathBuf, desired: PathBuf },
    /// Prove the profile can cross the offline apply-readiness gate.
    ApplyReady {
        file: PathBuf,
        /// SHA-256 of the exact reviewed profile representation.
        #[arg(long)]
        profile_digest: String,
    },
}

#[derive(Debug, Subcommand)]
enum RewCommand {
    /// Parse and quantize one REW Generic EQ text export.
    Import {
        file: PathBuf,
        /// Explicit physical DCX output, 1 through 6.
        #[arg(long)]
        target_output: u8,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    match Cli::parse().command {
        Command::Decode(args) => decode(args)?,
        Command::Query { command } => query(command)?,
        Command::Discovery { command } => discovery(command)?,
        Command::Profile { command } => profile(command)?,
        Command::Rew { command } => rew(command)?,
    }
    Ok(())
}

fn discovery(command: DiscoveryCommand) -> Result<(), Box<dyn Error>> {
    match command {
        DiscoveryCommand::Plan { expected_device } => {
            let expected_device = DeviceId::new(expected_device)?;
            let mut discovery = QueryOnlyDiscovery::new(expected_device);
            let primary = discovery.current_attempt().expect("new plan has primary");
            discovery.timeout_current()?;
            let fallback = discovery
                .current_attempt()
                .expect("primary timeout has one fallback");
            let receipt = serde_json::json!({
                "mode": "offline_query_only",
                "expected_device": expected_device,
                "attempts": [
                    {
                        "kind": primary.kind(),
                        "settings": primary.settings(),
                        "query_hex": hex::encode_upper(primary.query().encode()?),
                    },
                    {
                        "kind": fallback.kind(),
                        "settings": fallback.settings(),
                        "query_hex": hex::encode_upper(fallback.query().encode()?),
                    },
                ],
                "transport_opened": false,
            });
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        DiscoveryCommand::ValidateResponse {
            file,
            expected_device,
        } => {
            let bytes = decode_fixture(&read_bounded(
                &file,
                MAX_DECODE_FILE_BYTES,
                "search response fixture",
            )?)?;
            let mut discovery = QueryOnlyDiscovery::new(DeviceId::new(expected_device)?);
            let response = discovery.accept_candidates(&[&bytes])?;
            println!("{}", serde_json::to_string_pretty(&response)?);
        }
        #[cfg(all(feature = "live-discovery", target_os = "macos"))]
        DiscoveryCommand::LiveSearch {
            tty,
            expected_device,
        } => live_discovery::run(tty, expected_device)?,
    }
    Ok(())
}

fn decode(args: DecodeArgs) -> Result<(), Box<dyn Error>> {
    let bytes = match (args.hex, args.file) {
        (Some(text), None) => decode_hex(&text)?,
        (None, Some(path)) => decode_fixture(&read_bounded(
            &path,
            MAX_DECODE_FILE_BYTES,
            "decode fixture",
        )?)?,
        _ => unreachable!("clap enforces exactly one input"),
    };
    let message = protocol::parse_frame(&bytes)?;
    let decoded = protocol::decode(message)?;
    println!("{}", serde_json::to_string_pretty(&decoded)?);
    Ok(())
}

fn query(command: QueryCommand) -> Result<(), Box<dyn Error>> {
    let query = match command {
        QueryCommand::Search => Query::Search,
        QueryCommand::Ping { device } => Query::Ping(DeviceId::new(device)?),
        QueryCommand::Dump { device, part } => Query::Dump {
            device: DeviceId::new(device)?,
            part: match part {
                PartArg::Part0 => DumpPart::Part0,
                PartArg::Part1 => DumpPart::Part1,
            },
        },
    };
    println!("{}", hex::encode_upper(query.encode()?));
    Ok(())
}

fn profile(command: ProfileCommand) -> Result<(), Box<dyn Error>> {
    match command {
        ProfileCommand::Validate { file } => {
            let profile = load_profile(&file)?;
            println!("{}", serde_json::to_string_pretty(&profile)?);
        }
        ProfileCommand::Diff { baseline, desired } => {
            let baseline = load_profile(&baseline)?;
            let desired = load_profile(&desired)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&baseline.diff(&desired)?)?
            );
        }
        ProfileCommand::ApplyReady {
            file,
            profile_digest,
        } => {
            let profile = load_profile(&file)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&profile.binding_for_apply(&profile_digest)?)?
            );
        }
    }
    Ok(())
}

fn rew(command: RewCommand) -> Result<(), Box<dyn Error>> {
    match command {
        RewCommand::Import {
            file,
            target_output,
        } => {
            let bytes = read_bounded(&file, MAX_REW_BYTES, "REW export")?;
            let text = std::str::from_utf8(&bytes)?;
            let report = rew::import_rew(text, target_output)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn load_profile(path: &Path) -> Result<LabProfileV1, Box<dyn Error>> {
    Ok(LabProfileV1::from_json(&read_bounded(
        path,
        MAX_PROFILE_FILE_BYTES,
        "profile",
    )?)?)
}

fn decode_fixture(bytes: &[u8]) -> Result<Vec<u8>, Box<dyn Error>> {
    if bytes
        .iter()
        .all(|byte| byte.is_ascii_hexdigit() || byte.is_ascii_whitespace() || b"_:-".contains(byte))
    {
        Ok(decode_hex(std::str::from_utf8(bytes)?)?)
    } else {
        if bytes.len() > MAX_FRAME_LEN {
            return Err(input_too_large("binary frame", bytes.len(), MAX_FRAME_LEN).into());
        }
        Ok(bytes.to_vec())
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    if text.len() > MAX_DECODE_FILE_BYTES {
        return Err(input_too_large("hex frame", text.len(), MAX_DECODE_FILE_BYTES).into());
    }
    let compact: String = text
        .chars()
        .filter(|character| !character.is_ascii_whitespace() && !"_:-".contains(*character))
        .collect();
    let bytes = hex::decode(compact)?;
    if bytes.len() > MAX_FRAME_LEN {
        return Err(input_too_large("decoded frame", bytes.len(), MAX_FRAME_LEN).into());
    }
    Ok(bytes)
}

fn read_bounded(path: &Path, maximum: usize, kind: &str) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > maximum as u64 {
        return Err(input_too_large(kind, maximum.saturating_add(1), maximum));
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(input_too_large(kind, bytes.len(), maximum));
    }
    Ok(bytes)
}

fn input_too_large(kind: &str, actual: usize, maximum: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{kind} has {actual} bytes; maximum is {maximum}"),
    )
}
