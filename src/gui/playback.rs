use crate::{artnet, show};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_TARGET: &str = "255.255.255.255";

#[derive(Debug, Clone)]
pub struct PlaybackSettings {
    pub target: String,
    pub universe: u16,
    pub r#loop: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            target: DEFAULT_TARGET.into(),
            universe: 0,
            r#loop: false,
        }
    }
}

pub enum PlaybackEvent {
    Started,
    Frame(usize, u64),
    Finished,
    Error(String),
}

pub struct PlaybackHandle {
    stop: Arc<AtomicBool>,
    seek_tx: mpsc::Sender<(usize, u64)>,
    gen: Arc<AtomicU64>,
}

impl PlaybackHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn seek(&self, index: usize) -> u64 {
        let gen = self.gen.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.seek_tx.send((index, gen));
        gen
    }
}

pub fn start(
    frames: Arc<Vec<show::Frame>>,
    start_index: usize,
    settings: PlaybackSettings,
    events: mpsc::Sender<PlaybackEvent>,
) -> Result<PlaybackHandle, String> {
    let target = parse_target(&settings.target)?;
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let (seek_tx, seek_rx) = mpsc::channel();
    let gen = Arc::new(AtomicU64::new(0));
    thread::spawn(move || {
        let _ = events.send(PlaybackEvent::Started);
        let socket = match artnet::socket() {
            Ok(socket) => socket,
            Err(err) => {
                let _ = events.send(PlaybackEvent::Error(format!("socket: {err}")));
                return;
            }
        };
        let mut play_gen = 0u64;
        loop {
            if play_once(
                &socket,
                target,
                settings.universe,
                &frames,
                start_index,
                &stop_flag,
                &events,
                &seek_rx,
                &mut play_gen,
            )
            .is_err()
            {
                return;
            }
            if !settings.r#loop || stop_flag.load(Ordering::Relaxed) {
                break;
            }
        }
        let _ = events.send(PlaybackEvent::Finished);
    });
    Ok(PlaybackHandle { stop, seek_tx, gen })
}

fn play_once(
    socket: &std::net::UdpSocket,
    target: SocketAddr,
    universe: u16,
    frames: &[show::Frame],
    start_index: usize,
    stop: &AtomicBool,
    events: &mpsc::Sender<PlaybackEvent>,
    seek_rx: &mpsc::Receiver<(usize, u64)>,
    gen: &mut u64,
) -> Result<(), ()> {
    if frames.is_empty() || start_index >= frames.len() {
        let _ = events.send(PlaybackEvent::Finished);
        return Ok(());
    }
    let mut index = start_index;
    let mut origin = Instant::now();
    let mut origin_ms = frames[index].at_ms;
    let mut sequence = 1u8;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(PlaybackEvent::Finished);
            return Ok(());
        }
        if let Some((seek_index, seek_gen)) = latest_seek(seek_rx) {
            index = seek_index.min(frames.len() - 1);
            *gen = seek_gen;
            origin = Instant::now();
            origin_ms = frames[index].at_ms;
        }
        if index >= frames.len() {
            return Ok(());
        }
        let frame = &frames[index];
        let due = Duration::from_millis(u64::from(frame.at_ms.saturating_sub(origin_ms)));
        let elapsed = origin.elapsed();
        if due > elapsed {
            thread::sleep((due - elapsed).min(Duration::from_millis(16)));
            continue;
        }
        let pkt = artnet::packet(universe, sequence, &frame.slots);
        if socket.send_to(&pkt, target).is_err() {
            let _ = events.send(PlaybackEvent::Error("send failed".into()));
            return Err(());
        }
        sequence = sequence.wrapping_add(1);
        if sequence == 0 {
            sequence = 1;
        }
        let _ = events.send(PlaybackEvent::Frame(index, *gen));
        index += 1;
    }
}

fn latest_seek(seek_rx: &mpsc::Receiver<(usize, u64)>) -> Option<(usize, u64)> {
    let mut latest = None;
    while let Ok(seek) = seek_rx.try_recv() {
        latest = Some(seek);
    }
    latest
}

pub fn parse_target(value: &str) -> Result<SocketAddr, String> {
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
    query
        .to_socket_addrs()
        .map_err(|_| format!("invalid target {value}"))?
        .find(|addr| matches!(addr.ip(), IpAddr::V4(_)))
        .ok_or_else(|| format!("Art-Net target must be IPv4: {value}"))
}
