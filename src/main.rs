use ncod_artnet::{artnet, curve, show, showfile};

use std::env;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

const USAGE: &str = "\
Play a DMX recorder show as Art-Net. Reads .jsonl, or a Showtec TR-512 .NCOD.

ncod-artnet <file> [options]

  -t, --target <ip[:port]>   Art-Net node or broadcast (default 255.255.255.255:6454)
  -u, --universe <0-32767>   Art-Net universe (default 0)
  -l, --loop                 repeat until interrupted
  -h, --help
";

struct Args {
    file: PathBuf,
    target: SocketAddr,
    universe: u16,
    loop_play: bool,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let bytes = std::fs::read(&args.file)
        .map_err(|err| format!("cannot read {}: {err}", args.file.display()))?;
    let show = showfile::parse_show(&bytes)?;
    let seconds = show.end_ms() / 1000;
    let millis = show.end_ms() % 1000;
    println!(
        "{}  {} frames  {seconds}.{millis:03}s  -> {}  universe {}{}",
        args.file.display(),
        show.frames.len(),
        args.target,
        args.universe,
        if args.loop_play { "  looping" } else { "" },
    );

    let socket = artnet::socket().map_err(|err| format!("cannot open UDP socket: {err}"))?;
    let mut sequence = 1u8;
    loop {
        play(&socket, args.target, args.universe, &show, &mut sequence)?;
        if !args.loop_play {
            break;
        }
    }
    println!("done");
    Ok(())
}

fn play(
    socket: &std::net::UdpSocket,
    target: SocketAddr,
    universe: u16,
    show: &show::Show,
    sequence: &mut u8,
) -> Result<(), String> {
    let frames = &show.frames;
    if frames.is_empty() {
        return Ok(());
    }
    let curves = curve::TrackCurves::new();
    curves.restore(&show.output)?;
    let origin = Instant::now();
    let mut next_tick = Instant::now();
    let end_ms = frames.last().map(|frame| frame.at_ms).unwrap_or(0);
    let tracks = ncod_artnet::show::index_tracks(frames);
    loop {
        let elapsed_ms = u32::try_from(origin.elapsed().as_millis()).unwrap_or(u32::MAX);
        let now_ms = elapsed_ms.min(end_ms);
        let mut slots = levels_from(&tracks, now_ms);
        curves.apply(&mut slots);
        let master = curves.map_master(show.output.master);
        if master != 255 {
            for slot in &mut slots {
                *slot = ((u16::from(*slot) * u16::from(master) + 127) / 255) as u8;
            }
        }
        let pkt = artnet::packet(universe, *sequence, &slots);
        socket
            .send_to(&pkt, target)
            .map_err(|err| format!("send failed: {err}"))?;
        *sequence = sequence.wrapping_add(1);
        if *sequence == 0 {
            *sequence = 1;
        }
        if elapsed_ms >= end_ms {
            break;
        }
        next_tick += Duration::from_millis(25);
        let now = Instant::now();
        if now > next_tick {
            if now.saturating_duration_since(next_tick) > Duration::from_millis(25) {
                next_tick = now + Duration::from_millis(25);
            }
        } else {
            thread::sleep(next_tick - now);
        }
    }
    Ok(())
}

fn levels_from(tracks: &[Vec<(u32, u8)>], at_ms: u32) -> [u8; 512] {
    let mut levels = [0u8; 512];
    for (slot, keys) in tracks.iter().enumerate() {
        levels[slot] = ncod_artnet::show::lerp_keys(keys, at_ms);
    }
    levels
}

fn parse_args() -> Result<Args, String> {
    let mut file = None;
    let mut target = String::from("255.255.255.255");
    let mut universe = 0u16;
    let mut loop_play = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.trim_end().to_string()),
            "-l" | "--loop" => loop_play = true,
            "-t" | "--target" => target = next_value(&mut args, &arg)?,
            "-u" | "--universe" => {
                let value = next_value(&mut args, &arg)?;
                universe = parse_universe(&value)?;
            }
            other if other.starts_with("--target=") => {
                target = other["--target=".len()..].to_string();
            }
            other if other.starts_with("--universe=") => {
                universe = parse_universe(&other["--universe=".len()..])?;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other}\n\n{USAGE}")
                    .trim_end()
                    .to_string());
            }
            other => {
                if file.is_some() {
                    return Err(format!("unexpected argument {other}\n\n{USAGE}")
                        .trim_end()
                        .to_string());
                }
                file = Some(PathBuf::from(other));
            }
        }
    }
    let file = file.ok_or_else(|| USAGE.trim_end().to_string())?;
    if target.is_empty() {
        return Err("missing value for --target".into());
    }
    Ok(Args {
        file,
        target: parse_target(&target)?,
        universe,
        loop_play,
    })
}

fn next_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing value for {flag}"))
}

fn parse_universe(value: &str) -> Result<u16, String> {
    let universe = value
        .parse::<u16>()
        .map_err(|_| format!("invalid universe {value}"))?;
    if universe > 0x7fff {
        return Err(format!("universe {universe} is outside 0-32767"));
    }
    Ok(universe)
}

fn parse_target(value: &str) -> Result<SocketAddr, String> {
    let specified = if let Some((host, port)) = value.rsplit_once(':') {
        !host.is_empty() && !host.contains(':') && port.parse::<u16>().is_ok()
    } else {
        false
    };
    let query = if specified {
        value.to_string()
    } else {
        format!("{value}:{port}", port = artnet::PORT)
    };
    let addr = query
        .to_socket_addrs()
        .map_err(|_| format!("invalid target {value}"))?
        .find(|addr| matches!(addr.ip(), IpAddr::V4(_)))
        .ok_or_else(|| format!("Art-Net target must be IPv4: {value}"))?;
    Ok(addr)
}
