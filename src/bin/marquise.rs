use std::env;
use std::f64::consts::{PI, TAU};
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

const END_MS: u32 = 180_000;
const PERIOD_MS: u32 = 20;
const STEP: f64 = TAU / 3.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Show {
    Ghost,
    Zen,
    Window,
}

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / ((1u64 << 53) as f64)
    }

    fn range(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.f64()
    }

    fn range_u(&mut self, a: u32, b: u32) -> u32 {
        if b <= a {
            return a;
        }
        a + (self.f64() * f64::from(b - a)) as u32
    }
}

struct Key {
    t: u32,
    v: f64,
}

struct Line {
    keys: Vec<Key>,
    t: u32,
    v: f64,
}

impl Line {
    fn new(v: f64) -> Self {
        Self {
            keys: vec![Key { t: 0, v }],
            t: 0,
            v,
        }
    }

    fn room(&self) -> u32 {
        END_MS.saturating_sub(self.t)
    }

    fn to(&mut self, v: f64, dur: u32) {
        let dur = dur.min(self.room());
        if dur == 0 {
            return;
        }
        self.t += dur;
        self.v = v;
        self.keys.push(Key { t: self.t, v });
    }

    fn hold(&mut self, dur: u32) {
        let v = self.v;
        self.to(v, dur);
    }
}

struct Score {
    phase: Line,
    width: Line,
    gain: Line,
    lung: Line,
    neon: [Line; 3],
}

impl Score {
    fn lit(width: f64, lung: f64) -> Self {
        Self {
            phase: Line::new(0.0),
            width: Line::new(width),
            gain: Line::new(1.0),
            lung: Line::new(lung),
            neon: [
                Line::new(NEON_FLOOR),
                Line::new(NEON_FLOOR),
                Line::new(NEON_FLOOR),
            ],
        }
    }

    fn room(&self) -> u32 {
        self.phase.room()
    }

    fn kick(&mut self, dp: f64, width: f64, gain: f64, lung: f64, dur: u32) {
        let dest = self.phase.v + dp;
        self.phase.to(dest, dur);
        self.width.to(width, dur);
        self.gain.to(gain, dur);
        self.lung.to(lung, dur);
    }

    fn hop(&mut self, dir: f64, width: f64, gain: f64, lung: f64, dur: u32) {
        let dest = next_phase(self.phase.v, dir);
        let delta = dest - self.phase.v;
        self.kick(delta, width, gain, lung, dur);
    }

    fn rest(&mut self, width: f64, gain: f64, lung: f64, dur: u32) {
        self.kick(0.0, width, gain, lung, dur);
    }

    fn finish(&mut self) {
        let left = END_MS.saturating_sub(self.phase.t);
        if left == 0 {
            return;
        }
        let width = self.width.v;
        let gain = self.gain.v;
        let lung = self.lung.v;
        self.kick(0.0, width, gain, lung, left);
    }

    fn hold_neons(&mut self, dur: u32) {
        for line in &mut self.neon {
            line.hold(dur);
        }
    }

    fn rise_neons(&mut self, peaks: [f64; 3], dur: u32) {
        for (line, peak) in self.neon.iter_mut().zip(peaks) {
            line.to(peak, dur);
        }
    }

    fn fall_neons(&mut self, dur: u32) {
        for line in &mut self.neon {
            line.to(NEON_FLOOR, dur);
        }
    }

    fn close_neons(&mut self) {
        for line in &mut self.neon {
            let left = END_MS.saturating_sub(line.t);
            if left > 0 {
                line.to(NEON_FLOOR, left);
            }
        }
    }

    fn neon_room(&self) -> u32 {
        self.neon[0].room()
    }

    fn stalk(&mut self, rng: &mut Rng) {
        let start = self.phase.t;
        let budget = rng.range_u(2_400, 6_200);
        while self.phase.t < start + budget && self.room() > 180 {
            let width = rng.range(1.55, 1.95);
            let gain = rng.range(0.55, 1.0);
            let roll = rng.f64();
            if roll < 0.55 {
                let dur = rng.range_u(90, 320).min(self.room());
                let lung = breath(self.phase.t, 1_800, 0.02, 0.95);
                self.hop(1.0, width, gain, lung, dur);
                let dur = rng.range_u(80, 900).min(self.room());
                let lung = breath(self.phase.t, 1_800, 0.02, 0.95);
                let gain = rng.range(0.5, 1.0);
                self.rest(width, gain, lung, dur);
            } else if roll < 0.78 {
                self.lurch(rng, width);
            } else {
                let hops = rng.range_u(2, 4);
                for _ in 0..hops {
                    if self.room() < 70 {
                        break;
                    }
                    let dur = rng.range_u(70, 160).min(self.room());
                    let gain = rng.range(0.55, 1.0);
                    let lung = breath(self.phase.t, 700, 0.05, 1.0);
                    let width = rng.range(1.5, 1.85);
                    self.hop(1.0, width, gain, lung, dur);
                }
            }
        }
    }

