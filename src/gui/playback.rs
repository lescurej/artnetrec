use crate::curve::TrackCurves;
use crate::{artnet, show};
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc, RwLock};
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

pub struct SoloMask {
    words: [AtomicU64; 8],
    generation: AtomicU64,
}

impl SoloMask {
    pub fn new() -> Self {
        Self {
            words: std::array::from_fn(|_| AtomicU64::new(0)),
            generation: AtomicU64::new(0),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.words
            .iter()
            .all(|word| word.load(Ordering::Relaxed) == 0)
    }

    pub fn contains(&self, channel: u16) -> bool {
        let Some(bit) = bit_index(channel) else {
            return false;
        };
        self.words[bit / 64].load(Ordering::Relaxed) & (1u64 << (bit % 64)) != 0
    }

    pub fn toggle(&self, channel: u16) {
        let Some(bit) = bit_index(channel) else {
            return;
        };
        self.words[bit / 64].fetch_xor(1u64 << (bit % 64), Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn clear(&self) {
        self.set_channels(&[]);
    }

    pub fn set_channels(&self, channels: &[u16]) {
        let mut words = [0u64; 8];
        for channel in channels {
            if let Some(bit) = bit_index(*channel) {
                words[bit / 64] |= 1u64 << (bit % 64);
            }
        }
        for (word, value) in self.words.iter().zip(words) {
            word.store(value, Ordering::Relaxed);
        }
        self.generation.fetch_add(1, Ordering::Release);
    }

    pub fn channels(&self) -> Vec<u16> {
        (1..=512)
            .filter(|channel| self.contains(*channel))
            .collect()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn apply(&self, slots: &mut [u8; 512]) {
        let words: [u64; 8] =
            std::array::from_fn(|index| self.words[index].load(Ordering::Relaxed));
        if words.iter().all(|word| *word == 0) {
            return;
        }
        for (index, slot) in slots.iter_mut().enumerate() {
            if words[index / 64] & (1u64 << (index % 64)) == 0 {
                *slot = 0;
            }
        }
    }
}

pub struct TrackLevels {
    levels: [AtomicU8; 512],
}

impl TrackLevels {
    pub fn new() -> Self {
        Self {
            levels: std::array::from_fn(|_| AtomicU8::new(255)),
        }
    }

    pub fn get(&self, channel: u16) -> u8 {
        bit_index(channel)
            .map(|index| self.levels[index].load(Ordering::Relaxed))
            .unwrap_or(255)
    }

    pub fn set(&self, channel: u16, level: u8) {
        if let Some(index) = bit_index(channel) {
            self.levels[index].store(level, Ordering::Relaxed);
        }
    }

    pub fn apply(&self, slots: &mut [u8; 512]) {
        for (slot, level) in slots.iter_mut().zip(self.levels.iter()) {
            let level = level.load(Ordering::Relaxed);
            *slot = scale_level(*slot, level);
        }
    }
}

pub fn scale_level(level: u8, gain: u8) -> u8 {
    if gain == 255 {
        level
    } else {
        ((u16::from(level) * u16::from(gain) + 127) / 255) as u8
    }
}

fn bit_index(channel: u16) -> Option<usize> {
    if (1..=512).contains(&channel) {
        Some(usize::from(channel - 1))
    } else {
        None
    }
}

pub enum PlaybackEvent {
    Started,
    Frame(usize, u32, u64),
    Finished,
    #[allow(dead_code)]
    Error(String),
}

enum Seek {
    Frame(usize, u64),
    Time(u32, u64),
}

const TICK: Duration = Duration::from_millis(25);

pub struct Output {
    socket: std::net::UdpSocket,
    sequence: AtomicU8,
}

impl Output {
    pub fn open() -> Result<Self, String> {
        let socket = artnet::socket().map_err(|err| format!("socket: {err}"))?;
        Ok(Self {
            socket,
            sequence: AtomicU8::new(0),
        })
    }

    pub fn emit(
        &self,
        target: SocketAddr,
        universe: u16,
        slots: &[u8; 512],
        master: u8,
        solo: &SoloMask,
        levels: &TrackLevels,
        curves: &TrackCurves,
    ) -> bool {
        let scaled = shape_output(slots, master, solo, levels, curves);
        let sequence = bump_sequence(&self.sequence);
        let pkt = artnet::packet(universe, sequence, &scaled);
        self.socket.send_to(&pkt, target).is_ok()
    }
}

fn shape_output(
    slots: &[u8; 512],
    master: u8,
    solo: &SoloMask,
    levels: &TrackLevels,
    curves: &TrackCurves,
) -> [u8; 512] {
    let mut shaped = *slots;
    levels.apply(&mut shaped);
    curves.apply(&mut shaped);
    solo.apply(&mut shaped);
    scale_slots(&shaped, curves.map_master(master))
}

fn bump_sequence(sequence: &AtomicU8) -> u8 {
    let mut current = sequence.load(Ordering::Relaxed);
    loop {
        let next = if current >= 255 { 1 } else { current + 1 };
        match sequence.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(seen) => current = seen,
        }
    }
}

#[derive(Clone)]
pub struct Live {
    slots: Arc<RwLock<[u8; 512]>>,
    target: Arc<RwLock<String>>,
    universe: Arc<AtomicU16>,
    master: Arc<RwLock<Arc<AtomicU8>>>,
    solo: Arc<RwLock<Arc<SoloMask>>>,
    levels: Arc<RwLock<Arc<TrackLevels>>>,
    curves: Arc<RwLock<Arc<TrackCurves>>>,
}

impl Live {
    pub fn publish(&self, slots: &[u8; 512]) {
        if let Ok(mut guard) = self.slots.write() {
            *guard = *slots;
        }
    }

    pub fn set_endpoint(&self, target: &str, universe: u16) {
        if let Ok(mut guard) = self.target.write() {
            if guard.as_str() != target {
                *guard = target.to_string();
            }
        }
        self.universe.store(universe, Ordering::Relaxed);
    }

    pub fn bind(
        &self,
        master: Arc<AtomicU8>,
        solo: Arc<SoloMask>,
        levels: Arc<TrackLevels>,
        curves: Arc<TrackCurves>,
    ) {
        if let Ok(mut guard) = self.master.write() {
            *guard = master;
        }
        if let Ok(mut guard) = self.solo.write() {
            *guard = solo;
        }
        if let Ok(mut guard) = self.levels.write() {
            *guard = levels;
        }
        if let Ok(mut guard) = self.curves.write() {
            *guard = curves;
        }
    }
}

pub struct Stream {
    live: Live,
    stop: Arc<AtomicBool>,
}

impl Stream {
    pub fn start() -> Result<Self, String> {
        let output = Output::open()?;
        let live = Live {
            slots: Arc::new(RwLock::new([0u8; 512])),
            target: Arc::new(RwLock::new(DEFAULT_TARGET.to_string())),
            universe: Arc::new(AtomicU16::new(0)),
            master: Arc::new(RwLock::new(Arc::new(AtomicU8::new(255)))),
            solo: Arc::new(RwLock::new(Arc::new(SoloMask::new()))),
            levels: Arc::new(RwLock::new(Arc::new(TrackLevels::new()))),
            curves: Arc::new(RwLock::new(Arc::new(TrackCurves::new()))),
        };
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let slots = Arc::clone(&live.slots);
        let target = Arc::clone(&live.target);
        let universe = Arc::clone(&live.universe);
        let master = Arc::clone(&live.master);
        let solo = Arc::clone(&live.solo);
        let levels = Arc::clone(&live.levels);
        let curves = Arc::clone(&live.curves);
        thread::spawn(move || {
            stream_loop(
                output, slots, target, universe, master, solo, levels, curves, stop_flag,
            )
        });
        Ok(Self { live, stop })
    }

    pub fn live(&self) -> Live {
        self.live.clone()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn stream_loop(
    output: Output,
    slots: Arc<RwLock<[u8; 512]>>,
    target: Arc<RwLock<String>>,
    universe: Arc<AtomicU16>,
    master: Arc<RwLock<Arc<AtomicU8>>>,
    solo: Arc<RwLock<Arc<SoloMask>>>,
    levels: Arc<RwLock<Arc<TrackLevels>>>,
    curves: Arc<RwLock<Arc<TrackCurves>>>,
    stop: Arc<AtomicBool>,
) {
    let mut next_tick = Instant::now();
    let mut cached: Option<(String, SocketAddr)> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let target_str = target.read().map(|guard| guard.clone()).unwrap_or_default();
        let addr = if cached
            .as_ref()
            .is_some_and(|(known, _)| known == &target_str)
        {
            cached.as_ref().map(|(_, addr)| *addr)
        } else {
            match parse_target(&target_str) {
                Ok(addr) => {
                    cached = Some((target_str, addr));
                    Some(addr)
                }
                Err(_) => {
                    cached = None;
                    None
                }
            }
        };
        if let Some(addr) = addr {
            let universe = universe.load(Ordering::Relaxed);
            let frame = slots.read().map(|guard| *guard).unwrap_or([0u8; 512]);
            let level = master
                .read()
                .map(|guard| guard.load(Ordering::Relaxed))
                .unwrap_or(255);
            if let (Ok(solo), Ok(levels), Ok(curves)) = (solo.read(), levels.read(), curves.read())
            {
                let _ = output.emit(addr, universe, &frame, level, &solo, &levels, &curves);
            }
        }
        next_tick += TICK;
        let now = Instant::now();
        if now > next_tick {
            if now.saturating_duration_since(next_tick) > TICK {
                next_tick = now + TICK;
            }
        } else {
            thread::sleep(next_tick - now);
        }
    }
}

pub struct PlaybackHandle {
    stop: Arc<AtomicBool>,
    seek_tx: mpsc::Sender<Seek>,
    gen: Arc<AtomicU64>,
}

impl PlaybackHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn seek(&self, index: usize) -> u64 {
        let gen = self.gen.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.seek_tx.send(Seek::Frame(index, gen));
        gen
    }

    pub fn seek_time(&self, at_ms: u32) -> u64 {
        let gen = self.gen.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.seek_tx.send(Seek::Time(at_ms, gen));
        gen
    }
}

pub struct SharedShow {
    pub frames: Vec<show::Frame>,
    pub revision: u64,
}

pub fn start(
    live: Option<Live>,
    frames: Arc<RwLock<SharedShow>>,
    start_ms: u32,
    settings: PlaybackSettings,
    events: mpsc::Sender<PlaybackEvent>,
) -> PlaybackHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);
    let (seek_tx, seek_rx) = mpsc::channel();
    let gen = Arc::new(AtomicU64::new(0));
    thread::spawn(move || {
        let _ = events.send(PlaybackEvent::Started);
        let mut play_gen = 0u64;
        loop {
            if play_once(
                live.as_ref(),
                &frames,
                start_ms,
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
    PlaybackHandle { stop, seek_tx, gen }
}

fn play_once(
    live: Option<&Live>,
    frames: &Arc<RwLock<SharedShow>>,
    start_ms: u32,
    stop: &AtomicBool,
    events: &mpsc::Sender<PlaybackEvent>,
    seek_rx: &mpsc::Receiver<Seek>,
    gen: &mut u64,
) -> Result<(), ()> {
    let mut origin_ms = match frames.read() {
        Ok(show) if show.frames.is_empty() => {
            let _ = events.send(PlaybackEvent::Finished);
            return Ok(());
        }
        Ok(show) => start_ms.min(show.frames.last().map(|frame| frame.at_ms).unwrap_or(0)),
        Err(_) => return Err(()),
    };
    let mut origin = Instant::now();
    let mut next_tick = Instant::now();
    let mut tracks: Vec<Vec<(u32, u8)>> = Vec::new();
    let mut track_revision = u64::MAX;
    loop {
        if stop.load(Ordering::Relaxed) {
            let _ = events.send(PlaybackEvent::Finished);
            return Ok(());
        }
        if let Some(seek) = latest_seek(seek_rx) {
            let locked = match frames.read() {
                Ok(show) if !show.frames.is_empty() => show,
                Ok(_) => return Ok(()),
                Err(_) => return Err(()),
            };
            match seek {
                Seek::Frame(seek_index, seek_gen) => {
                    let index = seek_index.min(locked.frames.len() - 1);
                    *gen = seek_gen;
                    origin_ms = locked.frames[index].at_ms;
                }
                Seek::Time(at_ms, seek_gen) => {
                    *gen = seek_gen;
                    origin_ms =
                        at_ms.min(locked.frames.last().map(|frame| frame.at_ms).unwrap_or(0));
                }
            }
            origin = Instant::now();
            next_tick = Instant::now();
        }
        let elapsed_ms = u32::try_from(origin.elapsed().as_millis()).unwrap_or(u32::MAX);
        let now_ms = origin_ms.saturating_add(elapsed_ms);
        let (index, slots, end_ms) = {
            let locked = match frames.read() {
                Ok(show) if !show.frames.is_empty() => show,
                Ok(_) => return Ok(()),
                Err(_) => return Err(()),
            };
            let end_ms = locked.frames.last().map(|frame| frame.at_ms).unwrap_or(0);
            let at = now_ms.min(end_ms);
            let index = show::index_at(&locked.frames, at);
            if locked.revision != track_revision {
                tracks = show::index_tracks(&locked.frames);
                track_revision = locked.revision;
            }
            (index, levels_from_tracks(&tracks, at), end_ms)
        };
        if let Some(live) = live {
            live.publish(&slots);
        }
        let _ = events.send(PlaybackEvent::Frame(index, now_ms.min(end_ms), *gen));
        if now_ms >= end_ms {
            return Ok(());
        }
        next_tick += TICK;
        let now = Instant::now();
        if now > next_tick {
            if now.saturating_duration_since(next_tick) > TICK {
                next_tick = now + TICK;
            }
        } else {
            thread::sleep(next_tick - now);
        }
    }
}

pub fn scale_slots(slots: &[u8; 512], master: u8) -> [u8; 512] {
    if master == 255 {
        return *slots;
    }
    let mut out = [0u8; 512];
    for (out_slot, slot) in out.iter_mut().zip(slots.iter()) {
        *out_slot = ((u16::from(*slot) * u16::from(master) + 127) / 255) as u8;
    }
    out
}

fn levels_from_tracks(tracks: &[Vec<(u32, u8)>], at_ms: u32) -> [u8; 512] {
    let mut levels = [0u8; 512];
    for (slot, keys) in tracks.iter().enumerate() {
        levels[slot] = show::lerp_keys(keys, at_ms);
    }
    levels
}

fn latest_seek(seek_rx: &mpsc::Receiver<Seek>) -> Option<Seek> {
    let mut latest = None;
    while let Ok(seek) = seek_rx.try_recv() {
        latest = Some(seek);
    }
    latest
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::Frame;

    fn frame(at_ms: u32) -> Frame {
        Frame::full(at_ms, [0; 512])
    }

    #[test]
    fn sequence_advances_and_skips_zero() {
        let sequence = AtomicU8::new(0);
        assert_eq!(bump_sequence(&sequence), 1);
        assert_eq!(bump_sequence(&sequence), 2);
        sequence.store(255, Ordering::Relaxed);
        assert_eq!(bump_sequence(&sequence), 1);
        assert_eq!(bump_sequence(&sequence), 2);
    }

    #[test]
    fn solo_mutes_every_other_channel() {
        let mask = SoloMask::new();
        let mut slots = [9u8; 512];
        mask.apply(&mut slots);
        assert!(slots.iter().all(|level| *level == 9));
        mask.toggle(1);
        mask.toggle(512);
        mask.apply(&mut slots);
        assert_eq!(slots[0], 9);
        assert_eq!(slots[511], 9);
        assert!(slots[1..511].iter().all(|level| *level == 0));
        mask.clear();
        let mut open = [4u8; 512];
        mask.apply(&mut open);
        assert!(open.iter().all(|level| *level == 4));
    }

    #[test]
    fn master_scales_output_and_full_is_exact() {
        let mut slots = [0u8; 512];
        slots[0] = 255;
        slots[1] = 100;
        slots[2] = 1;
        assert_eq!(scale_slots(&slots, 255), slots);
        let half = scale_slots(&slots, 128);
        assert_eq!(half[0], 128);
        assert_eq!(half[1], 50);
        assert_eq!(half[2], 1);
        assert_eq!(scale_slots(&slots, 0), [0u8; 512]);
    }

    #[test]
    fn track_level_scales_one_channel_and_full_is_exact() {
        let levels = TrackLevels::new();
        let mut slots = [0u8; 512];
        slots[0] = 200;
        slots[4] = 255;
        levels.apply(&mut slots);
        assert_eq!(slots[0], 200);
        assert_eq!(slots[4], 255);
        levels.set(1, 128);
        levels.set(5, 0);
        levels.apply(&mut slots);
        assert_eq!(slots[0], 100);
        assert_eq!(slots[4], 0);
        assert_eq!(levels.get(2), 255);
        levels.set(0, 10);
        assert_eq!(levels.get(1), 128);
    }

    #[test]
    fn transmission_curve_follows_the_track_fader_and_solo_kills_preheat() {
        use crate::curve::{CurvePoint, TransferLaw};
        let curves = TrackCurves::new();
        curves.set(1, TransferLaw::Gamma { gamma: 2.0 }).unwrap();
        curves
            .set(
                2,
                TransferLaw::Points {
                    points: vec![
                        CurvePoint {
                            input: 0,
                            output: 40,
                        },
                        CurvePoint {
                            input: 255,
                            output: 255,
                        },
                    ],
                },
            )
            .unwrap();
        let levels = TrackLevels::new();
        levels.set(1, 128);
        let mut slots = [0u8; 512];
        slots[0] = 255;
        slots[1] = 0;
        let open = shape_output(&slots, 255, &SoloMask::new(), &levels, &curves);
        assert_eq!(open[0], 64);
        assert_eq!(open[1], 40);
        let solo = SoloMask::new();
        solo.toggle(1);
        let muted = shape_output(&slots, 255, &solo, &levels, &curves);
        assert_eq!(muted[0], 64);
        assert_eq!(muted[1], 0);
    }

    #[test]
    fn master_gamma_scales_the_output_and_full_stays_exact() {
        use crate::curve::TrackCurves;
        let curves = TrackCurves::new();
        curves.set_master_gamma(2.0).unwrap();
        let mut slots = [0u8; 512];
        slots[0] = 255;
        let shaped = shape_output(&slots, 128, &SoloMask::new(), &TrackLevels::new(), &curves);
        assert_eq!(shaped[0], 64);
        let full = shape_output(&slots, 255, &SoloMask::new(), &TrackLevels::new(), &curves);
        assert_eq!(full[0], 255);
    }

    #[test]
    fn playback_samples_the_keyframe_already_reached() {
        let frames = vec![frame(0), frame(1_000), frame(5_000)];
        assert_eq!(show::index_at(&frames, 2_500), 1);
        assert_eq!(show::index_at(&frames, 1_000), 1);
        assert_eq!(show::index_at(&frames, 0), 0);
        assert_eq!(show::index_at(&frames, 5_000), 2);
    }
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