    fn lurch(&mut self, rng: &mut Rng, width: f64) {
        if self.room() < 300 {
            return;
        }
        let dur = rng.range_u(70, 180).min(self.room());
        let gain = rng.range(0.5, 0.85);
        let lung = rng.range(0.15, 0.55);
        self.hop(-1.0, width, gain, lung, dur);
        let hops = rng.range_u(1, 4);
        for _ in 0..hops {
            if self.room() < 90 {
                return;
            }
            let dur = rng.range_u(50, 140).min(self.room());
            let lung = rng.range(0.55, 1.0);
            let gain = rng.range(0.6, 1.0);
            self.hop(1.0, width, gain, lung, dur);
            if self.room() < 40 {
                return;
            }
            let dur = rng.range_u(20, 110).min(self.room());
            self.rest(width, gain, lung, dur);
        }
    }

    fn drop_ride(&mut self, rng: &mut Rng) {
        let start = self.phase.t;
        let budget = rng.range_u(1_600, 3_400);
        let width = rng.range(1.5, 1.8);
        let spike = 50.min(self.room());
        self.rest(width, 1.0, 1.0, spike);
        while self.phase.t < start + budget && self.room() > 120 {
            let dur = rng.range_u(50, 120).min(self.room());
            let lung = breath(self.phase.t, 420, 0.0, 1.0);
            let gain = rng.range(0.7, 1.0);
            let width = rng.range(1.5, 1.8);
            self.hop(1.0, width, gain, lung, dur);
            let dur = rng.range_u(30, 140).min(self.room());
            let lung = breath(self.phase.t, 420, 0.0, 1.0);
            self.rest(width, gain, lung, dur);
        }
    }

    fn tunnel(&mut self, rng: &mut Rng) {
        if self.room() < 400 {
            return;
        }
        let burst = rng.range_u(4, 8);
        for _ in 0..burst {
            if self.room() < 80 {
                return;
            }
            let width = rng.range(1.5, 1.9);
            let dur = rng.range_u(50, 140).min(self.room());
            let gain = rng.range(0.55, 1.0);
            let lung = breath(self.phase.t, 900, 0.05, 1.0);
            self.hop(1.0, width, gain, lung, dur);
            if self.room() < 40 {
                return;
            }
            let dur = rng.range_u(20, 90).min(self.room());
            self.rest(width, gain, lung, dur);
        }
    }
}

fn next_phase(phase: f64, dir: f64) -> f64 {
    if dir >= 0.0 {
        let mut dest = (phase / STEP).floor() * STEP + STEP;
        if dest < phase + 0.05 {
            dest += STEP;
        }
        dest
    } else {
        let mut dest = (phase / STEP).ceil() * STEP - STEP;
        if dest > phase - 0.05 {
            dest -= STEP;
        }
        dest
    }
}

fn breath(t: u32, period: u32, low: f64, high: f64) -> f64 {
    let phase = (t % period) as f64 / f64::from(period);
    let wave = if phase < 0.36 {
        phase / 0.36
    } else {
        (1.0 - phase) / 0.64
    };
    low + (high - low) * wave
}

fn ghost(rng: &mut Rng) -> Score {
    let mut score = Score::lit(1.08, 0.42);
    while score.room() > 700 {
        let roll = rng.f64();
        if roll < 0.48 {
            score.stalk(rng);
        } else if roll < 0.74 {
            score.drop_ride(rng);
        } else if roll < 0.90 {
            score.tunnel(rng);
        } else {
            let width = rng.range(1.55, 1.9);
            score.lurch(rng, width);
        }
    }
    score.finish();
    breathe_lung(&mut score, rng, Show::Ghost);
    crackle(&mut score, rng, false);
    score
}

fn zen(rng: &mut Rng) -> Score {
    let mut score = Score::lit(3.4, 0.22);
    score.gain.v = 0.5;
    score.gain.keys[0].v = 0.5;
    while score.room() > 1_200 {
        let travel = rng.range(0.85, 1.85);
        let dur = rng.range_u(2_800, 7_200).min(score.room());
        let width = rng.range(2.35, 3.55);
        let gain = rng.range(0.24, 0.43);
        score.kick(travel, width, gain, 0.2, dur);
    }
    score.finish();
    breathe_lung(&mut score, rng, Show::Zen);
    oscillate(&mut score, rng);
    score
}

fn window(rng: &mut Rng) -> Score {
    let mut score = Score::lit(1.45, 0.4);
    score.gain.v = 0.78;
    score.gain.keys[0].v = 0.78;
    while score.room() > 1_200 {
        let village = rng.f64() < 0.34;
        let dur = if village {
            rng.range_u(5_000, 9_000)
        } else {
            rng.range_u(9_000, 18_000)
        };
        let dur = dur.min(score.room());
        let rate = if village {
            rng.range(0.42, 0.7)
        } else {
            rng.range(0.16, 0.32)
        };
        let width = if village {
            rng.range(2.05, 2.45)
        } else {
            rng.range(2.5, 3.15)
        };
        let gain = if village {
            rng.range(0.7, 0.92)
        } else {
            rng.range(0.34, 0.52)
        };
        let travel = rate * f64::from(dur) / 1_000.0;
        score.kick(travel, width, gain, 0.4, dur);
    }
    score.finish();
    breathe_lung(&mut score, rng, Show::Window);
    passing(&mut score, rng);
    score
}

fn breathe_lung(score: &mut Score, rng: &mut Rng, show: Show) {
    let (inhale, exhale) = match show {
        Show::Ghost => ((900, 1_500), (1_400, 2_400)),
        Show::Zen => ((5_500, 9_500), (8_000, 14_000)),
        Show::Window => ((8_000, 13_000), (10_000, 16_000)),
    };
    let mut line = Line::new(match show {
        Show::Zen => rng.range(0.18, 0.26),
        _ => rng.range(0.34, 0.5),
    });
    let mut up = true;
    while line.room() > 400 {
        if show == Show::Ghost && rng.f64() < 0.16 && line.room() > 900 {
            frighten(&mut line, rng);
            up = true;
            continue;
        }
        let span = if up { inhale } else { exhale };
        let dur = rng.range_u(span.0, span.1).min(line.room());
        let target = if up {
            match show {
                Show::Zen => rng.range(0.46, 0.52),
                Show::Window => rng.range(0.5, 0.64),
                Show::Ghost => rng.range(0.94, 1.0),
            }
        } else {
            match show {
                Show::Window => rng.range(0.24, 0.34),
                _ => rng.range(0.0, 0.05),
            }
        };
        line.to(target, dur);
        up = !up;
    }
    let left = END_MS.saturating_sub(line.t);
    if left > 0 {
        line.hold(left);
    }
    score.lung = line;
}

fn frighten(line: &mut Line, rng: &mut Rng) {
    let blinks = rng.range_u(3, 7);
    for _ in 0..blinks {
        if line.room() < 140 {
            return;
        }
        let attack = rng.range_u(30, 70).min(line.room());
        line.to(1.0, attack);
        let hold = rng.range_u(40, 140).min(line.room());
        line.to(1.0, hold);
        let drop = rng.range_u(20, 50).min(line.room());
        line.to(0.0, drop);
    }
}

fn snap(dur: u32, room: u32) -> u32 {
    let room = room - room % PERIOD_MS;
    let dur = dur.max(PERIOD_MS) / PERIOD_MS * PERIOD_MS;
    if room == 0 {
        0
    } else {
        dur.min(room)
    }
}

const NEON_FLOOR: f64 = 7.0 / 255.0;

fn spark_level(rng: &mut Rng, soften: bool) -> u32 {
    let level = spark(rng);
    if soften {
        7 + (level - 7) / 2
    } else {
        level
    }
}

fn spark(rng: &mut Rng) -> u32 {
    if rng.f64() < 0.35 {
        rng.range_u(90, 201)
    } else {
        rng.range_u(18, 70)
    }
}

fn crackle(score: &mut Score, rng: &mut Rng, soften: bool) {
    while score.neon_room() >= 80 {
        let gap = snap(rng.range_u(40, 380), score.neon_room());
        if gap == 0 {
            break;
        }
        score.hold_neons(gap);
        let ticks = if rng.f64() < 0.7 {
            rng.range_u(2, 8)
        } else {
            1
        };
        for _ in 0..ticks {
            let rise = snap(PERIOD_MS, score.neon_room());
            if rise == 0 {
                break;
            }
            let mut peaks = [NEON_FLOOR; 3];
            let index = rng.range_u(0, 3) as usize;
            peaks[index] = f64::from(spark_level(rng, soften)) / 255.0;
            if rng.f64() < 0.4 {
                let other = rng.range_u(0, 3) as usize;
                if other != index {
                    peaks[other] = f64::from(spark_level(rng, soften)) / 255.0;
                }
            }
            score.rise_neons(peaks, rise);
            let stay = snap(rng.range_u(PERIOD_MS, 80), score.neon_room());
            if stay > 0 {
                score.rise_neons(peaks, stay);
            }
            let fall = snap(PERIOD_MS, score.neon_room());
            if fall > 0 {
                score.fall_neons(fall);
            }
            let pause = snap(rng.range_u(PERIOD_MS, 120), score.neon_room());
            if pause > 0 {
                score.hold_neons(pause);
            }
        }
    }
    score.close_neons();
}

fn oscillate(score: &mut Score, rng: &mut Rng) {
    let mut up = true;
    while score.neon_room() > 400 {
        let dur = snap(rng.range_u(4_500, 9_000), score.neon_room());
        if dur == 0 {
            break;
        }
        let level = if up { rng.range_u(12, 16) } else { 7 };
        let glow = f64::from(level) / 255.0;
        score.rise_neons([glow, glow, glow], dur);
        up = !up;
    }
    score.close_neons();
}

fn passing(score: &mut Score, rng: &mut Rng) {
    let mut index = 0usize;
    while score.neon_room() > 800 {
        let quiet = snap(rng.range_u(2_500, 9_000), score.neon_room());
        if quiet == 0 {
            break;
        }
        score.hold_neons(quiet);
        let rise = snap(rng.range_u(1_000, 2_400), score.neon_room());
        if rise == 0 {
            break;
        }
        let mut peaks = [NEON_FLOOR; 3];
        peaks[index] = f64::from(rng.range_u(28, 85)) / 255.0;
        if rng.f64() < 0.25 {
            let other = (index + 1) % 3;
            peaks[other] = f64::from(rng.range_u(18, 50)) / 255.0;
        }
        index = (index + 1) % 3;
        score.rise_neons(peaks, rise);
        let stay = snap(rng.range_u(800, 2_200), score.neon_room());
        if stay > 0 {
            score.rise_neons(peaks, stay);
        }
        let fall = snap(rng.range_u(1_600, 3_600), score.neon_room());
        if fall > 0 {
            score.fall_neons(fall);
        }
    }
    score.close_neons();
}

fn lobe(phase: f64, pole: f64, width: f64) -> f64 {
    let mut d = (phase - pole).rem_euclid(TAU);
    if d > PI {
        d = TAU - d;
    }
    if d >= width {
        return 0.0;
    }
    let x = d / width;
    0.5 * (1.0 + (PI * x).cos())
}

fn shade(phase: f64, width: f64, gain: f64) -> [f64; 3] {
    let mut levels = [0.0; 3];
    for (index, level) in levels.iter_mut().enumerate() {
        let pole = f64::from(index as u32) * STEP;
        *level = gain.clamp(0.0, 1.0) * lobe(phase, pole, width) * 0.55;
    }
    levels
}

fn byte(level: f64) -> u8 {
    (level.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn smooth(show: Show, lung: bool, neon: bool) -> bool {
    if neon {
        return show != Show::Ghost;
    }
    lung || show == Show::Zen
}

fn sample(keys: &[Key], t: u32, ease: bool) -> f64 {
    if t <= keys[0].t {
        return keys[0].v;
    }
    let last = keys.last().unwrap();
    if t >= last.t {
        return last.v;
    }
    let index = keys.partition_point(|key| key.t <= t) - 1;
    let start = &keys[index];
    let end = &keys[index + 1];
    if end.t <= start.t {
        return start.v;
    }
    let mut u = f64::from(t - start.t) / f64::from(end.t - start.t);
    if ease {
        u = 0.5 - 0.5 * (PI * u).cos();
    }
    start.v + (end.v - start.v) * u
}

struct Frame {
    groups: [u8; 3],
    lung: u8,
    neon: [u8; 3],
}

fn frame_at(score: &Score, show: Show, t: u32) -> Frame {
    let phase = sample(&score.phase.keys, t, smooth(show, false, false));
    let width = sample(&score.width.keys, t, smooth(show, false, false));
    let gain = sample(&score.gain.keys, t, smooth(show, false, false));
    let lung = sample(&score.lung.keys, t, smooth(show, true, false));
    let shaped = shade(phase, width, gain);
    let mut groups = [0u8; 3];
    for (index, level) in shaped.iter().enumerate() {
        groups[index] = byte(*level);
    }
    let mut neon = [0u8; 3];
    for (index, line) in score.neon.iter().enumerate() {
        neon[index] = byte(sample(&line.keys, t, false)).max(7);
    }
    Frame {
        groups,
        lung: byte(lung),
        neon,
    }
}

fn slots_of(frame: &Frame) -> [u8; 512] {
    let mut slots = [0u8; 512];
    slots[..3].copy_from_slice(&frame.groups);
    slots[4..7].copy_from_slice(&frame.groups);
    slots[3] = frame.lung;
    slots[7] = frame.lung;
    slots[8..11].copy_from_slice(&frame.neon);
    slots
}

fn leader(groups: [u8; 3]) -> Option<usize> {
    let mut best_i = 0usize;
    let mut best = 0u8;
    let mut second = 0u8;
    for (index, level) in groups.iter().enumerate() {
        if *level > best {
            second = best;
            best = *level;
            best_i = index;
        } else if *level > second {
            second = *level;
        }
    }
    if best < 12 || u16::from(best) * 2 <= u16::from(second) * 3 {
        None
    } else {
        Some(best_i)
    }
}

struct Stats {
    turns: f64,
    forward: u32,
    back: u32,
    dark_ms: u32,
    lead_ms: [u32; 3],
    lung_min: u8,
    lung_max: u8,
    breaths: u32,
    neon_max: [u8; 3],
    neon_ms: u32,
    neon_run: u32,
}

fn analyze(score: &Score, show: Show) -> Stats {
    let start = sample(&score.phase.keys, 0, smooth(show, false, false));
    let end = sample(&score.phase.keys, END_MS, smooth(show, false, false));
    let mut stats = Stats {
        turns: (end - start) / TAU,
        forward: 0,
        back: 0,
        dark_ms: 0,
        lead_ms: [0; 3],
        lung_min: 255,
        lung_max: 0,
        breaths: 0,
        neon_max: [0; 3],
        neon_ms: 0,
        neon_run: 0,
    };
    let mut prev: Option<usize> = None;
    let mut run = [0u32; 3];
    let mut emptied = true;
    let mut t = 0u32;
    loop {
        let frame = frame_at(score, show, t);
        let step = if t >= END_MS { 0 } else { PERIOD_MS };
        if frame.groups.iter().all(|level| *level < 12) {
            stats.dark_ms += step;
        }
        if let Some(now) = leader(frame.groups) {
            stats.lead_ms[now] += step;
            if let Some(was) = prev {
                let hop = (now + 3 - was) % 3;
                if hop == 1 {
                    stats.forward += 1;
                } else if hop == 2 {
                    stats.back += 1;
                }
            }
            prev = Some(now);
        }
        stats.lung_min = stats.lung_min.min(frame.lung);
        stats.lung_max = stats.lung_max.max(frame.lung);
        if emptied && frame.lung > 105 {
            stats.breaths += 1;
            emptied = false;
        } else if frame.lung < 95 {
            emptied = true;
        }
        let mut lit = false;
        for (index, run) in run.iter_mut().enumerate() {
            stats.neon_max[index] = stats.neon_max[index].max(frame.neon[index]);
            if frame.neon[index] > 7 {
                lit = true;
                *run += step;
                stats.neon_run = stats.neon_run.max(*run);
            } else {
                *run = 0;
            }
        }
        if lit {
            stats.neon_ms += step;
        }
        if t >= END_MS {
            break;
        }
        t = (t + PERIOD_MS).min(END_MS);
    }
    stats
}

fn enforce(show: Show, stats: &Stats) {
    for max in stats.neon_max {
        match show {
            Show::Zen => assert!((12..=15).contains(&max), "zen neon {max}"),
            Show::Window => assert!((20..=100).contains(&max), "window neon {max}"),
            Show::Ghost => assert!(max > 80, "ghost neon {max}"),
        }
    }
    if show == Show::Ghost {
        assert!(stats.neon_run < 160, "neon held {} ms", stats.neon_run);
    }
    assert!(stats.neon_ms > 8_000, "neon time {}", stats.neon_ms);
    match show {
        Show::Ghost => {
            assert!(
                stats.turns > 40.0 && stats.turns < 400.0,
                "ghost turns {}",
                stats.turns
            );
            assert!(stats.forward > 40, "ghost forward {}", stats.forward);
            assert!(stats.back > 3, "ghost back {}", stats.back);
            assert!(
                stats.forward > stats.back * 2,
                "ghost chase {} / {}",
                stats.forward,
                stats.back
            );
            assert!(stats.dark_ms < 800, "ghost dark {}", stats.dark_ms);
            assert!(stats.lung_min < 20, "ghost lung min {}", stats.lung_min);
            assert!(stats.lung_max > 230, "ghost lung max {}", stats.lung_max);
            assert!(stats.breaths > 20, "ghost breaths {}", stats.breaths);
        }
        Show::Zen => {
            assert!(
                stats.turns > 2.2 && stats.turns < 11.0,
                "zen turns {}",
                stats.turns
            );
            assert_eq!(stats.back, 0, "zen reversed");
            assert!(stats.dark_ms < 500, "zen dark {}", stats.dark_ms);
            assert!(stats.lung_min < 25, "zen lung min {}", stats.lung_min);
            assert!(
                stats.lung_max > 100 && stats.lung_max < 160,
                "zen lung max {}",
                stats.lung_max
            );
            assert!(stats.breaths > 4, "zen breaths {}", stats.breaths);
        }
        Show::Window => {
            assert!(
                stats.turns > 4.0 && stats.turns < 22.0,
                "window turns {}",
                stats.turns
            );
            assert_eq!(stats.back, 0, "window reversed");
            assert!(stats.forward > 8, "window forward {}", stats.forward);
            assert!(stats.dark_ms < 800, "window dark {}", stats.dark_ms);
            assert!(
                stats.lung_min > 40 && stats.lung_min < 110,
                "window lung min {}",
                stats.lung_min
            );
            assert!(
                stats.lung_max > 120 && stats.lung_max < 180,
                "window lung max {}",
                stats.lung_max
            );
            assert!(stats.breaths > 3, "window breaths {}", stats.breaths);
        }
    }
    for (index, lead) in stats.lead_ms.iter().enumerate() {
        let least = if show == Show::Window { 4_000 } else { 8_000 };
        assert!(*lead > least, "{index} leads only {} ms", lead);
    }
}

fn report(name: &str, stats: &Stats) {
    println!(
        "{name}  {turns:.1} turns  fwd {fwd}  back {back}  dark {dark}s",
        turns = stats.turns,
        fwd = stats.forward,
        back = stats.back,
        dark = stats.dark_ms / 1000
    );
    println!(
        "  groups {a}/{b}/{c}s  lung {lo}..{hi}  breaths {breaths}  neons {n0}/{n1}/{n2} lit {lit}s",
        a = stats.lead_ms[0] / 1000,
        b = stats.lead_ms[1] / 1000,
        c = stats.lead_ms[2] / 1000,
        lo = stats.lung_min,
        hi = stats.lung_max,
        breaths = stats.breaths,
        n0 = stats.neon_max[0],
        n1 = stats.neon_max[1],
        n2 = stats.neon_max[2],
        lit = stats.neon_ms / 1000
    );
}

fn write_show(path: &PathBuf, score: &Score, show: Show) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    let mut header = [0u8; 512];
    header[4..8].copy_from_slice(&END_MS.to_be_bytes());
    header[8..12].copy_from_slice(&END_MS.to_be_bytes());
    header[12..14].copy_from_slice(&512u16.to_be_bytes());
    file.write_all(&header)?;
    let mut t = 0u32;
    let mut prev = 0u32;
    loop {
        let delta = t - prev;
        let slots = slots_of(&frame_at(score, show, t));
        file.write_all(b"framedmx")?;
        file.write_all(&[0])?;
        file.write_all(&512u16.to_be_bytes())?;
        file.write_all(&(delta * 100).to_be_bytes())?;
        file.write_all(&t.to_be_bytes())?;
        file.write_all(&slots)?;
        if t >= END_MS {
            break;
        }
        prev = t;
        t = (t + PERIOD_MS).min(END_MS);
    }
    file.flush()
}

fn confirm(path: &PathBuf, show: Show) {
    let data = fs::read(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    let end = u32::from_be_bytes(data[4..8].try_into().unwrap());
    let slots_n = u16::from_be_bytes(data[12..14].try_into().unwrap());
    assert_eq!(end, END_MS);
    assert_eq!(slots_n, 512);
    let mut pos = 512usize;
    let mut frames = 0u32;
    while pos + 19 + 512 <= data.len() {
        assert_eq!(&data[pos..pos + 8], b"framedmx");
        let at = u32::from_be_bytes(data[pos + 15..pos + 19].try_into().unwrap());
        let frame = &data[pos + 19..pos + 19 + 512];
        for index in 0..4 {
            assert_eq!(frame[index], frame[index + 4], "pair {index} at {at}");
        }
        assert!(frame[11..].iter().all(|byte| *byte == 0), "spill at {at}");
        for level in &frame[8..11] {
            assert!(*level >= 7, "neon {level} at {at}");
        }
        if at == 0 {
            assert_eq!(frame[8], 7);
            assert_eq!(frame[9], 7);
            assert_eq!(frame[10], 7);
            assert!(frame[3] > 40, "lung dark at start");
            let opening = if show == Show::Zen { 45 } else { 100 };
            assert!(frame[0] > opening, "chase does not open on group 1");
            match show {
                Show::Ghost | Show::Window => {
                    assert_eq!(frame[1], 0);
                    assert_eq!(frame[2], 0);
                }
                Show::Zen => {
                    assert!(frame[1] > 12 && frame[2] > 12, "zen is not floating");
                }
            }
        }
        if at == END_MS / 2 && show != Show::Ghost {
            assert!(
                frame[0] > 15 || frame[1] > 15 || frame[2] > 15,
                "mid show is dark"
            );
        }
        frames += 1;
        pos += 19 + 512;
    }
    assert_eq!(frames, END_MS / PERIOD_MS + 1);
}

fn render(show: Show, name: &'static str, seed: u64) -> Track {
    let mut rng = Rng(seed);
    let score = match show {
        Show::Ghost => ghost(&mut rng),
        Show::Zen => zen(&mut rng),
        Show::Window => window(&mut rng),
    };
    let stats = analyze(&score, show);
    enforce(show, &stats);
    report(name, &stats);
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(name);
    write_show(&path, &score, show).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    confirm(&path, show);
    let label = name.trim_end_matches(".NCOD");
    let blob = cue_track(&score, show);
    check_blob(&blob, CUE_FRAMES);
    Track {
        label,
        frames: CUE_FRAMES,
        blob,
    }
}

const CUE_RATE: u32 = 40;
const CUE_FRAMES: u32 = END_MS / 1_000 * CUE_RATE;

struct Track {
    label: &'static str,
    frames: u32,
    blob: Vec<u8>,
}

fn cue_byte(level: u8) -> u8 {
    match level {
        0xd0 | 0xe0 => level - 1,
        level => level,
    }
}

fn cue_levels(score: &Score, show: Show, t: u32) -> [u8; 11] {
    let slots = slots_of(&frame_at(score, show, t));
    let mut levels = [0u8; 11];
    for (level, slot) in levels.iter_mut().zip(slots) {
        *level = cue_byte(slot);
    }
    levels
}

fn encode_frames(frames: &[[u8; 11]]) -> Vec<u8> {
    let mut previous = [0u8; 11];
    let mut out = Vec::new();
    for levels in frames {
        if *levels == previous {
            out.push(0xd0);
        } else {
            out.extend_from_slice(&[0x30, 0x00, 0x0b]);
            out.extend_from_slice(levels);
            out.push(0xd0);
            previous = *levels;
        }
    }
    out.push(0xe0);
    out
}

fn cue_track(score: &Score, show: Show) -> Vec<u8> {
    let mut frames = Vec::with_capacity(CUE_FRAMES as usize);
    for index in 0..CUE_FRAMES {
        frames.push(cue_levels(score, show, index * 25));
    }
    encode_frames(&frames)
}

fn check_blob(blob: &[u8], frames: u32) {
    assert_eq!(blob.last().copied(), Some(0xe0));
    let mut pos = 0usize;
    let mut seen = 0u32;
    let mut previous = [0u8; 11];
    while pos + 1 < blob.len() {
        if blob[pos] == 0xd0 {
            seen += 1;
            pos += 1;
            continue;
        }
        assert_eq!(&blob[pos..pos + 3], &[0x30, 0x00, 0x0b]);
        let mut levels = [0u8; 11];
        levels.copy_from_slice(&blob[pos + 3..pos + 14]);
        assert!(levels.iter().all(|level| *level != 0xd0 && *level != 0xe0));
        assert_ne!(levels, previous);
        previous = levels;
        pos += 14;
        assert_eq!(blob[pos], 0xd0);
        seen += 1;
        pos += 1;
    }
    assert_eq!(pos, blob.len() - 1);
    assert_eq!(seen, frames);
}

fn base64_char(value: u8) -> char {
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"[value as usize] as char
}

fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    let mut index = 0;
    while index + 3 <= data.len() {
        let n = (u32::from(data[index]) << 16)
            | (u32::from(data[index + 1]) << 8)
            | u32::from(data[index + 2]);
        out.push(base64_char(((n >> 18) & 63) as u8));
        out.push(base64_char(((n >> 12) & 63) as u8));
        out.push(base64_char(((n >> 6) & 63) as u8));
        out.push(base64_char((n & 63) as u8));
        index += 3;
    }
    let rest = data.len() - index;
    if rest == 1 {
        let n = u32::from(data[index]) << 16;
        out.push(base64_char(((n >> 18) & 63) as u8));
        out.push(base64_char(((n >> 12) & 63) as u8));
        out.push('=');
        out.push('=');
    } else if rest == 2 {
        let n = (u32::from(data[index]) << 16) | (u32::from(data[index + 1]) << 8);
        out.push(base64_char(((n >> 18) & 63) as u8));
        out.push(base64_char(((n >> 12) & 63) as u8));
        out.push(base64_char(((n >> 6) & 63) as u8));
        out.push('=');
    }
    out
}

fn base64_value(byte: u8) -> u8 {
    match byte {
        b'A'..=b'Z' => byte - b'A',
        b'a'..=b'z' => byte - b'a' + 26,
        b'0'..=b'9' => byte - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => panic!("bad base64"),
    }
}

fn decode64(data: &str) -> Vec<u8> {
    let bytes = data.as_bytes();
    assert_eq!(bytes.len() % 4, 0);
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut index = 0;
    while index < bytes.len() {
        let a = base64_value(bytes[index]);
        let b = base64_value(bytes[index + 1]);
        let c = bytes[index + 2] != b'=';
        let d = bytes[index + 3] != b'=';
        out.push((a << 2) | (b >> 4));
        if c {
            let c = base64_value(bytes[index + 2]);
            out.push((b << 4) | (c >> 2));
            if d {
                out.push((c << 6) | base64_value(bytes[index + 3]));
            }
        }
        index += 4;
    }
    out
}

fn check_encoder() {
    let blob = encode_frames(&[[255; 11], [255; 11]]);
    assert_eq!(
        blob,
        [
            0x30, 0x00, 0x0b, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0xd0, 0xd0,
            0xe0,
        ]
    );
    assert_eq!(base64(&blob), "MAAL///////////////Q0OA=");
    assert_eq!(decode64(&base64(&blob)), blob);
    assert_eq!(decode64(&base64(&[0u8; 256])).len(), 256);
}

fn write_backup(path: &PathBuf, tracks: &[Track]) -> std::io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    writeln!(file, r#"<?xml version="1.0" ?>"#)?;
    writeln!(
        file,
        r#"<core device="CueCore2" version_pcb="2.12" version_firmware="1.46" pcb_serial="202551035" mac_address="B8:D8:12:80:7B:F2" backup_utility="vManager" utility_version="1.9.1" protocol_version="0.06">"#
    )?;
    writeln!(
        file,
        r#"    <tracks source="Art-Net" intensity_map="{}">"#,
        base64(&[0u8; 256])
    )?;
    for (index, track) in tracks.iter().enumerate() {
        writeln!(
            file,
            r#"        <track index="{}" label="{}" version="1.46" frames="{}" filesize="{}" sample_rate="40">{}</track>"#,
            index + 1,
            track.label,
            track.frames,
            track.blob.len(),
            base64(&track.blob)
        )?;
    }
    writeln!(file, r#"    </tracks>"#)?;
    writeln!(file, r#"</core>"#)?;
    file.flush()
}

fn confirm_backup(path: &PathBuf, tracks: &[Track]) {
    let text = fs::read_to_string(path).unwrap_or_else(|_| panic!("read {}", path.display()));
    assert!(text.starts_with("<?xml version=\"1.0\" ?>"));
    assert_eq!(text.matches("<track ").count(), tracks.len());
    assert!(text.contains(&base64(&[0u8; 256])));
    for track in tracks {
        let tag = format!("label=\"{}\"", track.label);
        let start = text
            .find(&tag)
            .unwrap_or_else(|| panic!("missing {}", track.label));
        let open = text[start..].find('>').unwrap() + start + 1;
        let close = text[open..].find("</track>").unwrap() + open;
        let blob = decode64(&text[open..close]);
        assert_eq!(blob, track.blob);
        check_blob(&blob, track.frames);
        let head = &text[start..open];
        assert!(head.contains(&format!("frames=\"{}\"", track.frames)));
        assert!(head.contains(&format!("filesize=\"{}\"", track.blob.len())));
        assert!(head.contains("sample_rate=\"40\""));
    }
}

fn main() {
    check_encoder();
    let tracks = [
        render(Show::Ghost, "fantome.NCOD", 0xFA17_0001),
        render(Show::Zen, "zen.NCOD", 0x2E40_0001),
        render(Show::Window, "paysage.NCOD", 0x7E11_0001),
    ];
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cuecore2/marquise.xml");
    write_backup(&path, &tracks).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    confirm_backup(&path, &tracks);
    let bytes = fs::metadata(&path)
        .unwrap_or_else(|_| panic!("stat {}", path.display()))
        .len();
    println!(
        "Cuecore2/marquise.xml  {} tracks  {} frames at {} Hz  {bytes} bytes",
        tracks.len(),
        CUE_FRAMES,
        CUE_RATE
    );
}
